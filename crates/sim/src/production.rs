//! The production and construction system (plan §9.4, M5): one progress
//! model, two surfaces.
//!
//! Any entity with `Produce` owns an ordered queue of [`QueueItem`]s — costs
//! are paid on enqueue (the command gate) and refunded on cancel; only the
//! head progresses. Population headroom is checked at enqueue and again at
//! spawn; a completed item whose player lacks headroom waits at the front
//! until the cap grows (classic supply-block, resolved deterministically).
//! Construction is the same model with a committed builder instead of a
//! queue: the site is an entity in `Lifecycle::UnderConstruction` carrying a
//! `Construction` block; a builder standing near the site advances the
//! progress; completion removes the block, activates the structure, and pops
//! the builders' orders.
//!
//! One requirements checker serves both surfaces (plan §9.4): a requirement
//! is met when the player owns a *completed* entity of the required kind.
//!
//! Everything runs in stage 3 (plan §6.3 "Production & construction —
//! advance queues and construction progress; spawn/complete"), producers and
//! sites in ascending id order, collect-then-mutate.

use pandemonium_fx::{Fx, Rng, Vec2Fx};
use pandemonium_sim_api::{EntityId, Event, PlayerId};

use crate::economy;
use crate::fixture::{KindTemplate, TrivialWorld};
use crate::nav::NavGrid;
use crate::world::{Lifecycle, Order, QueueItem, World};

/// How far from a producer's footprint a produced unit may spawn: the first
/// free tile of an expanding row-major ring scan (up to 3 tiles out).
const SPAWN_RING_LIMIT: i32 = 3;

/// Advances stage 3: production queues, construction progress, spawns and
/// completions, then refreshes every player's population usage and cap so the
/// rest of the tick (and the hash) sees settled numbers.
pub(crate) fn advance_production(
    world: &mut World,
    fixture: &TrivialWorld,
    nav: &NavGrid,
    next_entity_id: &mut u64,
    rng: &mut Rng,
    events: &mut Vec<Event>,
) {
    advance_queues(world, fixture, nav, next_entity_id, rng, events);
    advance_construction(world, events);
    recompute_population(world, &fixture.kinds, fixture.base_population_cap);
}

/// The production queues (producers in ascending id order).
fn advance_queues(
    world: &mut World,
    fixture: &TrivialWorld,
    nav: &NavGrid,
    next_entity_id: &mut u64,
    rng: &mut Rng,
    events: &mut Vec<Event>,
) {
    let producers: Vec<EntityId> = world.produce.iter().map(|(id, _)| *id).collect();
    for producer in producers {
        let Some(entity) = world.entity(producer) else {
            continue;
        };
        // Sites do not produce while under construction (their queues are
        // empty by the gate; defensive skip).
        if entity.lifecycle != Lifecycle::Active {
            continue;
        }
        let Some(def) = world.produce_of(producer) else {
            continue;
        };
        let Some(head) = def.queue.first() else {
            continue;
        };
        let producible = head.producible;
        let progress = head.progress_ticks;
        let Some(economy) = fixture
            .kinds
            .get(producible.0 as usize)
            .map(|kind| &kind.economy)
        else {
            continue;
        };
        let total = economy.build_time_ticks;
        let started = progress == 0;
        let mut advanced = progress;
        if progress < total {
            advanced = progress + 1;
        }
        let owner = entity.owner;
        // Population headroom at spawn (plan §9.4) — checked fresh, so
        // same-tick spawns see each other.
        let pop_cost = economy.population.max(0) as u32;
        let headroom = population_cap(world, owner, fixture.base_population_cap)
            .saturating_sub(population_usage(world, &fixture.kinds, owner));
        let can_spawn = advanced >= total && headroom >= pop_cost;

        // Mutate: advance the head's progress (frozen at completion while
        // headroom is missing — the item waits at the front).
        if let Some(def) = world.produce_of_mut(producer) {
            if let Some(item) = def.queue.first_mut() {
                if item.progress_ticks < total {
                    item.progress_ticks = advanced;
                }
            }
        }
        if started {
            events.push(Event::ProductionStarted {
                producer,
                producible,
            });
        }
        if can_spawn {
            let popped = world.produce_of_mut(producer).and_then(|def| {
                if def
                    .queue
                    .first()
                    .is_some_and(|item| item.producible == producible)
                {
                    Some(def.queue.remove(0))
                } else {
                    None
                }
            });
            if let Some(_item) = popped {
                let rally = world.produce_of(producer).and_then(|def| def.rally);
                let pos = spawn_position(world, nav, producer);
                let id = spawn_kind_entity(
                    world,
                    fixture,
                    &mut SpawnCtx {
                        next_entity_id,
                        rng,
                    },
                    owner,
                    producible,
                    pos,
                    events,
                );
                if let Some(rally) = rally {
                    if let Some(entity) = world.entity_mut(id) {
                        entity.orders.push(Order::MoveTo { target: rally });
                    }
                }
                events.push(Event::ProductionCompleted {
                    producer,
                    producible,
                });
            }
        }
    }
}

/// The construction sites (ascending id order): a committed builder near the
/// site advances the progress; completion activates the structure, releases
/// the runtime block, and pops the builders' orders.
fn advance_construction(world: &mut World, events: &mut Vec<Event>) {
    let sites: Vec<EntityId> = world.construction.iter().map(|(id, _)| *id).collect();
    for site in sites {
        let Some(def) = world.construction_of(site) else {
            continue;
        };
        let builder = def.builder;
        // A dead or absent builder stalls the site (A-045) — no implicit
        // reassignment exists in the command vocabulary.
        let builder_near = world
            .entity(builder)
            .is_some_and(|_| economy::near_footprint(world, builder, site));
        if !builder_near {
            continue;
        }
        let progress = def.progress_ticks + 1;
        if progress >= def.total_ticks {
            // Complete: drop the runtime block, activate the structure.
            if let Ok(index) = world
                .construction
                .binary_search_by_key(&site, |(eid, _)| *eid)
            {
                world.construction.remove(index);
            }
            if let Some(entity) = world.entity_mut(site) {
                entity.lifecycle = Lifecycle::Active;
            }
            events.push(Event::ConstructionCompleted { site });
            // Pop the builders' BuildAt orders (the work is done).
            let builders: Vec<EntityId> = world
                .entities
                .iter()
                .filter(|entity| {
                    matches!(entity.orders.first(), Some(Order::BuildAt { site: s }) if *s == site)
                })
                .map(|entity| entity.id)
                .collect();
            for builder in builders {
                if let Some(entity) = world.entity_mut(builder) {
                    entity.orders.remove(0);
                }
                if let Some(def) = world.move_of_mut(builder) {
                    def.reset_runtime();
                }
            }
        } else if let Some(def) = world.construction_of_mut(site) {
            def.progress_ticks = progress;
        }
    }
}

/// The shared requirements checker (plan §9.4): every required kind must
/// appear as a *completed* entity owned by the player. Used by both the Train
/// and Build gates.
pub(crate) fn requirements_met(world: &World, owner: PlayerId, requires: &[u32]) -> bool {
    requires.iter().all(|required| {
        world.entities.iter().any(|entity| {
            entity.owner == owner
                && entity.kind.0 == *required
                && entity.lifecycle == Lifecycle::Active
        })
    })
}

/// A player's population usage: the sum of population costs of their live
/// entities (queued items reserve nothing — the enqueue gate and the spawn
/// re-check carry the invariant instead; A-042).
pub(crate) fn population_usage(world: &World, kinds: &[KindTemplate], player: PlayerId) -> u32 {
    let mut usage = 0u32;
    for entity in &world.entities {
        if entity.owner != player {
            continue;
        }
        if let Some(kind) = kinds.get(entity.kind.0 as usize) {
            usage = usage.saturating_add(kind.economy.population.max(0) as u32);
        }
    }
    usage
}

/// A player's population cap: the rules' base plus every *completed*
/// ProvidesPopulation structure they own.
pub(crate) fn population_cap(world: &World, player: PlayerId, base: u32) -> u32 {
    let mut cap = base;
    for (id, def) in &world.provides_population {
        let Some(entity) = world.entity(*id) else {
            continue;
        };
        if entity.owner == player && entity.lifecycle == Lifecycle::Active {
            cap = cap.saturating_add(def.amount.max(0) as u32);
        }
    }
    cap
}

/// Refreshes every player's population usage and cap (the hashed fields —
/// always settled at the end of the stage that changed them).
pub(crate) fn recompute_population(world: &mut World, kinds: &[KindTemplate], base: u32) {
    let players: Vec<PlayerId> = world.players.iter().map(|p| p.player).collect();
    for player in players {
        let usage = population_usage(world, kinds, player);
        let cap = population_cap(world, player, base);
        if let Some(state) = world.player_mut(player) {
            state.population = usage;
            state.population_cap = cap;
        }
    }
}

/// The spawn position for a produced unit: the first free tile of an
/// expanding row-major ring scan around the producer's footprint (inner tiles
/// fail the occupancy check, so the scan naturally starts at the ring).
/// Falls back to the producer's own position in the degenerate fully-walled
/// case (A-053).
fn spawn_position(world: &World, nav: &NavGrid, producer: EntityId) -> Vec2Fx {
    let Some(entity) = world.entity(producer) else {
        return Vec2Fx::ZERO;
    };
    let (x0, y0, w, h) = match world.footprint_of(producer) {
        Some(footprint) => {
            let (x, y) = footprint.top_left(entity.pos);
            (x, y, footprint.w as i32, footprint.h as i32)
        }
        None => (entity.pos.x.floor_int(), entity.pos.y.floor_int(), 1, 1),
    };
    for ring in 1..=SPAWN_RING_LIMIT {
        for y in (y0 - ring)..=(y0 + h - 1 + ring) {
            for x in (x0 - ring)..=(x0 + w - 1 + ring) {
                if nav.passable(x, y) {
                    return Vec2Fx::new(
                        Fx::from_milli(x * 1000 + 500),
                        Fx::from_milli(y * 1000 + 500),
                    );
                }
            }
        }
    }
    entity.pos
}

/// The id allocator and RNG a spawn needs, bundled so the spawn helper stays
/// within the argument budget (the caller destructures `Sim`'s fields).
pub(crate) struct SpawnCtx<'a> {
    /// The monotonic id allocator.
    pub(crate) next_entity_id: &'a mut u64,
    /// The shared simulation RNG (spawn jitter draws).
    pub(crate) rng: &'a mut Rng,
}

/// Spawns an entity from a kind template: converts the capability templates,
/// applies the deterministic spawn jitter (zero jitter draws nothing, keeping
/// the RNG stream stable), allocates the monotonic id, emits `Spawned`.
pub(crate) fn spawn_kind_entity(
    world: &mut World,
    fixture: &TrivialWorld,
    ctx: &mut SpawnCtx<'_>,
    owner: PlayerId,
    kind: pandemonium_sim_api::KindId,
    pos: Vec2Fx,
    events: &mut Vec<Event>,
) -> EntityId {
    let id = EntityId(*ctx.next_entity_id);
    *ctx.next_entity_id += 1;
    let jitter = draw_jitter(ctx.rng, fixture.spawn_jitter_milli);
    let caps: Vec<_> = fixture
        .kinds
        .get(kind.0 as usize)
        .map(|template| {
            template
                .caps
                .iter()
                .map(|cap| cap.to_runtime(crate::TICKS_PER_SECOND))
                .collect()
        })
        .unwrap_or_default();
    let pos = pos + jitter;
    world.spawn(id, owner, kind, pos, caps);
    events.push(Event::Spawned {
        entity: id,
        owner,
        kind,
        pos,
    });
    id
}

/// The spawn jitter (the same draw Sim::new's initial spawns use): two draws
/// (x then y), uniform in `±spawn_jitter_milli` milli-tiles; zero jitter
/// consumes no draws.
fn draw_jitter(rng: &mut Rng, jitter_milli: i32) -> Vec2Fx {
    let radius = jitter_milli.max(0);
    if radius == 0 {
        return Vec2Fx::ZERO;
    }
    let span = (2 * radius + 1) as u32;
    let dx = rng.bounded(span) as i32 - radius;
    let dy = rng.bounded(span) as i32 - radius;
    Vec2Fx::new(Fx::from_milli(dx), Fx::from_milli(dy))
}

/// Enqueues a producible onto a producer (the Train command's application):
/// the gate validated affordability; the cost is spent here and travels with
/// the item so cancellation refunds it verbatim.
pub(crate) fn enqueue(
    world: &mut World,
    producer: EntityId,
    producible: pandemonium_sim_api::KindId,
    cost: Vec<(pandemonium_sim_api::ResourceId, i64)>,
) {
    let owner = world.entity(producer).map(|entity| entity.owner);
    if let Some(owner) = owner {
        if let Some(player) = world.player_mut(owner) {
            player.spend(&cost);
        }
    }
    if let Some(def) = world.produce_of_mut(producer) {
        def.queue.push(QueueItem {
            producible,
            cost_paid: cost,
            progress_ticks: 0,
        });
    }
}

/// Cancels one queued item by index (the CancelQueueItem command's
/// application): refunds the paid cost verbatim to the producer's owner.
pub(crate) fn cancel(world: &mut World, producer: EntityId, index: u16) {
    let refund = world.produce_of(producer).and_then(|def| {
        def.queue
            .get(index as usize)
            .map(|item| item.cost_paid.clone())
    });
    let Some(cost_paid) = refund else {
        return;
    };
    if let Some(def) = world.produce_of_mut(producer) {
        if (index as usize) < def.queue.len() {
            def.queue.remove(index as usize);
        }
    }
    let owner = world.entity(producer).map(|entity| entity.owner);
    if let Some(owner) = owner {
        if let Some(player) = world.player_mut(owner) {
            player.refund(&cost_paid);
        }
    }
}

/// The kind economy of a kind id, for the command gate's checks.
pub(crate) fn kind_economy(
    fixture: &TrivialWorld,
    kind: pandemonium_sim_api::KindId,
) -> Option<&crate::fixture::KindEconomy> {
    fixture.kinds.get(kind.0 as usize).map(|kind| &kind.economy)
}

/// Whether a producer kind may produce a kind (the faction's production list
/// — faction data flowing through the world seam).
pub(crate) fn producible_by(
    fixture: &TrivialWorld,
    producer_kind: pandemonium_sim_api::KindId,
    producible: pandemonium_sim_api::KindId,
) -> bool {
    fixture
        .production
        .iter()
        .any(|(producer, list)| *producer == producer_kind && list.contains(&producible))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::economy::tests_support::economy_world_with_production;
    use crate::sim::Sim;
    use pandemonium_sim_api::{
        Command, CommandKind, ControllerKind, Event, MatchSetup, PlayerSetup, RejectReason,
    };

    /// The support fixture's kinds: 0 worker, 1 grunt (producible: 50 Ore,
    /// 10 ticks, pop 1), 2 producer, 3 ore node. Entities: 1 worker,
    /// 2 producer, 3 node.
    const GRUNT: pandemonium_sim_api::KindId = pandemonium_sim_api::KindId(1);

    fn fixture_setup() -> MatchSetup {
        MatchSetup {
            seed: 11,
            players: vec![PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            }],
        }
    }

    #[test]
    fn training_spends_pays_back_on_cancel_and_spawns() {
        let world = economy_world_with_production();
        let mut sim = Sim::new(&world, fixture_setup());
        let producer = EntityId(2);
        // Train a grunt: costs 50 of the 200 balance.
        sim.step(&[Command::new(
            PlayerId(0),
            0,
            1,
            CommandKind::Train {
                producer,
                unit: GRUNT,
            },
        )]);
        assert_eq!(sim.player_view(PlayerId(0)).resources[0].amount, 150);
        // Cancel it: the 50 comes back.
        sim.step(&[Command::new(
            PlayerId(0),
            1,
            2,
            CommandKind::CancelQueueItem { producer, index: 0 },
        )]);
        assert_eq!(sim.player_view(PlayerId(0)).resources[0].amount, 200);
        // A bad index is rejected precisely.
        let out = sim.step(&[Command::new(
            PlayerId(0),
            2,
            3,
            CommandKind::CancelQueueItem { producer, index: 0 },
        )]);
        assert!(out.events.iter().any(|event| {
            matches!(event, Event::CommandRejected { reject, .. }
                if reject.reason == RejectReason::QueueIndexInvalid)
        }));
        // Train again and let it finish: 10 ticks, 50 ore, pop 1. The
        // ProductionStarted event fires on the train tick itself.
        let out = sim.step(&[Command::new(
            PlayerId(0),
            3,
            4,
            CommandKind::Train {
                producer,
                unit: GRUNT,
            },
        )]);
        let mut started = out
            .events
            .iter()
            .any(|event| matches!(event, Event::ProductionStarted { .. }));
        let mut completed = false;
        for _ in 0..20 {
            let out = sim.step(&[]);
            for event in &out.events {
                match event {
                    Event::ProductionStarted { .. } => started = true,
                    Event::ProductionCompleted { .. } => completed = true,
                    _ => {}
                }
            }
        }
        assert!(
            started && completed,
            "the queue never ran ({started}, {completed})"
        );
        assert_eq!(
            sim.player_view(PlayerId(0)).resources[0].amount,
            150,
            "the second training spent its 50"
        );
        assert_eq!(sim.player_view(PlayerId(0)).population, 1);
        // Three initial entities plus the produced grunt: the allocator's
        // watermark stands at 5.
        assert_eq!(sim.next_entity_id(), 5);
    }

    #[test]
    fn population_headroom_holds_spawns_and_blocks_full_trains() {
        let world = economy_world_with_production();
        let mut sim = Sim::new(&world, fixture_setup());
        let producer = EntityId(2);
        // Queued items reserve no population (A-042): while the first grunt is
        // still queued (live usage 0), a second train also passes the gate.
        sim.step(&[Command::new(
            PlayerId(0),
            0,
            1,
            CommandKind::Train {
                producer,
                unit: GRUNT,
            },
        )]);
        let out = sim.step(&[Command::new(
            PlayerId(0),
            1,
            2,
            CommandKind::Train {
                producer,
                unit: GRUNT,
            },
        )]);
        assert!(
            !out.events
                .iter()
                .any(|event| matches!(event, Event::CommandRejected { .. })),
            "queued items reserve no population"
        );
        assert_eq!(
            sim.player_view(PlayerId(0)).resources[0].amount,
            100,
            "both trains paid their 50"
        );
        // Both complete, but only the first spawns (usage 1 == cap 1); the
        // second holds at the front of the queue.
        let mut spawned = 0;
        for _ in 0..30 {
            let out = sim.step(&[]);
            spawned += out
                .events
                .iter()
                .filter(|event| matches!(event, Event::ProductionCompleted { .. }))
                .count();
        }
        assert_eq!(spawned, 1, "the second item must hold for headroom");
        assert_eq!(sim.player_view(PlayerId(0)).population, 1);
        // The held item sits at the front, its progress frozen at completion
        // (10 ticks for the grunt kind).
        assert_eq!(sim.queue_probe(producer), vec![(1, 10)]);
        // With live usage at the cap, further trains are refused outright.
        let out = sim.step(&[Command::new(
            PlayerId(0),
            sim.tick(),
            3,
            CommandKind::Train {
                producer,
                unit: GRUNT,
            },
        )]);
        assert!(out.events.iter().any(|event| {
            matches!(event, Event::CommandRejected { reject, .. }
                if reject.reason == RejectReason::PopulationFull)
        }));
        // Cancelling the held item refunds its cost.
        sim.step(&[Command::new(
            PlayerId(0),
            sim.tick(),
            4,
            CommandKind::CancelQueueItem { producer, index: 0 },
        )]);
        assert_eq!(sim.player_view(PlayerId(0)).resources[0].amount, 150);
    }

    #[test]
    fn rally_points_order_spawned_units() {
        let world = economy_world_with_production();
        let mut sim = Sim::new(&world, fixture_setup());
        let producer = EntityId(2);
        let rally = Vec2Fx::from_ints(8, 8);
        sim.step(&[Command::new(
            PlayerId(0),
            0,
            1,
            CommandKind::SetRally {
                producer,
                target: rally,
            },
        )]);
        sim.step(&[Command::new(
            PlayerId(0),
            1,
            2,
            CommandKind::Train {
                producer,
                unit: GRUNT,
            },
        )]);
        let mut spawned_id = None;
        for _ in 0..15 {
            let out = sim.step(&[]);
            for event in &out.events {
                if let Event::Spawned { entity, .. } = event {
                    spawned_id = Some(*entity);
                }
            }
        }
        let spawned = spawned_id.expect("the unit spawned");
        assert_eq!(
            sim.order_probe(spawned),
            Some(Order::MoveTo { target: rally })
        );
    }

    #[test]
    fn unproducible_and_non_producer_trains_are_refused() {
        let world = economy_world_with_production();
        let mut sim = Sim::new(&world, fixture_setup());
        let producer = EntityId(2);
        // The worker kind is not in the producer's list.
        let out = sim.step(&[Command::new(
            PlayerId(0),
            0,
            1,
            CommandKind::Train {
                producer,
                unit: pandemonium_sim_api::KindId(0),
            },
        )]);
        assert!(out.events.iter().any(|event| {
            matches!(event, Event::CommandRejected { reject, .. }
                if reject.reason == RejectReason::MissingCapability)
        }));
        // A structure kind is trained, not built-by-command -> InvalidTarget.
        let out = sim.step(&[Command::new(
            PlayerId(0),
            1,
            2,
            CommandKind::Train {
                producer,
                unit: pandemonium_sim_api::KindId(2),
            },
        )]);
        assert!(out.events.iter().any(|event| {
            matches!(event, Event::CommandRejected { reject, .. }
                if reject.reason == RejectReason::InvalidTarget)
        }));
        // A non-producer cannot train.
        let out = sim.step(&[Command::new(
            PlayerId(0),
            2,
            3,
            CommandKind::Train {
                producer: EntityId(1),
                unit: GRUNT,
            },
        )]);
        assert!(out.events.iter().any(|event| {
            matches!(event, Event::CommandRejected { reject, .. }
                if reject.reason == RejectReason::MissingCapability)
        }));
        // An unknown kind is unknown.
        let out = sim.step(&[Command::new(
            PlayerId(0),
            3,
            4,
            CommandKind::Train {
                producer,
                unit: pandemonium_sim_api::KindId(9),
            },
        )]);
        assert!(out.events.iter().any(|event| {
            matches!(event, Event::CommandRejected { reject, .. }
                if reject.reason == RejectReason::UnknownKind)
        }));
    }

    #[test]
    fn production_runs_are_deterministic() {
        let world = economy_world_with_production();
        let mut a = Sim::new(&world, fixture_setup());
        let mut b = Sim::new(&world, fixture_setup());
        for sim in [&mut a, &mut b] {
            sim.step(&[Command::new(
                PlayerId(0),
                0,
                1,
                CommandKind::Train {
                    producer: EntityId(2),
                    unit: GRUNT,
                },
            )]);
        }
        for _ in 0..30 {
            a.step(&[]);
            b.step(&[]);
        }
        assert_eq!(a.state_hash(), b.state_hash());
    }
}

#[cfg(test)]
mod construction_tests {
    use super::*;
    use crate::economy::construction_support::construction_world;
    use crate::sim::Sim;
    use crate::world::Lifecycle;
    use pandemonium_sim_api::{
        Command, CommandKind, ControllerKind, Event, MatchSetup, PlayerSetup, RejectReason,
        TilePos, Vec2Fx,
    };
    use pandemonium_sim_api::{EntityId, PlayerId};

    fn setup() -> MatchSetup {
        MatchSetup {
            seed: 13,
            players: vec![PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            }],
        }
    }

    fn build(at: (i32, i32), tick: u32, seq: u32) -> Command {
        Command::new(
            PlayerId(0),
            tick,
            seq,
            CommandKind::Build {
                worker: EntityId(1),
                structure: pandemonium_sim_api::KindId(1),
                at: TilePos { x: at.0, y: at.1 },
            },
        )
    }

    #[test]
    fn sites_spawn_block_and_complete() {
        let world = construction_world();
        let mut sim = Sim::new(&world, setup());
        // Build at (6,6): pays 100, spawns the site, orders the builder.
        sim.step(&[build((6, 6), 0, 1)]);
        assert_eq!(sim.player_view(PlayerId(0)).resources[0].amount, 100);
        assert_eq!(sim.next_entity_id(), 4, "the site took the next id");
        assert_eq!(
            sim.order_probe(EntityId(1)),
            Some(Order::BuildAt { site: EntityId(3) })
        );
        // The site is under construction and provides no population yet.
        assert_eq!(
            sim.lifecycle_probe(EntityId(3)),
            Some(Lifecycle::UnderConstruction)
        );
        assert_eq!(sim.player_view(PlayerId(0)).population_cap, 5);
        // The builder (4,4) reaches the site and hammers; 30 build ticks plus
        // travel land well inside 200 ticks.
        let mut completed = false;
        let mut tick = 1;
        while tick < 200 {
            let out = sim.step(&[]);
            completed |= out
                .events
                .iter()
                .any(|event| matches!(event, Event::ConstructionCompleted { site } if *site == EntityId(3)));
            tick += 1;
        }
        assert!(completed, "the site never completed");
        assert_eq!(sim.lifecycle_probe(EntityId(3)), Some(Lifecycle::Active));
        // The completed depot provides population.
        assert_eq!(sim.player_view(PlayerId(0)).population_cap, 15);
        // The builder's order popped.
        assert_eq!(sim.order_probe(EntityId(1)), None);
    }

    #[test]
    fn placement_is_validated_against_terrain_bodies_and_movers() {
        let world = construction_world();
        let mut sim = Sim::new(&world, setup());
        // Rock at (8,8): a 2x2 site at (8,8) overlaps it.
        let out = sim.step(&[build((8, 8), 0, 1)]);
        assert!(out.events.iter().any(|event| {
            matches!(event, Event::CommandRejected { reject, .. }
                if reject.reason == RejectReason::PlacementBlocked)
        }));
        // The node at (12,12) (2x2): its tiles are occupied.
        let out = sim.step(&[build((12, 12), sim.tick(), 2)]);
        assert!(out.events.iter().any(|event| {
            matches!(event, Event::CommandRejected { reject, .. }
                if reject.reason == RejectReason::PlacementBlocked)
        }));
        // Out-of-bounds placements are blocked.
        let out = sim.step(&[build((-1, 0), sim.tick(), 3)]);
        assert!(out.events.iter().any(|event| {
            matches!(event, Event::CommandRejected { reject, .. }
                if reject.reason == RejectReason::PlacementBlocked)
        }));
        // A mover standing on a placement tile blocks it: order the worker to
        // (6,6), wait for arrival, then try to build under it.
        sim.step(&[Command::new(
            PlayerId(0),
            sim.tick(),
            4,
            CommandKind::Move {
                units: vec![EntityId(1)],
                target: Vec2Fx::from_ints(6, 6),
            },
        )]);
        let mut moving = true;
        while moving {
            sim.step(&[]);
            moving = sim.snapshot().entities.iter().any(|e| {
                e.id == EntityId(1) && e.move_state == pandemonium_sim_api::MoveState::Moving
            });
        }
        let out = sim.step(&[build((5, 5), sim.tick(), 5)]);
        assert!(
            out.events.iter().any(|event| {
                matches!(event, Event::CommandRejected { reject, .. }
                if reject.reason == RejectReason::PlacementBlocked)
            }),
            "a mover on the placement tile must block it"
        );
    }

    #[test]
    fn unaffordable_and_non_structure_builds_are_refused() {
        let world = construction_world();
        let mut sim = Sim::new(&world, setup());
        // Spend down to 50: one build at (6,6) costs 100 -> 100 left; a second
        // build attempt at 100 left succeeds, so instead refuse via cost:
        // build twice at different tiles with only 200 -> the third is broke.
        sim.step(&[build((6, 6), 0, 1)]);
        sim.step(&[build((9, 6), sim.tick(), 2)]);
        assert_eq!(sim.player_view(PlayerId(0)).resources[0].amount, 0);
        let out = sim.step(&[build((6, 9), sim.tick(), 3)]);
        assert!(out.events.iter().any(|event| {
            matches!(event, Event::CommandRejected { reject, .. }
                if reject.reason == RejectReason::CannotAfford)
        }));
        // A unit kind (no Footprint) cannot be built.
        let out = sim.step(&[Command::new(
            PlayerId(0),
            sim.tick(),
            4,
            CommandKind::Build {
                worker: EntityId(1),
                structure: pandemonium_sim_api::KindId(0),
                at: TilePos { x: 3, y: 3 },
            },
        )]);
        assert!(out.events.iter().any(|event| {
            matches!(event, Event::CommandRejected { reject, .. }
                if reject.reason == RejectReason::InvalidTarget)
        }));
    }

    #[test]
    fn construction_is_deterministic() {
        let world = construction_world();
        let mut a = Sim::new(&world, setup());
        let mut b = Sim::new(&world, setup());
        a.step(&[build((6, 6), 0, 1)]);
        b.step(&[build((6, 6), 0, 1)]);
        for _ in 0..100 {
            a.step(&[]);
            b.step(&[]);
        }
        assert_eq!(a.state_hash(), b.state_hash());
    }
}
