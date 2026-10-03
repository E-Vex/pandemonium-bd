//! The economy system (plan §9.3, M5): the worker gather loop — travel to a
//! node, gather on a timer, carry, deposit at the nearest storage, and
//! auto-seek the nearest node when one depletes.
//!
//! Stage 4 (plan §6.3 "Economy — gathering timers, deliveries, storage,
//! spending effects") advances every worker whose head order is
//! [`Order::GatherAt`], in ascending id order, collect-then-mutate. The
//! movement leg is shared with stage 6 through [`travel_target`]: an economy
//! order resolves to a travel target exactly when the worker is not yet where
//! its current phase needs it to be, so the mover's existing path machinery
//! does the walking.
//!
//! Phase machine (per worker, per tick):
//!
//! ```text
//! cargo not full ── near node?    ── yes: timer++ / extract at cycle end
//!       │                          no : travel target = node center
//! cargo full     ── near storage? ── yes: deposit (ResourceDelivered)
//!       │                          no : travel target = nearest storage center
//! ```
//!
//! All comparisons are squared-distance where a length is not needed (plan
//! §5.8); the one place a true distance is needed (approach tests) uses the
//! exact fixed-point `dist`.

use pandemonium_fx::{Fx, Vec2Fx};
use pandemonium_sim_api::{EntityId, Event, PlayerId, ResourceId};

use crate::nav::NavGrid;
use crate::world::{Lifecycle, Order, World};

/// How close a worker must stand to a footprint to work on it (gather from a
/// node, deposit at a storage, hammer a site): the distance from the worker's
/// body to the nearest point of the footprint rectangle. Generous by design —
/// 1.5 tiles plus the worker's own radius — so arrival under crowd jostle
/// counts without needing the mover's crowded-arrival machinery for economy
/// orders (A-043).
const APPROACH_MILLI: i32 = 1500;

/// Advances stage 4: gathering timers, extraction, deliveries, depletion, and
/// economy-order hygiene (auto-seek, ghost-site cleanup).
pub(crate) fn advance_economy(world: &mut World, nav: &mut NavGrid, events: &mut Vec<Event>) {
    gather_loop(world, events);
    build_order_hygiene(world);
    remove_depleted_nodes(world, nav, events);
}

/// The gather loop, workers in ascending id order.
fn gather_loop(world: &mut World, events: &mut Vec<Event>) {
    let workers: Vec<EntityId> = world
        .entities
        .iter()
        .filter(|entity| matches!(entity.orders.first(), Some(Order::GatherAt { .. })))
        .map(|entity| entity.id)
        .collect();

    for id in workers {
        let Some(entity) = world.entity(id) else {
            continue;
        };
        let Some(Order::GatherAt { node }) = entity.orders.first().copied() else {
            continue;
        };
        // A gatherer without the Gather capability cannot exist (the gate
        // checks it); skip defensively rather than guess.
        let Some(gather) = world.gather_of(id) else {
            continue;
        };
        // Copy the phase inputs out — the loop body mutates the world.
        let carry_amount = gather.carry_amount;
        let gather_time = gather.gather_time_ticks;
        let cargo = gather.cargo;
        let timer_start = gather.timer;
        let cargo_full = cargo.is_some_and(|(_, amount)| amount >= carry_amount);
        // Node liveness: a dead or non-resource target auto-seeks (depletion
        // removal and any future combat removal both land here).
        let node_alive = world
            .entity(node)
            .filter(|_| world.resource_of(node).is_some());
        let Some(_node_entity) = node_alive else {
            auto_seek(world, id);
            continue;
        };

        if cargo_full {
            // Return leg: deposit at the nearest completed storage that
            // accepts the carried resource.
            let (resource, amount) = cargo.expect("checked above");
            match nearest_storage(world, entity.pos, entity.owner, resource) {
                None => {} // no storage yet: stand and wait (A-051)
                // Not near yet: stage 6 walks the travel target (the
                // storage's center, resolved to reachable ground by the
                // pathfinder). The approach path is left alone — a path
                // legitimately ends on the nearest reachable tile, so
                // comparing its terminus to the goal center would clear it
                // every tick and freeze the worker on the start waypoint. If
                // the target storage dies mid-travel, the worker finishes the
                // stale leg and the next empty-path request retargets —
                // deterministic, eventually correct.
                Some(storage) if near_footprint(world, id, storage) => {
                    deposit(world, id, storage, resource, amount, events);
                }
                Some(_) => {}
            }
        } else if near_footprint(world, id, node) {
            // Gather leg, arrived: run the timer; extract at cycle end.
            clear_runtime(world, id); // arrived: stop steering
            let timer = timer_start + 1;
            if timer >= gather_time {
                let carrying = cargo.map(|(_, amount)| amount).unwrap_or(0);
                let room = (carry_amount - carrying).max(0);
                let node_amount = world.resource_of(node).map(|body| body.amount).unwrap_or(0);
                let extracted = room.min(node_amount);
                if extracted > 0 {
                    let resource = world
                        .resource_of(node)
                        .expect("the liveness check guaranteed a body")
                        .resource;
                    if let Some(def) = world.gather_of_mut(id) {
                        def.cargo = Some((resource, carrying + extracted));
                        def.timer = 0;
                    }
                }
                if let Some(body) = world.resource_of_mut(node) {
                    body.amount -= extracted;
                }
            } else if let Some(def) = world.gather_of_mut(id) {
                def.timer = timer;
            }
        }
        // Not near the node: stage 6 walks the travel target (the node
        // center). Nothing to do here.
    }
}

/// Deposits cargo into the storage owner's ledger (the worker's own player).
fn deposit(
    world: &mut World,
    worker: EntityId,
    storage: EntityId,
    resource: ResourceId,
    amount: i64,
    events: &mut Vec<Event>,
) {
    let owner = world.entity(storage).map(|entity| entity.owner);
    if let Some(owner) = owner {
        if let Some(player) = world.player_mut(owner) {
            player.credit(&[(resource, amount)]);
        }
    }
    if let Some(def) = world.gather_of_mut(worker) {
        def.cargo = None;
        def.timer = 0;
    }
    clear_runtime(world, worker);
    events.push(Event::ResourceDelivered {
        worker,
        resource,
        amount: amount.clamp(0, i32::MAX as i64) as i32,
    });
}

/// Auto-seek (plan §9.3: "workers auto-seek the nearest node" on depletion):
/// rewrite the gather order to the nearest node carrying the same resource as
/// any partial cargo (any node when empty), or drop the order when none is
/// left. Nearest by center distance, ties to the lower id (A-044).
fn auto_seek(world: &mut World, worker: EntityId) {
    let Some(entity) = world.entity(worker) else {
        return;
    };
    let cargo_resource = world
        .gather_of(worker)
        .and_then(|def| def.cargo.map(|(resource, _)| resource));
    let replacement = nearest_node(world, entity.pos, cargo_resource);
    match replacement {
        None => {
            if let Some(entity) = world.entity_mut(worker) {
                if !entity.orders.is_empty() {
                    entity.orders.remove(0);
                }
            }
            clear_runtime(world, worker);
        }
        Some(node) => {
            if let Some(entity) = world.entity_mut(worker) {
                if matches!(entity.orders.first(), Some(Order::GatherAt { .. })) {
                    entity.orders[0] = Order::GatherAt { node };
                }
            }
            clear_runtime(world, worker);
        }
    }
}

/// Drops `BuildAt` orders whose site no longer exists or no longer carries
/// construction state (completed sites pop their builders' orders at
/// completion in stage 3; this catches removals and stale references).
fn build_order_hygiene(world: &mut World) {
    let stale: Vec<EntityId> = world
        .entities
        .iter()
        .filter(|entity| {
            matches!(entity.orders.first(), Some(Order::BuildAt { .. }))
                && entity.orders.first().is_some_and(|order| match order {
                    Order::BuildAt { site } => world
                        .entity(*site)
                        .is_none_or(|_| world.construction_of(*site).is_none()),
                    _ => false,
                })
        })
        .map(|entity| entity.id)
        .collect();
    for id in stale {
        if let Some(entity) = world.entity_mut(id) {
            entity.orders.remove(0);
        }
        clear_runtime(world, id);
    }
}

/// Removes nodes depleted this tick (ascending id — the store order), fires
/// `NodeDepleted`, and releases their footprint tiles.
fn remove_depleted_nodes(world: &mut World, nav: &mut NavGrid, events: &mut Vec<Event>) {
    let dead: Vec<EntityId> = world
        .resources
        .iter()
        .filter(|(_, body)| body.amount <= 0)
        .map(|(id, _)| *id)
        .collect();
    for node in dead {
        // Workers whose order still points here re-target now (same tick) —
        // the end-of-tick invariant checker requires no stale references.
        let affected: Vec<EntityId> = world
            .entities
            .iter()
            .filter(|entity| {
                matches!(entity.orders.first(), Some(Order::GatherAt { node: n }) if *n == node)
            })
            .map(|entity| entity.id)
            .collect();
        let tiles = world
            .entity(node)
            .and_then(|entity| world.footprint_of(node).map(|fp| fp.tiles(entity.pos)));
        world.remove(node);
        if let Some(tiles) = tiles {
            for (x, y) in tiles {
                nav.vacate(x, y);
            }
        }
        events.push(Event::NodeDepleted { node });
        for worker in affected {
            auto_seek(world, worker);
        }
    }
}

/// The travel target of an economy order for stage 6: `Some` position to walk
/// toward, `None` when the worker is where it should be (or is waiting).
/// `MoveTo` is not an economy order — movement owns its targeting.
/// `AttackUnit` is a combat order — its chase target is resolved by the
/// movement system through `movement_target` (the target's current position);
/// this function returns `None` for it so the economy system does not feed a
/// conflicting travel target into the mover.
pub(crate) fn travel_target(world: &World, id: EntityId) -> Option<Vec2Fx> {
    let entity = world.entity(id)?;
    match entity.orders.first()? {
        Order::MoveTo { .. } | Order::AttackUnit { .. } => None,
        Order::GatherAt { node } => gather_travel_target(world, id, *node),
        Order::BuildAt { site } => build_travel_target(world, id, *site),
    }
}

/// The gather phases' travel target: the node center while filling, the
/// nearest storage's center while full.
fn gather_travel_target(world: &World, id: EntityId, node: EntityId) -> Option<Vec2Fx> {
    let entity = world.entity(id)?;
    let gather = world.gather_of(id)?;
    let node_alive = world
        .entity(node)
        .filter(|_| world.resource_of(node).is_some());
    let carry_amount = gather.carry_amount;
    let cargo = gather.cargo;
    let cargo_full = cargo.is_some_and(|(_, amount)| amount >= carry_amount);
    if cargo_full {
        let (resource, _) = cargo.expect("checked above");
        match nearest_storage(world, entity.pos, entity.owner, resource) {
            None => None,                                                // waiting (A-051)
            Some(storage) if near_footprint(world, id, storage) => None, // depositing
            Some(storage) => world.entity(storage).map(|target| target.pos),
        }
    } else {
        match node_alive {
            // Dead node: stage 4 auto-seeks before stage 6 ever asks.
            None => None,
            Some(node_entity) => {
                if near_footprint(world, id, node) {
                    None // gathering
                } else {
                    Some(node_entity.pos)
                }
            }
        }
    }
}

/// The build order's travel target: the site center until the builder is
/// close enough to hammer.
fn build_travel_target(world: &World, id: EntityId, site: EntityId) -> Option<Vec2Fx> {
    world.entity(site)?;
    world.construction_of(site)?;
    if near_footprint(world, id, site) {
        None // hammering (stage 3 advances the progress)
    } else {
        world.entity(site).map(|target| target.pos)
    }
}

/// Whether a mover stands close enough to a footprint entity to work on it:
/// distance from the mover to the nearest point of the footprint rectangle,
/// at most [`APPROACH_MILLI`] plus the mover's collision radius. Shared with
/// the construction system (the builder's hammering range).
pub(crate) fn near_footprint(world: &World, mover: EntityId, target: EntityId) -> bool {
    let Some(mover_entity) = world.entity(mover) else {
        return false;
    };
    let Some(target_entity) = world.entity(target) else {
        return false;
    };
    let Some(footprint) = world.footprint_of(target) else {
        return false;
    };
    let Some(radius) = world.move_of(mover).map(|def| def.radius) else {
        return false;
    };
    // The footprint rectangle in tile units: [x0, x0 + w] x [y0, y0 + h]; the
    // nearest point of it to the mover is the clamped position (Fx's total
    // ordering makes clamping deterministic).
    let (x0, y0) = footprint.top_left(target_entity.pos);
    let near_x = mover_entity
        .pos
        .x
        .clamp(Fx::from_int(x0), Fx::from_int(x0 + footprint.w as i32));
    let near_y = mover_entity
        .pos
        .y
        .clamp(Fx::from_int(y0), Fx::from_int(y0 + footprint.h as i32));
    let nearest = Vec2Fx::new(near_x, near_y);
    let limit = Fx::from_milli(APPROACH_MILLI) + radius;
    Vec2Fx::dist(mover_entity.pos, nearest) <= limit
}

/// The nearest completed storage of one owner accepting a resource, measured
/// from `from`: center distance (squared), ties to the lower id (A-051).
fn nearest_storage(
    world: &World,
    from: Vec2Fx,
    owner: PlayerId,
    resource: ResourceId,
) -> Option<EntityId> {
    let mut best: Option<(u64, EntityId)> = None;
    for (id, def) in &world.storage {
        if !def.resources.contains(&resource) {
            continue;
        }
        let Some(entity) = world.entity(*id) else {
            continue;
        };
        if entity.owner != owner || entity.lifecycle != Lifecycle::Active {
            continue;
        }
        let d_sq = (entity.pos - from).len_sq_raw();
        match best {
            Some((best_sq, _)) if best_sq <= d_sq => {}
            _ => best = Some((d_sq, *id)),
        }
    }
    best.map(|(_, id)| id)
}

/// The nearest node carrying a resource (any node when `resource` is `None`),
/// measured from `from`: center distance squared, ties to the lower id.
fn nearest_node(world: &World, from: Vec2Fx, resource: Option<ResourceId>) -> Option<EntityId> {
    let mut best: Option<(u64, EntityId)> = None;
    for (id, body) in &world.resources {
        if body.amount <= 0 {
            continue;
        }
        if let Some(wanted) = resource {
            if body.resource != wanted {
                continue;
            }
        }
        let Some(entity) = world.entity(*id) else {
            continue;
        };
        let d_sq = (entity.pos - from).len_sq_raw();
        match best {
            Some((best_sq, _)) if best_sq <= d_sq => {}
            _ => best = Some((d_sq, *id)),
        }
    }
    best.map(|(_, id)| id)
}

/// Clears a mover's runtime path and counters (a phase or target change
/// invalidates the old lanes — the same contract as replacing Move orders).
fn clear_runtime(world: &mut World, id: EntityId) {
    if let Some(def) = world.move_of_mut(id) {
        def.reset_runtime();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{CapTemplate, KindTemplate, ResourceDef, SpawnDef, TrivialWorld};
    use crate::sim::Sim;
    use pandemonium_sim_api::{
        Command, CommandKind, ControllerKind, Event, KindId, MatchSetup, MoveState, PlayerSetup,
        RejectReason, Vec2Fx,
    };

    /// One player (200 Ore), a worker, a storage depot, and two ore nodes.
    /// Kinds: 0 worker, 1 depot, 2 ore node (15), 3 mover without Gather,
    /// 4 rich ore node (500).
    fn economy_world() -> TrivialWorld {
        TrivialWorld {
            map_id: 0x0000_0EC0,
            width_tiles: 16,
            height_tiles: 16,
            passability: TrivialWorld::open_passability(16, 16),
            buildability: TrivialWorld::open_buildability(16, 16),
            kinds: vec![
                KindTemplate::from_caps(vec![
                    CapTemplate::Health {
                        max_hp: 40,
                        regen_per_tick: 0,
                    },
                    CapTemplate::Move {
                        speed_milli_tiles_per_s: 2600,
                        radius_milli_tiles: 300,
                    },
                    CapTemplate::Gather {
                        carry_amount: 10,
                        gather_time_ms: 2000,
                    },
                    CapTemplate::Build {},
                    CapTemplate::Vision {
                        radius_milli_tiles: 7000,
                    },
                ]),
                KindTemplate::from_caps(vec![
                    CapTemplate::Health {
                        max_hp: 300,
                        regen_per_tick: 0,
                    },
                    CapTemplate::Vision {
                        radius_milli_tiles: 6000,
                    },
                    CapTemplate::Footprint { w: 2, h: 2 },
                    CapTemplate::Storage {
                        resources: vec![ResourceId(0)],
                    },
                    CapTemplate::ProvidesPopulation { amount: 10 },
                ]),
                KindTemplate::from_caps(vec![
                    CapTemplate::Resource {
                        resource: ResourceId(0),
                        amount: 15,
                    },
                    CapTemplate::Footprint { w: 2, h: 2 },
                ]),
                KindTemplate::from_caps(vec![
                    CapTemplate::Health {
                        max_hp: 40,
                        regen_per_tick: 0,
                    },
                    CapTemplate::Move {
                        speed_milli_tiles_per_s: 2600,
                        radius_milli_tiles: 300,
                    },
                    CapTemplate::Vision {
                        radius_milli_tiles: 5000,
                    },
                ]),
                // Kind 4: the rich node (survives a whole test run).
                KindTemplate::from_caps(vec![
                    CapTemplate::Resource {
                        resource: ResourceId(0),
                        amount: 500,
                    },
                    CapTemplate::Footprint { w: 2, h: 2 },
                ]),
            ],
            resources: vec![ResourceDef {
                resource: ResourceId(0),
                starting: 200,
            }],
            production: vec![],
            base_population_cap: 0,
            initial_spawns: vec![
                // Player 0: the worker, a storage depot, a spare mover.
                SpawnDef {
                    owner: PlayerId(0),
                    kind: KindId(0),
                    pos: Vec2Fx::from_ints(4, 4),
                },
                SpawnDef {
                    owner: PlayerId(0),
                    kind: KindId(1),
                    pos: Vec2Fx::from_ints(10, 3),
                },
                SpawnDef {
                    owner: PlayerId(0),
                    kind: KindId(3),
                    pos: Vec2Fx::from_ints(12, 12),
                },
                // Nodes: A near the worker (small), B farther (rich), neutral.
                SpawnDef {
                    owner: PlayerId::NEUTRAL,
                    kind: KindId(2),
                    pos: Vec2Fx::from_ints(4, 10),
                },
                SpawnDef {
                    owner: PlayerId::NEUTRAL,
                    kind: KindId(4),
                    pos: Vec2Fx::from_ints(13, 10),
                },
            ],
            scheduled_spawns: vec![],
            spawn_jitter_milli: 0,
        }
    }

    fn setup() -> MatchSetup {
        MatchSetup {
            seed: 3,
            players: vec![PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            }],
        }
    }

    fn gather_order(sim: &mut Sim, node: EntityId, seq: u32) {
        sim.step(&[Command::new(
            PlayerId(0),
            sim.tick(),
            seq,
            CommandKind::Gather {
                units: vec![EntityId(1)],
                node,
            },
        )]);
    }

    fn deliveries(events: &[Event]) -> i64 {
        events
            .iter()
            .map(|event| match event {
                Event::ResourceDelivered { amount, .. } => *amount as i64,
                _ => 0,
            })
            .sum()
    }

    #[test]
    fn workers_gather_carry_and_deposit() {
        let mut sim = Sim::new(&economy_world(), setup());
        let node_a = EntityId(4);
        gather_order(&mut sim, node_a, 1);
        // Travel (~40 ticks) + gather (60 ticks) + return (~70 ticks): the
        // first deposit lands well inside 250 ticks.
        let mut delivered = 0;
        let mut tick = 1;
        while tick < 250 {
            let out = sim.step(&[]);
            delivered += deliveries(&out.events);
            if delivered > 0 {
                break;
            }
            tick += 1;
        }
        assert!(delivered >= 10, "no deposit by tick {tick}");
        assert_eq!(
            sim.player_view(PlayerId(0)).resources[0].amount,
            200 + delivered
        );
        // The node lost exactly what was carried.
        assert_eq!(world_resource_amount(&sim, node_a), 5);
        // The gather order is still active (the loop continues).
        assert!(matches!(
            sim_snapshot_order(&sim),
            Some(Order::GatherAt { node }) if node == node_a
        ));
    }

    #[test]
    fn depleted_nodes_emit_and_workers_auto_seek() {
        let mut sim = Sim::new(&economy_world(), setup());
        let node_a = EntityId(4);
        let node_b = EntityId(5);
        gather_order(&mut sim, node_a, 1);
        let mut depleted_a = false;
        let mut delivered = 0;
        for _ in 0..900 {
            let out = sim.step(&[]);
            delivered += deliveries(&out.events);
            depleted_a |= out
                .events
                .iter()
                .any(|event| matches!(event, Event::NodeDepleted { node } if *node == node_a));
        }
        // Node A (15 ore) depleted after two cycles (10 + 5), the worker
        // re-targeted node B, and the deliveries kept coming.
        assert!(depleted_a, "node A never depleted");
        assert!(delivered >= 15, "delivered only {delivered}");
        assert!(
            sim.snapshot()
                .entities
                .iter()
                .all(|entity| entity.id != node_a),
            "depleted node removed"
        );
        assert!(
            sim.snapshot()
                .entities
                .iter()
                .any(|entity| entity.id == node_b),
            "node B survives"
        );
        // Auto-seek rewrote the order to node B.
        assert!(matches!(
            sim_snapshot_order(&sim),
            Some(Order::GatherAt { node }) if node == node_b
        ));
    }

    #[test]
    fn gather_gate_checks_capability_and_target() {
        let mut sim = Sim::new(&economy_world(), setup());
        // The mover without Gather cannot be ordered to gather.
        let out = sim.step(&[Command::new(
            PlayerId(0),
            sim.tick(),
            1,
            CommandKind::Gather {
                units: vec![EntityId(3)],
                node: EntityId(4),
            },
        )]);
        assert!(out.events.iter().any(|event| {
            matches!(event, Event::CommandRejected { reject, .. }
                if reject.reason == RejectReason::MissingCapability)
        }));
        // A storage depot is not a resource node.
        let out = sim.step(&[Command::new(
            PlayerId(0),
            sim.tick(),
            2,
            CommandKind::Gather {
                units: vec![EntityId(1)],
                node: EntityId(2),
            },
        )]);
        assert!(out.events.iter().any(|event| {
            matches!(event, Event::CommandRejected { reject, .. }
                if reject.reason == RejectReason::InvalidTarget)
        }));
        // Unknown nodes are invalid targets.
        let out = sim.step(&[Command::new(
            PlayerId(0),
            sim.tick(),
            3,
            CommandKind::Gather {
                units: vec![EntityId(1)],
                node: EntityId(99),
            },
        )]);
        assert!(out.events.iter().any(|event| {
            matches!(event, Event::CommandRejected { reject, .. }
                if reject.reason == RejectReason::InvalidTarget)
        }));
        // The worker may not be ordered by a player who does not own it.
        let out = sim.step(&[Command::new(
            PlayerId(1),
            sim.tick(),
            1,
            CommandKind::Gather {
                units: vec![EntityId(1)],
                node: EntityId(4),
            },
        )]);
        assert!(out.events.iter().any(|event| {
            matches!(event, Event::CommandRejected { reject, .. }
                if reject.reason == RejectReason::PlayerMissing)
        }));
    }

    #[test]
    fn depleted_ground_reopens_for_movers() {
        let mut sim = Sim::new(&economy_world(), setup());
        let node_a = EntityId(4);
        gather_order(&mut sim, node_a, 1);
        // Drain node A (15 ore) completely.
        let mut ticks = 0;
        while sim.resource_probe(node_a).is_some() && ticks < 800 {
            sim.step(&[]);
            ticks += 1;
        }
        assert!(
            sim.resource_probe(node_a).is_none(),
            "node A never depleted"
        );
        assert!(sim
            .snapshot()
            .entities
            .iter()
            .all(|entity| entity.id != node_a));
        // The freed tiles accept a mover again: order the spare mover onto
        // the node's old center.
        sim.step(&[Command::new(
            PlayerId(0),
            sim.tick(),
            2,
            CommandKind::Move {
                units: vec![EntityId(3)],
                target: Vec2Fx::from_ints(4, 10),
            },
        )]);
        let mut moved = 0;
        while sim
            .snapshot()
            .entities
            .iter()
            .any(|entity| entity.id == EntityId(3) && entity.move_state == MoveState::Moving)
        {
            sim.step(&[]);
            moved += 1;
            assert!(moved < 300, "the mover never arrived on the freed ground");
        }
        let pos = sim
            .snapshot()
            .entities
            .iter()
            .find(|entity| entity.id == EntityId(3))
            .unwrap()
            .pos;
        assert_eq!(pos, Vec2Fx::from_ints(4, 10));
    }

    // -- test-only windows into the sim's internals ------------------------

    fn world_resource_amount(sim: &Sim, node: EntityId) -> i64 {
        sim.resource_probe(node)
            .map(|body| body.amount)
            .unwrap_or(-1)
    }

    fn sim_snapshot_order(sim: &Sim) -> Option<Order> {
        sim.order_probe(EntityId(1))
    }

    #[test]
    fn gather_loop_is_deterministic() {
        let world = economy_world();
        let mut a = Sim::new(&world, setup());
        let mut b = Sim::new(&world, setup());
        gather_order(&mut a, EntityId(4), 1);
        gather_order(&mut b, EntityId(4), 1);
        for _ in 0..300 {
            a.step(&[]);
            b.step(&[]);
        }
        assert_eq!(a.state_hash(), b.state_hash());
    }
}

/// Shared test fixtures for the economy and production systems' tests.
#[cfg(test)]
pub(crate) mod tests_support {
    use crate::fixture::{CapTemplate, KindEconomy, KindTemplate, ResourceDef, SpawnDef};
    use pandemonium_sim_api::{PlayerId, ResourceId, Vec2Fx};

    /// One player (200 Ore, base cap 1), a worker, a producer structure at
    /// (6,6), and an ore node. Kinds: 0 worker (gather), 1 grunt (producible:
    /// 50 Ore, 10 ticks, pop 1), 2 producer (Produce + Footprint), 3 ore node.
    /// The producer trains grunts.
    pub(crate) fn economy_world_with_production() -> crate::fixture::TrivialWorld {
        crate::fixture::TrivialWorld {
            map_id: 0x0000_0D11,
            width_tiles: 16,
            height_tiles: 16,
            passability: crate::fixture::TrivialWorld::open_passability(16, 16),
            buildability: crate::fixture::TrivialWorld::open_buildability(16, 16),
            kinds: vec![
                KindTemplate::from_caps(vec![
                    CapTemplate::Health {
                        max_hp: 40,
                        regen_per_tick: 0,
                    },
                    CapTemplate::Move {
                        speed_milli_tiles_per_s: 2600,
                        radius_milli_tiles: 300,
                    },
                    CapTemplate::Gather {
                        carry_amount: 10,
                        gather_time_ms: 2000,
                    },
                    CapTemplate::Vision {
                        radius_milli_tiles: 7000,
                    },
                ]),
                // Kind 1: the grunt — producible, costs 50 Ore / 10 ticks / pop 1.
                KindTemplate {
                    caps: vec![
                        CapTemplate::Health {
                            max_hp: 40,
                            regen_per_tick: 0,
                        },
                        CapTemplate::Move {
                            speed_milli_tiles_per_s: 2600,
                            radius_milli_tiles: 300,
                        },
                    ],
                    economy: KindEconomy {
                        cost: vec![(ResourceId(0), 50)],
                        build_time_ticks: 10,
                        population: 1,
                        requires: vec![],
                    },
                },
                // Kind 2: the producer structure.
                KindTemplate {
                    caps: vec![
                        CapTemplate::Health {
                            max_hp: 300,
                            regen_per_tick: 0,
                        },
                        CapTemplate::Footprint { w: 2, h: 2 },
                        CapTemplate::Produce {},
                    ],
                    economy: KindEconomy::default(),
                },
                // Kind 3: the ore node.
                KindTemplate::from_caps(vec![
                    CapTemplate::Resource {
                        resource: ResourceId(0),
                        amount: 500,
                    },
                    CapTemplate::Footprint { w: 2, h: 2 },
                ]),
            ],
            resources: vec![ResourceDef {
                resource: ResourceId(0),
                starting: 200,
            }],
            production: vec![(
                pandemonium_sim_api::KindId(2),
                vec![pandemonium_sim_api::KindId(1)],
            )],
            base_population_cap: 1,
            initial_spawns: vec![
                SpawnDef {
                    owner: PlayerId(0),
                    kind: pandemonium_sim_api::KindId(0),
                    pos: Vec2Fx::from_ints(3, 3),
                },
                SpawnDef {
                    owner: PlayerId(0),
                    kind: pandemonium_sim_api::KindId(2),
                    pos: Vec2Fx::from_ints(6, 6),
                },
                SpawnDef {
                    owner: PlayerId::NEUTRAL,
                    kind: pandemonium_sim_api::KindId(3),
                    pos: Vec2Fx::from_ints(12, 12),
                },
            ],
            scheduled_spawns: vec![],
            spawn_jitter_milli: 0,
        }
    }
}

/// A construction-scene fixture: a builder worker, a buildable depot kind
/// (2x2, +10 pop, 30 ticks), and a rock tile at (8,8) for placement tests.
#[cfg(test)]
pub(crate) mod construction_support {
    use crate::fixture::{CapTemplate, KindEconomy, KindTemplate, ResourceDef, SpawnDef};
    use pandemonium_sim_api::{PlayerId, ResourceId, Vec2Fx};

    /// Kinds: 0 builder worker (Build), 1 depot (Footprint 2x2 +
    /// ProvidesPopulation 10), 2 node. Entities: 1 worker, 2 node.
    pub(crate) fn construction_world() -> crate::fixture::TrivialWorld {
        let mut passability = crate::fixture::TrivialWorld::open_passability(16, 16);
        let mut buildability = crate::fixture::TrivialWorld::open_buildability(16, 16);
        // A rock tile at (8, 8).
        passability[8 * 16 + 8] = 0;
        buildability[8 * 16 + 8] = 0;
        crate::fixture::TrivialWorld {
            map_id: 0x0000_C0DE,
            width_tiles: 16,
            height_tiles: 16,
            passability,
            buildability,
            kinds: vec![
                KindTemplate::from_caps(vec![
                    CapTemplate::Health {
                        max_hp: 40,
                        regen_per_tick: 0,
                    },
                    CapTemplate::Move {
                        speed_milli_tiles_per_s: 2600,
                        radius_milli_tiles: 300,
                    },
                    CapTemplate::Build {},
                    CapTemplate::Vision {
                        radius_milli_tiles: 7000,
                    },
                ]),
                // Kind 1: the depot — 100 Ore, 30 ticks, +10 population.
                KindTemplate {
                    caps: vec![
                        CapTemplate::Health {
                            max_hp: 300,
                            regen_per_tick: 0,
                        },
                        CapTemplate::Footprint { w: 2, h: 2 },
                        CapTemplate::ProvidesPopulation { amount: 10 },
                    ],
                    economy: KindEconomy {
                        cost: vec![(ResourceId(0), 100)],
                        build_time_ticks: 30,
                        population: 0,
                        requires: vec![],
                    },
                },
                // Kind 2: the ore node (an occupied-tile placement target).
                KindTemplate::from_caps(vec![
                    CapTemplate::Resource {
                        resource: ResourceId(0),
                        amount: 500,
                    },
                    CapTemplate::Footprint { w: 2, h: 2 },
                ]),
            ],
            resources: vec![ResourceDef {
                resource: ResourceId(0),
                starting: 200,
            }],
            production: vec![],
            base_population_cap: 5,
            initial_spawns: vec![
                SpawnDef {
                    owner: PlayerId(0),
                    kind: pandemonium_sim_api::KindId(0),
                    pos: Vec2Fx::from_ints(4, 4),
                },
                SpawnDef {
                    owner: PlayerId::NEUTRAL,
                    kind: pandemonium_sim_api::KindId(2),
                    pos: Vec2Fx::from_ints(12, 12),
                },
            ],
            scheduled_spawns: vec![],
            spawn_jitter_milli: 0,
        }
    }
}
