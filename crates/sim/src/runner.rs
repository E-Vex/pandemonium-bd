//! The headless command-log driver: the one canonical way to re-simulate a
//! recorded command stream (plan §6.5, A2).
//!
//! A match is seed + content + ordered command log (FD-1); this function is
//! the "seed + log" half made concrete — it feeds every command at its
//! *declared* tick (a stable sort by tick preserves the recorded feed order
//! within each tick, which is exactly what duplicate-sequence resolution
//! needs) and collects the checkpoint trail a replay verifier compares
//! against. It lives in `sim` because the `replay` crate must stay above
//! `sim` never below it (plan §4) — tools, the acceptance tests, and any
//! future soak runner all share this one driver (docs/DEBT.md DEBT-005,
//! repaid with M7).
//!
//! The driver is pure: no I/O, no clocks, no randomness of its own — the
//! only mutator it touches the world through is [`Sim::step`] (FD-2).

use pandemonium_sim_api::{Command, MatchSetup, MissionData, Tick};

use crate::fixture::TrivialWorld;
use crate::sim::Sim;

/// The shared feed loop of the drivers below (one canonical way to feed a
/// recorded command stream, `run_command_log`'s doc): advances `sim` to
/// `ticks` feeding each command at its declared tick (a stable sort
/// preserves the recorded feed order within each tick), and returns the
/// periodic checkpoint hashes the steps produced, ascending. Pure over the
/// simulation — the only mutator it touches the world through is
/// [`Sim::step`] (FD-2).
fn advance_by_log(sim: &mut Sim, commands: &[Command], ticks: u32) -> Vec<(Tick, u64)> {
    let mut sorted: Vec<&Command> = commands.iter().collect();
    sorted.sort_by_key(|cmd| cmd.tick); // stable: preserves feed order within a tick
    let mut cursor = 0usize;
    let mut periodic = Vec::new();
    while sim.tick() < ticks {
        let tick = sim.tick();
        let mut feed: Vec<Command> = Vec::new();
        while cursor < sorted.len() && sorted[cursor].tick == tick {
            feed.push(sorted[cursor].clone());
            cursor += 1;
        }
        let out = sim.step(&feed);
        if let Some(hash) = out.hash {
            periodic.push((sim.tick(), hash));
        }
    }
    periodic
}

/// What one full run of a command log observed: the checkpoint trail and the
/// allocator watermark, everything a replay verifier or a soak runner needs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CommandLogRun {
    /// The tick-0 checkpoint plus every periodic one (every
    /// [`crate::CHECKPOINT_INTERVAL`] ticks), ascending, with a forced final
    /// checkpoint exactly at the last simulated tick.
    pub checkpoints: Vec<(Tick, u64)>,
    /// The state hash at the final tick.
    pub final_hash: u64,
    /// The id allocator watermark at the end (the never-reuse ceiling).
    pub next_entity_id: u64,
}

/// Re-simulates `commands` over `world` for `ticks` ticks (plan §6.5: the
/// re-simulation behind `tools replay-verify` and the acceptance suites).
///
/// Feed semantics: commands are grouped by their declared tick (a *stable*
/// sort — the within-tick feed order is significant for duplicate-sequence
/// resolution and must survive verbatim), and each group is fed to the tick's
/// [`Sim::step`]. Commands declaring ticks at or past `ticks` never run.
pub fn run_command_log(
    world: &TrivialWorld,
    setup: &MatchSetup,
    commands: &[Command],
    ticks: u32,
) -> CommandLogRun {
    run_command_log_with_mission(world, setup, commands, ticks, None)
}

/// The mission-carrying form of [`run_command_log`] (ADR-0201 §7): a mission
/// match's re-simulation builds the simulation through [`Sim::with_mission`]
/// — the same construction seam the engine's `MatchHost::with_mission` and
/// the client's `begin_mission` use — so the record's objectives are
/// evaluated during the re-simulation exactly as they were during the match
/// (`tools replay-verify` passes the resolved set for a mission-bearing
/// record; every existing caller keeps the mission-less `None` path through
/// [`run_command_log`]). The objective state is derived and never hashed (the
/// A-066 discipline, pinned by `tests/mission_phase1.rs::
/// mission_less_path_is_byte_equivalent`), so both forms produce identical
/// checkpoints for the same command log — the mission rides along for
/// semantics (Phase 2 restrictions will gate real state), not for hashes.
pub fn run_command_log_with_mission(
    world: &TrivialWorld,
    setup: &MatchSetup,
    commands: &[Command],
    ticks: u32,
    mission: Option<MissionData>,
) -> CommandLogRun {
    let mut sim = match mission {
        Some(mission) => Sim::with_mission(world, setup.clone(), mission),
        None => Sim::new(world, setup.clone()),
    };
    let mut checkpoints = vec![(0, sim.state_hash())];
    checkpoints.extend(advance_by_log(&mut sim, commands, ticks));

    // Force a final checkpoint exactly at the last simulated tick so a
    // replay's end is unambiguous (its own validation requires final ==
    // last checkpoint).
    let final_hash = sim.state_hash();
    if checkpoints.last().map(|cp| cp.0) != Some(sim.tick()) {
        checkpoints.push((sim.tick(), final_hash));
    }
    CommandLogRun {
        checkpoints,
        final_hash,
        next_entity_id: sim.next_entity_id(),
    }
}

/// ADR-0201 §6/§9 Phase 4 (B-027, the mid-mission save): the rehost
/// driver — [`run_command_log_with_mission`]'s exact construction and feed
/// loop, exposed as the live [`Sim`] advanced to `ticks` instead of a
/// checkpoint trail, for the engine's resume seam (the engine's
/// `MatchHost::resume_with_mission` builds its host around what this
/// returns). The returned simulation is the one the original match held at
/// `ticks` — state hash, tick, and id watermark identical, by A2's
/// re-simulation property over the recorded prefix (the same law
/// `tools replay-verify` verifies); the periodic hashes the loop observes
/// are the record's own trail, already persisted in the save file, and are
/// discarded here. What the continuation does from that point is the ADR's
/// documented divergence (the controllers restart with fresh bookkeeping,
/// §6 caveat i): no test may assert it equals the never-interrupted
/// timeline (§6 caveat iii).
pub fn rehost_sim(
    world: &TrivialWorld,
    setup: &MatchSetup,
    commands: &[Command],
    ticks: u32,
    mission: Option<MissionData>,
) -> Sim {
    let mut sim = match mission {
        Some(mission) => Sim::with_mission(world, setup.clone(), mission),
        None => Sim::new(world, setup.clone()),
    };
    advance_by_log(&mut sim, commands, ticks);
    sim
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{CapTemplate, KindEconomy, KindTemplate, ResourceDef, SpawnDef};
    use pandemonium_fx::Vec2Fx;
    use pandemonium_sim_api::{
        AiSettings, CommandKind, ControllerKind, EntityId, FogPreset, MissionData,
        MissionObjective, MissionObjectiveKind, MissionObjectiveParameters, PlayerId, PlayerSetup,
        ResourceId,
    };

    fn world() -> TrivialWorld {
        TrivialWorld {
            map_id: 0x0052_554E_4E45_5230,
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
            resources: vec![ResourceDef {
                resource: ResourceId(0),
                starting: 200,
            }],
            production: vec![],
            base_population_cap: 0,
            initial_spawns: vec![SpawnDef {
                owner: PlayerId(0),
                kind: pandemonium_sim_api::KindId(0),
                pos: Vec2Fx::from_ints(8, 8),
            }],
            scheduled_spawns: vec![],
            spawn_jitter_milli: 0,
            roster: Vec::new(),
        }
        .with_default_roster()
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

    #[test]
    fn the_trail_carries_tick_zero_periodics_and_a_forced_final() {
        let commands = vec![Command::new(
            PlayerId(0),
            0,
            1,
            CommandKind::Move {
                units: vec![EntityId(1)],
                target: Vec2Fx::from_ints(4, 4),
            },
        )];
        // 65 ticks: periodic hashes at 30 and 60 plus a forced final at 65.
        let run = run_command_log(&world(), &setup(), &commands, 65);
        assert_eq!(
            run.checkpoints.len(),
            4,
            "tick 0 + 30 + 60 + forced 65: {:?}",
            run.checkpoints
        );
        assert_eq!(run.checkpoints[0].0, 0);
        assert_eq!(run.checkpoints[1].0, 30);
        assert_eq!(run.checkpoints[2].0, 60);
        assert_eq!(run.checkpoints[3].0, 65);
        assert_eq!(run.checkpoints[3].1, run.final_hash);
        assert_eq!(run.next_entity_id, 2, "one spawn: the watermark is next");
    }

    #[test]
    fn the_same_log_re_simulates_identically() {
        let commands = vec![
            Command::new(
                PlayerId(0),
                0,
                1,
                CommandKind::Move {
                    units: vec![EntityId(1)],
                    target: Vec2Fx::from_ints(4, 4),
                },
            ),
            Command::new(
                PlayerId(0),
                2,
                2,
                CommandKind::Move {
                    units: vec![EntityId(1)],
                    target: Vec2Fx::from_ints(12, 8),
                },
            ),
        ];
        let a = run_command_log(&world(), &setup(), &commands, 90);
        let b = run_command_log(&world(), &setup(), &commands, 90);
        assert_eq!(a, b);
    }

    #[test]
    fn commands_declaring_late_ticks_never_run() {
        let late = vec![Command::new(
            PlayerId(0),
            500,
            1,
            CommandKind::Move {
                units: vec![EntityId(1)],
                target: Vec2Fx::from_ints(4, 4),
            },
        )];
        let quiet = run_command_log(&world(), &setup(), &[], 30);
        let fed = run_command_log(&world(), &setup(), &late, 30);
        assert_eq!(fed, quiet, "a command past the run length changes nothing");
    }

    /// B-027: a MissionData with one optional survive objective that
    /// resolves at tick 1 — the fixture for the objective-status query's
    /// rehost pins (a resolution the rehost must carry, since the event
    /// itself never re-fires).
    fn mission_ticking() -> MissionData {
        MissionData {
            player: PlayerId(0),
            objectives: vec![MissionObjective {
                kind: MissionObjectiveKind::Survive,
                parameters: MissionObjectiveParameters::Survive { ticks: 1 },
                optional: true,
            }],
            fog_preset: FogPreset::Default,
            ai_settings: AiSettings::default(),
        }
    }

    #[test]
    fn rehost_matches_the_canonical_driver() {
        // ADR-0201 §6/§9 Phase 4: the rehost at N ticks is the simulation
        // the canonical driver's trail describes at N — same state hash,
        // same tick, same id watermark (A2's property over the recorded
        // prefix, not a claim about any continuation from there).
        let commands = vec![Command::new(
            PlayerId(0),
            0,
            1,
            CommandKind::Move {
                units: vec![EntityId(1)],
                target: Vec2Fx::from_ints(4, 4),
            },
        )];
        let run = run_command_log(&world(), &setup(), &commands, 65);
        let sim = rehost_sim(&world(), &setup(), &commands, 65, None);
        assert_eq!(sim.tick(), 65);
        assert_eq!(sim.state_hash(), run.final_hash);
        assert_eq!(sim.next_entity_id(), run.next_entity_id);
    }

    #[test]
    fn rehost_at_zero_ticks_is_the_fresh_construction() {
        // ticks = 0 runs no steps: the rehost is exactly Sim::new's state.
        let fresh = Sim::new(&world(), setup());
        let rehosted = rehost_sim(&world(), &setup(), &[], 0, None);
        assert_eq!(rehosted.tick(), fresh.tick());
        assert_eq!(rehosted.state_hash(), fresh.state_hash());
    }

    #[test]
    fn rehost_carries_a_mission_without_changing_state() {
        // The A-066 byte-equivalence law at the rehost seam: a mission rides
        // the re-simulation for semantics (the objective statuses evaluate)
        // and never moves the state hash.
        let commands = vec![Command::new(
            PlayerId(0),
            0,
            1,
            CommandKind::Move {
                units: vec![EntityId(1)],
                target: Vec2Fx::from_ints(4, 4),
            },
        )];
        let plain = rehost_sim(&world(), &setup(), &commands, 30, None);
        let missioned = rehost_sim(&world(), &setup(), &commands, 30, Some(mission_ticking()));
        assert_eq!(plain.state_hash(), missioned.state_hash());
        assert_eq!(plain.tick(), missioned.tick());
    }

    #[test]
    fn rehost_carries_the_resolved_objective_statuses() {
        // The survive-1 objective resolves at tick 1, before the rehost's
        // horizon: the rehosted simulation carries the resolution in its
        // queryable statuses (the save's presentation half seeds from this
        // — the ObjectiveState event itself fired during the recorded
        // prefix and never re-fires).
        let sim = rehost_sim(&world(), &setup(), &[], 30, Some(mission_ticking()));
        assert_eq!(sim.tick(), 30);
        assert_eq!(
            sim.objective_states(),
            vec![Some(pandemonium_sim_api::ObjectiveState::Completed)],
            "the optional survive objective resolved during the rehost window"
        );
    }
}
