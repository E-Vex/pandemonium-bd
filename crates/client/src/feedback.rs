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

use crate::text::{TextAtlas, UiQuad};
use pandemonium_engine::{logical_to_world, RtsCamera};
use pandemonium_sim_api::{EntityId, Event, Vec2Fx};

/// How many presented frames a hit flash lasts (a third of a second at the
/// client's frame rate).
pub const FLASH_FRAMES: u64 = 18;

/// How many presented frames a command ping lasts (three quarters of a
/// second — enough to confirm the click, not enough to clutter).
pub const PING_FRAMES: u64 = 45;

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
    /// Total `AttackHit` events seen (the smoke summary's evidence).
    pub hits_seen: u32,
    /// Total command pings issued.
    pub pings_issued: u32,
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

    /// Drops expired flashes and pings. Called once per presented frame.
    pub fn expire(&mut self, frame: u64) {
        self.flashes.retain(|(_, expires_at)| *expires_at > frame);
        self.pings.retain(|(_, expires_at)| *expires_at > frame);
    }

    /// Whether an entity is flashing this frame (the renderer tints it).
    pub fn is_flashing(&self, entity: EntityId) -> bool {
        self.flashes.iter().any(|(id, _)| *id == entity)
    }

    /// The active command pings, oldest first.
    pub fn active_pings(&self) -> &[(Vec2Fx, u64)] {
        &self.pings
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
