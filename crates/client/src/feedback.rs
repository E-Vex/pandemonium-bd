//! The M9 feel pass's presentation layer (plan §11.2's overlay items and
//! §11.5's feedback cues): hit flashes, health bars, and command
//! acknowledgment pings — all event-driven (FD-9: events flow outward;
//! presentation never polls simulation internals beyond the snapshot it is
//! handed).
//!
//! Everything here is geometry math over plain data (positions arrive as
//! camera-projected NDC, bars and pings are pixel rectangles the UI pass
//! already draws), so the whole module is unit-testable without a GPU:
//! `tests` below pins the NDC → pixel conversion, the bar's fill fraction
//! and color thresholds, the flash bookkeeping, and the expiry. The
//! pixel-level "does it read well" judgement is the human half of DEBT-008.

use crate::input::CursorOrderHint;
use crate::text::{TextAtlas, UiQuad};
use pandemonium_engine::{logical_to_world, RenderSnapshot, RtsCamera};
use pandemonium_sim_api::{EntityId, Event, KindId, PlayerId, RejectReason, Vec2Fx};

/// How many presented frames a hit flash lasts (a third of a second at the
/// client's frame rate).
pub const FLASH_FRAMES: u64 = 18;

/// How many presented frames a command ping lasts (three quarters of a
/// second — enough to confirm the click, not enough to clutter).
pub const PING_FRAMES: u64 = 45;

/// How many presented frames a refusal cue lasts (a second — long enough
/// to read the HUD line, short enough to not nag).
pub const REFUSAL_FRAMES: u64 = 60;

/// How many presented frames a death fade lasts (a bit under half a second:
/// long enough to read as "that unit is gone", short enough to not clutter
/// a battle where deaths come in batches).
pub const DEATH_FRAMES: u64 = 14;

/// One dying entity's presentation cue: where it stood, what it was, and
/// the frame its fade completes at. The renderer shrinks and chars it over
/// the fade window — the entity is already gone from the simulation, so
/// this is purely the "death has weight" cue (plan §11.5).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DeathCue {
    /// Last world position (ground plane).
    pub pos: glam::Vec3,
    /// Which kind it was (the silhouette to shrink).
    pub kind: KindId,
    /// Owning slot (the team color to char).
    pub owner: PlayerId,
    /// The frame the fade completes at.
    pub expires_at: u64,
}

/// The event-driven feedback state: which entities were hit recently, where
/// commands were acknowledged recently, and how much feedback has flowed
/// (the smoke summary's evidence that the wiring ran).
#[derive(Default)]
pub struct FeedbackState {
    /// Entities hit in the last [`FLASH_FRAMES`] frames, with the frame the
    /// flash expires at.
    flashes: Vec<(EntityId, u64)>,
    /// Ground positions a command was issued at, with the expiry frame.
    pings: Vec<(Vec2Fx, u64)>,
    /// Where refused orders were clicked (the red refusal square), with
    /// the expiry frame (M9.1).
    refusals: Vec<(Vec2Fx, u64)>,
    /// The latest refusal's HUD line (its text and expiry frame) —
    /// `"order refused: <reason>"` (M9.1).
    pub refusal_notice: Option<(String, u64)>,
    /// Total `AttackHit` events seen (the smoke summary's evidence).
    pub hits_seen: u32,
    /// Total command pings issued.
    pub pings_issued: u32,
    /// Total refused orders seen (the smoke summary's evidence — a silent
    /// refusal reads as "I can't command my army", so they now count).
    pub refusals_seen: u32,
    /// Entities that died recently, with their last-seen position and kind.
    deaths: Vec<DeathCue>,
    /// Total deaths seen (the smoke summary's evidence).
    pub deaths_seen: u32,
}

impl FeedbackState {
    /// Feeds one step's events: every landed attack marks its victim for a
    /// flash. Presentation-only bookkeeping — the simulation already
    /// committed the hit by the time this runs.
    pub fn on_events(&mut self, events: &[Event], frame: u64) {
        for event in events {
            if let Event::AttackHit { target, .. } = event {
                self.flashes.push((*target, frame + FLASH_FRAMES));
                self.hits_seen += 1;
            }
        }
    }

    /// Acknowledges a command at a ground position: a ping the player sees
    /// the instant they click (plan §8.4's feel budget — intent to
    /// visible-response; the motion itself follows within the A8 tick
    /// bounds, this is the immediate cue).
    pub fn ping(&mut self, ground: Vec2Fx, frame: u64) {
        self.pings.push((ground, frame + PING_FRAMES));
        self.pings_issued += 1;
    }

    /// Records a refused order at its click point: a red square where it
    /// was clicked and the HUD line with the reason (M9.1 — the DEBT-008
    /// human pass: rejected commands were silent, which read as "the
    /// controls don't work").
    pub fn refuse(&mut self, ground: Vec2Fx, reason: RejectReason, frame: u64) {
        self.refusals.push((ground, frame + REFUSAL_FRAMES));
        self.refusal_notice = Some((
            format!("order refused: {}", rejection_text(reason)),
            frame + REFUSAL_FRAMES,
        ));
        self.refusals_seen += 1;
    }

    /// Records a death: the dying entity's last-seen position and kind
    /// become a shrinking, charring cue (plan §11.5's death fade). The
    /// caller resolves the last-seen data from its own snapshot history —
    /// the `Died` event itself carries only the id.
    pub fn death(&mut self, pos: glam::Vec3, kind: KindId, owner: PlayerId, frame: u64) {
        self.deaths.push(DeathCue {
            pos,
            kind,
            owner,
            expires_at: frame + DEATH_FRAMES,
        });
        self.deaths_seen += 1;
    }

    /// The active death fades, oldest first.
    pub fn active_deaths(&self) -> &[DeathCue] {
        &self.deaths
    }

    /// Drops expired flashes, pings, and refusals. Called once per
    /// presented frame.
    pub fn expire(&mut self, frame: u64) {
        self.flashes.retain(|(_, expires_at)| *expires_at > frame);
        self.pings.retain(|(_, expires_at)| *expires_at > frame);
        self.refusals.retain(|(_, expires_at)| *expires_at > frame);
        self.deaths.retain(|cue| cue.expires_at > frame);
        if self
            .refusal_notice
            .as_ref()
            .is_some_and(|(_, expires_at)| *expires_at <= frame)
        {
            self.refusal_notice = None;
        }
    }

    /// Whether an entity is flashing this frame (the renderer tints it).
    pub fn is_flashing(&self, entity: EntityId) -> bool {
        self.flashes.iter().any(|(id, _)| *id == entity)
    }

    /// The active command pings, oldest first.
    pub fn active_pings(&self) -> &[(Vec2Fx, u64)] {
        &self.pings
    }

    /// The active refusal squares, oldest first.
    pub fn active_refusals(&self) -> &[(Vec2Fx, u64)] {
        &self.refusals
    }
}

/// Converts NDC (y up, -1..1) to screen pixels (y down from the top-left).
pub fn ndc_to_pixels(ndc: (f32, f32), viewport: (f32, f32)) -> (f32, f32) {
    (
        (ndc.0 + 1.0) * 0.5 * viewport.0,
        (1.0 - ndc.1) * 0.5 * viewport.1,
    )
}

/// One health bar's color by remaining health (thousandths): green above
/// 60%, amber above 30%, red below.
pub fn health_color(hp_fraction_milli: u32) -> [f32; 4] {
    if hp_fraction_milli > 600 {
        [0.25, 0.85, 0.30, 0.95]
    } else if hp_fraction_milli > 300 {
        [0.95, 0.75, 0.20, 0.95]
    } else {
        [0.90, 0.25, 0.20, 0.95]
    }
}

/// Builds the health-bar quads for every damaged entity in the snapshot
/// (full-health entities draw nothing — a healthy field is not noise).
/// One bar per entity: a dark backing rect and a colored fill whose width
/// is the remaining fraction. Bars sit above the entity's projected head.
pub fn health_bar_quads(
    atlas: &TextAtlas,
    snapshot: &pandemonium_engine::RenderSnapshot,
    camera: &RtsCamera,
    viewport: (f32, f32),
) -> Vec<UiQuad> {
    const BAR_W: f32 = 36.0;
    const BAR_H: f32 = 4.0;
    const LIFT: f32 = 14.0;
    let mut quads = Vec::new();
    for entity in &snapshot.entities {
        if entity.hp_fraction_milli >= 999 {
            continue;
        }
        let ndc = camera.project(glam::Vec3::new(
            entity.pos.x,
            entity.pos.y + 0.6,
            entity.pos.z,
        ));
        if !ndc.x.is_finite() || !ndc.y.is_finite() {
            continue; // behind the camera
        }
        let (cx, top) = ndc_to_pixels((ndc.x, ndc.y), viewport);
        let top = top - LIFT;
        let x = cx - BAR_W * 0.5;
        quads.push(atlas.solid_rect(
            x - 1.0,
            top - 1.0,
            BAR_W + 2.0,
            BAR_H + 2.0,
            [0.0, 0.0, 0.0, 0.6],
        ));
        let fill = BAR_W * entity.hp_fraction_milli as f32 / 1000.0;
        quads.push(atlas.solid_rect(x, top, fill, BAR_H, health_color(entity.hp_fraction_milli)));
    }
    quads
}

/// Builds the command-ping quads: a fading crosshair at each active ping's
/// ground position. The alpha decays linearly with the ping's remaining
/// frames so the marker dissolves instead of popping off.
pub fn ping_quads(
    atlas: &TextAtlas,
    pings: &[(Vec2Fx, u64)],
    camera: &RtsCamera,
    frame: u64,
    viewport: (f32, f32),
) -> Vec<UiQuad> {
    const ARM: f32 = 6.0;
    const THICK: f32 = 1.5;
    let mut quads = Vec::new();
    for (ground, expires_at) in pings {
        let world = logical_to_world(*ground);
        let ndc = camera.project(world);
        if !ndc.x.is_finite() || !ndc.y.is_finite() {
            continue;
        }
        let (cx, cy) = ndc_to_pixels((ndc.x, ndc.y), viewport);
        let remaining = expires_at.saturating_sub(frame) as f32 / PING_FRAMES as f32;
        let alpha = 0.9 * remaining.clamp(0.0, 1.0);
        let color = [1.0, 1.0, 1.0, alpha];
        // A crosshair: two thin rects crossing at the point.
        quads.push(atlas.solid_rect(cx - ARM, cy - THICK * 0.5, 2.0 * ARM, THICK, color));
        quads.push(atlas.solid_rect(cx - THICK * 0.5, cy - ARM, THICK, 2.0 * ARM, color));
    }
    quads
}

/// Builds the selection-quads: corner brackets around each selected
/// entity's projected position (M9.1 — the DEBT-008 human pass found the
/// brightened team color alone did not read as "selected"; the brackets
/// are the persistent selection state the player can trust).
pub fn selection_quads(
    atlas: &TextAtlas,
    selection: &[EntityId],
    snapshot: &RenderSnapshot,
    camera: &RtsCamera,
    viewport: (f32, f32),
) -> Vec<UiQuad> {
    /// Half the bracket box's side, in pixels.
    const HALF: f32 = 11.0;
    /// How far each bracket's arm reaches, in pixels.
    const ARM: f32 = 4.0;
    /// Bracket stroke thickness, in pixels.
    const THICK: f32 = 1.5;
    let selected: std::collections::BTreeSet<EntityId> = selection.iter().copied().collect();
    let mut quads = Vec::new();
    for entity in &snapshot.entities {
        if !selected.contains(&entity.id) {
            continue;
        }
        let ndc = camera.project(entity.pos);
        if !ndc.x.is_finite() || !ndc.y.is_finite() {
            continue; // behind the camera
        }
        let (cx, cy) = ndc_to_pixels((ndc.x, ndc.y), viewport);
        let color = [0.55, 1.0, 0.75, 0.95];
        let (left, right) = (cx - HALF, cx + HALF);
        let (top, bottom) = (cy - HALF, cy + HALF);
        // Four corner brackets, two rects each: an L at every corner.
        quads.push(atlas.solid_rect(left, top, ARM, THICK, color));
        quads.push(atlas.solid_rect(left, top, THICK, ARM, color));
        quads.push(atlas.solid_rect(right - ARM, top, ARM, THICK, color));
        quads.push(atlas.solid_rect(right - THICK, top, THICK, ARM, color));
        quads.push(atlas.solid_rect(left, bottom - THICK, ARM, THICK, color));
        quads.push(atlas.solid_rect(left, bottom - ARM, THICK, ARM, color));
        quads.push(atlas.solid_rect(right - ARM, bottom - THICK, ARM, THICK, color));
        quads.push(atlas.solid_rect(right - THICK, bottom - ARM, THICK, ARM, color));
    }
    quads
}

/// Builds the refusal quads: a fading red square outline at each refused
/// order's click point — the "no" to the ping's "yes" (M9.1).
pub fn refusal_quads(
    atlas: &TextAtlas,
    refusals: &[(Vec2Fx, u64)],
    camera: &RtsCamera,
    frame: u64,
    viewport: (f32, f32),
) -> Vec<UiQuad> {
    /// Half the refusal square's side, in pixels.
    const HALF: f32 = 7.0;
    /// Square stroke thickness, in pixels.
    const THICK: f32 = 1.5;
    let mut quads = Vec::new();
    for (ground, expires_at) in refusals {
        let world = logical_to_world(*ground);
        let ndc = camera.project(world);
        if !ndc.x.is_finite() || !ndc.y.is_finite() {
            continue;
        }
        let (cx, cy) = ndc_to_pixels((ndc.x, ndc.y), viewport);
        let remaining = expires_at.saturating_sub(frame) as f32 / REFUSAL_FRAMES as f32;
        let alpha = 0.9 * remaining.clamp(0.0, 1.0);
        let color = [1.0, 0.3, 0.25, alpha];
        let (left, right) = (cx - HALF, cx + HALF);
        let (top, bottom) = (cy - HALF, cy + HALF);
        // A hollow square: one thin rect per side.
        quads.push(atlas.solid_rect(left, top, 2.0 * HALF, THICK, color));
        quads.push(atlas.solid_rect(left, bottom - THICK, 2.0 * HALF, THICK, color));
        quads.push(atlas.solid_rect(left, top, THICK, 2.0 * HALF, color));
        quads.push(atlas.solid_rect(right - THICK, top, THICK, 2.0 * HALF, color));
    }
    quads
}

/// The player-facing text for a rejection reason (plan §8.2's vocabulary,
/// phrased for the HUD line — data-driven wording would buy nothing here).
pub fn rejection_text(reason: RejectReason) -> &'static str {
    match reason {
        RejectReason::TickMismatch => "the order missed its tick",
        RejectReason::DuplicateSeq => "duplicate order",
        RejectReason::PlayerMissing => "not a player in this match",
        RejectReason::UnknownEntity => "no such unit",
        RejectReason::NotOwnedByIssuer => "not yours to command",
        RejectReason::MissingCapability => "they can't do that",
        RejectReason::UnknownKind => "unknown kind",
        RejectReason::InvalidTarget => "invalid target",
        RejectReason::NotVisible => "target not visible",
        RejectReason::CannotAfford => "not enough Ore",
        RejectReason::PopulationFull => "population full",
        RejectReason::PlacementBlocked => "no room there",
        RejectReason::RequirementsUnmet => "requirements unmet",
        RejectReason::QueueIndexInvalid => "nothing queued there",
    }
}

/// The cursor order-feedback marker's color per hint (M10.2, PLAN §1.3's
/// "draw a small marker for Move / Attack / invalid"): green Move, red
/// Attack, amber Gather, blue rally, orange attack-move — and no marker at
/// all when nothing is orderable (`None`: the plan's "invalid" reads
/// cleanest as silence; a permanent gray marker under a cursor with
/// nothing selected would be noise). The colors avoid the red/green
/// confusion pair by value as well as hue (the plan's §2.2 color-blind
/// note, applied early where cheap).
pub fn cursor_marker_color(hint: CursorOrderHint) -> Option<[f32; 4]> {
    match hint {
        CursorOrderHint::Move => Some([0.35, 1.0, 0.45, 0.9]),
        CursorOrderHint::Attack => Some([1.0, 0.35, 0.30, 0.9]),
        CursorOrderHint::Gather => Some([1.0, 0.78, 0.30, 0.9]),
        CursorOrderHint::Rally => Some([0.45, 0.75, 1.0, 0.9]),
        CursorOrderHint::AttackMove => Some([1.0, 0.55, 0.25, 0.9]),
        CursorOrderHint::None => None,
    }
}

/// Builds the cursor's order-feedback marker quads (M10.2, PLAN §1.3): a
/// small filled square inside a hollow outline, drawn at the ground point
/// under the cursor (it sticks to the world, not the pixel), in the hint's
/// color. Nothing orderable or no ground under the cursor draws nothing.
pub fn cursor_marker_quads(
    atlas: &TextAtlas,
    hint: CursorOrderHint,
    ground: Option<glam::Vec3>,
    camera: &RtsCamera,
    viewport: (f32, f32),
) -> Vec<UiQuad> {
    /// Half the outline square's side, in pixels.
    const HALF: f32 = 8.0;
    /// Outline stroke thickness, in pixels.
    const THICK: f32 = 1.5;
    /// Half the filled center square's side, in pixels.
    const CENTER: f32 = 3.0;
    let Some(color) = cursor_marker_color(hint) else {
        return Vec::new();
    };
    let Some(ground) = ground else {
        return Vec::new();
    };
    let ndc = camera.project(ground);
    if !ndc.x.is_finite() || !ndc.y.is_finite() {
        return Vec::new();
    }
    let (cx, cy) = ndc_to_pixels((ndc.x, ndc.y), viewport);
    let (left, right) = (cx - HALF, cx + HALF);
    let (top, bottom) = (cy - HALF, cy + HALF);
    let mut quads =
        vec![atlas.solid_rect(cx - CENTER, cy - CENTER, 2.0 * CENTER, 2.0 * CENTER, color)];
    // The hollow outline: one thin rect per side.
    quads.push(atlas.solid_rect(left, top, 2.0 * HALF, THICK, color));
    quads.push(atlas.solid_rect(left, bottom - THICK, 2.0 * HALF, THICK, color));
    quads.push(atlas.solid_rect(left, top, THICK, 2.0 * HALF, color));
    quads.push(atlas.solid_rect(right - THICK, top, THICK, 2.0 * HALF, color));
    quads
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::CursorOrderHint;

    #[test]
    fn the_cursor_marker_colors_name_the_order_and_none_is_silent() {
        assert_eq!(
            cursor_marker_color(CursorOrderHint::Move),
            Some([0.35, 1.0, 0.45, 0.9])
        );
        assert_eq!(
            cursor_marker_color(CursorOrderHint::Attack),
            Some([1.0, 0.35, 0.30, 0.9])
        );
        assert_eq!(
            cursor_marker_color(CursorOrderHint::Gather),
            Some([1.0, 0.78, 0.30, 0.9])
        );
        assert_eq!(
            cursor_marker_color(CursorOrderHint::Rally),
            Some([0.45, 0.75, 1.0, 0.9])
        );
        assert_eq!(
            cursor_marker_color(CursorOrderHint::AttackMove),
            Some([1.0, 0.55, 0.25, 0.9])
        );
        assert_eq!(cursor_marker_color(CursorOrderHint::None), None);
    }

    #[test]
    fn the_cursor_marker_draws_five_quads_at_the_ground_point() {
        let mut camera = RtsCamera::new(16, 16, 16.0 / 9.0);
        camera.focus(8.0, 8.0, 20.0);
        let atlas = TextAtlas::new();
        let ground = Some(glam::Vec3::new(8.0, 0.0, 8.0));
        let quads = cursor_marker_quads(
            &atlas,
            CursorOrderHint::Move,
            ground,
            &camera,
            (1920.0, 1080.0),
        );
        // One filled center + four outline sides, all in the hint color.
        assert_eq!(quads.len(), 5);
        assert_eq!(quads[0].color, [0.35, 1.0, 0.45, 0.9]);
        assert_eq!(quads[1].color, [0.35, 1.0, 0.45, 0.9]);
        // The center square is 6 px wide, the outline rects 16 px.
        assert!((quads[0].w - 6.0).abs() < 1e-3);
        assert!((quads[1].w - 16.0).abs() < 1e-3);
    }

    #[test]
    fn the_cursor_marker_is_silent_when_nothing_is_orderable_or_off_ground() {
        let mut camera = RtsCamera::new(16, 16, 16.0 / 9.0);
        camera.focus(8.0, 8.0, 20.0);
        let atlas = TextAtlas::new();
        let ground = Some(glam::Vec3::new(8.0, 0.0, 8.0));
        // Nothing orderable: no marker even over ground.
        assert!(cursor_marker_quads(
            &atlas,
            CursorOrderHint::None,
            ground,
            &camera,
            (1920.0, 1080.0)
        )
        .is_empty());
        // No ground under the cursor (above the horizon): no marker.
        assert!(cursor_marker_quads(
            &atlas,
            CursorOrderHint::Move,
            None,
            &camera,
            (1920.0, 1080.0)
        )
        .is_empty());
    }

    #[test]
    fn ndc_to_pixels_maps_the_corners_and_center() {
        // The viewport is (width, height) in pixels; NDC y is up, pixel y
        // is down from the top-left.
        let vp = (1920.0, 1080.0);
        assert_eq!(ndc_to_pixels((-1.0, -1.0), vp), (0.0, 1080.0));
        assert_eq!(ndc_to_pixels((1.0, 1.0), vp), (1920.0, 0.0));
        let (cx, cy) = ndc_to_pixels((0.0, 0.0), vp);
        assert!((cx - 960.0).abs() < 1e-3 && (cy - 540.0).abs() < 1e-3);
    }

    #[test]
    fn health_color_has_three_bands() {
        assert_eq!(health_color(1000), [0.25, 0.85, 0.30, 0.95]);
        assert_eq!(health_color(601), [0.25, 0.85, 0.30, 0.95]);
        assert_eq!(health_color(600), [0.95, 0.75, 0.20, 0.95]);
        assert_eq!(health_color(301), [0.95, 0.75, 0.20, 0.95]);
        assert_eq!(health_color(300), [0.90, 0.25, 0.20, 0.95]);
        assert_eq!(health_color(1), [0.90, 0.25, 0.20, 0.95]);
    }

    #[test]
    fn flashes_collect_from_events_and_expire() {
        let mut state = FeedbackState::default();
        let events = [Event::AttackHit {
            attacker: EntityId(1),
            target: EntityId(7),
            damage: 8,
        }];
        state.on_events(&events, 100);
        assert_eq!(state.hits_seen, 1);
        assert!(state.is_flashing(EntityId(7)));
        assert!(!state.is_flashing(EntityId(1)));
        // Flash expiry: at the expiry frame it is gone.
        state.expire(100 + FLASH_FRAMES);
        assert!(!state.is_flashing(EntityId(7)));
    }

    #[test]
    fn pings_expire_and_count() {
        let mut state = FeedbackState::default();
        state.ping(Vec2Fx::from_ints(10, 10), 0);
        state.ping(Vec2Fx::from_ints(20, 20), 5);
        assert_eq!(state.pings_issued, 2);
        assert_eq!(state.active_pings().len(), 2);
        state.expire(PING_FRAMES); // the first (expiry frame 45) is gone
        assert_eq!(state.active_pings().len(), 1);
        assert_eq!(state.active_pings()[0].0, Vec2Fx::from_ints(20, 20));
    }

    #[test]
    fn health_bars_skip_full_health_and_fill_by_fraction() {
        use pandemonium_engine::{RenderEntity, RenderSnapshot};
        // A camera looking straight down over the origin (the default RtsCamera
        // targets the map center; use a tiny map so the origin is in view).
        let camera = RtsCamera::new(4, 4, 16.0 / 9.0);
        let atlas = TextAtlas::new();
        let damaged = RenderEntity {
            id: EntityId(1),
            owner: pandemonium_sim_api::PlayerId(0),
            kind: pandemonium_sim_api::KindId(0),
            pos: glam::Vec3::new(1.0, 0.0, 1.0),
            facing: glam::Vec3::ZERO,
            hp_fraction_milli: 500,
            move_state: pandemonium_sim_api::MoveState::Idle,
        };
        let healthy = RenderEntity {
            hp_fraction_milli: 1000,
            ..damaged
        };
        let snapshot = RenderSnapshot {
            tick: 0,
            entities: vec![damaged, healthy],
        };
        let quads = health_bar_quads(&atlas, &snapshot, &camera, (1920.0, 1080.0));
        // One damaged entity: a backing rect + a fill rect; the healthy one
        // contributes nothing.
        assert_eq!(quads.len(), 2, "backing + fill for the damaged entity only");
        // The fill is half the bar width, the backing is the full bar plus
        // its border. Order: backing first, then fill.
        assert!((quads[0].w - 38.0).abs() < 1e-3, "backing: {:?}", quads[0]);
        assert!(
            (quads[1].w - 18.0).abs() < 1e-3,
            "half-health fill: {:?}",
            quads[1]
        );
    }

    #[test]
    fn selection_brackets_mark_only_the_selected() {
        use pandemonium_engine::{RenderEntity, RenderSnapshot};
        use pandemonium_sim_api::MoveState;
        let mut camera = RtsCamera::new(16, 16, 16.0 / 9.0);
        camera.focus(8.0, 8.0, 20.0);
        let atlas = TextAtlas::new();
        let at = |id: u64| RenderEntity {
            id: EntityId(id),
            owner: pandemonium_sim_api::PlayerId(0),
            kind: pandemonium_sim_api::KindId(0),
            pos: glam::Vec3::new(8.0, 0.0, 8.0),
            facing: glam::Vec3::ZERO,
            hp_fraction_milli: 1000,
            move_state: MoveState::Idle,
        };
        let snapshot = RenderSnapshot {
            tick: 0,
            entities: vec![at(1), at(2)],
        };
        // One selected entity: eight bracket rects (four corners, two rects
        // per corner). The unselected one contributes nothing.
        let quads = selection_quads(&atlas, &[EntityId(1)], &snapshot, &camera, (1920.0, 1080.0));
        assert_eq!(quads.len(), 8, "one entity's four corner brackets");
        assert!(
            selection_quads(&atlas, &[], &snapshot, &camera, (1920.0, 1080.0)).is_empty(),
            "no selection, no brackets"
        );
        // A selection holding a dead/absent id draws nothing for it.
        let stale = selection_quads(
            &atlas,
            &[EntityId(1), EntityId(77)],
            &snapshot,
            &camera,
            (1920.0, 1080.0),
        );
        assert_eq!(stale.len(), 8, "the absent id is skipped silently");
    }

    #[test]
    fn refusals_collect_expire_and_carry_the_reason_text() {
        let mut state = FeedbackState::default();
        state.refuse(Vec2Fx::from_ints(10, 10), RejectReason::CannotAfford, 100);
        assert_eq!(state.refusals_seen, 1);
        assert_eq!(state.active_refusals().len(), 1);
        let (text, expires_at) = state.refusal_notice.as_ref().expect("notice set");
        assert_eq!(text, "order refused: not enough Ore");
        assert_eq!(*expires_at, 100 + REFUSAL_FRAMES);
        // A later refusal replaces the notice (the HUD shows one line).
        state.refuse(Vec2Fx::from_ints(20, 20), RejectReason::NotVisible, 110);
        let (text, _) = state.refusal_notice.as_ref().expect("notice set");
        assert_eq!(text, "order refused: target not visible");
        // Expiry drops both the squares and the notice.
        state.expire(110 + REFUSAL_FRAMES);
        assert!(state.active_refusals().is_empty());
        assert!(state.refusal_notice.is_none());
    }

    #[test]
    fn refusal_quads_draw_a_hollow_square_per_refusal() {
        let mut camera = RtsCamera::new(16, 16, 16.0 / 9.0);
        camera.focus(8.0, 8.0, 20.0);
        let atlas = TextAtlas::new();
        let refusals = [(Vec2Fx::from_ints(8, 8), 60)];
        let quads = refusal_quads(&atlas, &refusals, &camera, 0, (1920.0, 1080.0));
        assert_eq!(quads.len(), 4, "one rect per side of the square");
        // All four rects share the fading red.
        assert_eq!(quads[0].color, [1.0, 0.3, 0.25, 0.9]);
    }

    #[test]
    fn every_rejection_reason_has_player_facing_text() {
        // The mapping must be total — a new RejectReason variant that
        // forgets its player text fails to compile (the match is
        // exhaustive); this test pins the wording for the HUD's sake.
        assert_eq!(
            rejection_text(RejectReason::NotVisible),
            "target not visible"
        );
        assert_eq!(
            rejection_text(RejectReason::MissingCapability),
            "they can't do that"
        );
        assert_eq!(rejection_text(RejectReason::UnknownEntity), "no such unit");
    }
}

#[cfg(test)]
mod death_tests {
    use super::*;
    use pandemonium_sim_api::PlayerId;

    #[test]
    fn a_death_cue_fades_and_expires() {
        let mut feedback = FeedbackState::default();
        feedback.death(glam::Vec3::new(3.0, 0.0, 4.0), KindId(1), PlayerId(0), 100);
        assert_eq!(feedback.deaths_seen, 1);
        let cue = feedback.active_deaths()[0];
        assert_eq!(cue.pos, glam::Vec3::new(3.0, 0.0, 4.0));
        assert_eq!(cue.expires_at, 100 + DEATH_FRAMES);
        // Still alive mid-fade, gone after the window.
        feedback.expire(100 + DEATH_FRAMES - 1);
        assert_eq!(feedback.active_deaths().len(), 1);
        feedback.expire(100 + DEATH_FRAMES);
        assert!(feedback.active_deaths().is_empty());
    }

    #[test]
    fn deaths_survive_the_unrelated_expiry_sweeps() {
        let mut feedback = FeedbackState::default();
        feedback.death(glam::Vec3::ZERO, KindId(0), PlayerId(1), 0);
        // A few hundred frames of unrelated expire calls (pings, refusals).
        for frame in 0..(DEATH_FRAMES / 2) {
            feedback.expire(frame);
        }
        assert_eq!(feedback.active_deaths().len(), 1, "mid-fade deaths persist");
    }
}
