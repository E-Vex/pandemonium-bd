//! Replay verification (plan §12 `tools replay-verify`): re-simulate a recorded
//! match headlessly and compare every checkpoint hash (acceptance A2).
//!
//! The re-simulation itself is the canonical driver
//! [`pandemonium_sim::run_command_log`] — the single shared home the `replay`
//! crate cannot offer (it must stay above `sim`, never below it; plan §4), so
//! tools and the acceptance tests share one feed loop (docs/DEBT.md DEBT-005,
//! repaid with M7). This module owns what verification adds on top: the
//! structural validation of the record, the identity checks, and the
//! checkpoint-by-checkpoint comparison. Since ADR-0201 §7 the identity checks
//! are three: content hash, map id, and — when the replay carries one — the
//! mission id against the loaded content's missions.

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
///
/// `mission_ids` is the loaded content's set of domain-separated mission ids
/// (ADR-0201 §7's third comparison): when the replay carries a mission id, it
/// must name one of them — a replay from a different mission (an edited
/// objective set is a different mission) fails the same way a replay from
/// different content does, before a single tick is spent. A mission-less
/// record (`mission_id: None`, the default match) skips the check entirely.
pub fn verify_replay(
    world: &TrivialWorld,
    replay: &ReplayFile,
    mission_ids: &[u64],
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
    if let Some(mission_id) = replay.mission_id {
        if !mission_ids.contains(&mission_id) {
            return Err(ReplayError::Invalid(format!(
                "replay mission id {mission_id:#018x} matches no mission in the loaded content \
                 ({} mission(s) loaded)",
                mission_ids.len()
            )));
        }
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
        let outcome = verify_replay(&demo_world(), &replay, &[]).expect("verify must run");
        assert!(matches!(outcome, Verification::Passed { .. }));
    }

    #[test]
    fn a_mission_replay_must_name_a_loaded_mission() {
        // ADR-0201 §7's third comparison: a mission-bearing replay against a
        // content set whose missions do not include that id fails before a
        // single tick is spent; against the right id it verifies like any
        // other record; a mission-less record skips the check whatever the
        // loaded set is.
        let replay = record_replay(11, 95);
        let mut mission_replay = replay.clone();
        mission_replay.mission_id = Some(0x004d_4953_0bad_c0de);
        // Wrong set: refused before simulation.
        let err = verify_replay(&demo_world(), &mission_replay, &[0x9999]).unwrap_err();
        assert!(err.to_string().contains("matches no mission"), "got: {err}");
        // Empty set (a mission-less content tree): equally refused.
        let err = verify_replay(&demo_world(), &mission_replay, &[]).unwrap_err();
        assert!(err.to_string().contains("matches no mission"), "got: {err}");
        // Right set: verifies cleanly (the re-simulation runs).
        let outcome = verify_replay(&demo_world(), &mission_replay, &[0x004d_4953_0bad_c0de])
            .expect("verify must run");
        assert!(matches!(outcome, Verification::Passed { .. }));
        // Mission-less record: the check is skipped, any set passes.
        let outcome =
            verify_replay(&demo_world(), &replay, &[0x004d_4953_0bad_c0de]).expect("verify");
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
        let outcome = verify_replay(&demo_world(), &replay, &[]).expect("verify must run");
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
        let err = verify_replay(&world, &replay, &[]).unwrap_err();
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
