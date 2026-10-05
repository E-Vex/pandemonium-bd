//! `MatchHost` — the engine's ownership of one running match (plan §11.1, A-003:
//! the engine's game loop owns stepping the simulation).
//!
//! This type is the M3 exit criterion made structural: the [`pandemonium_sim::Sim`]
//! is a *private* field, and the only way to affect it is [`MatchHost::submit`]
//! (a command) feeding the next [`MatchHost::advance`] (the step inputs) — there
//! is no `&mut Sim` escape hatch for the client. Presentation reads cross the
//! boundary as snapshots, views, events, and the interpolated render snapshot.
//!
//! Command timing (plan §6.5): submitted commands are (re-)stamped with the tick
//! the *next* step will apply. In single-player the client submits from its
//! (interpolated, slightly behind) viewpoint, so this is "current tick + 1" from
//! the player's perspective; the host keeps the invariant that whatever it holds
//! is applied exactly once, at the tick the simulation expects, so a submitted
//! command is never rejected for a stale tick.
//!
//! The initial `Spawned` batch that `Sim::new` buffers surfaces through the
//! first `advance` (the first step drains it — the documented M1 behavior).
//!
//! **M8** extends the host with optional AI controllers (plan §9.6, A-063: the
//! client hosts its vs-AI opponent through this same seam — one host, one tick,
//! the human's submitted commands and the controllers' emitted commands feed
//! the same step), a command log (for replay recording), the match outcome
//! (stage 10's `MatchEnded` surfaces through [`MatchHost::outcome`]), and a
//! [`MatchHost::restart`] that rebuilds the Sim from the same setup with no
//! leaked state (plan §9.7, A15). The M3 exit criterion holds: the Sim is still
//! private, `submit` + `advance` are still the only mutation paths, and
//! controllers receive an immutable `PlayerView` (FD-7) — they never see `&mut
//! Sim`.

use std::time::Duration;

use pandemonium_ai::Controller;
use pandemonium_sim::{MatchOutcome, Sim, TrivialWorld};
use pandemonium_sim_api::{
    Command, Event, MatchSetup, PlayerId, PlayerView, Snapshot, Tick, Vec2Fx,
};

use crate::clock::FixedTimestep;
use crate::interpolate::{Interpolator, RenderSnapshot};
use crate::renderer::HudState;

/// What one `advance` produced: the steps run, the events they emitted, and
/// the periodic checkpoint hashes due during them (plan §6.3 stage 11).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct FrameOutcome {
    /// How many simulation steps ran this frame (0..=catch-up cap).
    pub steps: u32,
    /// Every event emitted by those steps, in order.
    pub events: Vec<Event>,
    /// Checkpoint hashes due during those steps: `(tick, hash)`.
    pub hashes: Vec<(Tick, u64)>,
}

/// A running match: the simulation, the fixed-timestep clock, the
/// interpolator, the pending command batch, and (M8) the optional AI
/// controllers + the growing command log. Drive it from the client's frame
/// loop: submit commands as input produces them, then `advance` with the
/// frame's real delta, then render via [`MatchHost::render_snapshot`].
///
/// M8: `Clone`/`Debug` removed — `Box<dyn Controller>` carries neither, and
/// no caller clones a `MatchHost` (the M3 tests assert on outcomes, not on
/// the host itself). Restart is by drop + reconstruct (the client retains
/// its own `ContentBundle` and `MatchSetup` references, the A15 test
/// constructs two fresh `MatchHost` instances) — see [`Self::with_controllers`].
pub struct MatchHost {
    sim: Sim,
    clock: FixedTimestep,
    interpolator: Interpolator,
    pending: Vec<Command>,
    paused: bool,
    /// M8: optional AI controllers, one per driven slot, invoked in ascending
    /// slot order on the tick boundary (the same seam as [`crate::AiMatchHost`]).
    /// Empty by default — `MatchHost::new` is the human-only path the M3 tests
    /// use; `MatchHost::with_controllers` adds them for the windowed vs-AI
    /// client.
    controllers: Vec<(PlayerId, Box<dyn Controller>)>,
    /// M8: every command fed to the simulation, in feed order — the match's
    /// command log (rejections included; they changed no state). The same
    /// shape as [`crate::AiMatchHost::log`]; a windowed match's replay is
    /// this log plus the periodic checkpoints.
    log: Vec<Command>,
}

impl MatchHost {
    /// Starts a match: constructs the simulation (whose `Sim::new` spawns the
    /// initial entities and buffers the initial `Spawned` events), pushes the
    /// tick-0 snapshot, and arms the clock at 30 Hz. No AI controllers — the
    /// human-only path the M3 tests use. See [`Self::with_controllers`] for
    /// the windowed vs-AI client (M8).
    pub fn new(world: &TrivialWorld, setup: MatchSetup) -> Self {
        Self::with_controllers(world, setup, Vec::new())
    }

    /// Starts a match with optional AI controllers (M8, plan §9.6, A-063):
    /// the human's submitted commands and the controllers' emitted commands
    /// feed the same step. Every controller must drive a slot of the setup,
    /// no slot twice; controllers are invoked in ascending slot order every
    /// tick (part of the match's determinism contract — the same shape as
    /// [`crate::AiMatchHost::new`]).
    pub fn with_controllers(
        world: &TrivialWorld,
        setup: MatchSetup,
        controllers: Vec<(PlayerId, Box<dyn Controller>)>,
    ) -> Self {
        for (player, _) in &controllers {
            assert!(
                setup.players.iter().any(|entry| entry.player == *player),
                "controller for slot {player:?}, which is not in the match"
            );
        }
        let mut controllers = controllers;
        controllers.sort_by_key(|(player, _)| *player);
        assert!(
            controllers.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "one controller per slot at most"
        );
        let sim = Sim::new(world, setup);
        let mut interpolator = Interpolator::new();
        interpolator.push(sim.snapshot());
        Self {
            sim,
            clock: FixedTimestep::new(pandemonium_sim::TICKS_PER_SECOND),
            interpolator,
            pending: Vec::new(),
            paused: false,
            controllers,
            log: Vec::new(),
        }
    }

    /// The current simulation tick (the tick the next step applies).
    pub fn tick(&self) -> Tick {
        self.sim.tick()
    }

    /// Queues a command for the next step, (re-)stamping its tick so it
    /// applies exactly when the simulation expects it.
    pub fn submit(&mut self, mut command: Command) {
        command.tick = self.sim.tick();
        self.pending.push(command);
    }

    /// Advances real time: runs the due simulation steps (the pending commands
    /// feed the first of them), pushes each step's snapshot into the
    /// interpolator, and returns everything the frame produced.
    ///
    /// While paused (plan §11.6) the clock itself freezes: elapsed time is
    /// discarded rather than accumulated, so unpausing owes no catch-up burst.
    pub fn advance(&mut self, real_dt: Duration) -> FrameOutcome {
        if self.paused {
            return FrameOutcome::default();
        }
        let mut outcome = FrameOutcome::default();
        let steps = self.clock.update(real_dt);
        for _ in 0..steps {
            self.run_one_step(&mut outcome);
        }
        outcome
    }

    /// Runs exactly one simulation step regardless of the clock and the pause
    /// state (plan §11.6 single-step debugging). The clock's accumulator is
    /// untouched: stepping is a debug action, not simulated time passing.
    pub fn step_once(&mut self) -> FrameOutcome {
        let mut outcome = FrameOutcome::default();
        self.run_one_step(&mut outcome);
        outcome
    }

    /// One simulation step: feeds the pending commands, records events and
    /// checkpoint hashes, pushes the new snapshot.
    fn run_one_step(&mut self, outcome: &mut FrameOutcome) {
        // M8: AI controllers think on the tick boundary, on their fog-filtered
        // view of the tick being applied (the same contract as
        // [`crate::AiMatchHost::advance`]). Their commands feed the same step
        // alongside the human's submitted commands, through the same validation
        // gate; the log records everything fed (rejections included).
        let tick = self.sim.tick();
        let mut feed = std::mem::take(&mut self.pending);
        for (player, controller) in self.controllers.iter_mut() {
            let view = self.sim.player_view(*player);
            let mut out = Vec::new();
            controller.think(&view, tick, &mut out);
            debug_assert!(
                out.iter()
                    .all(|command| command.tick == tick && command.issuer == *player),
                "controllers must emit commands for their own slot at the view's tick"
            );
            feed.extend(out);
        }
        self.log.extend(feed.iter().cloned());
        let step = self.sim.step(&feed);
        outcome.steps += 1;
        outcome.events.extend(step.events);
        if let Some(hash) = step.hash {
            outcome.hashes.push((self.sim.tick(), hash));
        }
        self.interpolator.push(self.sim.snapshot());
    }

    /// Pauses or resumes (plan §11.6).
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Whether the host is paused.
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// The resolved match outcome (M8, plan §9.7). `None` while the match is
    /// ongoing; `Some` once stage 10 has fired `MatchEnded` (cached on the
    /// Sim — see [`pandemonium_sim::Sim::outcome`]). The host consults this
    /// to stop its match loop and to show the end screen; the simulation
    /// keeps stepping past `MatchEnded` for replay re-simulation fidelity.
    pub fn outcome(&self) -> Option<MatchOutcome> {
        self.sim.outcome()
    }

    /// Whether the match has ended (M8). Convenience over [`Self::outcome`].
    pub fn is_finished(&self) -> bool {
        self.sim.is_finished()
    }

    /// Every command fed to the simulation so far, in feed order (M8) — the
    /// match's command log (rejections included; they changed no state). A
    /// windowed match's replay is this log plus the periodic checkpoint
    /// hashes; re-simulating it with [`pandemonium_sim::run_command_log`]
    /// reproduces every checkpoint without running any controller.
    pub fn log(&self) -> &[Command] {
        &self.log
    }

    /// Gathers the HUD and debug overlay data for one player (plan §11.4
    /// resource/population display, §11.6 tick counter + state hash + pause
    /// state). Presentation-only plain values read through the boundary —
    /// never simulation internals.
    ///
    /// Performance (M10 prep): `state_hash` is left at zero here — the
    /// canonical hash is O(n) over the whole world, and the HUD only reads
    /// it inside the §11.6 debug overlay (which is off by default). The
    /// client now calls [`Self::hud_state_with_hash`] when the overlay is on,
    /// and the cheap [`Self::hud_state`] every other frame. The shape of
    /// [`HudState`] is unchanged; only the always-zero default in the cheap
    /// path differs. Tests that assert on `state_hash` use the with-hash
    /// variant explicitly.
    pub fn hud_state(&self, player: PlayerId) -> HudState {
        let view = self.sim.player_view(player);
        HudState {
            tick: self.sim.tick(),
            state_hash: 0,
            paused: self.paused,
            resources: view
                .resources
                .iter()
                .map(|resource| (resource.resource, resource.amount))
                .collect(),
            population: view.population,
            population_cap: view.population_cap,
        }
    }

    /// The full HUD including the canonical `state_hash` (plan §11.6). Use
    /// this only when the debug overlay is visible — the hash is O(n) over
    /// the world and recomputing it every frame wastes the per-tick
    /// checkpoint the simulation already pays for.
    pub fn hud_state_with_hash(&self, player: PlayerId) -> HudState {
        let mut hud = self.hud_state(player);
        hud.state_hash = self.sim.state_hash();
        hud
    }

    /// The interpolated presentation snapshot for this display frame
    /// (`alpha` comes from the host's clock).
    pub fn render_snapshot(&self) -> RenderSnapshot {
        self.interpolator.render(self.clock.alpha())
    }

    /// The clock's current interpolation fraction (0..=1) — the blend point
    /// between the previous and current snapshot. The client uses it when it
    /// interpolates a *view* snapshot itself (the fog-filtered entity set) so
    /// the fog-of-war rendering lands on exactly the same blend point the
    /// host's full-snapshot path uses.
    pub fn alpha(&self) -> f32 {
        self.clock.alpha()
    }

    /// The latest exact snapshot (no interpolation) — for UI panels that must
    /// not blend (production queues, resources).
    pub fn snapshot(&self) -> Snapshot {
        self.sim.snapshot()
    }

    /// The fog-filtered view for one player (the AI's and UI's only window).
    pub fn player_view(&self, player: PlayerId) -> PlayerView {
        self.sim.player_view(player)
    }

    /// The current state hash (debug overlays, plan §11.6).
    pub fn state_hash(&self) -> u64 {
        self.sim.state_hash()
    }

    /// The next entity id the simulation will allocate (debug overlays).
    pub fn next_entity_id(&self) -> u64 {
        self.sim.next_entity_id()
    }

    /// Converts a world ground-plane point (from camera picking) back to the
    /// logical-plane fixed-point position commands carry — the boundary
    /// conversion so the simulation never sees a float (ADR-0001 §11.3).
    pub fn world_to_logical(x: f32, z: f32) -> Vec2Fx {
        Vec2Fx::new(
            crate::interpolate::world_to_fx(x),
            crate::interpolate::world_to_fx(z),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandemonium_sim::{
        CapTemplate, KindEconomy, KindTemplate, ResourceDef as SimResourceDef, SpawnDef,
    };
    use pandemonium_sim_api::{
        CommandKind, ControllerKind, EntityId, KindId, PlayerSetup, ResourceId,
    };

    fn world() -> TrivialWorld {
        TrivialWorld {
            map_id: 0x0054_4553_5400_0001,
            width_tiles: 16,
            height_tiles: 16,
            passability: TrivialWorld::open_passability(16, 16),
            buildability: TrivialWorld::open_buildability(16, 16),
            kinds: vec![KindTemplate {
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
            }],
            resources: vec![SimResourceDef {
                resource: ResourceId(0),
                starting: 200,
            }],
            production: vec![],
            base_population_cap: 0,
            initial_spawns: vec![SpawnDef {
                owner: PlayerId(0),
                kind: KindId(0),
                pos: Vec2Fx::from_ints(8, 8),
            }],
            scheduled_spawns: vec![],
            spawn_jitter_milli: 0,
        }
    }

    fn setup() -> MatchSetup {
        MatchSetup {
            seed: 7,
            players: vec![PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            }],
        }
    }

    #[test]
    fn one_second_of_frames_steps_thirty_ticks_and_reports_checkpoints() {
        let mut host = MatchHost::new(&world(), setup());
        assert_eq!(host.tick(), 0);
        let frame = Duration::from_millis(16);
        let mut total_steps = 0u32;
        let mut all_events = Vec::new();
        for _ in 0..62 {
            let outcome = host.advance(frame);
            total_steps += outcome.steps;
            all_events.extend(outcome.events);
        }
        assert_eq!(host.tick(), total_steps);
        assert!(
            (29..=31).contains(&total_steps),
            "about one second of sim time"
        );
        // The initial Spawned batch surfaced through the first advance.
        assert!(all_events.iter().any(|event| matches!(
            event,
            Event::Spawned {
                entity: EntityId(1),
                ..
            }
        )));
    }

    #[test]
    fn submitted_commands_apply_without_tick_rejections() {
        let mut host = MatchHost::new(&world(), setup());
        // A command stamped with a nonsense tick is still accepted: the host
        // owns the tick stamp.
        host.submit(Command::new(
            PlayerId(0),
            999_999,
            1,
            CommandKind::Move {
                units: vec![EntityId(1)],
                target: Vec2Fx::from_ints(4, 4),
            },
        ));
        let outcome = host.advance(Duration::from_millis(34)); // one step
        assert_eq!(outcome.steps, 1);
        assert!(
            !outcome
                .events
                .iter()
                .any(|event| matches!(event, Event::CommandRejected { .. })),
            "the host's tick stamp must satisfy the gate: {:?}",
            outcome.events
        );
        assert_eq!(host.tick(), 1);
    }

    #[test]
    fn render_snapshot_tracks_the_sim_between_and_during_frames() {
        let mut host = MatchHost::new(&world(), setup());
        let before = host.render_snapshot();
        assert_eq!(before.entities.len(), 1);
        assert_eq!(before.entities[0].id, EntityId(1));
        // Order the unit to move, run a second, and check the render position
        // progressed toward the target.
        host.submit(Command::new(
            PlayerId(0),
            0,
            1,
            CommandKind::Move {
                units: vec![EntityId(1)],
                target: Vec2Fx::from_ints(12, 8),
            },
        ));
        let frame = Duration::from_millis(34);
        for _ in 0..30 {
            host.advance(frame);
        }
        let after = host.render_snapshot();
        assert!(
            after.entities[0].pos.x > before.entities[0].pos.x,
            "the rendered entity must have moved right: {:?}",
            after.entities[0].pos
        );
    }

    #[test]
    fn a_stall_caps_catch_up_and_recovers() {
        let mut host = MatchHost::new(&world(), setup());
        let outcome = host.advance(Duration::from_secs(5));
        assert_eq!(outcome.steps, crate::clock::MAX_CATCH_UP_STEPS);
        // Backlog dropped: the next small frame owes nothing.
        let next = host.advance(Duration::from_millis(1));
        assert_eq!(next.steps, 0);
    }

    #[test]
    fn world_to_logical_round_trips_whole_tiles() {
        let logical = MatchHost::world_to_logical(12.0, 8.0);
        assert_eq!(logical, Vec2Fx::from_ints(12, 8));
    }

    #[test]
    fn pause_freezes_the_clock_without_a_catch_up_burst() {
        let mut host = MatchHost::new(&world(), setup());
        host.set_paused(true);
        assert!(host.is_paused());
        // A full paused second steps nothing and discards the time.
        for _ in 0..60 {
            assert_eq!(host.advance(Duration::from_millis(16)).steps, 0);
        }
        assert_eq!(host.tick(), 0);
        // Unpausing owes no backlog: the next frames step normally (62 × 16 ms
        // ≈ one second of frames, matching the sibling test's shape).
        host.set_paused(false);
        let mut steps = 0;
        for _ in 0..62 {
            steps += host.advance(Duration::from_millis(16)).steps;
        }
        assert!((28..=31).contains(&steps), "no catch-up burst: {steps}");
        assert_eq!(host.tick(), steps);
    }

    #[test]
    fn step_once_advances_exactly_one_tick_and_surfaces_events() {
        let mut host = MatchHost::new(&world(), setup());
        let outcome = host.step_once();
        assert_eq!(outcome.steps, 1);
        assert_eq!(host.tick(), 1);
        // The initial Spawned batch surfaces through the single step.
        assert!(outcome
            .events
            .iter()
            .any(|event| matches!(event, Event::Spawned { .. })));
        // The clock is untouched: a tiny frame still owes nothing.
        assert_eq!(host.advance(Duration::from_millis(1)).steps, 0);
        // Step-once works while paused too (that is its purpose).
        host.set_paused(true);
        assert_eq!(host.step_once().steps, 1);
        assert_eq!(host.tick(), 2);
        assert_eq!(host.advance(Duration::from_secs(1)).steps, 0);
    }

    #[test]
    fn hud_state_gathers_the_boundary_values() {
        let mut host = MatchHost::new(&world(), setup());
        host.set_paused(true);
        host.step_once();
        // The cheap hud_state path leaves state_hash at zero (the debug
        // overlay reads it lazily — see hud_state_with_hash).
        let hud = host.hud_state(PlayerId(0));
        assert_eq!(hud.tick, 1);
        assert!(hud.paused);
        assert_eq!(hud.state_hash, 0, "the cheap path skips the hash");
        assert_eq!(
            hud.resources,
            vec![(pandemonium_sim_api::ResourceId(0), 200)]
        );
        assert_eq!(hud.population, 0); // economy systems arrive in M5
        assert_eq!(hud.population_cap, 0);
        // The with-hash variant carries the canonical hash.
        let hud_full = host.hud_state_with_hash(PlayerId(0));
        assert_eq!(hud_full.state_hash, host.state_hash());
        // A slot outside the match gets the empty ledger, not a panic.
        let empty = host.hud_state(PlayerId(9));
        assert!(empty.resources.is_empty());
    }

    // ---- M8 extensions: AI controllers, command log, match outcome -------

    /// A controller that always issues one Resign command on tick 0 — the
    /// simplest deterministic behavior to drive the host's controller path
    /// and end the match in one stroke.
    struct ResignOnTickZero {
        player: PlayerId,
    }

    impl pandemonium_ai::Controller for ResignOnTickZero {
        fn think(&mut self, _view: &PlayerView, tick: Tick, out: &mut Vec<Command>) {
            if tick == 0 {
                out.push(Command::new(self.player, tick, 1, CommandKind::Resign {}));
            }
        }
    }

    #[test]
    fn a_controller_runs_on_the_tick_boundary_and_its_command_is_logged() {
        // Two-player setup, player 1 driven by the resign controller.
        let world = world();
        let setup = MatchSetup {
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
        };
        let controllers: Vec<(PlayerId, Box<dyn pandemonium_ai::Controller>)> = vec![(
            PlayerId(1),
            Box::new(ResignOnTickZero {
                player: PlayerId(1),
            }),
        )];
        let mut host = MatchHost::with_controllers(&world, setup, controllers);
        let outcome = host.advance(Duration::from_millis(34));
        assert_eq!(outcome.steps, 1);
        // The controller's Resign was fed through the same gate the human's
        // commands use, and recorded in the log.
        assert_eq!(host.log().len(), 1);
        assert!(matches!(host.log()[0].kind, CommandKind::Resign {}));
        assert_eq!(host.log()[0].issuer, PlayerId(1));
    }

    #[test]
    fn match_outcome_surfaces_through_the_host() {
        // Player 1 resigns on tick 0 via a controller; stage 10 fires
        // MatchEnded with player 0 the winner; the host's outcome reflects it.
        let world = world();
        let setup = MatchSetup {
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
        };
        let controllers: Vec<(PlayerId, Box<dyn pandemonium_ai::Controller>)> = vec![(
            PlayerId(1),
            Box::new(ResignOnTickZero {
                player: PlayerId(1),
            }),
        )];
        let mut host = MatchHost::with_controllers(&world, setup, controllers);
        assert!(!host.is_finished());
        assert!(host.outcome().is_none());
        host.advance(Duration::from_millis(34));
        // The M3 test world has no Footprint kinds — the defeat check's
        // structure-bearing guard (A-067) keeps the match ongoing despite
        // player 1's resignation. The outcome stays None here; the M8
        // acceptance test (tests/match_rules.rs) drives a structure-bearing
        // world to assert MatchEnded actually fires.
        assert!(!host.is_finished());
    }

    #[test]
    fn fresh_construction_with_the_same_seed_produces_identical_hashes() {
        // A15 (restart cleanliness): two fresh MatchHost instances from the
        // same setup produce identical state hashes at every checkpoint.
        let world = world();
        let setup = setup();
        let mut a = MatchHost::new(&world, setup.clone());
        let mut b = MatchHost::new(&world, setup);
        for _ in 0..62 {
            let frame = Duration::from_millis(16);
            let oa = a.advance(frame);
            let ob = b.advance(frame);
            // Same tick, same hash after every step.
            assert_eq!(a.tick(), b.tick());
            assert_eq!(a.state_hash(), b.state_hash());
            // Same checkpoint trail — every checkpoint tick and its hash
            // match between the two hosts.
            assert_eq!(oa.hashes, ob.hashes, "the checkpoint trails must match");
        }
        assert_eq!(a.state_hash(), b.state_hash());
    }
}
