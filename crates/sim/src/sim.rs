//! `Sim` — the deterministic simulation spine (plan §6.2, §6.3).
//!
//! The fixed update order inside [`Sim::step`] below is the plan's §6.3 order,
//! verbatim, and it never varies per content item. Milestones fill the stages in;
//! every stage that does not exist yet runs as a documented no-op with a pointer
//! to the milestone that implements it. The order is the contract: inserting a
//! system into the wrong stage is a review-visible change, not a silent one.

use pandemonium_fx::{Fx, Rng};
use pandemonium_sim_api::{
    Command, EntityId, EntityView, Event, MatchSetup, PlayerId, PlayerView, QueueItemView,
    QueueView, Snapshot, Stage, StageObserver, Tick, TileFog, Vec2Fx, ViewResource,
};

use crate::command::apply_commands;
use crate::fixture::{ScheduledSpawnDef, SpawnDef, TrivialWorld};
use crate::hash::hash_state;
use crate::movement::advance_movement;
use crate::nav::NavGrid;
use crate::world::{Lifecycle, PlayerState, World};

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
    /// Per-player per-tile fog state (plan §9.5, M6). Derived state — NOT
    /// part of the canonical state hash (A-059: fog is a pure function of
    /// positions + Vision radii, both of which ARE hashed, so hashing fog
    /// would be redundant and would couple the hash to a per-player
    /// derivative that A10 explicitly wants "hashes equal fog on/off").
    fog: crate::vision::FogState,
    /// The resolved match outcome, once stage 10 has fired `MatchEnded` (M8,
    /// plan §9.7). Derived state — NOT part of the canonical hash (A-066:
    /// the outcome is a pure function of the entity set and the players'
    /// `resigned` flags, both of which ARE hashed). `None` while the match
    /// is ongoing; `Some` once it has ended, cached so `MatchEnded` is
    /// emitted exactly once.
    outcome: Option<crate::match_rules::MatchOutcome>,
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
            fog: crate::vision::FogState::new(players.len(), world.width_tiles, world.height_tiles),
            outcome: None,
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
        // Population usage and cap reflect the starting forces (1 command
        // center + 4 workers = 4/10 in the Alpha content).
        crate::production::recompute_population(
            &mut sim.world,
            &sim.fixture.kinds,
            sim.fixture.base_population_cap,
        );
        // Initial fog pass: mark every tile inside any friendly vision radius
        // Visible (plan §9.5: "full recompute only at match start"). The
        // per-tick incremental pass (stage 9) maintains it from here.
        crate::vision::advance_vision(&mut sim.fog, &sim.world);
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

    /// The resolved match outcome (M8, plan §9.7). `None` while the match is
    /// ongoing; `Some` once stage 10 has fired `MatchEnded`. The outcome is
    /// derived state (A-066: not part of the canonical hash) cached so the
    /// event is emitted exactly once. The host consults this to stop driving
    /// controllers and to show the end screen.
    pub fn outcome(&self) -> Option<crate::match_rules::MatchOutcome> {
        self.outcome
    }

    /// Whether the match has ended (plan §9.7, M8). Convenience over
    /// [`Self::outcome`] for the host's loop gate.
    pub fn is_finished(&self) -> bool {
        self.outcome.is_some()
    }

    /// Test-only window into one mover's runtime movement state (the
    /// movement-layer tests inspect paths and counters through this).
    #[cfg(test)]
    pub(crate) fn debug_move_state(&self, id: EntityId) -> Option<(Vec<Vec2Fx>, u32, u32)> {
        self.world
            .move_of(id)
            .map(|def| (def.path.clone(), def.stuck_ticks, def.repaths))
    }

    /// Test-only window into a node's remaining resource amount (the economy
    /// tests assert extraction and depletion through this).
    #[cfg(test)]
    pub(crate) fn resource_probe(&self, id: EntityId) -> Option<crate::world::ResourceBodyDef> {
        self.world.resource_of(id).copied()
    }

    /// Test-only window into an entity's head order (the economy tests assert
    /// phase transitions and auto-seek rewrites through this).
    #[cfg(test)]
    pub(crate) fn order_probe(&self, id: EntityId) -> Option<crate::world::Order> {
        self.world
            .entity(id)
            .and_then(|e| e.orders.first().copied())
    }

    /// Test-only window into an entity's lifecycle (the construction tests
    /// assert UnderConstruction -> Active through this).
    #[cfg(test)]
    pub(crate) fn lifecycle_probe(&self, id: EntityId) -> Option<crate::world::Lifecycle> {
        self.world.entity(id).map(|entity| entity.lifecycle)
    }

    /// Test-only window into a producer's queue (the production tests inspect
    /// enqueue/cancel behavior through this).
    #[cfg(test)]
    pub(crate) fn queue_probe(&self, id: EntityId) -> Vec<(u32, u32)> {
        self.world
            .produce_of(id)
            .map(|def| {
                def.queue
                    .iter()
                    .map(|item| (item.producible.0, item.progress_ticks))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Test-only window into an entity's Attack capability (the combat tests
    /// assert cooldown + target slot transitions through this).
    #[cfg(test)]
    pub(crate) fn attack_probe(&self, id: EntityId) -> Option<crate::world::AttackDef> {
        self.world.attack_of(id).copied()
    }

    /// The on-demand canonical state hash (plan §6.4).
    pub fn state_hash(&self) -> u64 {
        hash_state(&self.world, self.tick, &self.rng, self.next_entity_id)
    }

    /// The read-only presentation copy (plan §6.4): entities in ascending id
    /// order. The client interpolates between successive snapshots (plan §11.1).
    ///
    /// Performance: the `hp_fraction_milli` field is computed via a lockstep
    /// walk over `entities` and the `health` store (both ascending by id),
    /// avoiding a binary search through the health store per entity. The
    /// resulting `EntityView` is byte-identical to the per-entity lookup path
    /// — pinned by the acceptance suite's golden hashes.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            tick: self.tick,
            entities: self.entity_views_lockstep(self.world.entities.iter()),
        }
    }

    /// The fog-filtered view for one player (plan §9.6): own entities plus
    /// entities inside friendly vision radii — the *only* window an AI controller
    /// gets (FD-7, FD-8). A slot that is not in the match gets an empty view.
    ///
    /// M6 uses the cached fog state (stage 9 maintains it). The M1 path
    /// (`crate::command::visible_to`) is still used by the command gate for
    /// targeting; both produce the same answer — the cache is a pure
    /// function of the same positions + Vision radii the on-demand scan reads.
    pub fn player_view(&self, player: PlayerId) -> PlayerView {
        let state = self.world.player(player);
        let entities = self.entity_views_lockstep(
            self.world
                .entities
                .iter()
                .filter(|e| crate::vision::visible_to(&self.fog, &self.world, player, e.id)),
        );
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
        // The player's fog row and own production queues. A slot that is not
        // in the match has no fog row and no queues (empty vecs — the same
        // "absent player gets an empty view" rule the ledger follows).
        let player_index = self.world.players.iter().position(|p| p.player == player);
        let fog = player_index
            .and_then(|index| self.fog.players.get(index))
            .map(|row| {
                row.iter()
                    .map(|tile| match tile {
                        crate::vision::TileVisibility::Hidden => TileFog::Hidden,
                        crate::vision::TileVisibility::Explored => TileFog::Explored,
                        crate::vision::TileVisibility::Visible => TileFog::Visible,
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Own producers only, ascending entity id (the produce store's
        // invariant). Fog hides enemy production exactly as it hides enemy
        // entities, so "own" is the whole rule — no visibility scan needed.
        let production = self
            .world
            .produce
            .iter()
            .filter(|(id, _)| {
                self.world
                    .entity(*id)
                    .is_some_and(|entity| entity.owner == player)
            })
            .map(|(id, def)| QueueView {
                producer: *id,
                items: def
                    .queue
                    .iter()
                    .map(|item| QueueItemView {
                        kind: item.producible,
                        progress_milli: self.queue_item_progress_milli(item),
                    })
                    .collect(),
                rally: def.rally,
            })
            .collect();
        PlayerView {
            tick: self.tick,
            player,
            resources,
            population,
            population_cap,
            entities,
            fog,
            production,
        }
    }

    /// One queue item's work as thousandths of its kind's build time (the
    /// same 0..=1000 integer convention as `hp_fraction_milli`). A kind
    /// missing from the fixture, or with zero build time, reports 1000 —
    /// both are degenerate content cases that would otherwise divide by
    /// zero, and a completed (or free) item reads as done.
    fn queue_item_progress_milli(&self, item: &crate::world::QueueItem) -> u32 {
        let build_time = self
            .fixture
            .kinds
            .get(item.producible.0 as usize)
            .map(|kind| kind.economy.build_time_ticks)
            .unwrap_or(0);
        if build_time == 0 {
            return 1000;
        }
        ((item.progress_ticks as i64 * 1000) / build_time as i64).clamp(0, 1000) as u32
    }

    /// Builds [`EntityView`]s from an iterator of entities, computing each
    /// entity's `hp_fraction_milli` in lockstep with the `health` store —
    /// both sequences are ascending by id (the store invariant), so a single
    /// forward walk produces O(n + h) instead of n × O(log h). The output
    /// order matches the input iterator's (entity-ascending by the snapshot
    /// contract).
    fn entity_views_lockstep<'a, I>(&self, entities: I) -> Vec<EntityView>
    where
        I: Iterator<Item = &'a crate::world::EntityRecord>,
    {
        let mut health_iter = self.world.health.iter().peekable();
        let mut views = Vec::new();
        for entity in entities {
            // Advance the health cursor past entries below this entity's id
            // (the store's ascending invariant makes this safe — once a
            // health entry's id is below the current entity's, no later
            // entity will match it either).
            while health_iter.peek().is_some_and(|(hid, _)| *hid < entity.id) {
                health_iter.next();
            }
            let hp_fraction_milli = match health_iter.peek() {
                Some((hid, def)) if *hid == entity.id && def.max_hp > 0 => {
                    ((def.hp as i64 * 1000) / (def.max_hp as i64)).clamp(0, 1000) as u32
                }
                _ => 0,
            };
            views.push(EntityView {
                id: entity.id,
                owner: entity.owner,
                kind: entity.kind,
                pos: entity.pos,
                facing: entity.facing,
                hp_fraction_milli,
                move_state: entity.move_state(),
            });
        }
        views
    }

    /// **The only way state advances** (FD-2). All commands must target
    /// `self.tick` — mismatches are rejected as events, never silently dropped.
    /// Returns that tick's events and the periodic hash when due.
    ///
    /// The golden path: no observer. Delegates to [`Sim::step_observed`]
    /// with `None` — byte-identical behavior to the pre-B-002 pipeline.
    pub fn step(&mut self, commands: &[Command]) -> StepOutput {
        self.step_observed(commands, None)
    }

    /// The same fixed pipeline as [`Sim::step`], with an optional
    /// [`StageObserver`] notified at each of the eleven stage boundaries, in
    /// pipeline order, immediately before that stage's work (B-002's
    /// per-stage profiling seam).
    ///
    /// The observer is read-only telemetry by contract: it receives stage
    /// boundaries and can never reach simulation state, so its presence or
    /// absence cannot change an outcome, an event stream, or a checkpoint
    /// hash. Timing collectors (which need a clock) are implemented outside
    /// the determinism crates, in `tools`.
    ///
    /// All commands must target `self.tick` — mismatches are rejected as
    /// events, never silently dropped. Returns that tick's events and the
    /// periodic hash when due.
    pub fn step_observed(
        &mut self,
        commands: &[Command],
        mut observer: Option<&mut dyn StageObserver>,
    ) -> StepOutput {
        // Stage 1 — Apply commands: sorted by (issuer, seq), validated, applied;
        //           invalid ones emit CommandRejected (plan §6.3.1).
        if let Some(observer) = observer.as_mut() {
            observer.stage(Stage::Commands);
        }
        apply_commands(
            &mut self.world,
            &self.fixture,
            &mut self.nav,
            &mut self.next_entity_id,
            self.tick,
            commands,
            &mut self.events,
        );

        // Stage 2 — Orders: resolve each entity's current order into system
        //           intents. M5's slice: the economy travel targets (stage 6
        //           consumes them through `movement_target`) and the gather
        //           phase machine (stage 4). Richer resolution arrives with
        //           combat (M6). No direct call — the slice lives inside the
        //           systems below; the boundary is reported all the same.
        if let Some(observer) = observer.as_mut() {
            observer.stage(Stage::Orders);
        }
        // Stage 3 — Production & construction: advance queues and construction
        //           progress; spawn/complete (plan §9.4, M5). The fixture's
        //           scheduled spawns (the M1 stand-in) run first, then the
        //           production queues, then the construction sites; the
        //           population usage and cap settle at the stage's end.
        if let Some(observer) = observer.as_mut() {
            observer.stage(Stage::Production);
        }
        self.run_scheduled_spawns();
        crate::production::advance_production(
            &mut self.world,
            &self.fixture,
            &self.nav,
            &mut self.next_entity_id,
            &mut self.rng,
            &mut self.events,
        );

        // Stage 4 — Economy: gathering timers, deliveries, storage, spending
        //           effects (M5, plan §9.3): the worker gather loop — travel
        //           (through stage 6's shared travel targets), gather timers,
        //           cargo, deposits, depletion with auto-seek.
        if let Some(observer) = observer.as_mut() {
            observer.stage(Stage::Economy);
        }
        crate::economy::advance_economy(&mut self.world, &mut self.nav, &mut self.events);

        // Stage 5 — Target acquisition (M6). No direct call — the acquire
        //           step runs inside the combat system (plan §9.2); the
        //           boundary is reported for the profile's honest zero row.
        if let Some(observer) = observer.as_mut() {
            observer.stage(Stage::Acquisition);
        }
        // Stage 6 — Movement: path requests → path following → steering →
        //           collision push-apart, entities in id order (plan §6.3.6,
        //           §9.1). M4's three-layer mover: A* over the nav grid with
        //           deterministic tie-breaks, waypoint steering, spatial-hash
        //           push-apart, and stuck detection ending in MoveFailed.
        if let Some(observer) = observer.as_mut() {
            observer.stage(Stage::Movement);
        }
        advance_movement(&mut self.world, &self.nav, &mut self.events);

        // Stage 7 — Combat: resolve attacks, apply damage, fire AttackHit (M6,
        //           plan §9.2). Damage subtracts from Health.hp; stage 8
        //           detects the resulting deaths and removes the entities.
        if let Some(observer) = observer.as_mut() {
            observer.stage(Stage::Combat);
        }
        crate::combat::advance_combat(&mut self.world, &mut self.events);

        // Stage 8 — Death & cleanup: advance health, fire lifecycle events,
        //           remove the dead. Ids are never reused (plan §6.3.8). The
        //           A12 checker below runs at this stage's end in debug
        //           builds — its cost lands in this stage's profile row.
        if let Some(observer) = observer.as_mut() {
            observer.stage(Stage::Cleanup);
        }
        self.advance_health_and_cleanup();

        // A12 (plan §13): every invariant, every tick, debug builds only —
        // the soak and every dev-profile test enforce them continuously.
        #[cfg(debug_assertions)]
        crate::invariants::check(&self.world, &self.fixture, &self.nav, self.next_entity_id);

        // Stage 9 — Vision: incremental per-player visibility update (M6,
        //           plan §9.5). The fog state is derived (A-059: not part of
        //           the canonical hash — it is a pure function of positions +
        //           Vision radii, both of which ARE hashed).
        if let Some(observer) = observer.as_mut() {
            observer.stage(Stage::Vision);
        }
        crate::vision::advance_vision(&mut self.fog, &self.world);

        // Stage 10 — Match rules: defeat/victory evaluation (M8, plan §9.7).
        //            Idempotent: once `outcome` is `Some`, this is a no-op.
        //            When the match transitions to ended this tick, exactly
        //            one `MatchEnded` event is emitted. The outcome is
        //            derived state (A-066: not part of the canonical hash —
        //            a pure function of the entity set and `resigned` flags,
        //            both of which ARE hashed), so toggling evaluation on/off
        //            cannot change a checkpoint.
        if let Some(observer) = observer.as_mut() {
            observer.stage(Stage::MatchRules);
        }
        crate::match_rules::evaluate(&self.world, self.tick, &mut self.outcome, &mut self.events);

        // Stage 11 — Finalize: increment the tick, flush events, compute the
        //            periodic hash.
        if let Some(observer) = observer.as_mut() {
            observer.stage(Stage::Finalize);
        }
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
    /// release their footprint tiles on the way out, so the ground reopens,
    /// and the population usage and cap settle again after the removals.
    ///
    /// Performance: the dead are removed in one batched pass per store
    /// ([`crate::world::World::remove_batch`]) rather than `m` separate
    /// `World::remove` calls. The post-state is byte-identical (the same
    /// survivors, in the same order) — only the cost shape changes. Footprint
    /// release happens per-entity before the batched removal because it reads
    /// the entity's position and footprint (still in the world at that point).
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
        // The dead list (ascending id — the health store's invariant) is what
        // `clear_dead_targets` will match against attacker slots, so clone it
        // before the removal pass consumes it.
        let dead_ids = dead.clone();
        for id in &dead {
            if let Some(entity) = self.world.entity_mut(*id) {
                entity.lifecycle = Lifecycle::Dead;
            }
            self.release_footprint(*id);
            self.events.push(Event::Died { entity: *id });
        }
        // One batched sweep per store — O(n + m) instead of m · O(log n + n).
        self.world.remove_batch(&dead_ids);
        // After removal, drop any attacker target slots pointing at the dead
        // (the dead id is never reused, so the slot would otherwise dangle
        // forever). Ascending attacker-id order (combat.rs's contract).
        crate::combat::clear_dead_targets(&mut self.world, &dead_ids);
        crate::production::recompute_population(
            &mut self.world,
            &self.fixture.kinds,
            self.fixture.base_population_cap,
        );
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

    #[test]
    fn player_view_exposes_fog_tiles_in_map_order() {
        let world = trivial_world();
        let sim = Sim::new(&world, setup());
        let width = 64usize;
        let view = sim.player_view(PlayerId(0));
        assert_eq!(view.fog.len(), width * width, "one entry per tile");
        let at = |x: usize, y: usize| view.fog[y * width + x];
        // The tile under the player's own grunt (10,10) is currently visible;
        // the enemy grunt's corner (50,50) is beyond its 7-tile vision and
        // has never been seen — Hidden, not merely Explored.
        assert_eq!(at(10, 10), TileFog::Visible);
        assert_eq!(at(50, 50), TileFog::Hidden);
        // The enemy's own view mirrors it: its tile visible, ours hidden.
        let enemy = sim.player_view(PlayerId(1));
        assert_eq!(enemy.fog[50 * width + 50], TileFog::Visible);
        assert_eq!(enemy.fog[10 * width + 10], TileFog::Hidden);
        // A slot outside the match has no fog row at all.
        assert!(sim.player_view(PlayerId(9)).fog.is_empty());
    }

    #[test]
    fn player_view_exposes_own_production_queues_with_progress() {
        let mut world = trivial_world();
        // kind 3 "factory": a producer; kind 4 "slow grunt": its 10-tick product.
        world.kinds.push(KindTemplate {
            caps: vec![CapTemplate::Produce {}],
            economy: KindEconomy::default(),
        });
        world.kinds.push(KindTemplate {
            caps: vec![
                CapTemplate::Health {
                    max_hp: 10,
                    regen_per_tick: 0,
                },
                CapTemplate::Move {
                    speed_milli_tiles_per_s: 2600,
                    radius_milli_tiles: 350,
                },
            ],
            economy: KindEconomy {
                cost: Vec::new(),
                build_time_ticks: 10,
                population: 0,
                requires: Vec::new(),
            },
        });
        world.production = vec![(KindId(3), vec![KindId(4)])];
        world.initial_spawns.push(SpawnDef {
            owner: PlayerId(0),
            kind: KindId(3),
            pos: Vec2Fx::from_ints(12, 10),
        });
        let mut sim = Sim::new(&world, setup());
        let producer = EntityId(3); // after the two initial grunts
        let output = sim.step(&[Command::new(
            PlayerId(0),
            0,
            1,
            CommandKind::Train {
                producer,
                unit: KindId(4),
            },
        )]);
        assert!(
            output
                .events
                .iter()
                .any(|event| matches!(event, Event::ProductionStarted { .. })),
            "the train command must be accepted"
        );
        let view = sim.player_view(PlayerId(0));
        assert_eq!(view.production.len(), 1, "the own queue is visible");
        let queue = &view.production[0];
        assert_eq!(queue.producer, producer);
        assert_eq!(queue.items.len(), 1);
        assert_eq!(queue.items[0].kind, KindId(4));
        assert!(queue.items[0].progress_milli < 1000, "head in progress");
        // Progress advances monotonically toward 1000.
        for _ in 0..4 {
            sim.step(&[]);
        }
        let later = sim.player_view(PlayerId(0)).production[0].items[0].progress_milli;
        assert!(later > queue.items[0].progress_milli, "progress advanced");
        // Fog hides enemy production: player 1 owns no producers.
        assert!(sim.player_view(PlayerId(1)).production.is_empty());
    }
}
