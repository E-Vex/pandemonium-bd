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

use std::time::Duration;

use pandemonium_sim::{Sim, TrivialWorld};
use pandemonium_sim_api::{
    Command, Event, MatchSetup, PlayerId, PlayerView, Snapshot, Tick, Vec2Fx,
};

use crate::clock::FixedTimestep;
use crate::interpolate::{Interpolator, RenderSnapshot};

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
/// interpolator, and the pending command batch. Drive it from the client's
/// frame loop: submit commands as input produces them, then `advance` with the
/// frame's real delta, then render via [`MatchHost::render_snapshot`].
#[derive(Clone, Debug)]
pub struct MatchHost {
    sim: Sim,
    clock: FixedTimestep,
    interpolator: Interpolator,
    pending: Vec<Command>,
}

impl MatchHost {
    /// Starts a match: constructs the simulation (whose `Sim::new` spawns the
    /// initial entities and buffers the initial `Spawned` events), pushes the
    /// tick-0 snapshot, and arms the clock at 30 Hz.
    pub fn new(world: &TrivialWorld, setup: MatchSetup) -> Self {
        let sim = Sim::new(world, setup);
        let mut interpolator = Interpolator::new();
        interpolator.push(sim.snapshot());
        Self {
            sim,
            clock: FixedTimestep::new(pandemonium_sim::TICKS_PER_SECOND),
            interpolator,
            pending: Vec::new(),
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
    pub fn advance(&mut self, real_dt: Duration) -> FrameOutcome {
        let mut outcome = FrameOutcome::default();
        let steps = self.clock.update(real_dt);
        for _ in 0..steps {
            let feed = std::mem::take(&mut self.pending);
            let step = self.sim.step(&feed);
            outcome.steps += 1;
            outcome.events.extend(step.events);
            if let Some(hash) = step.hash {
                outcome.hashes.push((self.sim.tick(), hash));
            }
            self.interpolator.push(self.sim.snapshot());
        }
        outcome
    }

    /// The interpolated presentation snapshot for this display frame
    /// (`alpha` comes from the host's clock).
    pub fn render_snapshot(&self) -> RenderSnapshot {
        self.interpolator.render(self.clock.alpha())
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
    use pandemonium_sim::{CapTemplate, KindTemplate, ResourceDef as SimResourceDef, SpawnDef};
    use pandemonium_sim_api::{
        CommandKind, ControllerKind, EntityId, KindId, PlayerSetup, ResourceId,
    };

    fn world() -> TrivialWorld {
        TrivialWorld {
            map_id: 0x0054_4553_5400_0001,
            width_tiles: 16,
            height_tiles: 16,
            kinds: vec![KindTemplate {
                caps: vec![
                    CapTemplate::Health {
                        max_hp: 40,
                        regen_per_tick: 0,
                    },
                    CapTemplate::Move {
                        speed_milli_tiles_per_s: 2600,
                    },
                    CapTemplate::Vision {
                        radius_milli_tiles: 7000,
                    },
                ],
            }],
            resources: vec![SimResourceDef {
                resource: ResourceId(0),
                starting: 200,
            }],
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
}
