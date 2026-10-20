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
//! mission id against the loaded content's missions. A mission-bearing record
//! also re-simulates *mission-carrying* (AUD-002): the resolved objective set
//! rides the same `Sim::with_mission` seam the recording ran, so the
//! verification matches the recording's semantics — today identical hashes
//! (objectives are derived, unhashed state), and structurally sound the day
//! Phase 2 restrictions make the mission observable in state.

use std::path::Path;

use anyhow::{Context, Result};
use pandemonium_content::ContentTree;
use pandemonium_replay::{Checkpoint, ReplayError, ReplayFile};
use pandemonium_sim::run_command_log;
use pandemonium_sim::run_command_log_with_mission;
use pandemonium_sim::TrivialWorld;
use pandemonium_sim_api::{MatchSetup, MissionData, PlayerId};

/// One loaded mission, resolved for verification (AUD-002, ADR-0201 §7):
/// the domain-separated id — the third comparison's key — beside the
/// mission's objective set, resolved for the mission player exactly the
/// way the client's `begin_mission` resolves it for the recording. A
/// mission-bearing replay verifies against the id (membership) and
/// re-simulates through the objectives (semantics).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LoadedMission {
    /// The mission's domain-separated identity.
    pub id: u64,
    /// The mission's resolved objective set (the mission player's side).
    pub objectives: MissionData,
}

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
/// `missions` is the loaded content's mission set (ADR-0201 §7's third
/// comparison): when the replay carries a mission id, it must name one of
/// them — a replay from a different mission (an edited objective set is a
/// different mission) fails the same way a replay from different content
/// does, before a single tick is spent. A mission-less record
/// (`mission_id: None`, the default match) skips the check entirely. A
/// mission-bearing record that passes the check re-simulates through the
/// named mission's resolved objective set (`run_command_log_with_mission` —
/// the same `Sim::with_mission` seam the recording ran), so the verification
/// covers mission matches structurally, not just coincidentally.
pub fn verify_replay(
    world: &TrivialWorld,
    replay: &ReplayFile,
    missions: &[LoadedMission],
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
        if !missions.iter().any(|mission| mission.id == mission_id) {
            return Err(ReplayError::Invalid(format!(
                "replay mission id {mission_id:#018x} matches no mission in the loaded content \
                 ({} mission(s) loaded)",
                missions.len()
            )));
        }
    }

    // Re-simulate to the final checkpoint's tick through the shared driver —
    // mission-carrying when the record names a mission (the objectives ride
    // the same seam the recording ran), mission-less otherwise.
    let end_tick = replay.checkpoints[replay.checkpoints.len() - 1].tick;
    let setup = MatchSetup {
        seed: replay.seed,
        players: replay.player_setup.clone(),
    };
    let mission = replay
        .mission_id
        .and_then(|mission_id| missions.iter().find(|mission| mission.id == mission_id));
    let run = match mission {
        Some(mission) => run_command_log_with_mission(
            world,
            &setup,
            &replay.commands,
            end_tick,
            Some(mission.objectives.clone()),
        ),
        None => run_command_log(world, &setup, &replay.commands, end_tick),
    };
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

/// Resolves what a replay verifies against (AUD-002): the world and the
/// loaded content's mission set.
///
/// A mission-less record keeps the identity-based
/// [`resolve_replay_world`](crate::ai_match::resolve_replay_world)
/// resolution and an empty mission set — the historical path, unchanged.
///
/// A mission-bearing record loads the content tree and resolves **every**
/// loaded mission — its domain-separated id beside its objective set,
/// resolved for the mission player (the first start of the mission's map,
/// the Phase 1 law the client's `begin_mission` applies) — and bundles the
/// named mission's **own map** for the world, the same
/// `tree.bundle(&def.map)` construction the client recorded under. A record
/// whose mission names no loaded mission keeps the mission-less world
/// resolution, so the membership check inside [`verify_replay`] is what
/// refuses it — before a single tick is spent.
pub fn resolve_replay_verification(
    content_path: &Path,
    replay: &ReplayFile,
) -> Result<(TrivialWorld, Vec<LoadedMission>)> {
    if replay.mission_id.is_none() {
        let world = crate::ai_match::resolve_replay_world(content_path, replay)?;
        return Ok((world, Vec::new()));
    }
    let tree = ContentTree::load_dir(content_path)
        .with_context(|| format!("loading content at {}", content_path.display()))?;
    let mut missions = Vec::with_capacity(tree.missions.len());
    for def in &tree.missions {
        let objectives = mission_data_for_player(&tree, &def.id, &def.map)?;
        missions.push(LoadedMission {
            id: tree.mission_id(&def.id),
            objectives,
        });
    }
    // The world through the named mission's own map bundle — the client's
    // begin_mission law. B-019: the bundle resolves through the mission-aware
    // path (`bundle_for_mission`), so a mission carrying faction_overrides
    // re-simulates on the EFFECTIVE world the recording ran (the override
    // roster, filtered production, and replacement forces baked into the
    // world) — the fixture hash the record stamps is the effective world's.
    // A mission without overrides resolves the base faction and the plain
    // bundle world, exactly as before. An unnamed id (content whose mission
    // set excludes it) falls back to the mission-less resolution so the
    // third comparison refuses the record before any tick is spent.
    let named = tree.missions.iter().find(|def| {
        replay
            .mission_id
            .is_some_and(|mission_id| tree.mission_id(&def.id) == mission_id)
    });
    let world = match named {
        Some(def) => tree
            .bundle_for_mission(&def.id)
            .with_context(|| format!("bundling mission '{}' map '{}'", def.id, def.map))?
            .world(),
        None => crate::ai_match::resolve_replay_world(content_path, replay)?,
    };
    Ok((world, missions))
}

/// One mission's objective set resolved for the mission player: the first
/// start of the mission's map (`ContentTree::mission_data`' Phase 1
/// law — the client's `begin_mission` passes the same player).
fn mission_data_for_player(
    tree: &ContentTree,
    mission_id_str: &str,
    map_id_str: &str,
) -> Result<MissionData> {
    let map = tree
        .map(map_id_str)
        .with_context(|| format!("mission '{mission_id_str}' references map '{map_id_str}'"))?;
    let player = map
        .starts
        .first()
        .map(|start| PlayerId(start.player))
        .with_context(|| format!("map '{map_id_str}' declares no starts"))?;
    tree.mission_data(mission_id_str, player)
        .with_context(|| format!("resolving mission '{mission_id_str}' objectives"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::{demo_script, demo_setup, demo_world, record_replay, run_scripted};
    use pandemonium_replay::FORMAT_VERSION;
    use pandemonium_sim_api::{ControllerKind, PlayerSetup};

    /// The repository's content directory (the tools crate's depth).
    fn repo_content() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("the tools crate sits two levels under the workspace root")
            .to_path_buf()
            .join("content")
    }

    /// A loaded mission with empty objectives — the synthetic-id tests'
    /// stand-in (the demo world's re-simulation never evaluates them; an
    /// empty objective set is a no-op for stage 10).
    fn loaded(id: u64) -> LoadedMission {
        LoadedMission {
            id,
            objectives: MissionData {
                player: PlayerId(0),
                objectives: Vec::new(),
                fog_preset: pandemonium_sim_api::FogPreset::Default,
                ai_settings: pandemonium_sim_api::AiSettings::default(),
            },
        }
    }

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
        let err = verify_replay(&demo_world(), &mission_replay, &[loaded(0x9999)]).unwrap_err();
        assert!(err.to_string().contains("matches no mission"), "got: {err}");
        // Empty set (a mission-less content tree): equally refused.
        let err = verify_replay(&demo_world(), &mission_replay, &[]).unwrap_err();
        assert!(err.to_string().contains("matches no mission"), "got: {err}");
        // Right set: verifies cleanly (the re-simulation runs).
        let outcome = verify_replay(
            &demo_world(),
            &mission_replay,
            &[loaded(0x004d_4953_0bad_c0de)],
        )
        .expect("verify must run");
        assert!(matches!(outcome, Verification::Passed { .. }));
        // Mission-less record: the check is skipped, any set passes.
        let outcome = verify_replay(&demo_world(), &replay, &[loaded(0x004d_4953_0bad_c0de)])
            .expect("verify");
        assert!(matches!(outcome, Verification::Passed { .. }));
    }

    #[test]
    fn a_recorded_mission_match_verifies_end_to_end() {
        // AUD-002's acceptance shape on the real content: a recording that
        // carries crossroads_01's mission id — exactly what the client's
        // --record path stamps since this change — verifies against the
        // loaded content with the third comparison firing and a
        // mission-carrying re-simulation; the same record is refused
        // BEFORE any tick is spent against content whose mission set
        // excludes the id; and stripping the id makes it verify as a plain
        // match (the Phase 1 masking, pinned explicitly: objectives are
        // derived, unhashed state, so the mission-carrying and
        // mission-less re-simulations agree until Phase 2 changes that).
        let tree = ContentTree::load_dir(&repo_content()).expect("the repo content loads");
        let mission_id = tree.mission_id("crossroads_01");
        let bundle = tree
            .bundle("crossroads_64")
            .expect("the mission's map bundles");
        let world = bundle.world();
        let objectives = tree
            .mission_data("crossroads_01", PlayerId(0))
            .expect("crossroads_01 resolves for the first map start's player");
        let setup = MatchSetup {
            seed: 9,
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
        // The recording's checkpoint trail comes from the mission-carrying
        // driver — the same seam the client's mission host runs; the record
        // mirrors the client recorder's bytes (no commands: a fresh match
        // dumped before the player acted).
        let run = run_command_log_with_mission(&world, &setup, &[], 120, Some(objectives.clone()));
        let checkpoints: Vec<Checkpoint> = run
            .checkpoints
            .iter()
            .map(|&(tick, hash)| Checkpoint { tick, hash })
            .collect();
        let replay = ReplayFile {
            format_version: FORMAT_VERSION,
            content_hash: world.content_hash(),
            map_id: world.map_id,
            mission_id: Some(mission_id),
            seed: setup.seed,
            player_setup: setup.players.clone(),
            commands: Vec::new(),
            checkpoints,
            final_hash: run.final_hash,
        };
        // The file round-trips the id (the third comparison's key is on disk).
        let decoded = ReplayFile::decode(&replay.encode()).expect("the record decodes");
        assert_eq!(decoded.mission_id, Some(mission_id));

        let loaded_missions = vec![LoadedMission {
            id: mission_id,
            objectives,
        }];
        // (a) Against the loaded content: PASS — the third comparison fires
        // and the re-simulation runs mission-carrying.
        let outcome = verify_replay(&world, &decoded, &loaded_missions).expect("verify must run");
        assert!(matches!(outcome, Verification::Passed { .. }));
        // (b) Against content whose mission set excludes the id: refused at
        // the identity stage — before a single tick is spent, not a
        // divergence after re-simulation.
        let err = verify_replay(&world, &decoded, &[]).unwrap_err();
        assert!(err.to_string().contains("matches no mission"), "got: {err}");
        // (c) Stripping the id: verifies as a plain match — today's masking,
        // documented (derived, unhashed objective state).
        let mut stripped = decoded.clone();
        stripped.mission_id = None;
        let outcome = verify_replay(&world, &stripped, &[]).expect("verify must run");
        assert!(matches!(outcome, Verification::Passed { .. }));
    }

    #[test]
    fn mission_aware_resolution_bundles_the_named_missions_map() {
        // The resolver: a mission-bearing record resolves the world through
        // the named mission's OWN map bundle (the client's begin_mission
        // law) beside every loaded mission resolved for its mission player
        // (the first start of the mission's map); a mission-less record
        // keeps the identity-based resolution and an empty set.
        let content = repo_content();
        let tree = ContentTree::load_dir(&content).expect("the repo content loads");
        let mission_id = tree.mission_id("crossroads_01");
        let bundle = tree
            .bundle("crossroads_64")
            .expect("the mission's map bundles");
        let world = bundle.world();
        let setup = MatchSetup {
            seed: 9,
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
        let run = run_command_log(&world, &setup, &[], 30);
        let checkpoints: Vec<Checkpoint> = run
            .checkpoints
            .iter()
            .map(|&(tick, hash)| Checkpoint { tick, hash })
            .collect();
        let record_for = |mission_id: Option<u64>| ReplayFile {
            format_version: FORMAT_VERSION,
            content_hash: world.content_hash(),
            map_id: world.map_id,
            mission_id,
            seed: setup.seed,
            player_setup: setup.players.clone(),
            commands: Vec::new(),
            checkpoints: checkpoints.clone(),
            final_hash: run.final_hash,
        };

        // Mission-less: the identity-based resolution, no missions.
        let (plain_world, plain_missions) =
            resolve_replay_verification(&content, &record_for(None)).expect("resolves");
        assert!(plain_missions.is_empty());
        assert_eq!(plain_world.content_hash(), world.content_hash());

        // Mission-bearing: the named mission's map bundle + the loaded set.
        let (mission_world, missions) =
            resolve_replay_verification(&content, &record_for(Some(mission_id))).expect("resolves");
        assert_eq!(missions.len(), 1, "the tree loads exactly one mission");
        assert_eq!(missions[0].id, mission_id);
        // The mission player is the mission's map's first start (player 0
        // on crossroads_64 — the client's begin_mission law).
        assert_eq!(missions[0].objectives.player, PlayerId(0));
        assert_eq!(missions[0].objectives.objectives.len(), 2);
        // The world is the mission's own map's bundle (identical bytes on
        // this one-map tree; the law is what a multi-map tree exercises).
        assert_eq!(mission_world.content_hash(), world.content_hash());
        assert_eq!(mission_world.map_id, world.map_id);

        // An id the loaded set excludes: the mission-less world resolution
        // (verify_replay's third comparison is what refuses the record).
        let (foreign_world, foreign_missions) =
            resolve_replay_verification(&content, &record_for(Some(0x004d_4953_0bad_c0de)))
                .expect("resolves");
        assert!(!foreign_missions.is_empty());
        assert_eq!(foreign_world.content_hash(), world.content_hash());
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
