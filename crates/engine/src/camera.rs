//! The perspective RTS camera (plan §11.3 as amended by ADR-0001): an orbiting
//! view over the 2D logical ground plane (`world.x = sim.x`, `world.z = sim.y`,
//! height on `world.y`), with pan, zoom, and rotation, plus the two picking
//! primitives the input layer needs:
//!
//! - **ground-plane ray picking** — a cursor position becomes a world point on
//!   the logical plane (unprojected through the inverse view-projection and
//!   intersected with `y = 0`), converted back to fixed point at the boundary
//!   so the simulation never sees a float;
//! - **screen-space box selection** — entity positions project to normalized
//!   device coordinates and a rectangle test selects them.
//!
//! All math here is floating-point presentation math (ADR-0001); it never
//! enters the simulation.

use glam::{Mat4, Vec2, Vec3, Vec4};

/// Default field of view (vertical), in radians (~45 degrees).
const DEFAULT_FOV_Y: f32 = 45.0_f32.to_radians();
/// Near clip plane.
const NEAR: f32 = 0.1;
/// Far clip plane (ample for a 64-tile map viewed from ~100 tiles).
const FAR: f32 = 500.0;
/// Pitch clamp: never fully horizontal (degenerate), never fully top-down-only.
const PITCH_RANGE: (f32, f32) = (0.25, 1.45);
/// Distance clamp in tiles. Public since M10.2 Phase 3 (PLAN §3.4): the
/// client's settings file stores user zoom limits and clamps through
/// [`RtsCamera::clamp_distance_to`] — the defaults mirror this range so a
/// settings file that never narrows them behaves exactly like the engine's
/// own clamp.
pub const DISTANCE_RANGE: (f32, f32) = (8.0, 160.0);
/// Default yaw rotation speed (radians per second) — Generals-style Q/E
/// camera rotation: a full turn takes ~6 seconds, fast enough to re-orient
/// without inducing motion sickness.
pub const DEFAULT_ROTATE_RAD_PER_SEC: f32 = std::f32::consts::TAU / 6.0;
/// Default pitch adjustment per wheel notch (radians) — Generals-style
/// Ctrl+wheel pitch control: ~12 notches cover the full pitch range.
pub const DEFAULT_PITCH_PER_WHEEL_NOTCH: f32 = (PITCH_RANGE.1 - PITCH_RANGE.0) / 12.0;

/// An orbiting RTS camera over the ground plane.
#[derive(Clone, Copy, Debug)]
pub struct RtsCamera {
    target: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
    fov_y: f32,
    aspect: f32,
    /// The map's extent in tiles — panning clamps the target inside it so
    /// the view can never wander off the battlefield (M9.1: the DEBT-008
    /// human pass found "get lost in the void" was a real failure mode).
    bounds: Vec2,
}

impl RtsCamera {
    /// A camera looking at the center of a `width` x `height` tile map, from a
    /// comfortable default angle and distance. See [`RtsCamera::focus`] for
    /// the player-start framing the windowed client opens with (M9.1).
    pub fn new(map_width: u32, map_height: u32, aspect: f32) -> Self {
        Self {
            target: Vec3::new(map_width as f32 / 2.0, 0.0, map_height as f32 / 2.0),
            yaw: 0.0,
            pitch: 1.0,
            distance: (map_width.max(map_height) as f32) * 0.9,
            fov_y: DEFAULT_FOV_Y,
            aspect,
            bounds: Vec2::new(map_width as f32, map_height as f32),
        }
    }

    /// The orbit target on the ground plane.
    pub fn target(&self) -> Vec3 {
        self.target
    }

    /// The current yaw (radians). Q/E rotation adjusts this directly.
    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    /// The current pitch (radians, clamped to `PITCH_RANGE`).
    pub fn pitch(&self) -> f32 {
        self.pitch
    }

    /// The current orbit distance (in tiles, clamped to `DISTANCE_RANGE`).
    pub fn distance(&self) -> f32 {
        self.distance
    }

    /// Adjusts the pitch by `delta` radians, clamped to the supported range
    /// (Generals-style Ctrl+wheel pitch control). Positive deltas tilt the
    /// camera down toward the ground; negative deltas raise it toward the
    /// horizon.
    pub fn adjust_pitch(&mut self, delta: f32) {
        self.pitch = (self.pitch + delta).clamp(PITCH_RANGE.0, PITCH_RANGE.1);
    }

    /// The camera's world position.
    pub fn eye(&self) -> Vec3 {
        let horizontal = self.distance * self.pitch.cos();
        self.target
            + Vec3::new(
                horizontal * self.yaw.sin(),
                self.distance * self.pitch.sin(),
                horizontal * self.yaw.cos(),
            )
    }

    /// The right-handed view matrix (looking at the target, Y up).
    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.target, Vec3::Y)
    }

    /// The right-handed, zero-to-one-depth perspective projection (the wgpu
    /// clip convention).
    pub fn projection(&self) -> Mat4 {
        Mat4::perspective_rh(self.fov_y, self.aspect, NEAR, FAR)
    }

    /// The combined view-projection.
    pub fn view_projection(&self) -> Mat4 {
        self.projection() * self.view()
    }

    /// Pans the target along the camera-relative ground axes: `dx` is
    /// rightward on screen, `dz` forward (toward the top of the screen) —
    /// both in world units, already scaled by the caller (e.g. by the
    /// frame delta).
    ///
    /// The screen-right axis is `forward x up` — the same right-handed
    /// basis `Mat4::look_at_rh` builds, so a world-space pan to the right
    /// lands right on screen. (M9.1: the M3 implementation negated this
    /// axis, which mirrored D/A; the DEBT-008 human pass caught it — no
    /// sim-side test can, presentation direction is invisible to the
    /// hash. Direction tests now pin the convention.)
    pub fn pan(&mut self, dx: f32, dz: f32) {
        let forward = Vec3::new(-self.yaw.sin(), 0.0, -self.yaw.cos()).normalize();
        let right = Vec3::new(-forward.z, 0.0, forward.x);
        self.target += right * dx + forward * dz;
        self.clamp_target();
    }

    /// Translates the orbit target by a world-space ground-plane offset —
    /// the middle-drag "grab the ground" pan: the grabbed point stays
    /// under the cursor while the offset tracks the cursor's ground delta
    /// (M9.1, plan §11.3's mouse panning).
    pub fn pan_world(&mut self, delta: Vec3) {
        self.target.x += delta.x;
        self.target.z += delta.z;
        self.clamp_target();
    }

    /// Zooms by a multiplicative factor (values < 1 zoom in), clamped to
    /// [`DISTANCE_RANGE`].
    pub fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(DISTANCE_RANGE.0, DISTANCE_RANGE.1);
    }

    /// Clamps the orbit distance into a caller-supplied range, intersected
    /// with [`DISTANCE_RANGE`] (a user range can only narrow, never widen;
    /// an inverted range is treated as its own min/max). The client's
    /// settings (PLAN-M10.2 §3.4: "camera zoom limits") apply this after
    /// every distance mutation — presentation-only; the engine stores no
    /// setting.
    pub fn clamp_distance_to(&mut self, min: f32, max: f32) {
        let min = min.clamp(DISTANCE_RANGE.0, DISTANCE_RANGE.1);
        let max = max.clamp(DISTANCE_RANGE.0, DISTANCE_RANGE.1);
        if min <= max {
            self.distance = self.distance.clamp(min, max);
        } else {
            self.distance = self.distance.clamp(max, min);
        }
    }

    /// Zooms by `factor` while keeping the ground point currently under
    /// the cursor's NDC position under that same position — plan §11.3's
    /// "zoom toward the cursor" (M9.1: zooming the orbit distance alone
    /// reads as the view sliding away from where the player is looking).
    pub fn zoom_toward(&mut self, factor: f32, cursor_ndc: Vec2) {
        let anchor = self.ground_point(cursor_ndc);
        self.zoom(factor);
        if let (Some(before), Some(after)) = (anchor, self.ground_point(cursor_ndc)) {
            let delta = before - after;
            self.target.x += delta.x;
            self.target.z += delta.z;
        }
    }

    /// Re-centers the orbit target on a ground-plane point at a given
    /// distance (clamped) — the windowed client's opening framing on the
    /// player's start and its restart re-focus (M9.1: the map-center
    /// default opened on the empty crossroads at 58 tiles out, where the
    /// starting force reads as specks — "nothing appears on the screen").
    pub fn focus(&mut self, x: f32, z: f32, distance: f32) {
        self.target = Vec3::new(x, 0.0, z);
        self.clamp_target();
        self.distance = distance.clamp(DISTANCE_RANGE.0, DISTANCE_RANGE.1);
    }

    /// Keeps the orbit target on the map's ground rectangle (height zero).
    fn clamp_target(&mut self) {
        self.target.x = self.target.x.clamp(0.0, self.bounds.x);
        self.target.z = self.target.z.clamp(0.0, self.bounds.y);
        self.target.y = 0.0;
    }

    /// Rotates the camera around the target by `delta_yaw` radians.
    pub fn rotate(&mut self, delta_yaw: f32) {
        self.yaw += delta_yaw;
    }

    /// Sets the pitch (angle of the view axis below the horizontal), clamped
    /// to the supported range — used by camera controls and tests.
    pub fn set_pitch(&mut self, pitch: f32) {
        self.pitch = pitch.clamp(PITCH_RANGE.0, PITCH_RANGE.1);
    }

    /// Updates the projection aspect (on window resize).
    pub fn set_aspect(&mut self, aspect: f32) {
        self.aspect = aspect;
    }

    /// Projects a world point to normalized device coordinates (`x`, `y` in
    /// `-1.0..=1.0`, Y up). Points behind the camera get `w <= 0` — the
    /// caller treats that as unselectable.
    pub fn project(&self, world: Vec3) -> Vec2 {
        let clip = self.view_projection() * Vec4::new(world.x, world.y, world.z, 1.0);
        if clip.w.abs() < f32::EPSILON {
            return Vec2::new(f32::INFINITY, f32::INFINITY);
        }
        Vec2::new(clip.x / clip.w, clip.y / clip.w)
    }

    /// **Ground-plane ray picking**: a cursor's normalized device coordinates
    /// become the world point where the cursor ray intersects the logical
    /// ground plane (`y = 0`). `None` when the ray never hits the plane (the
    /// cursor is above the horizon).
    pub fn ground_point(&self, ndc: Vec2) -> Option<Vec3> {
        let inverse = self.view_projection().inverse();
        // Two points on the cursor ray: the near plane (z = 0) and the far
        // plane (z = 1) in the zero-to-one depth convention.
        let near = inverse * Vec4::new(ndc.x, ndc.y, 0.0, 1.0);
        let far = inverse * Vec4::new(ndc.x, ndc.y, 1.0, 1.0);
        if near.w.abs() < f32::EPSILON || far.w.abs() < f32::EPSILON {
            return None;
        }
        let near = near.truncate() / near.w;
        let far = far.truncate() / far.w;
        let direction = (far - near).normalize_or_zero();
        if direction.y.abs() < f32::EPSILON {
            return None; // Parallel to the ground plane.
        }
        let t = -near.y / direction.y;
        if t < 0.0 {
            return None; // The plane is behind the cursor ray.
        }
        Some(near + direction * t)
    }

    /// **Screen-space box selection**: returns the ids of entities whose
    /// projected positions fall inside the NDC rectangle (corners in any
    /// order). Entities behind the camera are never selected. The input order
    /// is preserved (the render snapshot is ascending by id, so the result is
    /// too).
    pub fn box_select(
        &self,
        entities: &[crate::interpolate::RenderEntity],
        corner_a: Vec2,
        corner_b: Vec2,
    ) -> Vec<pandemonium_sim_api::EntityId> {
        let min = corner_a.min(corner_b);
        let max = corner_a.max(corner_b);
        entities
            .iter()
            .filter(|entity| {
                let ndc = self.project(entity.pos);
                ndc.x.is_finite()
                    && ndc.y.is_finite()
                    && ndc.x >= min.x
                    && ndc.x <= max.x
                    && ndc.y >= min.y
                    && ndc.y <= max.y
            })
            .map(|entity| entity.id)
            .collect()
    }

    /// The view frustum's intersection with the ground plane, as four world
    /// corners (clockwise from the screen's top-left) — the minimap's
    /// viewport indicator. Presentation math only.
    ///
    /// At shallow pitches the frustum's top-edge rays can point *above* the
    /// horizon and never meet the plane; the top angle is clamped to a small
    /// positive one so the corners stay finite, and the caller clamps them
    /// to the map (the drawn indicator then hugs the map edge, which reads
    /// honestly at that pitch).
    pub fn ground_footprint_corners(&self) -> [Vec3; 4] {
        let height = self.distance * self.pitch.sin();
        let horizontal = self.distance * self.pitch.cos();
        let half_fov_y = self.fov_y * 0.5;
        // Forward extent (screen top): clamped so the ray always lands.
        let top_angle = (self.pitch - half_fov_y).max(0.08);
        let forward = height / top_angle.tan() - horizontal;
        // Back extent (screen bottom): always lands (the angle only grows).
        let back = height / (self.pitch + half_fov_y).tan() - horizontal;
        // Horizontal half-extent: tan(half horizontal fov) * horizontal dist.
        let tan_half_side = half_fov_y.tan() * self.aspect;
        let side = horizontal * tan_half_side;
        // The camera's ground basis (the same one pan() uses).
        let fwd = Vec3::new(-self.yaw.sin(), 0.0, -self.yaw.cos());
        let right = Vec3::new(-fwd.z, 0.0, fwd.x);
        let corner = |side_amount: f32, forward_amount: f32| {
            self.target + right * side_amount + fwd * forward_amount
        };
        [
            corner(-side, forward),
            corner(side, forward),
            corner(side, back),
            corner(-side, back),
        ]
    }
}
#[test]
fn the_ground_footprint_tracks_pitch_and_yaw() {
    let mut camera = RtsCamera::new(64, 64, 16.0 / 9.0);
    camera.focus(32.0, 32.0, 20.0);
    camera.set_pitch(1.2); // steep, near top-down
    let corners = camera.ground_footprint_corners();
    // All four corners sit near the target at a steep pitch, and all
    // are finite.
    for corner in &corners {
        assert!(corner.x.is_finite() && corner.z.is_finite());
        let dx = corner.x - 32.0;
        let dz = corner.z - 32.0;
        assert!(
            (dx * dx + dz * dz).sqrt() < 40.0,
            "steep view stays near the target"
        );
    }
    // A shallow pitch stretches the footprint toward the horizon.
    camera.set_pitch(0.35);
    let shallow = camera.ground_footprint_corners();
    let extent = |corners: &[Vec3; 4]| {
        let mut min_z = f32::MAX;
        let mut max_z = f32::MIN;
        for corner in corners {
            min_z = min_z.min(corner.z);
            max_z = max_z.max(corner.z);
        }
        max_z - min_z
    };
    assert!(
        extent(&shallow) > extent(&corners),
        "a shallow pitch sees more ground"
    );
}

#[cfg(test)]
mod clamp_distance_tests {
    use super::*;

    fn camera() -> RtsCamera {
        let mut camera = RtsCamera::new(64, 64, 16.0 / 9.0);
        camera.focus(32.0, 32.0, 40.0);
        camera
    }

    #[test]
    fn user_limits_narrow_the_distance_and_intersect_the_engine_range() {
        let mut camera = camera();
        // A user range inside the engine range clamps to the user range
        // (the client applies this after every zoom mutation).
        camera.clamp_distance_to(12.0, 60.0);
        assert_eq!(camera.distance(), 40.0, "40 is inside 12..=60");
        camera.zoom(0.1); // the engine's own clamp floors at 8...
        camera.clamp_distance_to(12.0, 60.0); // ...the user range lifts it back
        assert_eq!(camera.distance(), 12.0);
        camera.zoom(20.0); // the engine clamps at 160...
        camera.clamp_distance_to(12.0, 60.0); // ...the user range caps it
        assert_eq!(camera.distance(), 60.0, "the user max caps the zoom-out");
        // A user range *outside* the engine range intersects to the engine's.
        camera.clamp_distance_to(0.0, 1.0e6);
        assert!(
            camera.distance() >= DISTANCE_RANGE.0 && camera.distance() <= DISTANCE_RANGE.1,
            "the engine range always holds"
        );
    }

    #[test]
    fn an_inverted_user_range_is_treated_as_its_own_min_max() {
        let mut camera = camera();
        camera.clamp_distance_to(50.0, 20.0); // "min" above "max"
        assert!(
            camera.distance() >= 20.0 && camera.distance() <= 50.0,
            "the pair clamps as (20, 50), never panics"
        );
    }
}
