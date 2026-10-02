//! `Sim` — the deterministic simulation spine (plan §6.2, §6.3).
//!
//! The fixed update order inside [`Sim::step`] below is the plan's §6.3 order,
//! verbatim, and it never varies per content item. Milestones fill the stages in;
//! every stage that does not exist yet runs as a documented no-op with a pointer
//! to the milestone that implements it. The order is the contract: inserting a
//! system into the wrong stage is a review-visible change, not a silent one.

use pandemonium_fx::{Fx, Rng};
use pandemonium_sim_api::{
    Command, EntityId, EntityView, Event, MatchSetup, PlayerId, PlayerView, Snapshot, Tick, Vec2Fx,
    ViewResource,
};

use crate::command::apply_commands;
use crate::fixture::{ScheduledSpawnDef, SpawnDef, TrivialWorld};
use crate::hash::hash_state;
use crate::movement::advance_movement;
use crate::nav::NavGrid;
use crate::world::{HealthDef, Lifecycle, PlayerState, World};

/// What one call to [`Sim::step`] produced: the events of that tick (drained from
/// the buffer — plan §6.3 stage 11 "flush events") and, when the resulting tick
/// lands on the checkpoint interval, the periodic state hash (stage 11: "compute
/// periodic hash every 30 ticks and on demand").
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct StepOutput {
    /// Events emitted during the tick, in pipeline order.
    pub events: Vec<Event>,
    /// The periodic hash, present exactly when the post-step tick is a multiple of
    /// [`crate::CHECKPOINT_INTERVAL`]. The hash is of the state *at* the new tick.
    pub hash: Option<u64>,
}

/// The simulation. Construct it from a world description and a match setup, then
/// advance it *only* through [`Sim::step`] (FD-2) — there is no other mutator.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Sim {
    tick: Tick,
    rng: Rng,
    world: World,
    fixture: TrivialWorld,
    nav: NavGrid,
    next_entity_id: u64,
    events: Vec<Event>,
    /// Scheduled spawns, sorted by `(tick, fixture order)`, with a cursor.
    spawn_queue: Vec<ScheduledSpawnDef>,
    spawn_cursor: usize,
}

impl Sim {
    /// Builds the initial state: players, initial spawns (in fixture order, each
    /// consuming the RNG once for jitter), and the sorted spawn schedule.
    ///
    /// Setup invariants (unique, non-neutral player slots) are asserted — the
    /// setup is code in M1, so a violation is a programmer error surfaced loudly
    /// and deterministically, not data to repair. Content validation is M2's job.
    pub fn new(world: &TrivialWorld, setup: MatchSetup) -> Self {
        let mut players = setup.players;
        players.sort_by_key(|p| p.player);
        assert!(
            players.windows(2).all(|w| w[0].player < w[1].player),
            "duplicate player slots in the match setup"
        );
        assert!(
            players.iter().all(|p| p.player != PlayerId::NEUTRAL),
            "the neutral sentinel is not a controller slot"
        );

        let mut sim = Self {
            tick: 0,
            rng: Rng::seeded(setup.seed),
            world: World::new(),
            fixture: world.clone(),
            nav: NavGrid::new(world.width_tiles, world.height_tiles, &world.passability),
            next_entity_id: 1,
            events: Vec::new(),
            spawn_queue: world.scheduled_spawns.clone(),
            spawn_cursor: 0,
        };
        // The buildability grid must match the map shape (the fixture and the
        // content seam both guarantee it; a mismatch is programmer error).
        debug_assert_eq!(
            world.buildability.len(),
            (world.width_tiles as u64 * world.height_tiles as u64) as usize,
            "buildability must be width * height bytes"
        );
        // Deterministic schedule order: (tick, then fixture order preserved by the
        // stable sort).
        sim.spawn_queue.sort_by_key(|s| s.tick);

        sim.world.kind_count = sim.fixture.kinds.len();
        let registry: Vec<_> = sim
            .fixture
            .resources
            .iter()
            .map(|def| (def.resource, def.starting))
            .collect();
        for setup_entry in &players {
            sim.world
                .players
                .push(PlayerState::from_setup(setup_entry, &registry));
        }
        for spawn in sim.fixture.initial_spawns.clone() {
            sim.spawn(spawn);
        }
        sim
    }

    /// The current tick (the tick the next `step` applies commands for).
    pub fn tick(&self) -> Tick {
        self.tick
    }

    /// The id allocator watermark: the next spawned entity receives exactly this
    /// id. Monotonic within a match; paired with removal, the observable proof
    /// that ids are never reused.
    pub fn next_entity_id(&self) -> u64 {
        self.next_entity_id
    }

    /// Test-only window into one mover's runtime movement state (the
    /// movement-layer tests inspect paths and counters through this).
    #[cfg(test)]
    pub(crate) fn debug_move_state(&self, id: EntityId) -> Option<(Vec<Vec2Fx>, u32, u32)> {
        self.world
            .move_of(id)
            .map(|def| (def.path.clone(), def.stuck_ticks, def.repaths))
    }

    /// The on-demand canonical state hash (plan §6.4).
    pub fn state_hash(&self) -> u64 {
        hash_state(&self.world, self.tick, &self.rng, self.next_entity_id)
    }

    /// The read-only presentation copy (plan §6.4): entities in ascending id
    /// order. The client interpolates between successive snapshots (plan §11.1).
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            tick: self.tick,
            entities: self
                .world
                .entities
                .iter()
                .map(|e| self.entity_view(e.id))
                .collect(),
        }
    }

    /// The fog-filtered view for one player (plan §9.6): own entities plus
    /// entities inside friendly vision radii — the *only* window an AI controller
    /// gets (FD-7, FD-8). A slot that is not in the match gets an empty view.
    pub fn player_view(&self, player: PlayerId) -> PlayerView {
        let state = self.world.player(player);
        let entities = self
            .world
            .entities
            .iter()
            .filter(|e| crate::command::visible_to(&self.world, player, e.id))
            .map(|e| self.entity_view(e.id))
            .collect();
        let (resources, population, population_cap) = match state {
            Some(p) => (
                p.resources
                    .iter()
                    .map(|(resource, amount)| ViewResource {
                        resource: *resource,
                        amount: *amount,
                    })
                    .collect(),
                p.population,
                p.population_cap,
            ),
            None => (Vec::new(), 0, 0),
        };
        PlayerView {
            tick: self.tick,
            player,
            resources,
            population,
            population_cap,
            entities,
        }
    }

    /// One boundary-value projection of an entity (shared by snapshot and view).
    fn entity_view(&self, id: EntityId) -> EntityView {
        let entity = self.world.entity(id).expect("id comes from the store");
        let hp_fraction_milli = match self.world.health_of(id) {
            Some(HealthDef { hp, max_hp, .. }) if *max_hp > 0 => {
                ((*hp as i64 * 1000) / (*max_hp as i64)).clamp(0, 1000) as u32
            }
            _ => 0,
        };
        EntityView {
            id,
            owner: entity.owner,
            kind: entity.kind,
            pos: entity.pos,
            facing: entity.facing,
            hp_fraction_milli,
            move_state: entity.move_state(),
        }
    }

    /// **The only way state advances** (FD-2). All commands must target
    /// `self.tick` — mismatches are rejected as events, never silently dropped.
    /// Returns that tick's events and the periodic hash when due.
    pub fn step(&mut self, commands: &[Command]) -> StepOutput {
        // Stage 1 — Apply commands: sorted by (issuer, seq), validated, applied;
        //           invalid ones emit CommandRejected (plan §6.3.1).
        apply_commands(&mut self.world, self.tick, commands, &mut self.events);

        // Stage 2 — Orders: resolve each entity's current order into system
        //           intents. M1's order vocabulary is movement-only, and the
        //           placeholder mover consumes orders directly in stage 6; order
        //           resolution into richer intents begins with combat (M6) and
        //           the economy (M5).
        // Stage 3 — Production & construction: advance queues and progress;
        //           spawn/complete. M1's stand-in: the fixture's scheduled spawns
        //           (exercising the same entity-store append + Spawned events the
        //           real production spawning will use).
        self.run_scheduled_spawns();

        // Stage 4 — Economy: gathering timers, deliveries, storage, spending
        //           effects (M5).
        // Stage 5 — Target acquisition (M6).
        // Stage 6 — Movement: path requests → path following → steering →
        //           collision push-apart, entities in id order (plan §6.3.6,
        //           §9.1). M4's three-layer mover: A* over the nav grid with
        //           deterministic tie-breaks, waypoint steering, spatial-hash
        //           push-apart, and stuck detection ending in MoveFailed.
        advance_movement(&mut self.world, &self.nav, &mut self.events);

        // Stage 7 — Combat: resolve attacks, apply damage, mark deaths (M6).
        // Stage 8 — Death & cleanup: advance health, fire lifecycle events,
        //           remove the dead. Ids are never reused (plan §6.3.8).
        self.advance_health_and_cleanup();

        // Stage 9 — Vision: incremental per-player visibility update (M6; the
        //           view recomputes on demand until then).
        // Stage 10 — Match rules: defeat/victory evaluation (M8).
        // Stage 11 — Finalize: increment the tick, flush events, compute the
        //            periodic hash.
        self.tick = self.tick.wrapping_add(1);
        let hash = if self.tick.is_multiple_of(crate::CHECKPOINT_INTERVAL) {
            Some(self.state_hash())
        } else {
            None
        };
        StepOutput {
            events: std::mem::take(&mut self.events),
            hash,
        }
    }

    /// Spawns one fixture entity: converts its kind's capability templates,
    /// applies the deterministic spawn jitter (consuming the RNG in fixed spawn
    /// order — plan §5.4), allocates the monotonic id, and emits `Spawned`.
    /// Entities with a Footprint claim their tiles on the nav grid immediately
    /// (structures and nodes block tiles from their first tick — plan §9.1.3).
    fn spawn(&mut self, spawn: SpawnDef) {
        let id = EntityId(self.next_entity_id);
        self.next_entity_id += 1;
        let jitter = self.draw_spawn_jitter();
        let caps: Vec<_> = self
            .fixture
            .kinds
            .get(spawn.kind.0 as usize)
            .map(|template| {
                template
                    .caps
                    .iter()
                    .map(|cap| cap.to_runtime(crate::TICKS_PER_SECOND))
                    .collect()
            })
            .unwrap_or_default();
        let pos = spawn.pos + jitter;
        self.world.spawn(id, spawn.owner, spawn.kind, pos, caps);
        self.claim_footprint(id);
        self.events.push(Event::Spawned {
            entity: id,
            owner: spawn.owner,
            kind: spawn.kind,
            pos,
        });
    }

    /// Claims (or, with `release`, releases) the nav tiles of an entity's
    /// footprint — the occupancy half of "structures block tiles".
    fn claim_footprint(&mut self, id: EntityId) {
        let Some(entity) = self.world.entity(id) else {
            return;
        };
        let Some(footprint) = self.world.footprint_of(id) else {
            return;
        };
        for (x, y) in footprint.tiles(entity.pos) {
            self.nav.occupy(x, y);
        }
    }

    /// Releases the nav tiles of an entity's footprint (death, depletion).
    fn release_footprint(&mut self, id: EntityId) {
        let Some(entity) = self.world.entity(id) else {
            return;
        };
        let Some(footprint) = self.world.footprint_of(id) else {
            return;
        };
        for (x, y) in footprint.tiles(entity.pos) {
            self.nav.vacate(x, y);
        }
    }

    /// The spawn jitter for one spawn: two draws (x then y) from the shared RNG,
    /// uniform in `±spawn_jitter_milli` milli-tiles. Zero jitter consumes no
    /// draws, keeping the RNG stream fixed for jitter-free worlds.
    fn draw_spawn_jitter(&mut self) -> Vec2Fx {
        let radius = self.fixture.spawn_jitter_milli.max(0);
        if radius == 0 {
            return Vec2Fx::ZERO;
        }
        let span = (2 * radius + 1) as u32;
        let dx = self.rng.bounded(span) as i32 - radius;
        let dy = self.rng.bounded(span) as i32 - radius;
        Vec2Fx::new(Fx::from_milli(dx), Fx::from_milli(dy))
    }

    /// Stage 3 stand-in: spawn every scheduled entity whose tick has arrived, in
    /// `(tick, fixture order)`.
    fn run_scheduled_spawns(&mut self) {
        while self.spawn_cursor < self.spawn_queue.len()
            && self.spawn_queue[self.spawn_cursor].tick <= self.tick
        {
            let scheduled = self.spawn_queue[self.spawn_cursor].spawn;
            self.spawn_cursor += 1;
            self.spawn(scheduled);
        }
    }

    /// Stage 8: advance health regeneration (saturating, clamped to the pool),
    /// then remove everyone whose health reached zero — firing `Died` — in
    /// ascending id order. `Vec::remove` keeps the survivors' order stable, and
    /// the allocator never looks back, so ids are never reused. Structures
    /// release their footprint tiles on the way out, so the ground reopens.
    fn advance_health_and_cleanup(&mut self) {
        for (_, def) in &mut self.world.health {
            let max = def.max_hp.max(0);
            def.hp = def.hp.saturating_add(def.regen_per_tick).clamp(0, max);
        }
        let dead: Vec<EntityId> = self
            .world
            .health
            .iter()
            .filter(|(_, def)| def.hp <= 0)
            .map(|(id, _)| *id)
            .collect();
        for id in dead {
            if let Some(entity) = self.world.entity_mut(id) {
                entity.lifecycle = Lifecycle::Dead;
            }
            self.release_footprint(id);
            self.world.remove(id);
            self.events.push(Event::Died { entity: id });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{CapTemplate, KindEconomy, KindTemplate, ResourceDef};
    use pandemonium_sim_api::{
        CommandKind, ControllerKind, KindId, MoveState, PlayerSetup, ResourceId,
    };

    fn trivial_world() -> TrivialWorld {
        // kind 0 "grunt": health + move + vision; kind 1 "watcher": vision only;
        // kind 2 "decayer": dying health + move.
        TrivialWorld {
            map_id: 0x11,
            width_tiles: 64,
            height_tiles: 64,
            passability: TrivialWorld::open_passability(64, 64),
            buildability: TrivialWorld::open_buildability(64, 64),
            kinds: vec![
                KindTemplate {
                    caps: vec![
                        CapTemplate::Health {
                            max_hp: 40,
                            regen_per_tick: 0,
                        },
                        CapTemplate::Move {
                            speed_milli_tiles_per_s: 2600,
                            radius_milli_tiles: 350,
                        },
                        CapTemplate::Vision {
                            radius_milli_tiles: 7000,
                        },
                    ],
                    economy: KindEconomy::default(),
                },
                KindTemplate {
                    caps: vec![CapTemplate::Vision {
                        radius_milli_tiles: 9000,
                    }],
                    economy: KindEconomy::default(),
                },
                KindTemplate {
                    caps: vec![
                        CapTemplate::Health {
                            max_hp: 2,
                            regen_per_tick: -1,
                        },
                        CapTemplate::Move {
                            speed_milli_tiles_per_s: 1600,
                            radius_milli_tiles: 350,
                        },
                        CapTemplate::Vision {
                            radius_milli_tiles: 5000,
                        },
                    ],
                    economy: KindEconomy::default(),
                },
            ],
            resources: vec![ResourceDef {
                resource: ResourceId(0),
                starting: 200,
            }],
            production: vec![],
            base_population_cap: 0,
            initial_spawns: vec![
                SpawnDef {
                    owner: PlayerId(0),
                    kind: KindId(0),
                    pos: Vec2Fx::from_ints(10, 10),
                },
                SpawnDef {
                    owner: PlayerId(1),
                    kind: KindId(0),
                    pos: Vec2Fx::from_ints(50, 50),
                },
            ],
            scheduled_spawns: vec![],
            spawn_jitter_milli: 0,
        }
    }

    fn setup() -> MatchSetup {
        MatchSetup {
            seed: 7,
            players: vec![
                PlayerSetup {
                    player: PlayerId(0),
                    controller: ControllerKind::Human,
                },
                PlayerSetup {
                    player: PlayerId(1),
                    controller: ControllerKind::Ai,
                },
            ],
        }
    }

    #[test]
    fn new_spawns_initial_entities_and_emits_spawn_events() {
        let mut sim = Sim::new(&trivial_world(), setup());
        assert_eq!(sim.tick(), 0);
        assert_eq!(sim.snapshot().entities.len(), 2);
        assert_eq!(sim.next_entity_id(), 3);
        // The Spawned events sit in the buffer until the first step drains them.
        let out = sim.step(&[]);
        assert_eq!(out.events.len(), 2);
        assert!(out
            .events
            .iter()
            .all(|e| matches!(e, Event::Spawned { .. })));
    }

    #[test]
    fn ids_are_monotonic_across_scheduled_spawns_and_death() {
        let mut world = trivial_world();
        // A decayer dies after two ticks (hp 2, regen -1).
        world.initial_spawns.push(SpawnDef {
            owner: PlayerId(0),
            kind: KindId(2),
            pos: Vec2Fx::from_ints(20, 20),
        });
        // A grunt spawns at tick 5 — after the decayer is gone.
        world.scheduled_spawns.push(ScheduledSpawnDef {
            tick: 5,
            spawn: SpawnDef {
                owner: PlayerId(0),
                kind: KindId(0),
                pos: Vec2Fx::from_ints(30, 30),
            },
        });
        let mut sim = Sim::new(&world, setup());
        let mut saw_death_of: Option<EntityId> = None;
        let mut all_ids: Vec<EntityId> = vec![];
        for _ in 0..8 {
            let out = sim.step(&[]);
            for event in &out.events {
                match event {
                    Event::Spawned { entity, .. } => all_ids.push(*entity),
                    Event::Died { entity } => saw_death_of = Some(*entity),
                    _ => {}
                }
            }
        }
        let dead = saw_death_of.expect("decayer must die");
        assert_eq!(dead, EntityId(3));
        // Every id ever allocated is unique.
        let mut sorted = all_ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), all_ids.len());
        // The dead id never reappears, and post-death ids are strictly higher.
        let post_death: Vec<EntityId> = sim
            .snapshot()
            .entities
            .iter()
            .map(|e| e.id)
            .filter(|id| *id > dead)
            .collect();
        assert!(!post_death.is_empty());
        assert!(post_death.iter().all(|id| *id > dead));
        assert!(sim.next_entity_id() > dead.0);
        assert!(!sim.snapshot().entities.iter().any(|e| e.id == dead));
    }

    #[test]
    fn scheduled_spawn_fires_exactly_at_its_tick() {
        let mut world = trivial_world();
        world.scheduled_spawns.push(ScheduledSpawnDef {
            tick: 4,
            spawn: SpawnDef {
                owner: PlayerId(1),
                kind: KindId(0),
                pos: Vec2Fx::from_ints(12, 12),
            },
        });
        let mut sim = Sim::new(&world, setup());
        sim.step(&[]); // executes tick 0
        sim.step(&[]); // executes tick 1
        sim.step(&[]); // executes tick 2
        sim.step(&[]); // executes tick 3
        assert_eq!(sim.tick(), 4);
        assert_eq!(sim.snapshot().entities.len(), 2, "tick 4 not yet executed");
        let out = sim.step(&[]); // executes tick 4: the scheduled spawn fires now
        assert!(out
            .events
            .iter()
            .any(|e| matches!(e, Event::Spawned { .. })));
        assert_eq!(sim.snapshot().entities.len(), 3);
    }

    #[test]
    fn placeholder_mover_arrives_exactly_and_updates_facing() {
        let world = trivial_world();
        let mut sim = Sim::new(&world, setup());
        let grunt = EntityId(1);
        let target = Vec2Fx::from_ints(10, 14); // 4 tiles away, axis-aligned.
        sim.step(&[Command::new(
            PlayerId(0),
            0,
            1,
            CommandKind::Move {
                units: vec![grunt],
                target,
            },
        )]);
        // Speed 2600 mt/s -> per tick ~0.0867 tiles; distance 4 -> ~47 ticks.
        let mut tick = 1;
        while sim
            .snapshot()
            .entities
            .iter()
            .any(|e| e.id == grunt && e.move_state == MoveState::Moving)
        {
            sim.step(&[]);
            tick += 1;
            assert!(tick < 200, "mover never arrived");
        }
        let snapshot = sim.snapshot();
        let entity = snapshot.entities.iter().find(|e| e.id == grunt).unwrap();
        assert_eq!(entity.pos, target, "must land exactly on the target");
        // Facing was set during movement and preserved at arrival.
        assert!(!entity.facing.is_zero());
    }

    #[test]
    fn same_seed_same_everything_different_seed_changes_jitter() {
        let mut world = trivial_world();
        world.spawn_jitter_milli = 250;
        let a = Sim::new(&world, setup());
        let b = Sim::new(&world, setup());
        assert_eq!(a.state_hash(), b.state_hash());
        let c = Sim::new(
            &world,
            MatchSetup {
                seed: 8,
                players: setup().players,
            },
        );
        assert_ne!(a.state_hash(), c.state_hash());
    }

    #[test]
    fn periodic_hash_appears_every_checkpoint_interval() {
        let mut sim = Sim::new(&trivial_world(), setup());
        let mut hashes_at: Vec<Tick> = vec![];
        for _ in 0..95 {
            let out = sim.step(&[]);
            if out.hash.is_some() {
                hashes_at.push(sim.tick());
            }
        }
        assert_eq!(hashes_at, vec![30, 60, 90]);
    }

    #[test]
    fn invalid_commands_leave_the_hash_untouched() {
        let world = trivial_world();
        let mut quiet = Sim::new(&world, setup());
        let mut noisy = Sim::new(&world, setup());
        let invalid = vec![
            Command::new(
                PlayerId(9),
                0,
                1,
                CommandKind::Move {
                    units: vec![EntityId(1)],
                    target: Vec2Fx::from_ints(1, 1),
                },
            ),
            Command::new(
                PlayerId(0),
                1,
                1,
                CommandKind::Move {
                    units: vec![EntityId(1)],
                    target: Vec2Fx::from_ints(1, 1),
                },
            ),
            Command::new(
                PlayerId(0),
                0,
                1,
                CommandKind::Move {
                    units: vec![EntityId(99)],
                    target: Vec2Fx::from_ints(1, 1),
                },
            ),
        ];
        for tick in 0..35 {
            let quiet_out = quiet.step(&[]);
            let noisy_out = if tick == 0 {
                noisy.step(&invalid)
            } else {
                noisy.step(&[])
            };
            if tick == 0 {
                assert!(noisy_out
                    .events
                    .iter()
                    .any(|e| matches!(e, Event::CommandRejected { .. })));
            }
            if let Some(h) = quiet_out.hash {
                assert_eq!(Some(h), noisy_out.hash);
            }
            assert_eq!(quiet.state_hash(), noisy.state_hash());
        }
    }

    #[test]
    fn footprint_spawns_block_tiles_for_movers() {
        // Kind 3 "wall": a 2x2 footprint at center (10,12) -> tiles (9..10,
        // 11..12). The grunt must route around it and never stand inside.
        let mut world = trivial_world();
        world.kinds.push(KindTemplate::from_caps(vec![
            CapTemplate::Health {
                max_hp: 100,
                regen_per_tick: 0,
            },
            CapTemplate::Vision {
                radius_milli_tiles: 1000,
            },
            CapTemplate::Footprint { w: 2, h: 2 },
        ]));
        world.initial_spawns.push(SpawnDef {
            owner: PlayerId(0),
            kind: KindId(3),
            pos: Vec2Fx::from_ints(10, 12),
        });
        let mut sim = Sim::new(&world, setup());
        // March straight at the wall's center: the blocked goal resolves to
        // the nearest reachable tile, which is adjacent to the footprint.
        sim.step(&[Command::new(
            PlayerId(0),
            0,
            1,
            CommandKind::Move {
                units: vec![EntityId(1)],
                target: Vec2Fx::from_ints(10, 12),
            },
        )]);
        let mut tick = 1;
        while sim
            .snapshot()
            .entities
            .iter()
            .any(|entity| entity.id == EntityId(1) && entity.move_state == MoveState::Moving)
        {
            sim.step(&[]);
            tick += 1;
            assert!(tick < 200, "the mover never resolved its order");
            let pos = sim
                .snapshot()
                .entities
                .iter()
                .find(|entity| entity.id == EntityId(1))
                .unwrap()
                .pos;
            let (x, y) = (pos.x.floor_int(), pos.y.floor_int());
            assert!(
                !(x == 9 || x == 10) || !(y == 11 || y == 12),
                "mover stood inside the footprint at ({x},{y})"
            );
        }
        // It arrived next to the wall, close to the order target.
        let pos = sim
            .snapshot()
            .entities
            .iter()
            .find(|entity| entity.id == EntityId(1))
            .unwrap()
            .pos;
        assert!(Vec2Fx::dist(pos, Vec2Fx::from_ints(10, 12)) <= Fx::from_milli(1600));
    }

    #[test]
    fn dying_structures_release_their_tiles() {
        // A decaying 2x2 wall: when its health runs out, the ground reopens —
        // the next mover may stand where it stood.
        let mut world = trivial_world();
        world.kinds.push(KindTemplate::from_caps(vec![
            CapTemplate::Health {
                max_hp: 2,
                regen_per_tick: -1,
            },
            CapTemplate::Footprint { w: 2, h: 2 },
        ]));
        world.initial_spawns.push(SpawnDef {
            owner: PlayerId(0),
            kind: KindId(3),
            pos: Vec2Fx::from_ints(10, 12),
        });
        let mut sim = Sim::new(&world, setup());
        // Wait for the wall to die (hp 2, regen -1 -> gone after two steps).
        for _ in 0..4 {
            sim.step(&[]);
        }
        assert!(sim
            .snapshot()
            .entities
            .iter()
            .all(|entity| entity.kind != KindId(3)));
        // Now order the grunt onto the freed ground; it must be able to stand
        // there (the tile is passable again).
        sim.step(&[Command::new(
            PlayerId(0),
            4,
            1,
            CommandKind::Move {
                units: vec![EntityId(1)],
                target: Vec2Fx::from_ints(10, 12),
            },
        )]);
        let mut tick = 5;
        while sim
            .snapshot()
            .entities
            .iter()
            .any(|entity| entity.id == EntityId(1) && entity.move_state == MoveState::Moving)
        {
            sim.step(&[]);
            tick += 1;
            assert!(tick < 200, "the mover never arrived on the freed ground");
        }
        let pos = sim
            .snapshot()
            .entities
            .iter()
            .find(|entity| entity.id == EntityId(1))
            .unwrap()
            .pos;
        assert_eq!(pos, Vec2Fx::from_ints(10, 12));
    }

    #[test]
    fn player_view_filters_by_fog_and_exposes_ledger() {
        let world = trivial_world();
        let sim = Sim::new(&world, setup());
        let view = sim.player_view(PlayerId(0));
        // Own grunt visible; the enemy grunt at (50,50) is 57 tiles away — beyond
        // the 7-tile vision of player 0's units.
        assert_eq!(view.entities.len(), 1);
        assert_eq!(view.entities[0].id, EntityId(1));
        assert_eq!(view.resources.len(), 1);
        assert_eq!(view.resources[0].amount, 200);
        // A slot outside the match gets an empty view, not a panic.
        let empty = sim.player_view(PlayerId(9));
        assert!(empty.entities.is_empty());
        assert!(empty.resources.is_empty());
    }
}
