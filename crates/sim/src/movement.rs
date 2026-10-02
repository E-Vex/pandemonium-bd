//! The movement system (plan §9.1's three layers, M4): path requests over
//! the [`NavGrid`], waypoint steering, deterministic collision push-apart
//! with a spatial hash grid, and stuck detection with repath escalation that
//! ends in [`Event::MoveFailed`].
//!
//! Order of work inside stage 6 (entities always processed in ascending id
//! order, plan §6.3.6):
//!
//! 1. **Path requests** — every ordered mover with an empty path computes one;
//!    an unreachable goal (or a zero-speed mover with a distant goal) fails
//!    the order immediately (`MoveFailed`).
//! 2. **Steering** — each mover advances toward `path[0]` by its speed,
//!    snapping onto waypoints it reaches; consuming the final waypoint
//!    completes the order.
//! 3. **Collision push-apart** — unit pairs closer than the sum of their radii
//!    are separated symmetrically, each half-checking the terrain so a push
//!    never lands a unit on a blocked tile (or outside the map).
//! 4. **Stuck detection** — a mover whose net progress this tick is under a
//!    quarter of its speed accumulates `stuck_ticks`; past the threshold it
//!    either completes as a crowded arrival (close enough to the order
//!    target), repaths, or — after too many repaths — gives up with
//!    `MoveFailed`. No order can stay unresolved forever.
//!
//! All arithmetic is fixed point (plan §5): squared-distance comparisons, the
//! exact integer root only where a length is needed, and defined saturating
//! operations throughout.

use pandemonium_fx::{Fx, Vec2Fx};
use pandemonium_sim_api::{EntityId, Event};

use crate::nav::NavGrid;
use crate::world::{Order, World};

/// Consecutive blocked ticks before a stuck mover escalates (1 second).
const STUCK_TICKS: u32 = 30;
/// Repath attempts allowed per order before giving up with `MoveFailed`.
const MAX_REPATHS: u32 = 4;
/// A stuck mover within this distance *plus its own radius* of its order
/// target completes the order (a crowded arrival — units cluster around a
/// popular destination instead of grinding into it forever). The body-aware
/// radius keeps up with the ring a packed group naturally forms. In
/// milli-tiles.
const CROWD_ARRIVE_MILLI: i32 = 1000;
/// Net progress below this fraction of top speed counts as a blocked tick.
const BLOCKED_FRACTION: i64 = 4;
/// Spatial hash cell size in tiles (radii are fractions of a tile, so a
/// 3x3 neighborhood scan sees every interacting pair).
const CELL_SIZE: i32 = 1;

/// Advances stage 6: path requests, steering, push-apart, stuck detection.
pub(crate) fn advance_movement(world: &mut World, nav: &NavGrid, events: &mut Vec<Event>) {
    request_paths(world, nav, events);
    let positions_before = steer(world, nav);
    push_apart(world, nav);
    detect_stuck(world, &positions_before, events);
}

/// The destination of an order. `MoveTo` carries its target inline; economy
/// orders (`GatherAt`, `BuildAt`) compute theirs from world state in stage 4
/// — until their systems land (M5), they resolve to no movement target.
fn order_target(order: &Order) -> Option<Vec2Fx> {
    match order {
        Order::MoveTo { target } => Some(*target),
        Order::GatherAt { .. } | Order::BuildAt { .. } => None,
    }
}

/// Layer 1 — path requests: ordered movers with an empty path compute one
/// from their current position to the current order's target.
fn request_paths(world: &mut World, nav: &NavGrid, events: &mut Vec<Event>) {
    let needing: Vec<(EntityId, Option<Vec2Fx>)> = world
        .entities
        .iter()
        .filter(|entity| !entity.orders.is_empty())
        .filter(|entity| {
            world
                .move_of(entity.id)
                .is_some_and(|def| def.path.is_empty())
        })
        .map(|entity| (entity.id, order_target(&entity.orders[0])))
        .collect();
    for (id, target) in needing {
        let Some(target) = target else {
            continue; // the order resolves no movement target this tick
        };
        let from = world.entity(id).expect("the id came from the store").pos;
        let speed = world
            .move_of(id)
            .expect("the filter guaranteed a mover")
            .speed_per_tick;
        if speed <= Fx::ZERO && Vec2Fx::dist(from, target) > Fx::ZERO {
            // A mover with no speed can never make progress on a distant
            // order; fail it instead of leaving it permanently mid-order.
            fail_order(world, id, events);
            continue;
        }
        match nav.find_path(from, target) {
            Some(path) => {
                if let Some(def) = world.move_of_mut(id) {
                    def.path = path;
                }
            }
            None => fail_order(world, id, events),
        }
    }
}

/// Layer 2 — steering: each mover advances toward its next waypoint. A
/// landing on blocked terrain refuses the step (paths are built to make this
/// unreachable, but displacement by collision or a wedged start can bend a
/// leg — the refused step then counts as blocked progress and escalates
/// through the stuck machinery). Returns every mover's position before
/// steering (the stuck detector's reference).
fn steer(world: &mut World, nav: &NavGrid) -> Vec<(EntityId, Vec2Fx)> {
    let movers: Vec<(EntityId, Fx)> = world
        .entities
        .iter()
        .filter(|entity| !entity.orders.is_empty())
        .filter_map(|entity| {
            world
                .move_of(entity.id)
                .map(|def| (entity.id, def.speed_per_tick))
        })
        .collect();
    let mut positions = Vec::with_capacity(movers.len());
    for (id, speed) in movers {
        let Some(entity) = world.entity(id) else {
            continue;
        };
        positions.push((id, entity.pos));
        let next = world.move_of(id).and_then(|def| def.path.first().copied());
        let Some(target) = next else {
            continue; // the path is built this tick or the order just failed
        };
        let delta = target - entity.pos;
        if delta.is_zero() {
            consume_waypoint(world, id);
            continue;
        }
        let distance = delta.len();
        let direction = delta.normalized();
        let arrives = distance <= speed;
        let mut reached_waypoint = false;
        if let Some(entity) = world.entity_mut(id) {
            entity.facing = direction;
            let candidate = if arrives {
                target
            } else {
                entity.pos + direction.scale(speed)
            };
            let from_tile = (entity.pos.x.floor_int(), entity.pos.y.floor_int());
            let to_tile = (candidate.x.floor_int(), candidate.y.floor_int());
            let terrain_allows = nav.passable(to_tile.0, to_tile.1) || from_tile == to_tile;
            if terrain_allows {
                entity.pos = candidate;
                reached_waypoint = arrives;
            }
        }
        if reached_waypoint {
            consume_waypoint(world, id);
        }
    }
    positions
}

/// Consumes the reached waypoint; an emptied path completes the order.
fn consume_waypoint(world: &mut World, id: EntityId) {
    let completed = {
        let Some(def) = world.move_of_mut(id) else {
            return;
        };
        if !def.path.is_empty() {
            def.path.remove(0);
        }
        def.path.is_empty()
    };
    if completed {
        if let Some(entity) = world.entity_mut(id) {
            if !entity.orders.is_empty() {
                entity.orders.remove(0);
            }
        }
        if let Some(def) = world.move_of_mut(id) {
            def.reset_runtime();
        }
    }
}

/// Layer 3 — deterministic local push-apart (plan §9.1.3): a spatial hash
/// grid of one-tile cells (cell lists ordered by id, since units are inserted
/// in ascending id order), every pair `(a, b)` with `a < b` processed exactly
/// once in a fixed cell scan order, each entity pushed half the overlap along
/// the separation axis. A half-push whose landing tile is blocked (or off the
/// map) is skipped, so collision never shoves a unit into terrain.
fn push_apart(world: &mut World, nav: &NavGrid) {
    let units: Vec<EntityId> = world
        .entities
        .iter()
        .filter(|entity| world.move_of(entity.id).is_some())
        .map(|entity| entity.id)
        .collect();
    let cells_x = ((nav.width() as i64) / (CELL_SIZE as i64)).max(1) as i32;
    let cells_y = ((nav.height() as i64) / (CELL_SIZE as i64)).max(1) as i32;
    let mut cells: Vec<Vec<EntityId>> =
        vec![Vec::new(); (cells_x as u64 * cells_y as u64) as usize];
    let mut unit_cells: Vec<(EntityId, i32, i32)> = Vec::with_capacity(units.len());
    for id in &units {
        let Some(entity) = world.entity(*id) else {
            continue;
        };
        let cell_x = (entity.pos.x.floor_int() / CELL_SIZE).clamp(0, cells_x - 1);
        let cell_y = (entity.pos.y.floor_int() / CELL_SIZE).clamp(0, cells_y - 1);
        cells[cell_y as usize * cells_x as usize + cell_x as usize].push(*id);
        unit_cells.push((*id, cell_x, cell_y));
    }

    // Fixed neighbor scan order; pairs resolve once with a < b.
    for (a, cell_x, cell_y) in &unit_cells {
        for (dy, dx) in [
            (0, -1),
            (0, 0),
            (0, 1),
            (1, -1),
            (1, 0),
            (1, 1),
            (-1, -1),
            (-1, 0),
            (-1, 1),
        ] {
            let nx = cell_x + dx;
            let ny = cell_y + dy;
            if nx < 0 || ny < 0 || nx >= cells_x || ny >= cells_y {
                continue;
            }
            let neighbors: &[EntityId] = &cells[ny as usize * cells_x as usize + nx as usize];
            for b in neighbors {
                if a >= b {
                    continue; // each pair once, in ascending order
                }
                resolve_pair(world, nav, *a, *b);
            }
        }
    }
}

/// Separates one overlapping pair: symmetric halves, terrain-guarded.
fn resolve_pair(world: &mut World, nav: &NavGrid, a: EntityId, b: EntityId) {
    let (Some(entity_a), Some(entity_b)) = (world.entity(a), world.entity(b)) else {
        return;
    };
    let (Some(def_a), Some(def_b)) = (world.move_of(a), world.move_of(b)) else {
        return;
    };
    let radius_sum = def_a.radius + def_b.radius;
    let sum_raw = radius_sum.raw().max(0) as u64;
    let sum_sq = sum_raw * sum_raw;
    let delta = entity_b.pos - entity_a.pos;
    if delta.len_sq_raw() >= sum_sq {
        return; // not overlapping
    }
    // The separation axis: between the two, or +x for exact coincidence (a
    // deterministic tie-break).
    let direction = if delta.is_zero() {
        Vec2Fx::new(Fx::ONE, Fx::ZERO)
    } else {
        delta.normalized()
    };
    let overlap = radius_sum - delta.len();
    let half = overlap.div(Fx::from_int(2));
    // Terrain-guarded halves: a push that would land on a blocked tile (or
    // outside the map) is skipped for that entity only.
    let push_a = entity_a.pos - direction.scale(half);
    let push_b = entity_b.pos + direction.scale(half);
    if nav.passable(push_a.x.floor_int(), push_a.y.floor_int()) {
        if let Some(entity) = world.entity_mut(a) {
            entity.pos = push_a;
        }
    }
    if nav.passable(push_b.x.floor_int(), push_b.y.floor_int()) {
        if let Some(entity) = world.entity_mut(b) {
            entity.pos = push_b;
        }
    }
}

/// Layer 4 — stuck detection: movers whose net progress this tick is under a
/// quarter of their speed accumulate stuck ticks; past the threshold the
/// order resolves as a crowded arrival (within `CROWD_ARRIVE_MILLI` of the
/// target), a repath, or a `MoveFailed` give-up.
fn detect_stuck(
    world: &mut World,
    positions_before: &[(EntityId, Vec2Fx)],
    events: &mut Vec<Event>,
) {
    for (id, before) in positions_before {
        let Some(entity) = world.entity(*id) else {
            continue;
        };
        if entity.orders.is_empty() {
            continue; // completed this tick — nothing to detect
        }
        let Some(target) = order_target(&entity.orders[0]) else {
            continue; // no movement target: not a traveling order this tick
        };
        let Some(def) = world.move_of(*id) else {
            continue;
        };
        let moved = Vec2Fx::dist(*before, entity.pos);
        let quarter = def
            .speed_per_tick
            .div(Fx::from_int(BLOCKED_FRACTION as i32));
        if def.speed_per_tick > Fx::ZERO && moved < quarter {
            let stuck_ticks = def.stuck_ticks + 1;
            if stuck_ticks <= STUCK_TICKS {
                if let Some(def) = world.move_of_mut(*id) {
                    def.stuck_ticks = stuck_ticks;
                }
                continue;
            }
            // Escalation: crowded arrival, repath, or give up.
            let crowd_radius = def.radius + Fx::from_milli(CROWD_ARRIVE_MILLI);
            if Vec2Fx::dist(entity.pos, target) <= crowd_radius {
                if let Some(entity) = world.entity_mut(*id) {
                    entity.orders.remove(0);
                }
                if let Some(def) = world.move_of_mut(*id) {
                    def.reset_runtime();
                }
            } else if def.repaths >= MAX_REPATHS {
                fail_order(world, *id, events);
            } else {
                // One more chance: rebuild the path from the current position
                // on the next tick.
                if let Some(def) = world.move_of_mut(*id) {
                    def.repaths += 1;
                    def.stuck_ticks = 0;
                    def.path.clear();
                }
            }
        } else if def.stuck_ticks != 0 {
            if let Some(def) = world.move_of_mut(*id) {
                def.stuck_ticks = 0;
            }
        }
    }
}

/// Fails the entity's current order: emits `MoveFailed` and drops it.
fn fail_order(world: &mut World, id: EntityId, events: &mut Vec<Event>) {
    if let Some(entity) = world.entity_mut(id) {
        if !entity.orders.is_empty() {
            entity.orders.remove(0);
        }
    }
    if let Some(def) = world.move_of_mut(id) {
        def.reset_runtime();
    }
    events.push(Event::MoveFailed { entity: id });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{
        CapTemplate, KindEconomy, KindTemplate, ResourceDef, SpawnDef, TrivialWorld,
    };
    use crate::sim::Sim;
    use crate::world::{CapabilityData, MoveDef, PlayerState};
    use pandemonium_sim_api::{
        Command, CommandKind, ControllerKind, KindId, MatchSetup, MoveState, PlayerId, PlayerSetup,
        ResourceId,
    };

    /// A world with one mover kind on an open 16x16 grid.
    fn world_with_mover(speed: i32, radius: i32) -> TrivialWorld {
        TrivialWorld {
            map_id: 0x4D_34,
            width_tiles: 16,
            height_tiles: 16,
            passability: TrivialWorld::open_passability(16, 16),
            buildability: TrivialWorld::open_buildability(16, 16),
            kinds: vec![KindTemplate {
                caps: vec![
                    CapTemplate::Health {
                        max_hp: 10,
                        regen_per_tick: 0,
                    },
                    CapTemplate::Move {
                        speed_milli_tiles_per_s: speed,
                        radius_milli_tiles: radius,
                    },
                    CapTemplate::Vision {
                        radius_milli_tiles: 5000,
                    },
                ],
                economy: KindEconomy::default(),
            }],
            resources: vec![ResourceDef {
                resource: ResourceId(0),
                starting: 200,
            }],
            production: vec![],
            base_population_cap: 0,
            initial_spawns: vec![],
            scheduled_spawns: vec![],
            spawn_jitter_milli: 0,
        }
    }

    fn setup_one_player() -> MatchSetup {
        MatchSetup {
            seed: 7,
            players: vec![PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            }],
        }
    }

    fn spawn_at(world: &mut TrivialWorld, x: i32, y: i32) {
        world.initial_spawns.push(SpawnDef {
            owner: PlayerId(0),
            kind: KindId(0),
            pos: Vec2Fx::new(Fx::from_milli(x), Fx::from_milli(y)),
        });
    }

    fn milli(x: i32, y: i32) -> Vec2Fx {
        Vec2Fx::new(Fx::from_milli(x), Fx::from_milli(y))
    }

    fn order_to(sim: &mut Sim, id: u64, target: Vec2Fx) -> crate::sim::StepOutput {
        sim.step(&[Command::new(
            PlayerId(0),
            sim.tick(),
            1,
            CommandKind::Move {
                units: vec![EntityId(id)],
                target,
            },
        )])
    }

    fn position_of(sim: &Sim, id: u64) -> Vec2Fx {
        sim.snapshot()
            .entities
            .iter()
            .find(|entity| entity.id == EntityId(id))
            .expect("entity exists")
            .pos
    }

    #[test]
    fn a_lone_mover_walks_straight_and_lands_exactly() {
        let mut world = world_with_mover(2600, 300);
        spawn_at(&mut world, 2000, 2000);
        let mut sim = Sim::new(&world, setup_one_player());
        sim.step(&[]); // drain the initial Spawned events
        order_to(&mut sim, 1, milli(9000, 2000));
        let mut ticks = 0;
        while sim
            .snapshot()
            .entities
            .iter()
            .any(|entity| entity.id == EntityId(1) && entity.move_state == MoveState::Moving)
        {
            sim.step(&[]);
            ticks += 1;
            assert!(ticks < 400, "mover never arrived");
        }
        assert_eq!(position_of(&sim, 1), milli(9000, 2000));
        // 7000 milli at ~86.7 milli/tick is ~81 ticks — the unobstructed
        // beeline is a single waypoint, so there is no detour overhead.
        assert!(
            (75..95).contains(&ticks),
            "unreasonable tick count: {ticks}"
        );
    }

    #[test]
    fn movers_detour_around_walls() {
        let mut world = world_with_mover(2600, 300);
        // A vertical wall at x=8 with a single gap at y=8.
        for y in 0..16 {
            world.passability[y as usize * 16 + 8] = u8::from(y == 8);
        }
        spawn_at(&mut world, 2000, 2000);
        let mut sim = Sim::new(&world, setup_one_player());
        sim.step(&[]);
        order_to(&mut sim, 1, milli(13_000, 2000));
        let mut ticks = 0;
        let mut crossed = false;
        while sim
            .snapshot()
            .entities
            .iter()
            .any(|entity| entity.id == EntityId(1) && entity.move_state == MoveState::Moving)
        {
            sim.step(&[]);
            ticks += 1;
            assert!(ticks < 1500, "mover never arrived around the wall");
            let pos = position_of(&sim, 1);
            let (tx, ty) = (pos.x.floor_int(), pos.y.floor_int());
            assert!(
                !(tx == 8 && ty != 8),
                "mover walked onto the wall at ({tx},{ty})"
            );
            if tx > 8 {
                crossed = true;
            }
        }
        assert!(crossed, "the mover must reach the far side");
        assert_eq!(position_of(&sim, 1).x.floor_int(), 13);
    }

    #[test]
    fn push_apart_separates_stacked_units() {
        let mut world = world_with_mover(2600, 400);
        spawn_at(&mut world, 8000, 8000);
        spawn_at(&mut world, 8000, 8000); // exactly stacked
        spawn_at(&mut world, 8000, 8000);
        let mut sim = Sim::new(&world, setup_one_player());
        // Idle units still collide: relaxation over a few ticks separates the
        // stack to (at least) the sum of two radii, minus a small tolerance.
        for _ in 0..30 {
            sim.step(&[]);
        }
        let tolerance = Fx::from_milli(80);
        let minimum = Fx::from_milli(800) - tolerance;
        let positions: Vec<Vec2Fx> = (1..=3).map(|id| position_of(&sim, id)).collect();
        for index in 0..positions.len() {
            for other in index + 1..positions.len() {
                assert!(
                    Vec2Fx::dist(positions[index], positions[other]) >= minimum,
                    "units {index} and {other} still overlapping: {:?} {:?}",
                    positions[index],
                    positions[other]
                );
            }
        }
    }

    #[test]
    fn sealed_pocket_orders_resolve_as_the_closest_approach() {
        let mut world = world_with_mover(2600, 300);
        // A sealed pocket: the goal tile (10,10) is open, all 8 neighbors rock.
        for dy in -1..=1 {
            for dx in -1..=1 {
                let x = 10 + dx;
                let y = 10 + dy;
                world.passability[y as usize * 16 + x as usize] = u8::from(dx == 0 && dy == 0);
            }
        }
        spawn_at(&mut world, 2000, 2000);
        let mut sim = Sim::new(&world, setup_one_player());
        sim.step(&[]);
        order_to(&mut sim, 1, milli(10_500, 10_500));
        // The order resolves by walking to the closest approachable tile —
        // no MoveFailed (the goal is *unreachable*, not the mover blocked),
        // and no stuck mid-order state remains.
        let mut ticks = 0;
        while sim
            .snapshot()
            .entities
            .iter()
            .any(|entity| entity.move_state == MoveState::Moving)
        {
            sim.step(&[]);
            ticks += 1;
            assert!(ticks < 1500, "the closest approach never resolved");
        }
        let final_pos = position_of(&sim, 1);
        let (fx_, fy) = (final_pos.x.floor_int(), final_pos.y.floor_int());
        // The ring around the sealed goal is rock, so the closest walkable
        // tile is one ring further out: Chebyshev distance 2 from the goal.
        assert!(
            (fx_ - 10).abs() <= 2 && (fy - 10).abs() <= 2,
            "the mover parks beside the pocket ring, at {fx_},{fy}"
        );
        // And never on the ring itself (only the sealed center is open).
        assert!(!matches!((fx_, fy), (9..=11, 9..=11) if (fx_, fy) != (10, 10)));
    }

    #[test]
    fn zero_speed_movers_fail_distant_orders_but_complete_reached_ones() {
        let mut world = world_with_mover(0, 300);
        spawn_at(&mut world, 2000, 2000);
        let mut sim = Sim::new(&world, setup_one_player());
        sim.step(&[]);
        let out = order_to(&mut sim, 1, milli(8000, 8000));
        assert!(out.events.iter().any(|event| matches!(
            event,
            Event::MoveFailed {
                entity: EntityId(1)
            }
        )));
        // A zero-distance order completes immediately instead.
        order_to(&mut sim, 1, milli(2000, 2000));
        let out = sim.step(&[]);
        assert!(out
            .events
            .iter()
            .all(|event| !matches!(event, Event::MoveFailed { .. })));
        assert!(sim
            .snapshot()
            .entities
            .iter()
            .all(|entity| entity.move_state == MoveState::Idle));
    }

    #[test]
    fn crowded_arrivals_complete_orders_near_the_target() {
        // Eight movers in a compact block (the drag-select group shape)
        // ordered to the *same* point: they cannot all stand on it, but they
        // cluster around it and every order resolves — nobody stays mid-order
        // forever. (A line of units ordered across its own axis into one
        // point can funnel into a jam instead — the formation-less-movement
        // limitation recorded in docs/ASSUMPTIONS.md.)
        let mut world = world_with_mover(2600, 300);
        for index in 0..8 {
            let row = index / 4;
            let column = index % 4;
            spawn_at(&mut world, 2000 + column * 800, 2000 + row * 800);
        }
        let mut sim = Sim::new(&world, setup_one_player());
        sim.step(&[]);
        let ids: Vec<EntityId> = (1..=8).map(EntityId).collect();
        sim.step(&[Command::new(
            PlayerId(0),
            sim.tick(),
            1,
            CommandKind::Move {
                units: ids,
                target: milli(8000, 8000),
            },
        )]);
        let mut ticks = 0;
        while sim
            .snapshot()
            .entities
            .iter()
            .any(|entity| entity.move_state == MoveState::Moving)
        {
            sim.step(&[]);
            ticks += 1;
            assert!(ticks < 3000, "a crowded order never resolved");
        }
        // Every mover ended clustered around the destination (the crowd
        // radius is one tile beyond the body, plus a little slop for the
        // parked ring).
        let target = milli(8000, 8000);
        for entity in sim.snapshot().entities {
            assert!(
                Vec2Fx::dist(entity.pos, target) <= Fx::from_milli(1500),
                "unit {} parked at {:?}",
                entity.id.0,
                entity.pos
            );
        }
    }

    /// A direct `World` for the white-box layer tests below.
    fn bare_world_with_mover(
        speed: Fx,
        radius: Fx,
        stuck_ticks: u32,
        repaths: u32,
        path: Vec<Vec2Fx>,
        pos: Vec2Fx,
        order: Vec2Fx,
    ) -> World {
        let mut world = World::new();
        world.kind_count = 1;
        world.players.push(PlayerState::from_setup(
            &PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            },
            &[(ResourceId(0), 200)],
        ));
        world.spawn(
            EntityId(1),
            PlayerId(0),
            KindId(0),
            pos,
            vec![CapabilityData::Move(MoveDef {
                speed_per_tick: speed,
                radius,
                path,
                stuck_ticks,
                repaths,
            })],
        );
        world.entity_mut(EntityId(1)).unwrap().orders = vec![Order::MoveTo { target: order }];
        world
    }

    #[test]
    fn stuck_escalation_gives_up_with_move_failed_when_out_of_repaths() {
        let pos = milli(2000, 2000);
        let far_target = milli(14_000, 14_000);
        let mut world = bare_world_with_mover(
            Fx::from_milli(87),
            Fx::from_milli(300),
            STUCK_TICKS,      // one more blocked tick crosses the threshold
            MAX_REPATHS,      // and no repaths remain
            vec![far_target], // an active path (so it counts as steering)
            pos,
            far_target,
        );
        let mut events = Vec::new();
        // The mover made no progress this tick (positions_before == current).
        detect_stuck(&mut world, &[(EntityId(1), pos)], &mut events);
        assert_eq!(
            events,
            vec![Event::MoveFailed {
                entity: EntityId(1)
            }]
        );
        assert!(world.entity(EntityId(1)).unwrap().orders.is_empty());
        assert!(world.move_of(EntityId(1)).unwrap().path.is_empty());
    }

    #[test]
    fn stuck_near_the_target_completes_as_a_crowded_arrival() {
        let pos = milli(2000, 2000);
        let target = milli(2600, 2000); // within the crowd radius
        let mut world = bare_world_with_mover(
            Fx::from_milli(87),
            Fx::from_milli(300),
            STUCK_TICKS,
            MAX_REPATHS,
            vec![target],
            pos,
            target,
        );
        let mut events = Vec::new();
        detect_stuck(&mut world, &[(EntityId(1), pos)], &mut events);
        assert!(events.is_empty(), "a crowded arrival is not a failure");
        assert!(world.entity(EntityId(1)).unwrap().orders.is_empty());
    }

    #[test]
    fn stuck_with_repaths_remaining_clears_the_path_for_a_retry() {
        let pos = milli(2000, 2000);
        let target = milli(14_000, 14_000);
        let mut world = bare_world_with_mover(
            Fx::from_milli(87),
            Fx::from_milli(300),
            STUCK_TICKS,
            0, // repaths remain
            vec![target],
            pos,
            target,
        );
        let mut events = Vec::new();
        detect_stuck(&mut world, &[(EntityId(1), pos)], &mut events);
        assert!(events.is_empty());
        let def = world.move_of(EntityId(1)).unwrap();
        assert_eq!(def.repaths, 1);
        assert_eq!(def.stuck_ticks, 0);
        assert!(def.path.is_empty(), "the path rebuilds next tick");
        assert!(!world.entity(EntityId(1)).unwrap().orders.is_empty());
    }

    #[test]
    fn progress_resets_the_stuck_counter() {
        let pos = milli(2000, 2000);
        let target = milli(14_000, 14_000);
        let mut world = bare_world_with_mover(
            Fx::from_milli(87),
            Fx::from_milli(300),
            STUCK_TICKS / 2,
            0,
            vec![target],
            pos,
            target,
        );
        let mut events = Vec::new();
        // Full progress this tick (positions_before far behind).
        let before = milli(1000, 1000);
        detect_stuck(&mut world, &[(EntityId(1), before)], &mut events);
        assert!(events.is_empty());
        assert_eq!(world.move_of(EntityId(1)).unwrap().stuck_ticks, 0);
    }

    #[test]
    fn collision_never_pushes_a_unit_into_blocked_terrain() {
        // A mover wedged against a wall with an idle unit pushed into it: the
        // wall-side half of the push is skipped, the unit never lands on rock.
        let mut passability = TrivialWorld::open_passability(16, 16);
        for y in 0..16 {
            passability[y as usize * 16 + 10] = 0; // wall at x=10
        }
        let nav = NavGrid::new(16, 16, &passability);
        let mut world = World::new();
        world.kind_count = 1;
        let mover = MoveDef::new(Fx::from_milli(87), Fx::from_milli(400));
        let blocker = MoveDef::new(Fx::from_milli(0), Fx::from_milli(400));
        // The mover sits just west of the wall; the blocker is shoved west
        // into it by the push below (both ordered nowhere — pure collision).
        world.spawn(
            EntityId(1),
            PlayerId(0),
            KindId(0),
            milli(9600, 8000),
            vec![CapabilityData::Move(mover)],
        );
        world.spawn(
            EntityId(2),
            PlayerId(0),
            KindId(0),
            milli(9600, 8000),
            vec![CapabilityData::Move(blocker)],
        );
        let mut events = Vec::new();
        advance_movement(&mut world, &nav, &mut events);
        for id in [EntityId(1), EntityId(2)] {
            let pos = world.entity(id).unwrap().pos;
            assert!(
                nav.passable(pos.x.floor_int(), pos.y.floor_int()),
                "unit {} pushed onto blocked terrain at {:?}",
                id.0,
                pos
            );
        }
    }

    #[test]
    fn replacing_orders_rebuild_the_path_for_the_new_target() {
        // The spam-click contract: a replacing Move resets the runtime path
        // (command application) and the movement tick rebuilds it toward the
        // *new* target — the first waypoint always aims at the current order.
        let mut world = world_with_mover(2600, 300);
        spawn_at(&mut world, 2000, 2000);
        let mut sim = Sim::new(&world, setup_one_player());
        sim.step(&[]);
        order_to(&mut sim, 1, milli(9000, 9000));
        sim.step(&[]);
        let (path, _, _) = sim.debug_move_state(EntityId(1)).expect("a mover");
        let first = path.first().copied().expect("a rebuilt path");
        let toward_target =
            Vec2Fx::dist(first, milli(9000, 9000)) < Vec2Fx::dist(first, milli(2000, 2000));
        assert!(toward_target, "the path heads to the current target");
        // A replacing order re-aims: the new path's first waypoint serves the
        // new destination, not the old one.
        order_to(&mut sim, 1, milli(2000, 9000));
        sim.step(&[]);
        let (path, _, _) = sim.debug_move_state(EntityId(1)).expect("a mover");
        let first = path.first().copied().expect("a rebuilt path");
        assert!(
            Vec2Fx::dist(first, milli(2000, 9000)) < Vec2Fx::dist(first, milli(9000, 9000)),
            "the replacement re-aimed the path"
        );
    }
}
