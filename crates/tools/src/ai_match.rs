//! The AI-vs-AI headless match driver (plan §12 `tools headless --p1 ai
//! --p2 ai`): load the content, drive both slots with scripted controllers
//! through the engine's [`AiMatchHost`], and print the match's evidence —
//! checkpoints, final hash, and what actually happened (deliveries, builds,
//! trained units, combat).
//!
//! The match is a normal match: the controllers produce a command log through
//! the same validation gate a player's commands pass, and the replay records
//! exactly that log (A-063: the hosting loop is the engine's, shared with the
//! tests and later the client — one implementation).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use pandemonium_content::ContentBundle;
use pandemonium_engine::{alpha_controller, AiMatchHost};
use pandemonium_replay::{Checkpoint, ReplayFile, FORMAT_VERSION};
use pandemonium_sim::TrivialWorld;
use pandemonium_sim_api::{ControllerKind, EntityId, Event, MatchSetup, PlayerId, PlayerSetup};

/// What drives one player slot in a headless match.
#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
#[value(rename_all = "lower")]
pub enum Slot {
    /// The M1 scripted demo (both slots must choose it; the trivial world).
    Demo,
    /// The scripted Alpha opponent over the loaded content.
    Ai,
    /// Nothing: the slot issues no commands (watch the other side play).
    Idle,
}

/// The observable evidence that a headless match ran — what the tools print
/// and what the M7 exit suite asserts on (mirrored here as plain data).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct MatchSummary {
    /// How many commands the controllers issued (rejections included).
    pub commands: usize,
    /// Per-player Ore delivered (`ResourceDelivered`).
    pub delivered: Vec<i64>,
    /// Per-player completed training cycles (`ProductionCompleted`).
    pub trained: Vec<u32>,
    /// Per-player completed constructions (`ConstructionCompleted`).
    pub built: Vec<u32>,
    /// Per-player landed attacks (`AttackHit`).
    pub attack_hits: Vec<u32>,
    /// Per-player lost entities (`Died`).
    pub deaths: Vec<u32>,
    /// The match winner (M8, plan §9.7). `None` while the match is ongoing
    /// (the runner hit its tick budget before anyone was eliminated);
    /// `Some(PlayerId)` once stage 10 fired `MatchEnded`. `Some(NEUTRAL)`
    /// for the mutual-destruction edge case (no survivors).
    pub winner: Option<PlayerId>,
}

/// The full outcome of one headless AI match: the replay record (log +
/// checkpoints + final hash) and the summary.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AiMatch {
    /// The replay the match produced.
    pub replay: ReplayFile,
    /// The evidence summary.
    pub summary: MatchSummary,
}

/// Runs a two-slot headless match over the loaded content. Slots follow the
/// map's start order (the first start's player is `p1`, the second's `p2`).
pub fn run_ai_match(
    bundle: &ContentBundle,
    seed: u64,
    ticks: u32,
    p1: Slot,
    p2: Slot,
) -> Result<AiMatch> {
    let world = bundle.world();
    let starts = &bundle.map.starts;
    if starts.len() != 2 {
        bail!(
            "an AI-vs-AI match needs exactly two starts; {} declares {}",
            bundle.map.id,
            starts.len()
        );
    }
    let players = [
        (PlayerId(starts[0].player), p1),
        (PlayerId(starts[1].player), p2),
    ];
    let setup = MatchSetup {
        seed,
        players: players
            .iter()
            .map(|(player, slot)| PlayerSetup {
                player: *player,
                controller: match slot {
                    Slot::Ai => ControllerKind::Ai,
                    _ => ControllerKind::Ai, // labels are metadata (A-060)
                },
            })
            .collect(),
    };

    // Controllers for the driven slots; idle slots simply issue nothing.
    let controllers: Vec<(PlayerId, Box<dyn pandemonium_ai::Controller>)> = players
        .iter()
        .filter_map(|(player, slot)| match slot {
            Slot::Ai => Some((
                *player,
                Box::new(alpha_controller(bundle, &world, *player, seed))
                    as Box<dyn pandemonium_ai::Controller>,
            )),
            Slot::Demo | Slot::Idle => None,
        })
        .collect();

    let mut host = AiMatchHost::new(&world, setup.clone(), controllers);
    let mut checkpoints = vec![Checkpoint {
        tick: 0,
        hash: host.state_hash(),
    }];

    // Per-player counters plus the id -> owner map (spawn events carry both).
    let mut summary = MatchSummary {
        commands: 0,
        delivered: vec![0; players.len()],
        trained: vec![0; players.len()],
        built: vec![0; players.len()],
        attack_hits: vec![0; players.len()],
        deaths: vec![0; players.len()],
        winner: None,
    };
    let mut owners: BTreeMap<EntityId, PlayerId> = BTreeMap::new();
    let player_index = |owners: &BTreeMap<EntityId, PlayerId>, id: EntityId| -> Option<usize> {
        owners
            .get(&id)
            .and_then(|owner| players.iter().position(|(p, _)| *p == *owner))
    };

    while host.tick() < ticks {
        let out = host.advance();
        summary.commands = host.log().len();
        for event in &out.events {
            match event {
                Event::Spawned { entity, owner, .. } => {
                    owners.insert(*entity, *owner);
                }
                // Construction sites spawn without a Spawned event; their
                // owner is the builder's (the gate enforced ownership).
                Event::ConstructionStarted { builder, site } => {
                    if let Some(owner) = owners.get(builder).copied() {
                        owners.insert(*site, owner);
                    }
                }
                Event::Died { entity } => {
                    if let Some(index) = player_index(&owners, *entity) {
                        summary.deaths[index] += 1;
                    }
                }
                Event::ResourceDelivered { worker, .. } => {
                    if let Some(index) = player_index(&owners, *worker) {
                        summary.delivered[index] += 1;
                    }
                }
                Event::ProductionCompleted { producer, .. } => {
                    if let Some(index) = player_index(&owners, *producer) {
                        summary.trained[index] += 1;
                    }
                }
                Event::ConstructionCompleted { site, .. } => {
                    if let Some(index) = player_index(&owners, *site) {
                        summary.built[index] += 1;
                    }
                }
                Event::AttackHit { attacker, .. } => {
                    if let Some(index) = player_index(&owners, *attacker) {
                        summary.attack_hits[index] += 1;
                    }
                }
                // M8: MatchEnded records the winner. The host keeps running
                // its tick budget regardless — the M7 "matches complete"
                // contract is "ran the budget under the A12 checker without
                // crashing", not "the match resolved".
                Event::MatchEnded { winner } => {
                    summary.winner = Some(*winner);
                }
                _ => {}
            }
        }
        if let Some(hash) = out.hash {
            checkpoints.push(Checkpoint {
                tick: host.tick(),
                hash,
            });
        }
    }
    let final_hash = host.state_hash();
    if checkpoints.last().map(|cp| cp.tick) != Some(host.tick()) {
        checkpoints.push(Checkpoint {
            tick: host.tick(),
            hash: final_hash,
        });
    }

    Ok(AiMatch {
        replay: ReplayFile {
            format_version: FORMAT_VERSION,
            content_hash: world.content_hash(),
            map_id: world.map_id,
            seed,
            player_setup: setup.players,
            commands: host.log().to_vec(),
            checkpoints,
            final_hash,
        },
        summary,
    })
}

/// Resolves the world a replay was recorded over: the demo world or the
/// content directory, matched by content identity (the replay verifier's
/// first line — a replay from different content must fail before a single
/// tick is spent).
pub fn resolve_replay_world(content_path: &Path, replay: &ReplayFile) -> Result<TrivialWorld> {
    let demo = crate::demo::demo_world();
    if replay.content_hash == demo.content_hash() && replay.map_id == demo.map_id {
        return Ok(demo);
    }
    if content_path.is_dir() {
        let bundle = ContentBundle::load_dir(content_path)
            .with_context(|| format!("loading content at {}", content_path.display()))?;
        let world = bundle.world();
        if replay.content_hash == world.content_hash() && replay.map_id == world.map_id {
            return Ok(world);
        }
        bail!(
            "replay content hash {:#018x} matches neither the demo world ({:#018x}) nor the \
             content at {} ({:#018x})",
            replay.content_hash,
            demo.content_hash(),
            content_path.display(),
            world.content_hash()
        );
    }
    bail!(
        "replay content hash {:#018x} does not match the demo world ({:#018x}) and no content \
         directory exists at {}",
        replay.content_hash,
        demo.content_hash(),
        content_path.display()
    );
}

/// Loads and validates the content directory a headless match runs on.
pub fn load_content(path: &Path) -> Result<ContentBundle> {
    ContentBundle::load_dir(path).with_context(|| format!("loading content at {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn repo_content() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf()
            .join("content")
    }

    #[test]
    fn an_ai_vs_ai_match_runs_and_reports_evidence() {
        let bundle = ContentBundle::load_dir(&repo_content()).expect("repo content loads");
        let match_result =
            run_ai_match(&bundle, 7, 1800, Slot::Ai, Slot::Ai).expect("the match runs");
        // Sixty seconds of game time: the opening is done — workers gather,
        // supply and army infrastructure rises.
        assert!(match_result.summary.commands > 0);
        assert!(
            match_result.summary.delivered.iter().all(|ore| *ore > 0),
            "both economies delivered: {:?}",
            match_result.summary
        );
        // The replay is structurally valid and re-simulates to itself.
        assert!(match_result.replay.validate().is_ok());
        let world = bundle.world();
        let setup = MatchSetup {
            seed: match_result.replay.seed,
            players: match_result.replay.player_setup.clone(),
        };
        let resim =
            pandemonium_sim::run_command_log(&world, &setup, &match_result.replay.commands, 1800);
        let checkpoints: Vec<Checkpoint> = resim
            .checkpoints
            .iter()
            .map(|&(tick, hash)| Checkpoint { tick, hash })
            .collect();
        assert_eq!(checkpoints, match_result.replay.checkpoints);
        assert_eq!(resim.final_hash, match_result.replay.final_hash);
    }

    #[test]
    fn the_demo_world_resolves_for_demo_replays() {
        let replay = crate::demo::record_replay(7, 90);
        let world =
            resolve_replay_world(&repo_content(), &replay).expect("the demo replay resolves");
        assert_eq!(world.content_hash(), replay.content_hash);
    }

    #[test]
    fn an_ai_vs_ai_match_summary_carries_a_winner_field() {
        // Stage 10 (M8) adds the `winner` field to MatchSummary. Over the
        // real content at the flagship budget the scripted Alpha vs Alpha
        // match doesn't resolve — the opponent isn't aggressive enough to
        // eliminate the other's command center in four minutes (the plan
        // notes the Alpha is a scripted opponent, not an evaluative one).
        // The field is therefore `None` here; the M8 acceptance test
        // (tests/match_rules.rs) drives a forced defeat to assert
        // `MatchEnded` actually fires and sets the field.
        // (M9 note: the hash below moved once M9 fixed the frozen chase
        // orders — orders are canonical hashed state; see the re-pin
        // reasons in tests/ai.rs. The "doesn't resolve at 7200" narrative
        // is unchanged by that fix; the M9 tuning pass closes matches at
        // longer budgets — tests/alpha_loop.rs is the resolution exit.)
        let bundle = ContentBundle::load_dir(&repo_content()).expect("repo content loads");
        let match_result =
            run_ai_match(&bundle, 7, 7200, Slot::Ai, Slot::Ai).expect("the match runs");
        // The field exists and is readable (the structural assertion).
        let _ = match_result.summary.winner;
        // The golden contract holds: 241 checkpoints, final hash pinned.
        assert_eq!(
            match_result.replay.checkpoints.len(),
            (7200 / 30) as usize + 1
        );
        assert_eq!(match_result.replay.final_hash, 0x01b3_b60b_741f_03e9);
    }
}
