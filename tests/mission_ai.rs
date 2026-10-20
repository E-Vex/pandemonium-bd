//! B-021 — mission-controlled AI tuning (ADR-0201 Phase 2, the closing work
//! package): the scripted Alpha opponent's pacing consts become
//! mission-overridable through [`pandemonium_sim_api::AiSettings`] on
//! [`pandemonium_sim_api::MissionData`], with byte-exact defaults.
//!
//! This file owns the **baseline and identity surface**:
//!
//! - **The mission-less baseline** — the flagship-shape run (seed 7, both
//!   slots scripted, the default map's world) reproduces the registered
//!   flagship final hash AND this suite's own digests: the command
//!   stream's (the replay-wire encoding of the full feed order) and each
//!   controller RNG's full `(state, inc)` at ticks 300 and 7200. The
//!   controller is engine-side state that never enters the simulation's
//!   canonical state hash, so its random stream is pinned through
//!   `ScriptedController::rng_state`.
//! - **The None identity** — a mission whose `ai_settings` block is
//!   all-`None` (or absent) drives the exact same run: same final hash,
//!   same command stream, same controller RNG states. `None` means today's
//!   const (the brief's byte-exact-defaults law).
//! - **The override direction** — tuning values derived from the script's
//!   own consts (never hardcoded) move observable behavior in the known
//!   direction.
//!
//! The hosted runs step the exact `AiMatchHost::advance` contract (perceive
//! on the fog-filtered view, decide, act through the shared gate),
//! manually, so the concrete controllers stay inspectable at the tick
//! boundaries — the same shape `tests/mission_phase1.rs` uses for
//! mission-carrying hosted runs.

use std::path::{Path, PathBuf};

use pandemonium_ai::{Controller, ScriptedController};
use pandemonium_content::ContentBundle;
use pandemonium_fx::XxHash64;
use pandemonium_replay::{ReplayFile, FORMAT_VERSION};
use pandemonium_sim::Sim;
use pandemonium_sim_api::{Command, CommandKind, ControllerKind, MatchSetup, PlayerId, PlayerSetup};

/// The repository's content directory.
fn repo_content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("content")
}

/// The flagship shape's fixed inputs (the CLI's `tools headless --seed 7
/// --ticks 7200 --p1 ai --p2 ai` over the default content).
const SEED: u64 = 7;
/// The flagship budget.
const TICKS: u32 = 7200;
/// The digested tick boundaries (the brief's pre-flight: 300 and 7200).
const PROBE_TICKS: [u32; 2] = [300, 7200];

/// One hosted run's evidence: the command log, the final hash, and each
/// controller's RNG state at the probe boundaries — `[probe index][slot]`,
/// the slots in ascending player order (the host's invocation order).
struct Run {
    /// Every command fed to the simulation, in feed order.
    log: Vec<Command>,
    /// The canonical state hash after the budget.
    final_hash: u64,
    /// Per controller slot, the RNG `(state, inc)` after the step that
    /// reached each probe tick.
    rng_states: [[(u64, u64); 2]; 2],
}

/// Runs the flagship shape, stepping the exact `AiMatchHost::advance` body
/// (perceive fog-filtered view, decide, act through the one gate;
/// controllers invoked in ascending slot order) with the controllers kept
/// concrete so their RNG state stays readable at the boundaries.
fn flagship_run() -> Run {
    let bundle = ContentBundle::load_dir(&repo_content()).expect("repo content loads");
    let world = bundle.world();
    let starts = &bundle.map.starts;
    assert!(
        starts.len() == 2,
        "the flagship shape needs exactly two starts"
    );
    let players = [PlayerId(starts[0].player), PlayerId(starts[1].player)];
    let setup = MatchSetup {
        seed: SEED,
        players: players
            .iter()
            .map(|player| PlayerSetup {
                player: *player,
                controller: ControllerKind::Ai,
            })
            .collect(),
    };
    let mut controllers: Vec<(PlayerId, ScriptedController)> = players
        .iter()
        .map(|player| {
            (
                *player,
                ScriptedController::new(pandemonium_engine::alpha_plan(
                    &bundle, &world, *player, SEED, None,
                )),
            )
        })
        .collect();
    // The host's own discipline: ascending slot order every tick.
    controllers.sort_by_key(|(player, _)| *player);

    let mut sim = Sim::new(&world, setup);
    let mut log: Vec<Command> = Vec::new();
    let mut rng_states = [[(0u64, 0u64); 2]; 2];
    while sim.tick() < TICKS {
        let tick = sim.tick();
        let mut feed: Vec<Command> = Vec::new();
        for (player, controller) in controllers.iter_mut() {
            let view = sim.player_view(*player);
            controller.think(&view, tick, &mut feed);
        }
        log.extend(feed.iter().cloned());
        sim.step(&feed);
        if let Some(probe) = PROBE_TICKS.iter().position(|tick| *tick == sim.tick()) {
            rng_states[probe] = [controllers[0].1.rng_state(), controllers[1].1.rng_state()];
        }
    }
    Run {
        final_hash: sim.state_hash(),
        log,
        rng_states,
    }
}

/// The command-stream digest: the replay-wire encoding of the whole feed
/// order (`ReplayFile::encode` over the commands; every other field a
/// fixed constant) hashed through the canonical `fx::XxHash64` — the same
/// bytes a recorded replay of this run would carry in its command
/// section, so an identical digest is an identical stream.
fn command_stream_digest(log: &[Command]) -> u64 {
    let file = ReplayFile {
        format_version: FORMAT_VERSION,
        content_hash: 0,
        map_id: 0,
        mission_id: None,
        seed: SEED,
        player_setup: Vec::new(),
        commands: log.to_vec(),
        checkpoints: Vec::new(),
        final_hash: 0,
    };
    let mut h = XxHash64::new();
    h.write_bytes(&file.encode());
    h.finish()
}

/// The first two `AttackMove` (wave march) ticks per slot — the
/// wave-timing observable the override tests compare against the baseline
/// (the first march fires on the initial pressure timer crossing MIN_WAVE;
/// the second on the re-press cadence while a wave is out).
fn attack_moves(log: &[Command]) -> Vec<(PlayerId, Vec<u32>)> {
    let mut slots: Vec<(PlayerId, Vec<u32>)> = Vec::new();
    for command in log {
        if matches!(command.kind, CommandKind::AttackMove { .. }) {
            match slots.iter_mut().find(|(player, _)| *player == command.issuer) {
                Some((_, ticks)) => ticks.push(command.tick),
                None => slots.push((command.issuer, vec![command.tick])),
            }
        }
    }
    slots
}

/// The baseline digests, recorded at the pristine base (`6eaa82b`, master
/// after B-020) through this probe before any B-021 feature change:
///
/// - final hash `0x6e9a18bd7c5f699f` — the registered flagship golden (the
///   run's own cross-check: the loop here is `AiMatchHost::advance`'s exact
///   body, so it must reproduce the CLI value);
/// - command stream — 612 commands, digest `0x32ebb1e153a446e7`;
/// - controller RNG `(state, inc)` — the `inc` half is the PCG32 default
///   stream constant (`Rng::DEFAULT_STREAM` shifted odd) on every slot, the
///   sanity check that these are the real generators; the `state` halves
///   are the consumed streams at the probe boundaries.
const BASELINE_COMMAND_DIGEST: u64 = 0x32eb_b1e1_53a4_46e7;
const BASELINE_RNG_300_SLOT0: (u64, u64) = (5309_4413_4470_4258_470, 4354_6855_6493_6845_355);
const BASELINE_RNG_300_SLOT1: (u64, u64) = (7103_4074_1474_4833_111, 4354_6855_6493_6845_355);
const BASELINE_RNG_7200_SLOT0: (u64, u64) = (2037_3309_2041_1874_478, 4354_6855_6493_6845_355);
const BASELINE_RNG_7200_SLOT1: (u64, u64) = (2060_0323_2901_5198_335, 4354_6855_6493_6845_355);

#[test]
fn the_missionless_flagship_baseline_is_pinned() {
    let run = flagship_run();
    // The hosted loop must reproduce the registered flagship golden (the
    // run's own cross-check: this loop is AiMatchHost::advance's exact
    // body, so it must match the CLI value byte for byte).
    assert_eq!(
        run.final_hash,
        0x6e9a_18bd_7c5f_699f,
        "the hosted loop must reproduce the registered flagship golden"
    );
    // The command count the CLI reports for the same run.
    assert_eq!(
        run.log.len(),
        612,
        "the command count the CLI reports for the flagship run"
    );
    // The pinned baseline digests (observed at the pristine base through
    // this probe; a moved value is a stop condition, never a re-pin).
    assert_eq!(
        command_stream_digest(&run.log),
        BASELINE_COMMAND_DIGEST,
        "the full command stream is byte-identical to the recorded baseline"
    );
    assert_eq!(run.rng_states[0][0], BASELINE_RNG_300_SLOT0, "slot 0 RNG @300");
    assert_eq!(run.rng_states[0][1], BASELINE_RNG_300_SLOT1, "slot 1 RNG @300");
    assert_eq!(run.rng_states[1][0], BASELINE_RNG_7200_SLOT0, "slot 0 RNG @7200");
    assert_eq!(run.rng_states[1][1], BASELINE_RNG_7200_SLOT1, "slot 1 RNG @7200");
    // The wave-timing observables (reported for the record; the override
    // tests derive their expectations from these by comparison, not by
    // hardcoding).
    println!("baseline AttackMove ticks per slot: {:?}", attack_moves(&run.log));
}
