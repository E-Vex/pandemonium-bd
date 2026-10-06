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
/// signs the held-key pan uses. The magnitude grows linearly with the
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
    // "up-screen" is +z (forward), matching the held-key convention.
    z += depth(cursor.1);
    // Bottom edge.
    z -= depth(h - 1.0 - cursor.1);
    (x, z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandemonium_engine::RtsCamera;

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
        // Top edge: up-screen (+z), the held-key W convention.
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

    // ---- direction pinning: middle-drag rotate ---------------------------

    #[test]
    fn middle_drag_right_orbits_the_same_way_e_does() {
        // E (orbit right) applies a positive yaw rate; a rightward
        // middle-drag must apply positive yaw too, at both pinned yaws.
        // The wiring's exact formula: yaw delta = radians per pixel * dx.
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
}
