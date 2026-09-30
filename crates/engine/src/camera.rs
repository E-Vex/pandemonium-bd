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
/// Distance clamp in tiles.
const DISTANCE_RANGE: (f32, f32) = (8.0, 160.0);

/// An orbiting RTS camera over the ground plane.
#[derive(Clone, Copy, Debug)]
pub struct RtsCamera {
    target: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
    fov_y: f32,
    aspect: f32,
}

impl RtsCamera {
    /// A camera looking at the center of a `width` x `height` tile map, from a
    /// comfortable default angle and distance.
    pub fn new(map_width: u32, map_height: u32, aspect: f32) -> Self {
        Self {
            target: Vec3::new(map_width as f32 / 2.0, 0.0, map_height as f32 / 2.0),
            yaw: 0.0,
            pitch: 1.0,
            distance: (map_width.max(map_height) as f32) * 0.9,
            fov_y: DEFAULT_FOV_Y,
            aspect,
        }
    }

    /// The orbit target on the ground plane.
    pub fn target(&self) -> Vec3 {
        self.target
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
    /// both in world units, already scaled by the caller (e.g. by zoom).
    pub fn pan(&mut self, dx: f32, dz: f32) {
        let forward = Vec3::new(-self.yaw.sin(), 0.0, -self.yaw.cos()).normalize();
        let right = Vec3::new(forward.z, 0.0, -forward.x);
        self.target += right * dx + forward * dz;
        self.target.y = 0.0;
    }

    /// Zooms by a multiplicative factor (values < 1 zoom in), clamped to
    /// [`DISTANCE_RANGE`].
    pub fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(DISTANCE_RANGE.0, DISTANCE_RANGE.1);
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
}
