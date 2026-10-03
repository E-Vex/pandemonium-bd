//! Replay verification (plan §12 `tools replay-verify`): re-simulate a recorded
//! match headlessly and compare every checkpoint hash (acceptance A2).
//!
//! The re-simulation itself is the canonical driver
//! [`pandemonium_sim::run_command_log`] — the single shared home the `replay`
//! crate cannot offer (it must stay above `sim`, never below it; plan §4), so
//! tools and the acceptance tests share one feed loop (docs/DEBT.md DEBT-005,
//! repaid with M7). This module owns what verification adds on top: the
//! structural validation of the record, the content-identity check, and the
//! checkpoint-by-checkpoint comparison.

use pandemonium_replay::{Checkpoint, ReplayError, ReplayFile};
use pandemonium_sim::run_command_log;
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

    // Re-simulate to the final checkpoint's tick through the shared driver.
    let end_tick = replay.checkpoints[replay.checkpoints.len() - 1].tick;
    let setup = MatchSetup {
        seed: replay.seed,
        players: replay.player_setup.clone(),
    };
    let run = run_command_log(world, &setup, &replay.commands, end_tick);
    let checkpoints: Vec<Checkpoint> = run
        .checkpoints
        .iter()
        .map(|&(tick, hash)| Checkpoint { tick, hash })
        .collect();

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
    if replay.final_hash != run.final_hash {
        return Ok(Verification::Diverged {
            detail: format!(
                "final hash mismatch: replay has {:#018x}, re-simulation has {final_hash:#018x}",
                replay.final_hash,
                final_hash = run.final_hash
            ),
        });
    }
    Ok(Verification::Passed {
        checkpoints,
        final_hash: run.final_hash,
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
