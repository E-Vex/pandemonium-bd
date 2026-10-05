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
//! no stuck matches (> 20 min game time), no invariant violations". This
//! runner reports exactly those three signals — panics surface as `Err`
//! per match; stuck matches surface as "unresolved (ran the tick budget)";
//! invariant violations surface as the A12 debug-assert panic from inside
//! `Sim::step`. The caller passes `--release` for the real A7 run, since
//! the A12 checker is debug-only by design.
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

use crate::ai_match::{run_ai_match, MatchSummary, Slot};

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
/// aggregated into one report. Per-match errors (panics, host failures)
/// are recorded in the report rather than aborting the run — A7 wants
/// exactly that signal.
pub fn run_soak(cli: &SoakCli) -> Result<SoakReport> {
    let bundle = ContentBundle::load_dir(&cli.content)
        .with_context(|| format!("loading content from {}", cli.content.display()))?;
    let mut report = SoakReport {
        matches: cli.matches,
        wins: vec![0; bundle.map.starts.len()],
        ..SoakReport::default()
    };
    let start = Instant::now();
    for index in 0..cli.matches {
        let seed = cli.seed_base + index as u64;
        match run_ai_match(&bundle, seed, cli.ticks, Slot::Ai, Slot::Ai) {
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
            Err(error) => {
                report.crashed += 1;
                if cli.verbose {
                    println!("  match {index:>4} (seed {seed:>4}): CRASH — {error:#}");
                }
            }
        }
    }
    report.wall_seconds = start.elapsed().as_secs_f64();
    Ok(report)
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
    use pandemonium_sim_api::PlayerId;

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
}
