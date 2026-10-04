//! The M9 exit suite (plan §14 M9, "Alpha content & feel pass"): the minimum
//! viable loop is playable end to end against the AI.
//!
//! The M8 handoff left one honest gap: the scripted Alpha vs Alpha match at
//! 7200 ticks did not resolve — the opponent wasn't aggressive enough to
//! eliminate the other's command center, and the match-rules machinery
//! (pinned by `tests/match_rules.rs` through a forced resignation) never saw
//! a natural resolution. M9 closed that: the simulation now pops chase
//! orders whose target died (`crates/sim/src/combat.rs`), and the wave
//! machine presses on a cadence, focuses the sighted enemy command center,
//! and marches waves of ten (`crates/ai/src/scripted.rs`).
//!
//! What this suite proves, machine-checkable:
//!
//! 1. **Resolution within the budget** — Alpha vs Alpha matches resolve
//!    (stage 10 declares a winner) inside the fifteen-minute budget the M8
//!    exit names, across seeds (A7's "no stuck matches" at the milestone
//!    scale; the A12 invariant checker runs every debug tick of every match
//!    here — an invariant violation would panic, not fail silently).
//! 2. **Resolution is deterministic** — the same seed produces the same
//!    winner, the same end tick, the same command log, and the same final
//!    hash (A1 at the resolution scale, not just the 7200-tick flagship).
//! 3. **The vs-AI loop closes on the client's host** — the windowed
//!    client's `MatchHost` (the with-controllers seam, real-time clock and
//!    interpolation included, driven without a renderer) runs a human slot
//!    against the AI to a natural resolution: the AI's pressure eliminates
//!    an idle human's base and `MatchEnded` surfaces through the same
//!    boundary the end screen reads.
//!
//! The feel pass's presentation half (feedback cues, audio wiring) is
//! pinned by unit tests in the client and engine crates; the pixel-level
//! human pass remains DEBT-008.
//!
//! Manifest finalization (M9 task) is a data review, not a change: the
//! diagnosis attributed the stall to the frozen chase orders and the wave
//! machine's parked state, both code — the §10.4 values stayed as pinned by
//! `tests/content_pipeline.rs` (no golden moved, no content hash changed).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use pandemonium_ai::Controller;
use pandemonium_content::ContentBundle;
use pandemonium_engine::{alpha_controller, AiMatchHost, MatchHost};
use pandemonium_sim_api::{Command, ControllerKind, MatchSetup, PlayerId, PlayerSetup};

/// The resolution budget: fifteen minutes of game time at 30 Hz (the M8
/// exit's "a 10–15 minute match" window, read as its ceiling).
const RESOLUTION_BUDGET_TICKS: u32 = 27_000;

/// The resolution seeds: 7 (the flagship seed, a typical resolution) and 3
/// (one of the slowest resolutions measured across the 32-seed sweep —
/// ~21k ticks, proving the budget holds with margin on the tail).
const RESOLUTION_SEEDS: [u64; 2] = [7, 3];

fn repo_content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
        .join("content")
}

fn bundle() -> &'static ContentBundle {
    static BUNDLE: OnceLock<ContentBundle> = OnceLock::new();
    BUNDLE.get_or_init(|| ContentBundle::load_dir(&repo_content()).expect("content loads"))
}

fn ai_controllers(
    bundle: &ContentBundle,
    seed: u64,
    players: &[PlayerId],
) -> Vec<(PlayerId, Box<dyn Controller>)> {
    players
        .iter()
        .copied()
        .map(|player| {
            (
                player,
                Box::new(alpha_controller(bundle, &bundle.world(), player, seed))
                    as Box<dyn Controller>,
            )
        })
        .collect()
}

/// One resolved (or budget-exhausted) match: everything the exit assertions
/// read, gathered through the engine's headless hosting loop.
struct ResolvedMatch {
    winner: Option<PlayerId>,
    ended_tick: u32,
    final_hash: u64,
    log: Vec<Command>,
}

/// Runs an Alpha vs Alpha match to resolution or the budget, whichever
/// comes first. The A12 invariant checker runs on every debug tick.
fn run_to_resolution(seed: u64, budget: u32) -> ResolvedMatch {
    let bundle = bundle();
    let world = bundle.world();
    let setup = MatchSetup {
        seed,
        players: vec![
            PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Ai,
            },
            PlayerSetup {
                player: PlayerId(1),
                controller: ControllerKind::Ai,
            },
        ],
    };
    let controllers = ai_controllers(bundle, seed, &[PlayerId(0), PlayerId(1)]);
    let mut host = AiMatchHost::new(&world, setup, controllers);
    while host.tick() < budget && !host.is_finished() {
        host.advance();
    }
    let winner = host.outcome().map(|outcome| outcome.winner);
    ResolvedMatch {
        winner,
        ended_tick: host.tick(),
        final_hash: host.state_hash(),
        log: host.log().to_vec(),
    }
}

// ---------------------------------------------------------------------------
// Resolution: the loop closes, inside the budget, across seeds
// ---------------------------------------------------------------------------

#[test]
fn alpha_vs_alpha_resolves_within_the_fifteen_minute_budget() {
    for seed in RESOLUTION_SEEDS {
        let run = run_to_resolution(seed, RESOLUTION_BUDGET_TICKS);
        let ended = run.ended_tick;
        assert!(
            run.ended_tick < RESOLUTION_BUDGET_TICKS,
            "seed {seed}: the match must resolve before the budget (ran to {ended})"
        );
        let Some(winner) = run.winner else {
            panic!("seed {seed}: the match ran {ended} ticks without a MatchEnded");
        };
        assert!(
            winner == PlayerId(0) || winner == PlayerId(1),
            "seed {seed}: a two-player elimination ends with a survivor, got {winner:?}"
        );
        // A real elimination: the winner won by destroying every structure
        // the loser owned (the match ran long enough for the closing phase,
        // and not so long the budget bailed it out).
        assert!(
            run.ended_tick > 3_000,
            "seed {seed}: a real match has a build phase before it resolves ({ended})"
        );
    }
}

// ---------------------------------------------------------------------------
// Determinism at the resolution scale (A1's guarantee, end to end)
// ---------------------------------------------------------------------------

#[test]
fn resolution_is_deterministic_end_to_end() {
    // The same seed twice: identical winner, end tick, command log, and
    // final hash — the whole match, not just a checkpoint window.
    let first = run_to_resolution(RESOLUTION_SEEDS[0], RESOLUTION_BUDGET_TICKS);
    let second = run_to_resolution(RESOLUTION_SEEDS[0], RESOLUTION_BUDGET_TICKS);
    assert_eq!(first.winner, second.winner, "the winner repeats");
    assert_eq!(
        first.ended_tick, second.ended_tick,
        "the match ends on the same tick"
    );
    assert_eq!(
        first.final_hash, second.final_hash,
        "the final hash repeats"
    );
    assert_eq!(first.log, second.log, "the command log is bit-identical");

    // And a different seed diverges (the resolution is a function of the
    // seed, not a fixed script).
    let other = run_to_resolution(RESOLUTION_SEEDS[0] + 100, RESOLUTION_BUDGET_TICKS);
    assert_ne!(
        first.final_hash, other.final_hash,
        "a different seed produces a different match"
    );
}

// ---------------------------------------------------------------------------
// The vs-AI loop on the client's host: the human slot plays (idly) and the
// match closes through the same boundary the end screen reads
// ---------------------------------------------------------------------------

#[test]
fn the_windowed_host_loop_resolves_against_the_ai() {
    // The windowed client's exact hosting seam: `MatchHost::with_controllers`
    // with a Human slot (the local player) and an Ai slot (the opponent),
    // driven by the fixed-timestep clock at one tick per frame — no
    // renderer, no window. The human issues no commands (an idle player: a
    // spectator who can be eliminated); the AI's pressure must eliminate
    // their base and end the match naturally, through `outcome()` — the
    // same call the end screen and the restart gate read.
    let bundle = bundle();
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
    let controllers = ai_controllers(bundle, 7, &[PlayerId(1)]);
    let mut host = MatchHost::with_controllers(&world, setup, controllers);

    // One tick per advance: a 30 Hz frame budget (the client's loop shape,
    // without the vsync pacing).
    let frame = Duration::from_millis(1000 / 30);
    let mut guard = 0;
    while !host.is_finished() && host.tick() < RESOLUTION_BUDGET_TICKS {
        let _ = host.advance(frame);
        guard += 1;
        // The frame loop must actually advance the simulation; a host that
        // spins without ticking is a stuck loop, not a slow one.
        if guard > RESOLUTION_BUDGET_TICKS * 2 {
            panic!("the host spun without finishing or reaching the budget");
        }
    }
    assert!(
        host.is_finished(),
        "the vs-AI match must resolve within the budget (tick {})",
        host.tick()
    );
    let outcome = host.outcome().expect("finished means an outcome exists");
    assert_eq!(
        outcome.winner,
        PlayerId(1),
        "the AI opponent eliminates the idle human's base: {outcome:?}"
    );
    // A15's restart precondition holds on this host too: the log is the
    // match (a replay could be recorded from it).
    assert!(
        !host.log().is_empty(),
        "the AI issued commands through the loop"
    );
}
