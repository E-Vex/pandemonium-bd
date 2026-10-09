//! The M10 soak runner (plan §12 `tools soak --matches N`, A7 acceptance):
//! many seeded AI-vs-AI matches in one process, with crash/stall detection
//! and win-rate telemetry.
//!
//! Plan §12 calls for "many seeded AI-vs-AI matches in parallel processes";
//! this runner is the **sequential** version, suitable for a developer
//! machine or a CI step that wants the per-match telemetry the parallel
//! subprocess version would have to assemble from separate log files.
//! Nightly CI that wants parallelism can spawn N `tools headless --seed N
//! --ticks T --p1 ai --p2 ai` subprocesses and aggregate their output —
//! the engine's `run_command_log` re-simulation path is the same one this
//! runner drives.
//!
//! A7 (plan §13): "1000 seeded AI-vs-AI matches complete with no panics,
//! no stuck matches (> 20 min game time), no invariant violations". Where
//! each signal is actually observable:
//!
//! - **Panics** are isolated per match: [`run_soak`] wraps every match in
//!   `catch_unwind`, so a panic becomes a `crashed` count carrying the seed
//!   and the panic message instead of an abort (pre-isolation, one panic
//!   destroyed the whole run's report and every later match's telemetry).
//!   Isolation is observability, never recovery — the tool still exits
//!   non-zero at the end when anything crashed (see the soundness note on
//!   [`isolate_panic`]).
//! - **Stuck matches** surface as "unresolved (ran the tick budget)".
//! - **Invariant violations** are A12 debug-assert panics, and the A12
//!   checker is debug-only by design (plan §13) — so only a *dev-profile*
//!   soak observes them, where the isolation counts each violation as one
//!   `crashed` match with its seed. The release nightly is the
//!   panics/stuck/resolution/throughput tier; the nightly's dev-profile
//!   tier is the invariant sweep. (Before the isolation existed, a dev
//!   soak was useless at scale: the first violation aborted the run and
//!   took the evidence with it.)
//!
//! Determinism: each match's seed is `seed_base + index`, so two soak
//! runs over the same range produce byte-identical checkpoints per match.
//! The runner never reads wall-clock time inside the simulation (it only
//! times the wall-clock duration of each match for the telemetry line).

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::Args;
use pandemonium_content::ContentBundle;
use pandemonium_sim_api::PlayerId;

use crate::ai_match::{run_ai_match, AiMatch, MatchSummary, Slot};

/// The soak subcommand's CLI shape (plan §12 `tools soak`).
#[derive(Args, Clone, Debug)]
pub struct SoakCli {
    /// How many seeded AI-vs-AI matches to run. Plan §13 A7 calls for 1000;
    /// smaller values are fine for smoke checks.
    #[arg(long, default_value_t = 8)]
    pub matches: u32,
    /// The seed for the first match; subsequent matches use seed_base + i.
    /// Picking a non-zero base lets a re-run avoid re-treading the seeds a
    /// prior run already covered.
    #[arg(long, default_value_t = 0)]
    pub seed_base: u64,
    /// Per-match tick budget. Plan §13 A7 calls a match "stuck" past
    /// 20 minutes of game time — 36000 ticks at 30 Hz — so the budget
    /// defaults there, not lower: the M9 sweep measured legitimate matches
    /// resolving as late as tick ~23300, which an 18000 budget would have
    /// miscounted as stuck.
    #[arg(long, default_value_t = 36_000)]
    pub ticks: u32,
    /// The content directory to load.
    #[arg(long, default_value = "content")]
    pub content: PathBuf,
    /// Print one line per match (verbose); the default prints only the
    /// final summary.
    #[arg(long)]
    pub verbose: bool,
}

/// The aggregated telemetry from one soak run.
#[derive(Clone, Debug, Default)]
pub struct SoakReport {
    /// How many matches ran (== SoakCli::matches, unless a fatal load
    /// error aborted early).
    pub matches: u32,
    /// How many resolved with a winner (player 0 or player 1).
    pub resolved: u32,
    /// How many ended in mutual destruction (no winner).
    pub mutual_destruction: u32,
    /// How many ran the tick budget without resolving (A7's "stuck" signal).
    pub unresolved: u32,
    /// How many produced a panic or host error (A7's "crash" signal).
    pub crashed: u32,
    /// The crash evidence, one entry per crashed match: `(seed, reason)`
    /// (match order = ascending seed, deterministic). Panic reasons carry
    /// a `panic:` prefix so the report distinguishes a runtime panic or A12
    /// violation from a load-stage anyhow error.
    pub crashes: Vec<(u64, String)>,
    /// Per-player wins (index 0 = player 0, etc.).
    pub wins: Vec<u32>,
    /// End tick of each resolved match (the tick MatchEnded fired).
    pub end_ticks: Vec<u32>,
    /// Wall-clock duration of the whole soak, in seconds.
    pub wall_seconds: f64,
    /// Total ticks simulated across all matches (for throughput telemetry).
    pub total_ticks: u32,
}

impl SoakReport {
    /// The fraction of matches that resolved cleanly (win or mutual
    /// destruction). A7's "no stuck matches" gate wants this at 1.0.
    pub fn resolution_rate(&self) -> f64 {
        if self.matches == 0 {
            return 0.0;
        }
        (self.resolved + self.mutual_destruction) as f64 / self.matches as f64
    }

    /// Average end tick across resolved matches (0 when none resolved).
    pub fn avg_end_tick(&self) -> f64 {
        if self.end_ticks.is_empty() {
            0.0
        } else {
            self.end_ticks.iter().sum::<u32>() as f64 / self.end_ticks.len() as f64
        }
    }

    /// Maximum end tick across resolved matches (0 when none resolved).
    pub fn max_end_tick(&self) -> u32 {
        self.end_ticks.iter().copied().max().unwrap_or(0)
    }

    /// Effective sim throughput in ticks per second of wall-clock time.
    pub fn ticks_per_second(&self) -> f64 {
        if self.wall_seconds <= 0.0 {
            0.0
        } else {
            self.total_ticks as f64 / self.wall_seconds
        }
    }
}

/// Runs the soak: `matches` AI-vs-AI matches, seeds `seed_base + i`,
/// aggregated into one report. Per-match failures — host errors *and
/// panics* — are isolated, recorded with their seed, and counted in
/// `crashed` rather than aborting the run: A7 wants exactly that signal,
/// and a report that survives the first crash is the evidence. `main`
/// still exits non-zero afterwards when anything crashed.
pub fn run_soak(cli: &SoakCli) -> Result<SoakReport> {
    let bundle = ContentBundle::load_dir(&cli.content)
        .with_context(|| format!("loading content from {}", cli.content.display()))?;
    let starts = bundle.map.starts.len();
    let ticks = cli.ticks;
    Ok(sweep_matches(cli, starts, |seed| {
        run_ai_match(&bundle, seed, ticks, Slot::Ai, Slot::Ai)
    }))
}

/// The sweep loop behind [`run_soak`], over an injectable per-seed match
/// driver (the seam the crash-isolation tests use): every driver call is
/// panic-isolated, a panic or error is recorded with its seed, and the
/// loop continues to the next match.
fn sweep_matches<F>(cli: &SoakCli, starts: usize, driver: F) -> SoakReport
where
    F: Fn(u64) -> Result<AiMatch>,
{
    let mut report = SoakReport {
        matches: cli.matches,
        wins: vec![0; starts],
        ..SoakReport::default()
    };
    let start = Instant::now();
    for index in 0..cli.matches {
        let seed = cli.seed_base + index as u64;
        match isolate_panic(|| driver(seed)) {
            Ok(match_result) => {
                // The match's own end tick when it resolved; the budget when
                // it ran out (the runner simulates on after MatchEnded, so
                // the replay's last checkpoint is never the match's end).
                let end_tick = match_result.summary.ended_tick.unwrap_or(
                    match_result
                        .replay
                        .checkpoints
                        .last()
                        .map(|cp| cp.tick)
                        .unwrap_or(0),
                );
                report.total_ticks = report.total_ticks.saturating_add(end_tick);
                record_outcome(&mut report, &match_result.summary, end_tick);
                if cli.verbose {
                    println!(
                        "  match {:>4} (seed {:>4}): {} (tick {}, {} commands)",
                        index,
                        seed,
                        outcome_label(&match_result.summary),
                        end_tick,
                        match_result.summary.commands
                    );
                }
            }
            Err(reason) => {
                report.crashed += 1;
                report.crashes.push((seed, reason.clone()));
                if cli.verbose {
                    println!("  match {index:>4} (seed {seed:>4}): CRASH — {reason}");
                }
            }
        }
    }
    report.wall_seconds = start.elapsed().as_secs_f64();
    report
}

/// Runs one match driver call with panic isolation: `Ok` passes through,
/// an anyhow error keeps its full chain (`{error:#}`), and a panic unwinds
/// into this boundary and becomes a `panic:`-prefixed reason string
/// instead of an aborted process.
///
/// Soundness (why `AssertUnwindSafe` is correct here): the driver's
/// arguments are shared references and copies; every mutable value a match
/// owns — host, sim, world, controllers — is constructed inside the call
/// and dropped while unwinding, so no half-updated state survives the
/// boundary into the next match. The catch exists to *observe* the crash
/// (count it, attribute the seed, keep the report alive); the caller never
/// reuses suspect state, and `main` still exits non-zero afterwards. A
/// `panic = "abort"` profile makes this a no-op by construction — the
/// process aborts exactly as it did before the isolation existed.
fn isolate_panic<T>(run: impl FnOnce() -> Result<T>) -> Result<T, String> {
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
    match caught {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(format!("{error:#}")),
        Err(payload) => Err(format!("panic: {}", panic_reason(payload))),
    }
}

/// The message inside a panic payload: `&'static str`, `String`, or a
/// generic line for any other payload type.
fn panic_reason(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "a non-string panic payload".to_string()
    }
}

/// Records one match's outcome into the report.
fn record_outcome(report: &mut SoakReport, summary: &MatchSummary, end_tick: u32) {
    match summary.winner {
        Some(PlayerId(0)) => {
            report.resolved += 1;
            report.wins[0] += 1;
            report.end_ticks.push(end_tick);
        }
        Some(PlayerId(1)) => {
            report.resolved += 1;
            report.wins[1] += 1;
            report.end_ticks.push(end_tick);
        }
        Some(_) => {
            report.mutual_destruction += 1;
            report.end_ticks.push(end_tick);
        }
        None => {
            report.unresolved += 1;
        }
    }
}

/// The one-line label for a match's outcome (verbose mode).
fn outcome_label(summary: &MatchSummary) -> &'static str {
    match summary.winner {
        Some(PlayerId(0)) => "player 0 wins",
        Some(PlayerId(1)) => "player 1 wins",
        Some(_) => "mutual destruction",
        None => "unresolved",
    }
}

/// The per-crash evidence lines for the report: one line per crashed match
/// (seed + reason), capped at ten with an overflow pointer — a systematic
/// failure firing in every match must not bury the summary under a
/// thousand identical lines (the run order is the seed order, so the
/// dropped entries are exactly the later matches).
fn crash_lines(report: &SoakReport) -> Vec<String> {
    const CAP: usize = 10;
    let mut lines: Vec<String> = report
        .crashes
        .iter()
        .take(CAP)
        .map(|(seed, reason)| format!("    seed {seed}: {reason}"))
        .collect();
    let overflow = report.crashes.len().saturating_sub(CAP);
    if overflow > 0 {
        lines.push(format!("    ... and {overflow} more crashed match(es)"));
    }
    lines
}

/// Prints the soak report (the non-verbose summary line).
pub fn print_report(report: &SoakReport) {
    println!("pandemonium soak — sequential AI-vs-AI run");
    println!("  matches:        {}", report.matches);
    println!(
        "  resolved:       {} (player 0: {}, player 1: {})",
        report.resolved,
        report.wins.first().copied().unwrap_or(0),
        report.wins.get(1).copied().unwrap_or(0),
    );
    println!("  mutual destruct:{}", report.mutual_destruction);
    println!(
        "  unresolved:     {} (ran the tick budget; A7 stuck signal)",
        report.unresolved
    );
    println!("  crashed:        {} (A7 crash signal)", report.crashed);
    for line in crash_lines(report) {
        println!("{line}");
    }
    println!(
        "  end tick:       avg {:.0}, max {} (resolved matches only)",
        report.avg_end_tick(),
        report.max_end_tick()
    );
    println!(
        "  resolution rate:{:.1}% (A7 wants 100% at the soak scale)",
        report.resolution_rate() * 100.0
    );
    println!(
        "  wall time:      {:.2}s, {:.0} ticks/s throughput",
        report.wall_seconds,
        report.ticks_per_second()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::path::PathBuf;

    use pandemonium_replay::{Checkpoint, ReplayFile, FORMAT_VERSION};
    use pandemonium_sim_api::{ControllerKind, PlayerId, PlayerSetup};

    fn summary(winner: Option<PlayerId>) -> MatchSummary {
        MatchSummary {
            commands: 100,
            delivered: vec![10, 10],
            trained: vec![5, 5],
            built: vec![2, 2],
            attack_hits: vec![3, 3],
            deaths: vec![1, 1],
            winner,
            ended_tick: None,
        }
    }

    #[test]
    fn record_outcome_counts_wins_losses_and_unresolved() {
        let mut report = SoakReport {
            matches: 4,
            wins: vec![0, 0],
            ..SoakReport::default()
        };
        record_outcome(&mut report, &summary(Some(PlayerId(0))), 5000);
        record_outcome(&mut report, &summary(Some(PlayerId(1))), 7000);
        record_outcome(&mut report, &summary(Some(PlayerId(99))), 9000);
        record_outcome(&mut report, &summary(None), 18_000);
        assert_eq!(report.resolved, 2);
        assert_eq!(report.wins, vec![1, 1]);
        assert_eq!(report.mutual_destruction, 1);
        assert_eq!(report.unresolved, 1);
        assert_eq!(report.crashed, 0);
        assert_eq!(report.end_ticks, vec![5000, 7000, 9000]);
        assert_eq!(report.max_end_tick(), 9000);
        assert!((report.avg_end_tick() - 7000.0).abs() < 1e-3);
    }

    #[test]
    fn resolution_rate_handles_zero_matches() {
        let report = SoakReport::default();
        assert_eq!(report.resolution_rate(), 0.0);
    }

    #[test]
    fn outcome_label_covers_all_branches() {
        assert_eq!(outcome_label(&summary(Some(PlayerId(0)))), "player 0 wins");
        assert_eq!(outcome_label(&summary(Some(PlayerId(1)))), "player 1 wins");
        assert_eq!(
            outcome_label(&summary(Some(PlayerId(99)))),
            "mutual destruction"
        );
        assert_eq!(outcome_label(&summary(None)), "unresolved");
    }

    // --- Panic isolation (the A7 crash telemetry) --------------------------
    //
    // The regression this section pins: before the isolation existed, a
    // panic inside any match aborted the whole soak — the report, the
    // win-rate telemetry, and every later match were lost, and the
    // documented "crashed: N" line was unreachable for panics (only
    // pre-simulation anyhow errors could reach it). These tests print the
    // default panic hook's stderr lines on purpose — that output is the
    // debugging evidence; the reason string is the report's.

    /// A minimal well-formed match result for the sweep seam's tests.
    fn canned_match(winner: PlayerId, ended_tick: u32) -> AiMatch {
        AiMatch {
            replay: ReplayFile {
                format_version: FORMAT_VERSION,
                content_hash: 0,
                map_id: 0,
                seed: 0,
                player_setup: vec![
                    PlayerSetup {
                        player: PlayerId(0),
                        controller: ControllerKind::Ai,
                    },
                    PlayerSetup {
                        player: PlayerId(1),
                        controller: ControllerKind::Ai,
                    },
                ],
                commands: vec![],
                checkpoints: vec![Checkpoint { tick: 0, hash: 0 }],
                final_hash: 0,
            },
            summary: MatchSummary {
                commands: 1,
                delivered: vec![0, 0],
                trained: vec![0, 0],
                built: vec![0, 0],
                attack_hits: vec![0, 0],
                deaths: vec![0, 0],
                winner: Some(winner),
                ended_tick: Some(ended_tick),
            },
        }
    }

    fn sweep_cli(matches: u32) -> SoakCli {
        SoakCli {
            matches,
            seed_base: 100,
            ticks: 10,
            content: PathBuf::new(),
            verbose: false,
        }
    }

    fn repo_content() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf()
            .join("content")
    }

    #[test]
    fn ok_passes_through_and_errors_keep_their_chain() {
        assert_eq!(isolate_panic(|| Ok(7)), Ok(7));
        let reason = isolate_panic::<()>(|| {
            Err(anyhow::anyhow!("load failed").context("the content directory"))
        })
        .expect_err("the error arm maps to a reason");
        assert!(
            reason.contains("the content directory"),
            "chain kept: {reason}"
        );
        assert!(reason.contains("load failed"), "root kept: {reason}");
        assert!(
            !reason.contains("panic"),
            "an error is not a panic: {reason}"
        );
    }

    #[test]
    fn string_panics_carry_their_message() {
        // `panic!("literal")` payloads are `&'static str`; formatted panics
        // are `String` — both must surface verbatim behind the `panic:` tag.
        let literal = isolate_panic::<()>(|| panic!("A12 population: player 0 usage drifted"))
            .expect_err("a panic is a crash reason");
        assert_eq!(literal, "panic: A12 population: player 0 usage drifted");
        let formatted = isolate_panic::<()>(|| panic!("beta broke at seed {}", 101))
            .expect_err("a panic is a crash reason");
        assert_eq!(formatted, "panic: beta broke at seed 101");
    }

    #[test]
    fn non_string_panics_are_counted_with_a_generic_reason() {
        let reason = isolate_panic::<()>(|| std::panic::panic_any(7usize))
            .expect_err("any payload type is a crash reason");
        assert_eq!(reason, "panic: a non-string panic payload");
    }

    #[test]
    fn a_panicking_match_is_counted_and_the_sweep_continues() {
        // THE regression: with the pre-isolation loop, this driver's panic
        // propagated out of the sweep and aborted the run (the report and
        // both surviving matches' telemetry were lost). Isolated, it is one
        // crashed match attributed to its seed, and the sweep finishes.
        let report = sweep_matches(&sweep_cli(3), 2, |seed| {
            if seed == 101 {
                panic!("the invariant broke in match 1");
            }
            Ok(canned_match(PlayerId(0), 4321))
        });
        assert_eq!(report.matches, 3);
        assert_eq!(report.crashed, 1, "the panic is counted: {report:?}");
        assert_eq!(
            report.crashes,
            vec![(101, "panic: the invariant broke in match 1".to_string())],
            "the crash is attributed to its seed"
        );
        assert_eq!(report.resolved, 2, "the sweep continued past the crash");
        assert_eq!(report.end_ticks, vec![4321, 4321]);
        assert_eq!(report.wins, vec![2, 0]);
        assert_eq!(report.total_ticks, 8_642);
    }

    #[test]
    fn an_erroring_match_is_counted_with_its_chain() {
        // The pre-isolation Err arm (load-stage anyhow failures) keeps its
        // exact semantics: counted, attributed, sweep continues.
        let report = sweep_matches(&sweep_cli(2), 2, |seed| {
            if seed == 100 {
                anyhow::bail!("the host refused to advance");
            }
            Ok(canned_match(PlayerId(1), 999))
        });
        assert_eq!(report.crashed, 1);
        assert_eq!(
            report.crashes,
            vec![(100, "the host refused to advance".to_string())]
        );
        assert_eq!(report.resolved, 1);
        assert_eq!(report.wins, vec![0, 1]);
    }

    #[test]
    fn the_crash_list_is_capped_so_a_storm_cannot_bury_the_report() {
        let report = SoakReport {
            crashes: (0..12).map(|i| (i, format!("reason {i}"))).collect(),
            ..SoakReport::default()
        };
        let lines = crash_lines(&report);
        assert_eq!(lines.len(), 11, "ten seeds plus the overflow line");
        assert!(lines[0].contains("seed 0: reason 0"));
        assert!(lines[9].contains("seed 9: reason 9"));
        assert!(lines[10].contains("2 more"), "overflow line: {}", lines[10]);
    }

    #[test]
    fn run_soak_over_the_real_content_reports_zero_crashes() {
        // The real path end-to-end, through the isolation boundary: two
        // short real matches over the shipped content stay green and the
        // report's crash accounting lands on zero.
        let cli = SoakCli {
            matches: 2,
            seed_base: 7,
            ticks: 60,
            content: repo_content(),
            verbose: false,
        };
        let report = run_soak(&cli).expect("the real soak runs");
        assert_eq!(report.matches, 2);
        assert_eq!(report.crashed, 0);
        assert!(report.crashes.is_empty());
        assert_eq!(
            report.resolved + report.mutual_destruction + report.unresolved,
            2
        );
    }
}
