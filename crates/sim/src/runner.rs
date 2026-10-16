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

use pandemonium_sim_api::{Command, MatchSetup, Tick};

use crate::fixture::TrivialWorld;
use crate::sim::Sim;

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
    let mut sim = Sim::new(world, setup.clone());
    let mut checkpoints = vec![(0, sim.state_hash())];

    let mut sorted: Vec<&Command> = commands.iter().collect();
    sorted.sort_by_key(|cmd| cmd.tick); // stable: preserves feed order within a tick
    let mut cursor = 0usize;

    while sim.tick() < ticks {
        let tick = sim.tick();
        let mut feed: Vec<Command> = Vec::new();
        while cursor < sorted.len() && sorted[cursor].tick == tick {
            feed.push(sorted[cursor].clone());
            cursor += 1;
        }
        let out = sim.step(&feed);
        if let Some(hash) = out.hash {
            checkpoints.push((sim.tick(), hash));
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{CapTemplate, KindEconomy, KindTemplate, ResourceDef, SpawnDef};
    use pandemonium_fx::Vec2Fx;
    use pandemonium_sim_api::{
        CommandKind, ControllerKind, EntityId, PlayerId, PlayerSetup, ResourceId,
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
}
