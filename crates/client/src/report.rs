//! The §11.6 deterministic bug-report dump + `--record` replay assembly
//! (M10.1, plan §11.6 / §6.5).
//!
//! Two consumers share this module:
//! - **`--record <path>`** — the windowed player's replay recording: the
//!   match's `ReplayFile`, assembled exactly as the tools' AI-match recorder
//!   does (`crates/tools/src/ai_match.rs`), written at match end / clean
//!   exit so `tools replay-verify` accepts it.
//! - **F8, the bug-report dump** — plan §11.6's "deterministic seed + tick
//!   dump": a `.pdrp` replay record of the match so far *plus* a sidecar
//!   `…-info.txt` carrying the seed, tick, content identity, and the client
//!   state a reproducer needs. Deterministic content only — no wall-clock,
//!   no paths beyond the write location — so two dumps at the same tick
//!   produce identical bytes (which is itself a test).
//!
//! Everything that decides bytes is a pure function over its inputs; the
//! I/O wrappers are thin. The client is above the determinism boundary
//! (plan §5 governs the simulation crates), but the *dump* stays
//! deterministic anyway: a report a tester files twice from the same tick
//! must not claim two different matches.

use std::io;
use std::path::{Path, PathBuf};

use pandemonium_replay::{Checkpoint, ReplayFile, FORMAT_VERSION};
use pandemonium_sim::TrivialWorld;
use pandemonium_sim_api::{Command, MatchSetup};

/// The standing controls line (mirrors the README's controls paragraph and
/// the UI's first-time-player reminder, `ui.rs`).
pub const CONTROLS_LINE: &str = "left-click/drag select; right-click move/attack/gather; \
     A+click attack-move; S stop; Q/E rotate; Ctrl+wheel pitch; P pause; \
     F3 overlay; F8 bug report; R restart";

/// The armed-attack-move instruction (the HUD line shown while 'A' is armed).
pub const ATTACK_MOVE_LINE: &str = "ATTACK MOVE - left-click a target (Esc cancels)";

/// The controls line in effect at a moment of interest: the armed
/// attack-move instruction, a showing refusal notice, or the standing
/// controls summary. Pure — callers feed the same booleans the HUD reads.
pub fn controls_line(attack_move_armed: bool, refusal_showing: bool) -> &'static str {
    if attack_move_armed {
        ATTACK_MOVE_LINE
    } else if refusal_showing {
        "order refused - see HUD notice"
    } else {
        CONTROLS_LINE
    }
}

/// Assembles the match's replay record (plan §6.5's field list), mirroring
/// the tools' AI-match recorder: format version, the world's content hash +
/// map id, the setup, the host's command log (rejections included), the
/// checkpoint trail (tick 0 + every periodic checkpoint, forced final at
/// `current_tick` if the trail does not end there), and the final hash.
///
/// Pure over its inputs — the caller supplies what the `MatchHost` already
/// keeps (`log()`, `tick()`, `state_hash()`) plus the trail collected from
/// the per-frame outcomes.
pub fn replay_record(
    world: &TrivialWorld,
    setup: &MatchSetup,
    log: &[Command],
    trail: &[(u32, u64)],
    current_tick: u32,
    current_hash: u64,
) -> ReplayFile {
    let mut checkpoints: Vec<Checkpoint> = trail
        .iter()
        .map(|&(tick, hash)| Checkpoint { tick, hash })
        .collect();
    if checkpoints.last().map(|cp| cp.tick) != Some(current_tick) {
        checkpoints.push(Checkpoint {
            tick: current_tick,
            hash: current_hash,
        });
    }
    ReplayFile {
        format_version: FORMAT_VERSION,
        content_hash: world.content_hash(),
        map_id: world.map_id,
        seed: setup.seed,
        player_setup: setup.players.clone(),
        commands: log.to_vec(),
        checkpoints,
        final_hash: current_hash,
    }
}

/// The state snapshot the bug-report sidecar records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidecarInfo {
    /// The match seed (the report's reproduction handle).
    pub seed: u64,
    /// The tick at the moment of the dump.
    pub tick: u32,
    /// The match's content identity (the fixture-encoded world hash).
    pub content_hash: u64,
    /// The map the match runs on.
    pub map_id: u64,
    /// The human's player slot.
    pub player_slot: u8,
    /// How many frames have been presented (the client's own clock).
    pub frame_count: u64,
    /// How many entities the tester has selected.
    pub selection_size: usize,
    /// The controls line in effect (see [`controls_line`]).
    pub controls_line: &'static str,
}

/// Builds the sidecar text. Deterministic content only — no wall-clock, no
/// paths, no environment: two dumps with the same inputs produce identical
/// bytes (pinned by a test).
pub fn sidecar_text(info: &SidecarInfo) -> String {
    let SidecarInfo {
        seed,
        tick,
        content_hash,
        map_id,
        player_slot,
        frame_count,
        selection_size,
        controls_line,
    } = *info;
    format!(
        "pandemonium bug report\n\
         seed: {seed}\n\
         tick: {tick}\n\
         content hash: {content_hash:#018x}\n\
         map id: {map_id}\n\
         player slot: {player_slot}\n\
         frame count: {frame_count}\n\
         selection size: {selection_size}\n\
         controls: {controls_line}\n"
    )
}

/// The report file stem: `pandemonium-report-<seed>-tick<tick>`. Two dumps at
/// the same seed + tick overwrite each other — the name is a pure function
/// of the match state, matching the dump's byte determinism.
pub fn report_stem(seed: u64, tick: u32) -> String {
    format!("pandemonium-report-{seed}-tick{tick}")
}

/// Writes the two report files into `dir` and returns their paths: the
/// replay record `<stem>.pdrp` and the sidecar `<stem>-info.txt`.
pub fn write_files(
    dir: &Path,
    stem: &str,
    replay: &ReplayFile,
    sidecar: &str,
) -> io::Result<(PathBuf, PathBuf)> {
    let replay_path = dir.join(format!("{stem}.pdrp"));
    let info_path = dir.join(format!("{stem}-info.txt"));
    std::fs::write(&replay_path, replay.encode())?;
    std::fs::write(&info_path, sidecar)?;
    Ok((replay_path, info_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use pandemonium_content::ContentBundle;
    use pandemonium_engine::MatchHost;
    use pandemonium_sim::run_command_log;
    use pandemonium_sim_api::{ControllerKind, MatchSetup, PlayerId, PlayerSetup};

    use crate::alpha_controller;

    /// The repo's content directory (the client's own resolution depth).
    fn repo_content() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join("content")
    }

    /// A driven host over the real content: the same shape the windowed
    /// client builds (`App::build_host`), advanced for a few real frames.
    fn driven_host(frames: u32, paused: bool) -> (MatchHost, Vec<(u32, u64)>) {
        let bundle = ContentBundle::load_dir(&repo_content()).expect("repo content loads");
        let world = bundle.world();
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
        let controllers = vec![(
            PlayerId(1),
            Box::new(alpha_controller(&bundle, &world, PlayerId(1), setup.seed))
                as Box<dyn pandemonium_ai::Controller>,
        )];
        let mut host = MatchHost::with_controllers(&world, setup.clone(), controllers);
        let mut trail = vec![(0, host.state_hash())];
        let frame_dt = Duration::from_secs_f64(1.0 / 60.0);
        if paused {
            host.set_paused(true);
        }
        for _ in 0..frames {
            let outcome = host.advance(frame_dt);
            for &(tick, hash) in &outcome.hashes {
                trail.push((tick, hash));
            }
        }
        (host, trail)
    }

    #[test]
    fn the_record_re_simulates_to_identical_hashes() {
        // The A2 pattern at the client seam: the record built from a driven
        // host decodes and re-simulates through the canonical driver to the
        // same checkpoints and final hash.
        let bundle = ContentBundle::load_dir(&repo_content()).expect("repo content loads");
        let world = bundle.world();
        let (host, trail) = driven_host(120, false);
        let replay = replay_record(
            &world,
            &host_setup(),
            host.log(),
            &trail,
            host.tick(),
            host.state_hash(),
        );
        let decoded = ReplayFile::decode(&replay.encode()).expect("the record decodes");
        assert_eq!(decoded, replay);
        let resim = run_command_log(
            &world,
            &host_setup(),
            &replay.commands,
            replay.checkpoints.last().unwrap().tick,
        );
        assert_eq!(
            resim
                .checkpoints
                .iter()
                .map(|&(tick, hash)| Checkpoint { tick, hash })
                .collect::<Vec<_>>(),
            replay.checkpoints,
            "every checkpoint re-simulates"
        );
        assert_eq!(resim.final_hash, replay.final_hash);
    }

    #[test]
    fn f8_during_pause_produces_a_valid_record() {
        // Pause freezes the clock; the dump reads the current state, so a
        // paused match's record is as verifiable as a running one's.
        let bundle = ContentBundle::load_dir(&repo_content()).expect("repo content loads");
        let world = bundle.world();
        let (host, trail) = driven_host(120, true);
        let replay = replay_record(
            &world,
            &host_setup(),
            host.log(),
            &trail,
            host.tick(),
            host.state_hash(),
        );
        let resim = run_command_log(
            &world,
            &host_setup(),
            &replay.commands,
            replay.checkpoints.last().unwrap().tick,
        );
        assert_eq!(resim.final_hash, replay.final_hash);
    }

    #[test]
    fn the_sidecar_is_deterministic_and_state_dependent() {
        let base = SidecarInfo {
            seed: 42,
            tick: 300,
            content_hash: 0x9BC1_8C52_1107_B262,
            map_id: 0xD381_3640_1AB0_2FF1,
            player_slot: 0,
            frame_count: 512,
            selection_size: 3,
            controls_line: controls_line(false, false),
        };
        // Same inputs -> identical bytes.
        assert_eq!(sidecar_text(&base), sidecar_text(&base));
        // Two dumps at the same tick produce identical bytes: the byte
        // equality IS the determinism claim (plan §11.6's own test shape).
        let again = sidecar_text(&base);
        assert_eq!(sidecar_text(&base), again);
        // Every field matters.
        let mut moved = base;
        moved.tick = 301;
        assert_ne!(sidecar_text(&moved), sidecar_text(&base));
        let mut moved = base;
        moved.selection_size = 4;
        assert_ne!(sidecar_text(&moved), sidecar_text(&base));
        let mut moved = base;
        moved.controls_line = controls_line(true, false);
        assert_ne!(sidecar_text(&moved), sidecar_text(&base));
        // No wall-clock, no path, no environment: the text carries only the
        // reported fields (spot-check the stable fields' presence).
        let text = sidecar_text(&base);
        assert!(text.contains("seed: 42"));
        assert!(text.contains("tick: 300"));
        assert!(text.contains("player slot: 0"));
        assert!(!text.contains("instant") && !text.contains("Instant"));
    }

    #[test]
    fn the_report_stem_is_a_pure_function_of_seed_and_tick() {
        assert_eq!(report_stem(42, 300), "pandemonium-report-42-tick300");
        assert_eq!(report_stem(42, 300), report_stem(42, 300));
        assert_ne!(report_stem(42, 300), report_stem(42, 301));
    }

    #[test]
    fn the_controls_line_reflects_the_input_mode() {
        assert_eq!(controls_line(true, false), ATTACK_MOVE_LINE);
        assert_eq!(controls_line(false, true), "order refused - see HUD notice");
        assert_eq!(controls_line(false, false), CONTROLS_LINE);
    }

    /// The setup `driven_host` builds (kept out of the helper so the record
    /// builder's caller — the test — owns it, like the client does).
    fn host_setup() -> MatchSetup {
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
}
