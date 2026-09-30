//! Snapshot interpolation (plan §11.1, §6.4): the client keeps the previous and
//! current snapshot and renders the blend between them by the accumulator
//! fraction, so display-rate rendering looks smooth while the simulation steps
//! at a fixed 30 Hz.
//!
//! This is presentation code: the interpolation lives in floats, here in the
//! engine (ADR-0001 — floats are legal above the sim boundary; the simulation
//! itself never sees a blended position). Positions cross the boundary once per
//! snapshot via [`fx_to_world`] (Q16.16 raw -> ground-plane coordinates).
//!
//! Axis convention (ADR-0001): the simulation's 2D logical plane maps to the 3D
//! ground plane as `world.x = sim.x`, `world.z = sim.y` (height is `world.y`,
//! zero until the renderer displaces terrain by the display-only heightmap).

use glam::Vec3;
use pandemonium_fx::{Fx, Vec2Fx};
use pandemonium_sim_api::{
    EntityId, KindId, MoveState, PlayerId, Snapshot, Tick, Vec2Fx as LogicalPos,
};

/// Converts one fixed-point coordinate to a ground-plane world coordinate.
pub fn fx_to_world(value: Fx) -> f32 {
    value.raw() as f32 / 65536.0
}

/// Converts a ground-plane world coordinate back to fixed point (nearest
/// representable, saturating) — the picking boundary conversion, so cursor
/// positions become command targets the simulation accepts (ADR-0001 §11.3).
pub fn world_to_fx(value: f32) -> Fx {
    let raw = (value * 65536.0)
        .round()
        .clamp(i32::MIN as f32, i32::MAX as f32);
    Fx::from_raw(raw as i32)
}

/// Converts a logical-plane position to a ground-plane world position
/// (height zero — the renderer adds terrain height from the display-only
/// heightmap).
pub fn logical_to_world(pos: Vec2Fx) -> Vec3 {
    Vec3::new(fx_to_world(pos.x), 0.0, fx_to_world(pos.y))
}

/// One entity as rendering sees it: the interpolated projection of a
/// [`pandemonium_sim_api::EntityView`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RenderEntity {
    /// Identity (never reused — a stable render key).
    pub id: EntityId,
    /// Owning slot (team color).
    pub owner: PlayerId,
    /// Which kind template (placeholder model choice).
    pub kind: KindId,
    /// Interpolated ground-plane position.
    pub pos: Vec3,
    /// Interpolated facing direction (unit length or zero before first motion).
    pub facing: Vec3,
    /// Health as thousandths of maximum (health bars).
    pub hp_fraction_milli: u32,
    /// What the movement system is doing (animation state).
    pub move_state: MoveState,
}

/// One renderable frame: the interpolated entities (ascending by id, as the
/// snapshot projects them) at the frame's interpolation point.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct RenderSnapshot {
    /// The tick being interpolated *toward* (the current snapshot's tick).
    pub tick: Tick,
    /// Every live entity, ascending by id.
    pub entities: Vec<RenderEntity>,
}

/// Holds the previous and current snapshot and blends them on demand.
///
/// Push a snapshot after every simulation step; render every display frame
/// with [`Interpolator::render`] and the loop's alpha.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Interpolator {
    previous: Option<Snapshot>,
    current: Option<Snapshot>,
}

impl Interpolator {
    /// An empty interpolator (render yields an empty snapshot until the first
    /// push).
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the newest snapshot; the last current becomes the previous.
    pub fn push(&mut self, snapshot: Snapshot) {
        self.previous = self.current.take();
        self.current = Some(snapshot);
    }

    /// Blends toward the current snapshot by `alpha` (clamped to `0.0..=1.0`).
    ///
    /// Rules (documented, deterministic per frame): an entity present in both
    /// snapshots lerps position and facing; an entity only in the current
    /// snapshot appears at its current position (no backward extrapolation);
    /// an entity only in the previous snapshot is gone (death fades are a
    /// presentation effect driven by events, plan §11.5 — not fabricated
    /// here). Discrete fields (owner, kind, hp fraction, move state) always
    /// come from the current snapshot.
    pub fn render(&self, alpha: f32) -> RenderSnapshot {
        let Some(current) = &self.current else {
            return RenderSnapshot::default();
        };
        let alpha = alpha.clamp(0.0, 1.0);
        let previous = self.previous.as_ref();
        let entities = current
            .entities
            .iter()
            .map(|view| {
                let old = previous.and_then(|prev| {
                    prev.entities
                        .iter()
                        .find(|candidate| candidate.id == view.id)
                });
                let pos = match old {
                    Some(old) => lerp_world(&old.pos, &view.pos, alpha),
                    None => logical_to_world(view.pos),
                };
                let facing = match old {
                    Some(old) => {
                        let blended =
                            logical_to_world(old.facing).lerp(logical_to_world(view.facing), alpha);
                        let length = blended.length();
                        if length > f32::EPSILON {
                            blended / length
                        } else {
                            blended
                        }
                    }
                    None => logical_to_world(view.facing),
                };
                RenderEntity {
                    id: view.id,
                    owner: view.owner,
                    kind: view.kind,
                    pos,
                    facing,
                    hp_fraction_milli: view.hp_fraction_milli,
                    move_state: view.move_state,
                }
            })
            .collect();
        RenderSnapshot {
            tick: current.tick,
            entities,
        }
    }
}

/// Blends two logical positions into a ground-plane position.
fn lerp_world(from: &LogicalPos, to: &LogicalPos, alpha: f32) -> Vec3 {
    logical_to_world(*from).lerp(logical_to_world(*to), alpha.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandemonium_fx::Fx;
    use pandemonium_sim_api::EntityView;

    fn view(id: u64, milli_x: i32, milli_y: i32, _tick: Tick) -> EntityView {
        EntityView {
            id: EntityId(id),
            owner: PlayerId(0),
            kind: KindId(0),
            pos: Vec2Fx::new(Fx::from_milli(milli_x), Fx::from_milli(milli_y)),
            facing: Vec2Fx::ZERO,
            hp_fraction_milli: 1000,
            move_state: MoveState::Idle,
        }
    }

    #[test]
    fn fx_converts_to_ground_plane_coordinates() {
        assert_eq!(fx_to_world(Fx::from_int(2)), 2.0);
        assert!((fx_to_world(Fx::from_milli(500)) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn render_blends_positions_by_alpha() {
        let mut interp = Interpolator::new();
        interp.push(Snapshot {
            tick: 0,
            entities: vec![view(1, 0, 0, 0)],
        });
        interp.push(Snapshot {
            tick: 1,
            entities: vec![view(1, 10_000, 0, 1)],
        });
        let mid = interp.render(0.5);
        assert_eq!(mid.tick, 1);
        assert_eq!(mid.entities.len(), 1);
        assert!(
            (mid.entities[0].pos.x - 5.0).abs() < 1e-4,
            "halfway in tiles"
        );
        let start = interp.render(0.0);
        assert!((start.entities[0].pos.x - 0.0).abs() < 1e-6);
        let end = interp.render(1.0);
        assert!((end.entities[0].pos.x - 10.0).abs() < 1e-4);
        // Alpha outside the unit range clamps, never extrapolates.
        let over = interp.render(2.0);
        assert!((over.entities[0].pos.x - 10.0).abs() < 1e-4);
    }

    #[test]
    fn spawns_appear_at_current_and_deaths_disappear() {
        let mut interp = Interpolator::new();
        interp.push(Snapshot {
            tick: 0,
            entities: vec![view(1, 0, 0, 0), view(2, 1_000, 1_000, 0)],
        });
        interp.push(Snapshot {
            tick: 1,
            entities: vec![view(2, 3_000, 3_000, 1), view(3, 9_000, 9_000, 1)],
        });
        let frame = interp.render(0.5);
        let ids: Vec<EntityId> = frame.entities.iter().map(|e| e.id).collect();
        assert_eq!(
            ids,
            vec![EntityId(2), EntityId(3)],
            "ascending, dead gone, new present"
        );
        // Entity 3 exists only in current: appears at its current position.
        let spawned = frame.entities.iter().find(|e| e.id == EntityId(3)).unwrap();
        assert!(
            (spawned.pos.x - 9.0).abs() < 1e-4,
            "no backward extrapolation"
        );
        // Entity 2 blends 1.0 -> 3.0 tiles at alpha 0.5.
        let mover = frame.entities.iter().find(|e| e.id == EntityId(2)).unwrap();
        assert!((mover.pos.x - 2.0).abs() < 1e-4);
    }

    #[test]
    fn empty_interpolator_renders_empty() {
        assert!(Interpolator::new().render(0.5).entities.is_empty());
    }

    #[test]
    fn facing_blends_and_normalizes() {
        let mut interp = Interpolator::new();
        let mut a = view(1, 0, 0, 0);
        a.facing = Vec2Fx::new(Fx::from_int(1), Fx::from_int(0));
        let mut b = view(1, 0, 0, 1);
        b.facing = Vec2Fx::new(Fx::from_int(0), Fx::from_int(1));
        interp.push(Snapshot {
            tick: 0,
            entities: vec![a],
        });
        interp.push(Snapshot {
            tick: 1,
            entities: vec![b],
        });
        let frame = interp.render(0.5);
        let facing = frame.entities[0].facing;
        assert!(
            (facing.length() - 1.0).abs() < 1e-4,
            "blended facing stays unit"
        );
        assert!(
            (facing.x - facing.z).abs() < 1e-4,
            "halfway between +x and +z"
        );
    }
}
