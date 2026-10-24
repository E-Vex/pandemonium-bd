//! Input-layer pure logic (M10.2, PLAN §1.2 — the Generals ZH camera
//! controls): the right-button state machine that disambiguates "command
//! click" from "camera drag", the depth-scaled edge-scroll band, and the
//! middle-drag rotation rate.
//!
//! Everything here is pure data-in/data-out over pixels and (for the
//! direction-pinned tests) the engine's `RtsCamera` — the same
//! module-free testability discipline as `orders.rs` and `feedback.rs`.
//! The `main.rs` event handlers stay thin: they translate winit events
//! into calls here and act on the returned decision. No simulation state
//! is read or written; the command gate stays the sole authority on
//! legality (FD-2).
//!
//! Direction conventions are pinned by tests at yaw 0 and yaw 90 (the M9.1
//! lesson: presentation direction is invisible to the golden hashes, so
//! only tests can keep it honest).

pub(crate) mod actions;
pub(crate) mod event;
pub(crate) mod orders;

use pandemonium_engine::RenderEntity;
use pandemonium_sim_api::{EntityId, KindId};

use crate::HUMAN;

/// How far (in pixels) the cursor may travel between a right-button press
/// and its release for the gesture to count as a command click rather than
/// a camera drag (PLAN §1.2: "less than about 6 pixels of movement =
/// command; movement beyond the threshold = camera scroll and NO command on
/// release"). One named constant, deliberately small: a command click is a
/// click, not a small drag.
pub const RIGHT_DRAG_COMMAND_MAX_PX: f64 = 6.0;

/// How wide (in pixels) the screen-edge scroll band is (PLAN §1.2: "about
/// 12–16 px"). The old M9.1 behavior was a 24 px zone that switched on at
/// full speed the instant the cursor crossed it — the owner read it as
/// "only the very last pixel works", so the band is narrower and the speed
/// now *scales with depth* into it ([`edge_scroll_vector`]).
pub const EDGE_SCROLL_BAND_PX: f64 = 14.0;

/// How much camera yaw one pixel of middle-drag horizontal travel is worth
/// (radians per pixel). A full turn is `TAU / MIDDLE_DRAG_RAD_PER_PX` ≈
/// 1257 px of drag — slow enough to aim, fast enough to re-orient.
pub const MIDDLE_DRAG_RAD_PER_PX: f32 = 0.005;

/// The right button's disambiguation state (PLAN §1.2). The right button is
/// overloaded: a press-and-release with almost no travel issues the context
/// command; travel beyond [`RIGHT_DRAG_COMMAND_MAX_PX`] becomes the
/// Generals-style grab-and-drag map scroll, and the release then orders
/// nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RightButton {
    /// Not pressed.
    Idle,
    /// Pressed, but the cursor has not yet left the command threshold —
    /// this still might become a command click.
    Pressed {
        /// The press position in window pixels.
        start: (f64, f64),
    },
    /// Pressed and past the threshold — this gesture is a camera drag now,
    /// and its release must not issue a command.
    Dragging {
        /// The press position in window pixels.
        start: (f64, f64),
    },
}

impl RightButton {
    /// A fresh, unpressed state.
    pub fn new() -> Self {
        Self::Idle
    }

    /// The button went down at `at` (window pixels).
    pub fn press(&mut self, at: (f64, f64)) {
        *self = Self::Pressed { start: at };
    }

    /// The cursor moved while the button is held. Returns `true` on the
    /// single event where the gesture crosses the threshold into a drag
    /// (the caller starts grab-scrolling there); the caller is also free to
    /// poll [`RightButton::is_dragging`] per move.
    pub fn moved(&mut self, at: (f64, f64)) -> bool {
        let Self::Pressed { start } = *self else {
            return false; // idle, or already a drag
        };
        if pixel_distance(start, at) > RIGHT_DRAG_COMMAND_MAX_PX {
            *self = Self::Dragging { start };
            true
        } else {
            false
        }
    }

    /// The button came up at `at` (window pixels). `None` when the button
    /// was not pressed (the press was swallowed, e.g. by a placement
    /// cancel) or the gesture was already over (a stray release).
    pub fn release(&mut self, at: (f64, f64)) -> Option<RightRelease> {
        match *self {
            Self::Idle => None,
            Self::Pressed { start } => {
                *self = Self::Idle;
                Some(if pixel_distance(start, at) <= RIGHT_DRAG_COMMAND_MAX_PX {
                    RightRelease::Command
                } else {
                    RightRelease::Scroll
                })
            }
            Self::Dragging { .. } => {
                *self = Self::Idle;
                Some(RightRelease::Scroll)
            }
        }
    }

    /// Whether the held gesture is past the threshold (a camera drag).
    pub fn is_dragging(&self) -> bool {
        matches!(self, Self::Dragging { .. })
    }
}

impl Default for RightButton {
    fn default() -> Self {
        Self::new()
    }
}

/// What a right-button release means (see [`RightButton::release`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RightRelease {
    /// Press-to-release travel stayed under the threshold: this is the
    /// command click — issue the context order at the release position.
    Command,
    /// The gesture was (or became) a camera drag: order nothing.
    Scroll,
}

/// Euclidean pixel distance between two window positions.
pub fn pixel_distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (a.0 - b.0, a.1 - b.1);
    (dx * dx + dy * dy).sqrt()
}

/// The depth-scaled edge-scroll direction (PLAN §1.2) for a cursor at
/// `cursor` (window pixels) in a `window`-sized viewport. Each component of
/// the returned pair is a signed fraction of full pan speed (−1..1):
/// positive `x` pans screen-right, positive `z` pans screen-up — the same
/// signs the arrow-key pan uses. The magnitude grows linearly with the
/// cursor's depth into the [`EDGE_SCROLL_BAND_PX`] band, so the cursor at
/// the window edge scrolls at full speed and halfway into the band at half
/// speed. All four edges work and corners combine both axes.
///
/// The middle of the window returns `(0, 0)`, and so does any window too
/// small to have a middle (the caller never sees NaN).
pub fn edge_scroll_vector(cursor: (f64, f64), window: (f64, f64)) -> (f32, f32) {
    let band = EDGE_SCROLL_BAND_PX;
    let depth = |distance_from_edge: f64| -> f32 {
        (1.0 - (distance_from_edge / band).clamp(0.0, 1.0)) as f32
    };
    let (w, h) = window;
    if w <= 0.0 || h <= 0.0 {
        return (0.0, 0.0);
    }
    let mut x = 0.0;
    let mut z = 0.0;
    // Left edge: depth grows as the cursor approaches x = 0.
    x -= depth(cursor.0);
    // Right edge: depth grows as the cursor approaches x = w.
    x += depth(w - 1.0 - cursor.0);
    // Top edge: depth grows as the cursor approaches y = 0; panning
    // "up-screen" is +z (forward), matching the arrow-key convention.
    z += depth(cursor.1);
    // Bottom edge.
    z -= depth(h - 1.0 - cursor.1);
    (x, z)
}

/// The edge band's actual pan contribution for one frame (PLAN §1.2's
/// band + §3.4's toggle): the depth-scaled vector when edge scrolling is
/// enabled AND the window has focus AND a match owns the input; zero
/// otherwise (a menu never scrolls its own backdrop, and an unfocused game
/// never steals the desktop). DEBT-014's repayment: the flag the settings
/// screen flips is this function's first argument.
pub fn edge_pan_contribution(
    enabled: bool,
    focused: bool,
    in_match: bool,
    cursor: (f64, f64),
    window: (f64, f64),
) -> (f32, f32) {
    if enabled && focused && in_match {
        edge_scroll_vector(cursor, window)
    } else {
        (0.0, 0.0)
    }
}

/// How long (in milliseconds) two left clicks may sit apart and still count
/// as one double click (the same-kind select-all of PLAN §1.3). Generals
/// uses a similar few-hundred-millisecond window; long enough for a
/// deliberate double press, short enough that two unrelated clicks never
/// combine.
pub const DOUBLE_CLICK_MS: u64 = 350;

/// Whether two clicks `elapsed_ms` apart count as one double click
/// ([`DOUBLE_CLICK_MS`] is the window).
pub fn is_double_click(elapsed_ms: u64) -> bool {
    elapsed_ms <= DOUBLE_CLICK_MS
}

/// Shift+click on a picked entity (PLAN-M11 §2: **add-only** — the M10.2
/// toggle-remove is reversed): the picked id joins the selection when
/// absent; a member is never removed (removal is a re-select, per the
/// card). Order is otherwise preserved.
pub fn shift_click_selection(current: &[EntityId], picked: EntityId) -> Vec<EntityId> {
    let mut next = current.to_vec();
    if !next.contains(&picked) {
        next.push(picked);
    }
    next
}

/// Shift+box selection (PLAN §1.3: "shift+box adds"): the box's picked ids
/// join the current selection, keeping the current order first and the box's
/// ascending snapshot order after; ids already selected are not duplicated.
pub fn union_selection(current: &[EntityId], picked: &[EntityId]) -> Vec<EntityId> {
    let mut next = current.to_vec();
    for id in picked {
        if !next.contains(id) {
            next.push(*id);
        }
    }
    next
}

/// Double-click on a picked entity (PLAN-M11 §2: "select same-type units
/// on screen"): every snapshot entity of the clicked entity's kind owned
/// by the same player, **limited to the screen** (`on_screen` decides),
/// in snapshot order. The snapshot the caller passes is the fog-filtered
/// view, so "visible" is already the truth. `None` when the clicked
/// entity is not in the snapshot (nothing to expand).
///
/// (The M10.2 form was whole-visible-map; PLAN-M11 §2's table pins the
/// ZH target — same-type *on screen* — so the screen filter is the
/// truth-pass fix, recorded in the report's audit deltas.)
pub fn same_kind_selection(
    entities: &[RenderEntity],
    clicked: EntityId,
    on_screen: impl Fn(&RenderEntity) -> bool,
) -> Option<Vec<EntityId>> {
    let clicked = entities.iter().find(|entity| entity.id == clicked)?;
    let (owner, kind) = (clicked.owner, clicked.kind);
    Some(
        entities
            .iter()
            .filter(|entity| entity.owner == owner && entity.kind == kind && on_screen(entity))
            .map(|entity| entity.id)
            .collect(),
    )
}

/// How long (in milliseconds) two presses of the same control-group digit
/// may sit apart and still count as one double tap (the group-centering
/// camera jump of PLAN §1.3).
pub const DOUBLE_TAP_MS: u64 = 350;

/// Whether two presses of the same control-group digit `elapsed_ms` apart
/// count as one double tap ([`DOUBLE_TAP_MS`] is the window).
pub fn is_double_tap(elapsed_ms: u64) -> bool {
    elapsed_ms <= DOUBLE_TAP_MS
}

/// The ground-plane center (world x, z) of a control group's live members —
/// the double-tap camera jump's target (PLAN §1.3). Dead or absent ids are
/// skipped (a stale group reference is harmless); `None` when no member is
/// in the snapshot.
pub fn group_center(entities: &[RenderEntity], group: &[EntityId]) -> Option<(f32, f32)> {
    let mut sum = (0.0f32, 0.0f32);
    let mut count = 0u32;
    for id in group {
        if let Some(entity) = entities.iter().find(|entity| entity.id == *id) {
            sum.0 += entity.pos.x;
            sum.1 += entity.pos.z;
            count += 1;
        }
    }
    (count > 0).then(|| (sum.0 / count as f32, sum.1 / count as f32))
}

/// E's kind rule (PLAN-M11 §2): a uniform selection names its own kind; an
/// empty or mixed selection falls back to the most recently *clicked*
/// member's kind (the caller records it on every pick). `None` when no
/// kind can be named — E is inert then (the card says so).
pub fn same_kind_key_kind(
    entities: &[RenderEntity],
    selection: &[EntityId],
    last_clicked_kind: Option<KindId>,
) -> Option<KindId> {
    let mut kinds = selection.iter().filter_map(|id| {
        entities
            .iter()
            .find(|entity| entity.id == *id)
            .map(|entity| entity.kind)
    });
    match kinds.next() {
        // An empty selection (or one fully absent from the snapshot —
        // dead ids): the clicked-kind rule.
        None => last_clicked_kind,
        Some(first) if kinds.all(|kind| kind == first) => Some(first),
        // Mixed: the clicked-kind rule.
        _ => last_clicked_kind,
    }
}

/// E's target set (PLAN-M11 §2): every own entity of the named kind, on
/// screen for a single tap, the whole visible map for a double tap.
/// `None` when the set would be empty (an inert press keeps the
/// selection — pressing E must never drop it to nothing).
pub fn same_kind_key_selection(
    entities: &[RenderEntity],
    kind: KindId,
    on_screen: impl Fn(&RenderEntity) -> bool,
    whole_map: bool,
) -> Option<Vec<EntityId>> {
    let picked: Vec<EntityId> = entities
        .iter()
        .filter(|entity| {
            entity.owner == HUMAN && entity.kind == kind && (whole_map || on_screen(entity))
        })
        .map(|entity| entity.id)
        .collect();
    (!picked.is_empty()).then_some(picked)
}

/// Q's target set (PLAN-M11 §2): every own *combat* unit — on screen for
/// a single tap, the whole visible map for a double tap. `None` when the
/// set would be empty (inert, like E).
pub fn combat_key_selection(
    entities: &[RenderEntity],
    is_combat: impl Fn(&RenderEntity) -> bool,
    on_screen: impl Fn(&RenderEntity) -> bool,
    whole_map: bool,
) -> Option<Vec<EntityId>> {
    let picked: Vec<EntityId> = entities
        .iter()
        .filter(|entity| {
            entity.owner == HUMAN && is_combat(entity) && (whole_map || on_screen(entity))
        })
        .map(|entity| entity.id)
        .collect();
    (!picked.is_empty()).then_some(picked)
}

/// Ctrl+Left/Right and Ctrl+Up/Down (PLAN-M11 §2): the next or previous
/// own unit in ascending-id order, wrapping — the candidate list is the
/// fog-filtered view ("on map") filtered by `candidate` (the unit cycle
/// passes the movable kinds; the worker cycle passes the worker kind
/// alone). With nothing selected (or no selection member among the
/// candidates) the lowest-id candidate is selected; otherwise the unit
/// after the selection's highest-id candidate becomes the sole selection
/// (ZH's cycle walks one unit at a time). `None` when no candidate
/// exists at all.
pub fn cycled_selection(
    entities: &[RenderEntity],
    selection: &[EntityId],
    candidate: impl Fn(&RenderEntity) -> bool,
    forward: bool,
) -> Option<Vec<EntityId>> {
    // The candidates, ascending by id (the snapshot is already ascending;
    // sort defensively so the order is the function's own contract).
    let mut candidates: Vec<EntityId> = entities
        .iter()
        .filter(|entity| entity.owner == HUMAN && candidate(entity))
        .map(|entity| entity.id)
        .collect();
    candidates.sort();
    let first = *candidates.first()?;

    // The anchor: the highest-id candidate that is currently selected.
    // Nothing selected (or no selection member among the candidates — a
    // structure selected while worker-cycling): the lowest id, whichever
    // direction was pressed (PLAN-M11 §2: "with nothing selected, select
    // the lowest-id own unit").
    let next = match candidates
        .iter()
        .rev()
        .find(|id| selection.contains(id))
        .copied()
    {
        None => first,
        Some(anchor) => {
            if forward {
                // The smallest candidate id above the anchor, wrapping.
                candidates
                    .iter()
                    .find(|id| **id > anchor)
                    .copied()
                    .unwrap_or(first)
            } else {
                // The largest candidate id below the anchor, wrapping.
                candidates
                    .iter()
                    .rev()
                    .find(|id| **id < anchor)
                    .copied()
                    .unwrap_or(*candidates.last()?)
            }
        }
    };
    Some(vec![next])
}

/// H's jump target (PLAN-M11 §2): the first own Command Center in
/// snapshot order (ascending id). `None` when the kind is unknown or no
/// own CC is visible — the caller falls back to the base anchor.
pub fn command_center_position(
    entities: &[RenderEntity],
    command_center_kind: Option<KindId>,
) -> Option<(f32, f32)> {
    let kind = command_center_kind?;
    entities
        .iter()
        .find(|entity| entity.owner == HUMAN && entity.kind == kind)
        .map(|entity| (entity.pos.x, entity.pos.z))
}

/// X's scatter offsets (PLAN-M11 §2 phase 3): one world-space target per
/// selected unit, each its current position pushed along ray
/// `index mod 8` of an 8-ray ring — the index is the unit's position in
/// the ASCENDING-id order of the selection, so the same selection always
/// produces the same command list (deterministic, no RNG; the client
/// computes, the sim receives Moves). Units already moving receive it too
/// (the Move replaces, `queue: false` — the caller's flag).
pub fn scatter_targets(
    entities: &[RenderEntity],
    selection: &[EntityId],
) -> Vec<(EntityId, f32, f32)> {
    // Ascending-id order (the snapshot is already ascending; sort is the
    // function's own contract).
    let mut ids: Vec<EntityId> = selection.to_vec();
    ids.sort();
    let mut targets = Vec::with_capacity(ids.len());
    for (index, id) in ids.iter().enumerate() {
        let Some(entity) = entities.iter().find(|entity| entity.id == *id) else {
            continue; // a dead id weighs nothing
        };
        // Ray `index mod 8` around the ring: ray 0 is world +x, each next
        // ray 45 degrees on (the world x/z plane).
        let angle = (index % 8) as f32 * (std::f32::consts::TAU / 8.0);
        let (dx, dz) = (angle.cos(), angle.sin());
        targets.push((
            *id,
            entity.pos.x + dx * SCATTER_DISTANCE,
            entity.pos.z + dz * SCATTER_DISTANCE,
        ));
    }
    targets
}

/// How far X's scatter pushes each unit, in world tiles — one
/// acquisition-range-ish hop (the Alpha's acquire ranges run 5-9 tiles;
/// 3 is a visible, formation-preserving burst that never throws a unit
/// out of its own sight). Named and tuned once (PLAN-M11 §4 phase 3).
pub const SCATTER_DISTANCE: f32 = 3.0;

/// What Escape does first (PLAN §1.3: "Escape cancels an armed command or
/// clears the selection"): the cancellation ladder, one rung at a time per
/// press — armed command, placement, selection, then the pause menu
/// (PLAN-M10.2 §3.5: the exhausted ladder used to quit the application
/// mid-match; an accidental Esc with nothing selected cost the player
/// their window. It opens the pause menu now — Quit to Menu lives there).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscapeAction {
    /// An armed attack-move was pending — cancel it.
    CancelArmed,
    /// A structure placement was pending — cancel it.
    CancelPlacement,
    /// Something is selected — clear the selection.
    ClearSelection,
    /// Nothing armed, placing, or selected — open the pause menu.
    OpenPauseMenu,
}

/// Resolves one Escape press to its next action, in ladder order.
pub fn escape_action(armed: bool, placing: bool, selection_nonempty: bool) -> EscapeAction {
    if armed {
        EscapeAction::CancelArmed
    } else if placing {
        EscapeAction::CancelPlacement
    } else if selection_nonempty {
        EscapeAction::ClearSelection
    } else {
        EscapeAction::OpenPauseMenu
    }
}

/// Space's jump target (PLAN §1.3: "jump to the last event or base"): the
/// last event's ground position when one is known, else the player's start
/// anchor. The caller decides which event class it tracks.
pub fn jump_target(last_event: Option<(f32, f32)>, start_anchor: (f32, f32)) -> (f32, f32) {
    last_event.unwrap_or(start_anchor)
}

/// What the cursor's order-feedback marker shows (PLAN §1.3: "change the
/// cursor or draw a small marker for Move / Attack / invalid"): the
/// would-be order for the selection under the cursor, so the player sees
/// what the next right-click does before pressing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorOrderHint {
    /// Open ground under a live selection: Move.
    Move,
    /// An enemy under the cursor: Attack.
    Attack,
    /// A resource node under the cursor: Gather.
    Gather,
    /// A single selected producer over ground: rally point.
    Rally,
    /// Attack-move is armed: the next left-click attacks into that ground.
    AttackMove,
    /// Nothing orderable (empty selection, placement mode owns the cursor,
    /// or no ground under the cursor).
    None,
}

/// Resolves the cursor hint from the same context-order resolution the
/// right-click uses. Armed attack-move takes precedence (the armed flow
/// owns the next left-click); placement mode and an empty selection
/// suppress the marker (the ghost is the placement marker; nothing
/// selected means nothing to preview).
pub fn cursor_order_hint(
    order: Option<crate::input::orders::ContextOrder>,
    rally: bool,
    armed: bool,
    placing: bool,
    selection_empty: bool,
) -> CursorOrderHint {
    if selection_empty || placing {
        return CursorOrderHint::None;
    }
    if armed {
        return CursorOrderHint::AttackMove;
    }
    if rally {
        return CursorOrderHint::Rally;
    }
    match order {
        Some(crate::input::orders::ContextOrder::Attack { .. }) => CursorOrderHint::Attack,
        Some(crate::input::orders::ContextOrder::Gather { .. }) => CursorOrderHint::Gather,
        Some(crate::input::orders::ContextOrder::Move { .. }) => CursorOrderHint::Move,
        None => CursorOrderHint::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandemonium_engine::RtsCamera;
    use pandemonium_sim_api::{KindId, MoveState, PlayerId};

    // ---- right-button state machine -------------------------------------

    #[test]
    fn a_short_press_release_stays_a_command() {
        let mut right = RightButton::new();
        right.press((100.0, 100.0));
        // A tiny jitter stays under the threshold.
        right.moved((103.0, 101.0));
        assert_eq!(right.release((102.5, 99.0)), Some(RightRelease::Command));
        assert_eq!(right, RightButton::Idle);
    }

    #[test]
    fn a_release_exactly_at_the_threshold_is_still_a_command() {
        // "less than about 6 pixels" — exactly 6 is the boundary and stays
        // a command (the drag must be unambiguous).
        let mut right = RightButton::new();
        right.press((10.0, 10.0));
        assert_eq!(right.release((10.0, 16.0)), Some(RightRelease::Command));
    }

    #[test]
    fn travel_beyond_the_threshold_becomes_a_scroll_and_orders_nothing() {
        let mut right = RightButton::new();
        right.press((100.0, 100.0));
        let crossed = right.moved((120.0, 100.0));
        assert!(crossed, "the crossing event reports the drag start");
        assert!(right.is_dragging());
        // More travel, a lift, even a return to the press point: the
        // release is a scroll and never a command.
        right.moved((100.0, 100.0));
        assert_eq!(right.release((100.0, 100.0)), Some(RightRelease::Scroll));
        assert_eq!(right, RightButton::Idle);
    }

    #[test]
    fn a_diagonal_drag_crosses_on_euclidean_distance() {
        // 4 px right + 4 px down = ~5.66 px: under; 5 + 5 = ~7.07: over.
        let mut right = RightButton::new();
        right.press((0.0, 0.0));
        assert!(!right.moved((4.0, 4.0)));
        assert!(right.moved((5.0, 5.0)));
        assert_eq!(right.release((5.0, 5.0)), Some(RightRelease::Scroll));
    }

    #[test]
    fn a_release_without_a_press_is_ignored() {
        // A swallowed press (placement cancel leaves the machine idle)
        // never turns into a command.
        let mut right = RightButton::new();
        assert_eq!(right.release((5.0, 5.0)), None);
    }

    #[test]
    fn moves_while_idle_do_nothing() {
        let mut right = RightButton::new();
        assert!(!right.moved((500.0, 500.0)));
        assert_eq!(right, RightButton::Idle);
    }

    // ---- edge scroll band ------------------------------------------------

    #[test]
    fn the_interior_scrolls_nothing() {
        let window = (1920.0, 1080.0);
        assert_eq!(edge_scroll_vector((960.0, 540.0), window), (0.0, 0.0));
        // Just outside the band: dead zone by design (the band fades in
        // instead of snapping — the M9.1 finding was the snap).
        let (x, z) = edge_scroll_vector((20.0, 540.0), window);
        assert_eq!((x, z), (0.0, 0.0));
    }

    #[test]
    fn each_edge_points_its_own_way_and_scales_with_depth() {
        let window = (1920.0, 1080.0);
        // Left edge, full depth: pan screen-left only.
        assert_eq!(edge_scroll_vector((0.0, 540.0), window).0, -1.0);
        // Half into the band: half speed.
        let half = edge_scroll_vector((EDGE_SCROLL_BAND_PX * 0.5, 540.0), window).0;
        assert!(
            (half - (-0.5)).abs() < 1e-3,
            "half depth, half speed: {half}"
        );
        // Right edge: screen-right.
        assert_eq!(edge_scroll_vector((1919.0, 540.0), window).0, 1.0);
        // Top edge: up-screen (+z), the arrow-key Up convention.
        assert_eq!(edge_scroll_vector((960.0, 0.0), window).1, 1.0);
        // Bottom edge: down-screen (-z).
        assert_eq!(edge_scroll_vector((960.0, 1079.0), window).1, -1.0);
    }

    #[test]
    fn corners_combine_both_axes() {
        let window = (1920.0, 1080.0);
        // Top-left: pan up-screen and left at full depth.
        assert_eq!(edge_scroll_vector((0.0, 0.0), window), (-1.0, 1.0));
        // Bottom-right.
        assert_eq!(edge_scroll_vector((1919.0, 1079.0), window), (1.0, -1.0));
    }

    #[test]
    fn the_band_works_at_real_window_sizes() {
        // The owner tested at real sizes including fullscreen; the band is
        // absolute pixels, so it behaves the same everywhere.
        for window in [(1280.0, 720.0), (1920.0, 1080.0), (2560.0, 1440.0)] {
            let (w, h) = window;
            assert_eq!(edge_scroll_vector((0.0, h / 2.0), window).0, -1.0);
            assert_eq!(edge_scroll_vector((w - 1.0, h / 2.0), window).0, 1.0);
            assert_eq!(edge_scroll_vector((w / 2.0, 0.0), window).1, 1.0);
            assert_eq!(edge_scroll_vector((w / 2.0, h - 1.0), window).1, -1.0);
            assert_eq!(edge_scroll_vector((w / 2.0, h / 2.0), window), (0.0, 0.0));
        }
    }

    #[test]
    fn degenerate_windows_stay_quiet() {
        assert_eq!(edge_scroll_vector((0.0, 0.0), (0.0, 0.0)), (0.0, 0.0));
        assert_eq!(edge_scroll_vector((0.0, 0.0), (-5.0, 10.0)), (0.0, 0.0));
    }

    // ---- direction pinning: right-drag scroll, yaw 0 and 90 --------------

    #[test]
    fn right_drag_scroll_follows_the_cursor_at_yaw_0() {
        // Grab-the-ground: press at the screen center's ground point, drag
        // the cursor screen-right; the camera pans so the grabbed point
        // follows — the world appears to move left, the target moves LEFT
        // (−x world at yaw 0, where screen-right is +x).
        let mut camera = RtsCamera::new(64, 64, 16.0 / 9.0);
        camera.focus(32.0, 32.0, 24.0);
        let before = camera.target();
        let anchor = camera
            .ground_point(glam::Vec2::new(0.0, 0.0))
            .expect("center ground");
        // The cursor moves screen-right (NDC x: 0.0 -> 0.2).
        let current = camera
            .ground_point(glam::Vec2::new(0.2, 0.0))
            .expect("ground");
        camera.pan_world(anchor - current);
        let after = camera.target();
        assert!(
            after.x < before.x,
            "yaw 0: dragging right moves the view left ({:?} -> {:?})",
            before,
            after
        );
    }

    #[test]
    fn right_drag_scroll_follows_the_cursor_at_yaw_90() {
        // At yaw 90° the screen-right axis is −z world; the same drag must
        // move the target +z — the grab direction is camera-relative, not
        // world-fixed (the M9.1 mirrored-axes lesson, pinned per axis).
        let mut camera = RtsCamera::new(64, 64, 16.0 / 9.0);
        camera.focus(32.0, 32.0, 24.0);
        camera.rotate(std::f32::consts::FRAC_PI_2);
        let before = camera.target();
        let anchor = camera
            .ground_point(glam::Vec2::new(0.0, 0.0))
            .expect("center ground");
        let current = camera
            .ground_point(glam::Vec2::new(0.2, 0.0))
            .expect("ground");
        camera.pan_world(anchor - current);
        let after = camera.target();
        assert!(
            after.z > before.z,
            "yaw 90: dragging right moves the target +z ({:?} -> {:?})",
            before,
            after
        );
    }

    // ---- selection parity -------------------------------------------------

    fn entity(id: u64, owner: PlayerId, kind: KindId) -> RenderEntity {
        RenderEntity {
            id: EntityId(id),
            owner,
            kind,
            pos: glam::Vec3::new(id as f32, 0.0, 100.0 - id as f32),
            facing: glam::Vec3::ZERO,
            hp_fraction_milli: 1000,
            move_state: MoveState::Idle,
        }
    }

    #[test]
    fn shift_click_adds_only_it_never_removes() {
        // PLAN-M11 §5.2 (the reversal of M10.2's toggle): the picked id
        // joins the selection when absent; a member is never removed
        // (removal is a re-select, the card says so).
        let current = vec![EntityId(1), EntityId(2)];
        // Absent: appended.
        assert_eq!(
            shift_click_selection(&current, EntityId(3)),
            vec![EntityId(1), EntityId(2), EntityId(3)]
        );
        // Present: KEPT (add-only).
        assert_eq!(
            shift_click_selection(&current, EntityId(1)),
            vec![EntityId(1), EntityId(2)]
        );
        // An empty selection takes the pick.
        assert_eq!(shift_click_selection(&[], EntityId(3)), vec![EntityId(3)]);
    }

    #[test]
    fn shift_box_unions_without_duplicates() {
        let current = vec![EntityId(1), EntityId(2)];
        let picked = vec![EntityId(2), EntityId(4), EntityId(3)];
        assert_eq!(
            union_selection(&current, &picked),
            vec![EntityId(1), EntityId(2), EntityId(4), EntityId(3)]
        );
        assert_eq!(union_selection(&[], &picked), picked);
        assert!(!union_selection(&current, &[]).is_empty());
    }

    #[test]
    fn double_click_expands_to_the_same_kind_same_owner_on_screen() {
        let own = PlayerId(0);
        let other = PlayerId(1);
        let entities = vec![
            entity(1, own, KindId(2)),
            entity(2, own, KindId(2)),
            entity(3, other, KindId(2)), // enemy same kind: never joined
            entity(4, own, KindId(5)),   // own other kind
            entity(5, own, KindId(2)),
        ];
        // On screen: the whole same-kind set (PLAN-M11 §2's ZH target).
        let all = |_: &RenderEntity| true;
        assert_eq!(
            same_kind_selection(&entities, EntityId(2), all),
            Some(vec![EntityId(1), EntityId(2), EntityId(5)])
        );
        // The enemy's double-click (were it clickable) expands to its own
        // side's kind members only.
        assert_eq!(
            same_kind_selection(&entities, EntityId(3), all),
            Some(vec![EntityId(3)])
        );
        // An id outside the snapshot expands to nothing.
        assert_eq!(same_kind_selection(&entities, EntityId(99), all), None);
        // Off-screen members are left out (the screen filter is real).
        let none = |_: &RenderEntity| false;
        assert_eq!(
            same_kind_selection(&entities, EntityId(2), none),
            Some(vec![])
        );
    }

    // ---- PLAN-M11 §2 phase 2: E / Q / cycling / H ------------------------

    #[test]
    fn e_names_the_uniform_selection_kind_else_the_clicked_kind() {
        let own = PlayerId(0);
        let entities = vec![
            entity(1, own, KindId(2)),
            entity(2, own, KindId(2)),
            entity(3, own, KindId(7)),
        ];
        // A uniform selection names its own kind.
        assert_eq!(
            same_kind_key_kind(&entities, &[EntityId(1), EntityId(2)], None),
            Some(KindId(2))
        );
        // A mixed selection falls back to the clicked kind.
        assert_eq!(
            same_kind_key_kind(&entities, &[EntityId(1), EntityId(3)], Some(KindId(7))),
            Some(KindId(7))
        );
        // An empty selection uses the clicked kind, or is inert when
        // nothing was ever clicked.
        assert_eq!(
            same_kind_key_kind(&entities, &[], Some(KindId(2))),
            Some(KindId(2))
        );
        assert_eq!(same_kind_key_kind(&entities, &[], None), None);
        // A selection of dead ids also falls back to the clicked kind.
        assert_eq!(
            same_kind_key_kind(&entities, &[EntityId(77)], Some(KindId(5))),
            Some(KindId(5))
        );
    }

    #[test]
    fn e_selects_on_screen_and_the_double_tap_takes_the_whole_map() {
        let own = PlayerId(0);
        let entities = vec![
            entity(1, own, KindId(2)),
            entity(2, own, KindId(2)),
            entity(3, own, KindId(2)),
            entity(4, own, KindId(5)),
        ];
        let on_screen =
            |entity: &RenderEntity| entity.id == EntityId(1) || entity.id == EntityId(2);
        // Single tap: only the on-screen members of the kind.
        assert_eq!(
            same_kind_key_selection(&entities, KindId(2), on_screen, false),
            Some(vec![EntityId(1), EntityId(2)])
        );
        // Double tap: every visible member of the kind on the map.
        assert_eq!(
            same_kind_key_selection(&entities, KindId(2), on_screen, true),
            Some(vec![EntityId(1), EntityId(2), EntityId(3)])
        );
        // An empty set is inert (None keeps the selection).
        assert_eq!(
            same_kind_key_selection(&entities, KindId(9), on_screen, false),
            None
        );
        assert_eq!(
            same_kind_key_selection(&entities, KindId(5), |_| false, false),
            None
        );
    }

    #[test]
    fn q_selects_own_combat_units_and_only_those() {
        let own = PlayerId(0);
        let enemy = PlayerId(1);
        let entities = vec![
            entity(1, own, KindId(2)),   // own combat
            entity(2, own, KindId(3)),   // own worker (not combat)
            entity(3, enemy, KindId(2)), // enemy combat: never Q's
            entity(4, own, KindId(2)),   // own combat, off screen
        ];
        let combat = |entity: &RenderEntity| entity.kind == KindId(2);
        let on_screen = |entity: &RenderEntity| entity.id != EntityId(4);
        assert_eq!(
            combat_key_selection(&entities, combat, on_screen, false),
            Some(vec![EntityId(1)])
        );
        assert_eq!(
            combat_key_selection(&entities, combat, on_screen, true),
            Some(vec![EntityId(1), EntityId(4)])
        );
        // No combat units at all: inert.
        let no_combat = |_: &RenderEntity| false;
        assert_eq!(
            combat_key_selection(&entities, no_combat, on_screen, false),
            None
        );
    }

    #[test]
    fn ctrl_arrows_cycle_in_ascending_id_order_and_wrap() {
        let own = PlayerId(0);
        let units = |entity: &RenderEntity| entity.kind != KindId(9); // not a structure
        let entities = vec![
            entity(3, own, KindId(2)),
            entity(7, own, KindId(2)),
            entity(11, own, KindId(2)),
        ];
        // Nothing selected: the lowest id.
        assert_eq!(
            cycled_selection(&entities, &[], units, true),
            Some(vec![EntityId(3)])
        );
        // Forward from 7 -> 11.
        assert_eq!(
            cycled_selection(&entities, &[EntityId(7)], units, true),
            Some(vec![EntityId(11)])
        );
        // Forward from the max wraps to the lowest.
        assert_eq!(
            cycled_selection(&entities, &[EntityId(11)], units, true),
            Some(vec![EntityId(3)])
        );
        // Backward from 7 -> 3.
        assert_eq!(
            cycled_selection(&entities, &[EntityId(7)], units, false),
            Some(vec![EntityId(3)])
        );
        // Backward from the lowest wraps to the max.
        assert_eq!(
            cycled_selection(&entities, &[EntityId(3)], units, false),
            Some(vec![EntityId(11)])
        );
        // A multi-selection anchors on its highest-id member.
        assert_eq!(
            cycled_selection(&entities, &[EntityId(3), EntityId(7)], units, true),
            Some(vec![EntityId(11)])
        );
        // A structure selected while unit-cycling: the lowest unit.
        let with_structure = vec![
            entity(1, own, KindId(9)), // a structure, never a candidate
            entity(3, own, KindId(2)),
            entity(7, own, KindId(2)),
        ];
        assert_eq!(
            cycled_selection(&with_structure, &[EntityId(1)], units, true),
            Some(vec![EntityId(3)]),
            "a non-candidate selection selects the lowest candidate"
        );
    }

    #[test]
    fn the_worker_cycle_filters_to_the_worker_kind() {
        let own = PlayerId(0);
        let entities = vec![
            entity(2, own, KindId(3)), // worker
            entity(5, own, KindId(2)), // combat
            entity(8, own, KindId(3)), // worker
        ];
        let workers = |entity: &RenderEntity| entity.kind == KindId(3);
        let everyone = |_: &RenderEntity| true;
        assert_eq!(
            cycled_selection(&entities, &[], workers, true),
            Some(vec![EntityId(2)])
        );
        assert_eq!(
            cycled_selection(&entities, &[EntityId(2)], workers, true),
            Some(vec![EntityId(8)])
        );
        // The unit cycle (every own entity here) ignores the worker filter.
        assert_eq!(
            cycled_selection(&entities, &[EntityId(2)], everyone, true),
            Some(vec![EntityId(5)])
        );
        // No workers: inert.
        let none = |_: &RenderEntity| false;
        assert_eq!(cycled_selection(&entities, &[], none, true), None);
    }

    #[test]
    fn h_targets_the_first_own_command_center() {
        let own = PlayerId(0);
        let enemy = PlayerId(1);
        let entities = vec![
            entity(1, own, KindId(4)),
            entity(2, enemy, KindId(4)), // enemy CC: never H's target
            entity(3, own, KindId(4)),   // second own CC (first in id order wins)
        ];
        assert_eq!(
            command_center_position(&entities, Some(KindId(4))),
            Some((1.0, 99.0))
        );
        // Unknown kind or no CC: None (the base-anchor fallback).
        assert_eq!(command_center_position(&entities, None), None);
        assert_eq!(command_center_position(&entities, Some(KindId(9))), None);
    }

    #[test]
    fn edge_scrolling_off_or_unfocused_or_at_a_menu_pans_nothing() {
        // PLAN-M10.2 §3.4's exit item: the toggle (and the focus and menu
        // guards) produce no pan from the band. The cursor sits hard in the
        // corner — the strongest possible band reading.
        let corner = (0.0, 0.0);
        let window = (1920.0, 1080.0);
        let _ = edge_scroll_vector(corner, window); // nonzero when enabled
        assert!(
            edge_scroll_vector(corner, window) != (0.0, 0.0),
            "the band itself is live (sanity)"
        );
        assert_eq!(
            edge_pan_contribution(false, true, true, corner, window),
            (0.0, 0.0),
            "the settings toggle silences the band"
        );
        assert_eq!(
            edge_pan_contribution(true, false, true, corner, window),
            (0.0, 0.0),
            "an unfocused window never scrolls"
        );
        assert_eq!(
            edge_pan_contribution(true, true, false, corner, window),
            (0.0, 0.0),
            "a menu never scrolls its own backdrop"
        );
        assert_eq!(
            edge_pan_contribution(true, true, true, corner, window),
            edge_scroll_vector(corner, window),
            "on, focused, in-match: the band's own vector"
        );
    }

    #[test]
    fn the_double_click_window_is_a_few_hundred_ms() {
        assert!(is_double_click(0));
        assert!(is_double_click(DOUBLE_CLICK_MS));
        assert!(!is_double_click(DOUBLE_CLICK_MS + 1));
    }

    #[test]
    fn the_double_tap_window_matches_the_double_click_window() {
        assert!(is_double_tap(0));
        assert!(is_double_tap(DOUBLE_TAP_MS));
        assert!(!is_double_tap(DOUBLE_TAP_MS + 1));
    }

    #[test]
    fn group_center_averages_live_members_and_skips_the_dead() {
        let entities = vec![
            entity(1, PlayerId(0), KindId(2)),
            entity(2, PlayerId(0), KindId(2)),
        ];
        let group = vec![EntityId(1), EntityId(2), EntityId(77)]; // 77 is dead
        let center = group_center(&entities, &group).expect("live members exist");
        // entity(1) sits at (1, 99), entity(2) at (2, 98): mean (1.5, 98.5).
        assert!((center.0 - 1.5).abs() < 1e-4 && (center.1 - 98.5).abs() < 1e-4);
        // A fully dead group centers nowhere.
        assert_eq!(group_center(&entities, &[EntityId(77)]), None);
        assert_eq!(group_center(&entities, &[]), None);
    }

    #[test]
    fn escape_climbs_the_ladder_one_rung_per_press() {
        assert_eq!(escape_action(true, true, true), EscapeAction::CancelArmed);
        assert_eq!(
            escape_action(false, true, true),
            EscapeAction::CancelPlacement
        );
        assert_eq!(
            escape_action(false, false, true),
            EscapeAction::ClearSelection
        );
        // PLAN-M10.2 §3.5: the exhausted rung opens the pause menu, not the
        // exit — the earlier rungs stay exactly as Phase 1 re-tested them.
        assert_eq!(
            escape_action(false, false, false),
            EscapeAction::OpenPauseMenu
        );
    }

    #[test]
    fn jump_target_prefers_the_last_event_else_the_base() {
        assert_eq!(jump_target(None, (10.0, 20.0)), (10.0, 20.0));
        assert_eq!(jump_target(Some((33.0, 44.0)), (10.0, 20.0)), (33.0, 44.0));
    }

    // ---- cursor order hint -------------------------------------------------

    use pandemonium_sim_api::Vec2Fx;

    #[test]
    fn the_cursor_hint_names_the_order_the_next_click_would_issue() {
        // (order, rally, armed, placing, selection_empty) -> expected hint.
        let at = Vec2Fx::from_ints(3, 3);
        let units = vec![EntityId(1)];
        let cases: [(
            Option<crate::input::orders::ContextOrder>,
            bool,
            bool,
            bool,
            bool,
            CursorOrderHint,
        ); 8] = [
            (
                Some(crate::input::orders::ContextOrder::Move { target: at }),
                false,
                false,
                false,
                false,
                CursorOrderHint::Move,
            ),
            (
                Some(crate::input::orders::ContextOrder::Attack {
                    target: EntityId(9),
                    at,
                }),
                false,
                false,
                false,
                false,
                CursorOrderHint::Attack,
            ),
            (
                Some(crate::input::orders::ContextOrder::Gather {
                    node: EntityId(9),
                    at,
                    units,
                }),
                false,
                false,
                false,
                false,
                CursorOrderHint::Gather,
            ),
            // A selected producer over ground: rally (the resolver's own
            // rally path is main's; the hint takes the flag).
            (None, true, false, false, false, CursorOrderHint::Rally),
            // Armed attack-move owns the next click, even over a rally.
            (
                Some(crate::input::orders::ContextOrder::Move { target: at }),
                true,
                true,
                false,
                false,
                CursorOrderHint::AttackMove,
            ),
            // Empty selection: no marker.
            (None, false, false, false, true, CursorOrderHint::None),
            // Placement mode owns the cursor (the ghost is its marker).
            (
                Some(crate::input::orders::ContextOrder::Move { target: at }),
                false,
                false,
                true,
                false,
                CursorOrderHint::None,
            ),
            // No resolution (cursor above the horizon) and no rally.
            (None, false, false, false, false, CursorOrderHint::None),
        ];
        for (order, rally, armed, placing, empty, expected) in cases {
            assert_eq!(
                cursor_order_hint(order.clone(), rally, armed, placing, empty),
                expected,
                "order={order:?} rally={rally} armed={armed} placing={placing} empty={empty}"
            );
        }
    }

    // ---- direction pinning: middle-drag rotate ---------------------------

    #[test]
    fn middle_drag_right_orbits_clockwise_at_both_pinned_yaws() {
        // PLAN-M11 phase 1: middle-drag is the one always-available rotate
        // (the Q/E keys are Zero Hour selection keys now). A rightward
        // middle-drag applies positive yaw — the same sign the numpad
        // rotate keys and the old E key used, pinned at both yaws. The
        // wiring's exact formula: yaw delta = radians per pixel * dx.
        let delta_of = |dx_px: f32| MIDDLE_DRAG_RAD_PER_PX * dx_px;
        assert!(delta_of(120.0) > 0.0);
        let mut camera = RtsCamera::new(64, 64, 16.0 / 9.0);
        for yaw in [0.0f32, std::f32::consts::FRAC_PI_2] {
            camera.rotate(yaw - camera.yaw());
            let before = camera.yaw();
            camera.rotate(delta_of(120.0));
            assert!(
                camera.yaw() > before,
                "yaw {yaw}: rightward middle-drag increases yaw"
            );
            camera.rotate(delta_of(-120.0));
            assert!((camera.yaw() - before).abs() < 1e-6, "symmetric back");
        }
        // The scale: one pixel is MIDDLE_DRAG_RAD_PER_PX.
        assert!((delta_of(1.0) - MIDDLE_DRAG_RAD_PER_PX).abs() < 1e-6);
    }

    // ---- direction pinning: arrow-key scroll -----------------------------

    #[test]
    fn arrow_holds_pan_the_screen_axes_at_yaw_0_and_yaw_90() {
        // PLAN-M11 phase 1: the arrows are the keyboard scroll. The held
        // tags feed about_to_wait's pan with the M9.1 sign convention:
        // pan's +dz is "forward" (screen-up), pan's +dx is screen-right.
        // In world terms that is -z/up and +x/right at yaw 0, and
        // -x/up and -z/right at yaw 90 (the camera orbits the target).
        // Pinned at both yaws, the M9.1 discipline: presentation direction
        // is invisible to the golden hashes.
        for (yaw, up, right) in [
            (
                0.0f32,
                glam::Vec3::new(0.0, 0.0, -1.0),
                glam::Vec3::new(1.0, 0.0, 0.0),
            ),
            (
                std::f32::consts::FRAC_PI_2,
                glam::Vec3::new(-1.0, 0.0, 0.0),
                glam::Vec3::new(0.0, 0.0, -1.0),
            ),
        ] {
            let mut camera = RtsCamera::new(64, 64, 16.0 / 9.0);
            camera.focus(32.0, 32.0, 24.0);
            camera.rotate(yaw - camera.yaw());
            // "up" held: pan(0, +v) moves the target along `up`.
            let before = camera.target();
            camera.pan(0.0, 4.0);
            let delta = camera.target() - before;
            assert!(
                delta.dot(up) > 3.0 && delta.dot(right).abs() < 1e-3,
                "yaw {yaw}: up-arrow pans along {up} ({before:?} -> {:?})",
                camera.target()
            );
            // "right" held: pan(+v, 0) moves the target along `right`.
            let before = camera.target();
            camera.pan(4.0, 0.0);
            let delta = camera.target() - before;
            assert!(
                delta.dot(right) > 3.0 && delta.dot(up).abs() < 1e-3,
                "yaw {yaw}: right-arrow pans along {right} ({before:?} -> {:?})",
                camera.target()
            );
        }
    }
}
