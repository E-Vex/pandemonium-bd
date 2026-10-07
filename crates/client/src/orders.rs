//! Right-click context resolution (plan §11.3: "right-click context
//! command (move/attack/gather resolved from what's under the cursor)"):
//! what sits under the cursor decides the order. An enemy entity under the
//! cursor orders `Attack`; a neutral resource node under the cursor orders
//! `Gather` for the gather-capable part of the selection (soldiers
//! right-clicked onto a node just walk there); open ground orders `Move`.
//!
//! M9.1 (the DEBT-008 human pass): before this, right-click always issued a
//! plain `Move` — the player could not order an attack at all ("I can't
//! give my army command to attack"), and workers could not be sent to
//! gather by clicking a node.
//!
//! This is pure geometry over the render snapshot and the camera — the
//! same projection the renderer and the box select use, so what the
//! resolver sees is what is on screen. The command gate stays the sole
//! authority on legality (an enemy the fog hides still rejects `NotVisible`,
//! and the rejection surfaces as client feedback).

use pandemonium_engine::{world_to_fx, RenderEntity, RenderSnapshot, RtsCamera};
use pandemonium_sim_api::{EntityId, KindId, PlayerId, Vec2Fx};

/// How far (in NDC) the cursor may sit from an entity's projection for the
/// click to count as "on" it — the same radius single-click selection uses.
const PICK_RADIUS: f32 = 0.05;

/// What the right-click resolved to, and everything the submit path needs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ContextOrder {
    /// Attack the entity under the cursor. `at` is its ground position
    /// (the acknowledgment ping's point).
    Attack {
        /// The enemy entity to attack.
        target: EntityId,
        /// The target's ground position (for the ping).
        at: Vec2Fx,
    },
    /// Gather the resource node under the cursor, with the gather-capable
    /// subset of the selection (units whose kind is the content's worker).
    Gather {
        /// The node to gather from.
        node: EntityId,
        /// The node's ground position (for the ping).
        at: Vec2Fx,
        /// The selection's gather-capable units (workers).
        units: Vec<EntityId>,
    },
    /// Move to the picked ground point.
    Move {
        /// The fixed-point ground target.
        target: Vec2Fx,
    },
}

/// Resolves the right-click order for the selection from what is under the
/// cursor. `own` is the clicking player's slot; `node_kind` and
/// `worker_kind` are the content's gatherable-node and worker kind ids (the
/// engine's alpha-plan resolution supplies them). `None` when the cursor is
/// above the horizon (no ground, nothing picked) or the selection is empty.
pub fn resolve_context_order(
    camera: &RtsCamera,
    snapshot: &RenderSnapshot,
    cursor: glam::Vec2,
    own: PlayerId,
    node_kind: Option<KindId>,
    worker_kind: Option<KindId>,
    selection: &[EntityId],
) -> Option<ContextOrder> {
    if selection.is_empty() {
        return None;
    }
    // 1. An enemy under the cursor orders Attack.
    if let Some(enemy) = pick_entity(camera, &snapshot.entities, cursor, |entity| {
        entity.owner != own && entity.owner != PlayerId::NEUTRAL
    }) {
        return Some(ContextOrder::Attack {
            target: enemy.id,
            at: logical_of(enemy.pos),
        });
    }
    // 2. A resource node under the cursor orders Gather — but only when the
    //    selection holds gather-capable units (workers); otherwise the click
    //    falls through to a Move so soldiers walk to the node instead.
    if let Some(node) = pick_entity(camera, &snapshot.entities, cursor, |entity| {
        entity.owner == PlayerId::NEUTRAL && Some(entity.kind) == node_kind
    }) {
        let units: Vec<EntityId> = snapshot
            .entities
            .iter()
            .filter(|entity| Some(entity.kind) == worker_kind && selection.contains(&entity.id))
            .map(|entity| entity.id)
            .collect();
        if !units.is_empty() {
            return Some(ContextOrder::Gather {
                node: node.id,
                at: logical_of(node.pos),
                units,
            });
        }
    }
    // 3. Open ground orders Move.
    let ground = camera.ground_point(cursor)?;
    Some(ContextOrder::Move {
        target: Vec2Fx::new(world_to_fx(ground.x), world_to_fx(ground.z)),
    })
}

/// The nearest entity to the cursor whose projection is within
/// [`PICK_RADIUS`] and that passes `predicate`.
fn pick_entity<'a>(
    camera: &RtsCamera,
    entities: &'a [RenderEntity],
    cursor: glam::Vec2,
    predicate: impl Fn(&RenderEntity) -> bool,
) -> Option<&'a RenderEntity> {
    let mut best: Option<(f32, &'a RenderEntity)> = None;
    for entity in entities {
        if !predicate(entity) {
            continue;
        }
        let projected = camera.project(entity.pos);
        if !projected.x.is_finite() || !projected.y.is_finite() {
            continue; // behind the camera — not on screen, not clickable
        }
        let distance = (projected - cursor).length();
        if distance <= PICK_RADIUS && best.is_none_or(|(best_distance, _)| distance < best_distance)
        {
            best = Some((distance, entity));
        }
    }
    best.map(|(_, entity)| entity)
}

/// Converts a world ground-plane position to the logical fixed-point pair.
fn logical_of(pos: glam::Vec3) -> Vec2Fx {
    Vec2Fx::new(world_to_fx(pos.x), world_to_fx(pos.z))
}

/// The nearest visible entity of ANY owner under the cursor — the hover
/// tooltip's pick (PLAN-M10.2 §2.3). Pure geometry over the same projection
/// the selection and the context resolver use: what is under the cursor is
/// what the player sees. Passive by nature — it issues nothing and touches
/// no input state (the right-button machine is unaffected).
pub fn pick_visible_entity<'a>(
    camera: &RtsCamera,
    entities: &'a [RenderEntity],
    cursor: glam::Vec2,
) -> Option<&'a RenderEntity> {
    pick_entity(camera, entities, cursor, |_| true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandemonium_sim_api::MoveState;

    const OWN: PlayerId = PlayerId(0);
    const ENEMY_OWNER: PlayerId = PlayerId(1);
    const WORKER: KindId = KindId(1);
    const SOLDIER: KindId = KindId(2);
    const NODE: KindId = KindId(3);

    fn entity(id: u64, owner: PlayerId, kind: KindId, pos: glam::Vec3) -> RenderEntity {
        RenderEntity {
            id: EntityId(id),
            owner,
            kind,
            pos,
            facing: glam::Vec3::ZERO,
            hp_fraction_milli: 1000,
            move_state: MoveState::Idle,
        }
    }

    fn camera_on(pos: glam::Vec3) -> RtsCamera {
        let mut camera = RtsCamera::new(64, 64, 16.0 / 9.0);
        camera.focus(pos.x, pos.z, 24.0);
        camera
    }

    fn snapshot(entities: Vec<RenderEntity>) -> RenderSnapshot {
        RenderSnapshot { tick: 0, entities }
    }

    #[test]
    fn an_enemy_under_the_cursor_orders_attack() {
        let enemy = entity(7, ENEMY_OWNER, SOLDIER, glam::Vec3::new(20.0, 0.0, 30.0));
        let camera = camera_on(enemy.pos);
        let snapshot = snapshot(vec![enemy]);
        let cursor = camera.project(glam::Vec3::new(20.0, 0.0, 30.0));
        let resolved = resolve_context_order(
            &camera,
            &snapshot,
            cursor,
            OWN,
            Some(NODE),
            Some(WORKER),
            &[EntityId(1)],
        )
        .expect("the cursor is on the enemy");
        assert_eq!(
            resolved,
            ContextOrder::Attack {
                target: EntityId(7),
                at: Vec2Fx::new(world_to_fx(20.0), world_to_fx(30.0)),
            }
        );
    }

    #[test]
    fn a_node_under_a_worker_selection_orders_gather() {
        let node = entity(9, PlayerId::NEUTRAL, NODE, glam::Vec3::new(18.0, 0.0, 18.0));
        let worker = entity(2, OWN, WORKER, glam::Vec3::new(24.0, 0.0, 24.0));
        let camera = camera_on(node.pos);
        let snapshot = snapshot(vec![node, worker]);
        let cursor = camera.project(node.pos);
        let resolved = resolve_context_order(
            &camera,
            &snapshot,
            cursor,
            OWN,
            Some(NODE),
            Some(WORKER),
            &[EntityId(2)],
        )
        .expect("the cursor is on the node");
        assert_eq!(
            resolved,
            ContextOrder::Gather {
                node: EntityId(9),
                at: Vec2Fx::new(world_to_fx(18.0), world_to_fx(18.0)),
                units: vec![EntityId(2)],
            }
        );
    }

    #[test]
    fn a_node_under_soldiers_falls_through_to_move() {
        // Soldiers cannot gather: right-clicking a node sends them walking
        // to it (the gather subset is empty, so the ground click wins).
        let node = entity(9, PlayerId::NEUTRAL, NODE, glam::Vec3::new(18.0, 0.0, 18.0));
        let soldier = entity(3, OWN, SOLDIER, glam::Vec3::new(24.0, 0.0, 24.0));
        let camera = camera_on(node.pos);
        let snapshot = snapshot(vec![node, soldier]);
        let cursor = camera.project(node.pos);
        let resolved = resolve_context_order(
            &camera,
            &snapshot,
            cursor,
            OWN,
            Some(NODE),
            Some(WORKER),
            &[EntityId(3)],
        )
        .expect("open ground under the node");
        assert!(
            matches!(resolved, ContextOrder::Move { .. }),
            "soldiers walk to the node: {resolved:?}"
        );
    }

    #[test]
    fn a_mixed_selection_gathers_only_its_workers() {
        let node = entity(9, PlayerId::NEUTRAL, NODE, glam::Vec3::new(18.0, 0.0, 18.0));
        let worker = entity(2, OWN, WORKER, glam::Vec3::new(24.0, 0.0, 24.0));
        let soldier = entity(3, OWN, SOLDIER, glam::Vec3::new(24.0, 0.0, 24.0));
        let camera = camera_on(node.pos);
        let snapshot = snapshot(vec![node, worker, soldier]);
        let cursor = camera.project(node.pos);
        let resolved = resolve_context_order(
            &camera,
            &snapshot,
            cursor,
            OWN,
            Some(NODE),
            Some(WORKER),
            &[EntityId(2), EntityId(3)],
        )
        .expect("the cursor is on the node");
        assert_eq!(
            resolved,
            ContextOrder::Gather {
                node: EntityId(9),
                at: Vec2Fx::new(world_to_fx(18.0), world_to_fx(18.0)),
                units: vec![EntityId(2)],
            },
            "only the worker gathers; the soldier is left out of the order"
        );
    }

    #[test]
    fn open_ground_orders_move_at_the_picked_point() {
        let camera = camera_on(glam::Vec3::new(32.0, 0.0, 32.0));
        let snapshot = snapshot(vec![]);
        let cursor = glam::Vec2::new(0.3, -0.2);
        let ground = camera
            .ground_point(cursor)
            .expect("the cursor is on the ground");
        let resolved = resolve_context_order(
            &camera,
            &snapshot,
            cursor,
            OWN,
            Some(NODE),
            Some(WORKER),
            &[EntityId(1)],
        )
        .expect("open ground resolves");
        assert_eq!(
            resolved,
            ContextOrder::Move {
                target: Vec2Fx::new(world_to_fx(ground.x), world_to_fx(ground.z)),
            }
        );
    }

    #[test]
    fn an_empty_selection_resolves_to_nothing() {
        let camera = camera_on(glam::Vec3::new(32.0, 0.0, 32.0));
        let snapshot = snapshot(vec![]);
        assert!(resolve_context_order(
            &camera,
            &snapshot,
            glam::Vec2::ZERO,
            OWN,
            Some(NODE),
            Some(WORKER),
            &[]
        )
        .is_none());
    }

    #[test]
    fn a_cursor_above_the_horizon_resolves_to_nothing() {
        // Minimum pitch puts the top of the frustum above the horizontal:
        // a cursor there has no ground point and picks nothing.
        let mut camera = RtsCamera::new(64, 64, 16.0 / 9.0);
        camera.set_pitch(0.0); // clamps to the minimum
        let snapshot = snapshot(vec![]);
        assert!(resolve_context_order(
            &camera,
            &snapshot,
            glam::Vec2::new(0.0, 0.99),
            OWN,
            Some(NODE),
            Some(WORKER),
            &[EntityId(1)]
        )
        .is_none());
    }

    #[test]
    fn own_units_under_the_cursor_are_not_attack_targets() {
        // Right-clicking one of your own units must not order an attack on
        // it — the fall-through is a Move (walk to that ground point).
        let own_unit = entity(5, OWN, SOLDIER, glam::Vec3::new(20.0, 0.0, 30.0));
        let camera = camera_on(own_unit.pos);
        let snapshot = snapshot(vec![own_unit]);
        let cursor = camera.project(own_unit.pos);
        let resolved = resolve_context_order(
            &camera,
            &snapshot,
            cursor,
            OWN,
            Some(NODE),
            Some(WORKER),
            &[EntityId(6)],
        )
        .expect("the ground under the unit resolves");
        assert!(
            matches!(resolved, ContextOrder::Move { .. }),
            "own entities are never attack targets: {resolved:?}"
        );
    }

    #[test]
    fn the_hover_pick_finds_any_owner_under_the_cursor() {
        let worker = entity(2, OWN, WORKER, glam::Vec3::new(24.0, 0.0, 24.0));
        let camera = camera_on(worker.pos);
        let snapshot = snapshot(vec![worker]);
        let cursor = camera.project(glam::Vec3::new(24.0, 0.0, 24.0));
        let picked = pick_visible_entity(&camera, &snapshot.entities, cursor)
            .expect("the cursor rests on the worker");
        assert_eq!(picked.id, EntityId(2));
    }

    #[test]
    fn the_hover_pick_takes_the_nearest_and_skips_off_cursor() {
        let near = entity(2, OWN, WORKER, glam::Vec3::new(20.0, 0.0, 30.0));
        let far = entity(3, ENEMY_OWNER, SOLDIER, glam::Vec3::new(21.5, 0.0, 30.0));
        let camera = camera_on(near.pos);
        let snapshot = snapshot(vec![near, far]);
        let cursor = camera.project(glam::Vec3::new(20.0, 0.0, 30.0));
        let picked = pick_visible_entity(&camera, &snapshot.entities, cursor)
            .expect("two entities in radius");
        assert_eq!(picked.id, EntityId(2), "the nearest wins");
        // Off-entity: nothing hovers.
        let away = camera.project(glam::Vec3::new(40.0, 0.0, 40.0));
        assert!(pick_visible_entity(&camera, &snapshot.entities, away).is_none());
    }
}
