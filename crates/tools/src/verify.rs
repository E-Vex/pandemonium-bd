//! Replay verification (plan §12 `tools replay-verify`): re-simulate a recorded
//! match headlessly and compare every checkpoint hash (acceptance A2).
//!
//! The driver lives here — not in the `replay` crate — because re-simulation
//! needs `sim`, and the dependency law keeps `replay` above `sim` never below it
//! (plan §4). The acceptance test `tests/determinism.rs` mirrors this short
//! driver for the same reason (see docs/ASSUMPTIONS.md).

use pandemonium_replay::{Checkpoint, ReplayError, ReplayFile};
use pandemonium_sim::Sim;
use pandemonium_sim::TrivialWorld;
use pandemonium_sim_api::MatchSetup;

/// The outcome of a verification run.
#[derive(Debug, PartialEq, Eq)]
pub enum Verification {
    /// Every checkpoint and the final hash match the re-simulation.
    Passed {
        /// The re-simulated checkpoints.
        checkpoints: Vec<Checkpoint>,
        /// The re-simulated final hash.
        final_hash: u64,
    },
    /// The re-simulation diverged; the first differing checkpoint is reported.
    Diverged {
        /// Where the divergence was observed.
        detail: String,
    },
}

/// Re-simulates `replay` over `world` and compares all checkpoints and the final
/// hash. Content identity is checked first so a replay from different content
/// fails before a single tick is spent.
pub fn verify_replay(
    world: &TrivialWorld,
    replay: &ReplayFile,
) -> Result<Verification, ReplayError> {
    replay.validate()?;
    if replay.content_hash != world.content_hash() {
        return Err(ReplayError::Invalid(format!(
            "replay content hash {:#018x} does not match this world's {:#018x}",
            replay.content_hash,
            world.content_hash()
        )));
    }
    if replay.map_id != world.map_id {
        return Err(ReplayError::Invalid(format!(
            "replay map id {} does not match this world's {}",
            replay.map_id, world.map_id
        )));
    }

    // Re-simulate to the final checkpoint's tick.
    let end_tick = replay.checkpoints[replay.checkpoints.len() - 1].tick;
    let setup = MatchSetup {
        seed: replay.seed,
        players: replay.player_setup.clone(),
    };
    let mut sim = Sim::new(world, setup);
    let mut checkpoints = vec![Checkpoint {
        tick: 0,
        hash: sim.state_hash(),
    }];
    let mut sorted: Vec<&pandemonium_sim_api::Command> = replay.commands.iter().collect();
    sorted.sort_by_key(|cmd| cmd.tick); // stable: preserves feed order within a tick
    let mut cursor = 0usize;
    while sim.tick() < end_tick {
        let tick = sim.tick();
        let mut feed: Vec<pandemonium_sim_api::Command> = Vec::new();
        while cursor < sorted.len() && sorted[cursor].tick == tick {
            feed.push(sorted[cursor].clone());
            cursor += 1;
        }
        let out = sim.step(&feed);
        if let Some(hash) = out.hash {
            checkpoints.push(Checkpoint {
                tick: sim.tick(),
                hash,
            });
        }
    }
    // Force the final checkpoint exactly like the recorder did.
    let final_hash = sim.state_hash();
    if checkpoints.last().map(|cp| cp.tick) != Some(sim.tick()) {
        checkpoints.push(Checkpoint {
            tick: sim.tick(),
            hash: final_hash,
        });
    }

    // Compare checkpoint by checkpoint.
    if checkpoints.len() != replay.checkpoints.len() {
        return Ok(Verification::Diverged {
            detail: format!(
                "checkpoint count differs: replay has {}, re-simulation has {}",
                replay.checkpoints.len(),
                checkpoints.len()
            ),
        });
    }
    for (replayed, resimulated) in replay.checkpoints.iter().zip(&checkpoints) {
        if replayed != resimulated {
            return Ok(Verification::Diverged {
                detail: format!(
                    "checkpoint mismatch at tick {}: replay has {:#018x}, re-simulation has {:#018x}",
                    resimulated.tick, replayed.hash, resimulated.hash
                ),
            });
        }
    }
    if replay.final_hash != final_hash {
        return Ok(Verification::Diverged {
            detail: format!(
                "final hash mismatch: replay has {:#018x}, re-simulation has {final_hash:#018x}",
                replay.final_hash
            ),
        });
    }
    Ok(Verification::Passed {
        checkpoints,
        final_hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::{demo_script, demo_setup, demo_world, record_replay, run_scripted};

    #[test]
    fn fresh_record_verifies_cleanly() {
        let replay = record_replay(11, 95);
        let outcome = verify_replay(&demo_world(), &replay).expect("verify must run");
        assert!(matches!(outcome, Verification::Passed { .. }));
    }

    #[test]
    fn tampered_final_hash_diverges() {
        let mut replay = record_replay(11, 95);
        // Break the final hash while keeping the structural invariant (final ==
        // last checkpoint) so validate() still passes and the mismatch surfaces
        // in the hash comparison itself.
        let tampered = replay.final_hash ^ 1;
        let last = replay.checkpoints.len() - 1;
        replay.checkpoints[last].hash = tampered;
        replay.final_hash = tampered;
        let outcome = verify_replay(&demo_world(), &replay).expect("verify must run");
        match outcome {
            Verification::Diverged { detail } => assert!(detail.contains("mismatch")),
            other => panic!("expected divergence, got {other:?}"),
        }
    }

    #[test]
    fn foreign_content_is_refused_before_simulation() {
        let replay = record_replay(11, 95);
        let mut world = demo_world();
        world.map_id = 0x9999;
        let err = verify_replay(&world, &replay).unwrap_err();
        assert!(err.to_string().contains("content hash"), "got: {err}");
    }

    #[test]
    fn seed_and_script_reproduce_the_same_checkpoints() {
        // The scripted driver and a from-scratch replay of the same log agree.
        let world = demo_world();
        let (checkpoints, final_hash) = run_scripted(&world, &demo_setup(3), &demo_script(), 90);
        let replay = record_replay(3, 90);
        assert_eq!(replay.checkpoints, checkpoints);
        assert_eq!(replay.final_hash, final_hash);
    }
}
