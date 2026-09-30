//! Engine layer (plan §11, as amended by ADR-0001): the fixed-timestep game
//! loop, snapshot interpolation, the perspective RTS camera with ground-plane
//! ray picking and screen-space box selection, and the render abstraction with
//! a headless null renderer.
//!
//! The engine steps the simulation at exactly 30 Hz and renders at display
//! rate, interpolating between the previous and current snapshots by the
//! accumulator fraction. Time originates here and in the client — never inside
//! the simulation (FD-6). [`MatchHost`] owns the simulation privately: the only
//! way to affect it is submitting commands that feed the next step, which makes
//! the M3 exit criterion ("client never touches Sim mutably except via step
//! inputs") a compile-time property.
//!
//! This is a presentation-layer crate (ADR-0001): floating-point math and
//! `glam` are legal here and in the client, and never in the simulation's tree.
//! Window and GPU code live in `client` (plan §3.2).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod camera;
pub mod clock;
pub mod host;
pub mod interpolate;
pub mod renderer;

pub use camera::RtsCamera;
pub use clock::{FixedTimestep, MAX_CATCH_UP_STEPS};
pub use host::{FrameOutcome, MatchHost};
pub use interpolate::{
    fx_to_world, logical_to_world, world_to_fx, Interpolator, RenderEntity, RenderSnapshot,
};
pub use renderer::{Frame, NullRenderer, Renderer};

#[cfg(test)]
mod camera_tests {
    use super::*;
    use glam::Vec2;
    use pandemonium_fx::Fx;
    use pandemonium_sim_api::{EntityId, KindId, MoveState, PlayerId, Vec2Fx};

    fn entity_at(id: u64, x: f32, z: f32) -> RenderEntity {
        RenderEntity {
            id: EntityId(id),
            owner: PlayerId(0),
            kind: KindId(0),
            pos: glam::Vec3::new(x, 0.0, z),
            facing: glam::Vec3::ZERO,
            hp_fraction_milli: 1000,
            move_state: MoveState::Idle,
        }
    }

    #[test]
    fn the_camera_looks_at_the_map_center_from_above() {
        let camera = RtsCamera::new(64, 64, 16.0 / 9.0);
        let target = camera.target();
        assert!((target.x - 32.0).abs() < 1e-4 && (target.z - 32.0).abs() < 1e-4);
        let eye = camera.eye();
        assert!(eye.y > 0.0, "the camera hovers above the ground plane");
        // The map center projects to the screen center (NDC origin).
        let center = camera.project(target);
        assert!(
            center.length() < 1e-3,
            "center of the map at NDC origin: {center:?}"
        );
    }

    #[test]
    fn ground_picking_round_trips_through_the_screen_center() {
        let camera = RtsCamera::new(64, 64, 16.0 / 9.0);
        let picked = camera
            .ground_point(Vec2::new(0.0, 0.0))
            .expect("the screen center is on the map");
        let target = camera.target();
        // f32 through the inverse view-projection: sub-centi-tile precision is
        // ample for picking (a tile is the interaction quantum).
        assert!(
            (picked - target).length() < 0.01,
            "picking the screen center must return the orbit target: {picked:?} vs {target:?}"
        );
    }

    #[test]
    fn ground_picking_hits_expected_off_center_tiles() {
        let mut camera = RtsCamera::new(64, 64, 16.0 / 9.0);
        camera.zoom(0.4); // closer, for a tighter frustum
                          // Pick a point slightly above center on screen: it must land on the
                          // ground plane north of the target (world -z relative to view).
        let picked = camera.ground_point(Vec2::new(0.0, 0.5));
        let Some(picked) = picked else {
            panic!("a point inside the frustum must hit the ground");
        };
        assert!(picked.y.abs() < 1e-4, "always on the logical plane");
        assert!(
            (picked - camera.target()).length() > 1.0,
            "off-center picks move away from the target"
        );
    }

    #[test]
    fn picking_above_the_horizon_returns_none() {
        let mut camera = RtsCamera::new(64, 64, 16.0 / 9.0);
        // Minimum pitch (0.25 rad) minus half the 45-degree fov puts the top
        // edge of the frustum *above* the horizontal — rays through the upper
        // screen never meet the ground plane.
        camera.set_pitch(0.0); // clamps to the minimum
        assert!(
            camera.ground_point(Vec2::new(0.0, 0.99)).is_none(),
            "a cursor above the horizon must not hit the ground"
        );
        // The lower half of the screen still picks fine.
        assert!(camera.ground_point(Vec2::new(0.0, -0.5)).is_some());
    }

    #[test]
    fn box_selection_selects_only_entities_inside_the_rectangle() {
        let camera = RtsCamera::new(64, 64, 16.0 / 9.0);
        let center = camera.target();
        let entities = vec![
            entity_at(1, center.x - 8.0, center.z),
            entity_at(2, center.x, center.z),
            entity_at(3, center.x + 8.0, center.z),
        ];
        // Build the rectangle from the middle entity's actual projection (the
        // frustum math is the camera's job; this test pins the containment,
        // normalization, and ordering behavior of the selection itself).
        let middle_ndc = camera.project(entities[1].pos);
        let eps = 0.02;
        let selected = camera.box_select(
            &entities,
            Vec2::new(middle_ndc.x - eps, middle_ndc.y - eps),
            Vec2::new(middle_ndc.x + eps, middle_ndc.y + eps),
        );
        assert_eq!(selected, vec![EntityId(2)], "only the middle entity");
        // A full-screen rectangle selects everything, ascending by id.
        let all = camera.box_select(&entities, Vec2::new(-1.0, -1.0), Vec2::new(1.0, 1.0));
        assert_eq!(all, vec![EntityId(1), EntityId(2), EntityId(3)]);
        // Corners in either order (drag direction) select the same set.
        let flipped = camera.box_select(
            &entities,
            Vec2::new(middle_ndc.x + eps, middle_ndc.y + eps),
            Vec2::new(middle_ndc.x - eps, middle_ndc.y - eps),
        );
        assert_eq!(flipped, vec![EntityId(2)]);
    }

    #[test]
    fn pan_moves_the_target_and_zoom_rot_obey_their_ranges() {
        let mut camera = RtsCamera::new(64, 64, 16.0 / 9.0);
        let before = camera.target();
        camera.pan(4.0, 2.0);
        let after = camera.target();
        assert!((after - before).length() > 3.0, "the target moved");
        assert!(after.y.abs() < 1e-6, "the target stays on the ground plane");

        camera.zoom(0.001); // hard toward minimum
        camera.zoom(10.0);
        let eye = camera.eye();
        let distance = (eye - camera.target()).length();
        assert!(
            distance > 7.0 && distance < 165.0,
            "distance clamped: {distance}"
        );

        camera.rotate(std::f32::consts::PI);
        let rotated = camera.eye();
        assert!(
            (rotated.x - eye.x).abs() > 1.0 || (rotated.z - eye.z).abs() > 1.0,
            "rotation orbits the target"
        );
        assert!(
            (rotated.y - eye.y).abs() < 1e-4,
            "rotation keeps the height"
        );
    }

    #[test]
    fn picked_ground_points_convert_back_to_logical_positions() {
        let camera = RtsCamera::new(64, 64, 16.0 / 9.0);
        let picked = camera.ground_point(Vec2::ZERO).expect("center on map");
        let logical = MatchHost::world_to_logical(picked.x, picked.z);
        let back = logical_to_world(logical);
        assert!(
            (back.x - picked.x).abs() < 1e-3 && (back.z - picked.z).abs() < 1e-3,
            "round trip through fixed point stays put: {back:?} vs {picked:?}"
        );
        assert_eq!(
            MatchHost::world_to_logical(12.0, 8.0),
            Vec2Fx::new(Fx::from_int(12), Fx::from_int(8))
        );
    }
}
