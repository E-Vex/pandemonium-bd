//! The M10 benchmark runner (plan §12 `tools bench`, plan §15 perf budgets):
//! measures per-tick cost on the Alpha content and reports the budget
//! thresholds from plan §15.
//!
//! Plan §15 budgets (release build):
//! - Sim tick, Alpha-size match (≤ 200 entities): ≤ 1 ms avg, ≤ 4 ms p99
//! - Sim tick at 10× scale (≈ 2000 entities): ≤ 8 ms avg
//! - Headless throughput: ≥ 1000 ticks/s
//!
//! This runner is the **per-tick timing harness** — it samples each tick's
//! wall-clock duration with [`std::time::Instant`] (which lives in `tools`,
//! not in any determinism crate — the architecture-law scan exempts
//! `crates/tools/`), then prints avg / p50 / p95 / p99 / max against the
//! plan's thresholds. The samples themselves are presentation-only telemetry;
//! the simulation never sees them (FD-6).
//!
//! Criterion integration (plan §12 mentions it) is the natural follow-up —
//! this harness gives M10 the same numbers criterion would, without pulling
//! criterion's transitive deps into the workspace's build for every CI run.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Args;
use pandemonium_content::ContentBundle;
use pandemonium_engine::{alpha_controller, MatchHost};
use pandemonium_sim_api::{ControllerKind, MatchSetup, PlayerId, PlayerSetup};

/// The bench subcommand's CLI shape.
#[derive(Args, Clone, Debug)]
pub struct BenchCli {
    /// How many ticks to sample. 1000 gives a stable p99 without taking
    /// minutes; 10_000 is the headless-throughput reference (plan §15's
    /// "1000 ticks/s" target).
    #[arg(long, default_value_t = 1_000)]
    pub ticks: u32,
    /// The content directory to load.
    #[arg(long, default_value = "content")]
    pub content: PathBuf,
    /// The match seed (deterministic — two bench runs over the same seed
    /// trace the same per-tick work).
    #[arg(long, default_value_t = 7)]
    pub seed: u64,
    /// Skip the AI controllers and bench a no-command match. The tick
    /// pipeline still runs all 11 stages, but with no commands the
    /// economy and combat stages do less work. Useful for measuring the
    /// tick pipeline's overhead in isolation; default runs the full
    /// AI-driven match for a representative Alpha-scale workload.
    #[arg(long)]
    pub no_ai: bool,
}

/// One tick's measured cost (microseconds).
#[derive(Clone, Copy, Debug)]
struct Sample {
    us: u64,
}

/// The aggregated bench report.
#[derive(Clone, Debug, Default)]
pub struct BenchReport {
    /// How many ticks were sampled.
    pub ticks: u32,
    /// Average tick time in microseconds.
    pub avg_us: f64,
    /// p50 (median) tick time in microseconds.
    pub p50_us: f64,
    /// p95 tick time in microseconds.
    pub p95_us: f64,
    /// p99 tick time in microseconds.
    pub p99_us: f64,
    /// Max tick time in microseconds.
    pub max_us: u64,
    /// Total wall-clock duration of the bench, in seconds.
    pub wall_seconds: f64,
    /// Effective throughput in ticks per second (ticks / wall_seconds).
    pub ticks_per_second: f64,
    /// Entity count at the bench's end (plan §15: "Alpha-size match
    /// ≤ 200 entities" — the report shows where the workload landed).
    pub end_entities: usize,
}

impl BenchReport {
    /// Whether the avg meets plan §15's Alpha budget (≤ 1 ms avg).
    pub fn meets_alpha_avg_budget(&self) -> bool {
        self.avg_us <= 1_000.0
    }
    /// Whether the p99 meets plan §15's Alpha budget (≤ 4 ms p99).
    pub fn meets_alpha_p99_budget(&self) -> bool {
        self.p99_us <= 4_000.0
    }
    /// Whether throughput meets plan §15's headless budget (≥ 1000 tps).
    pub fn meets_throughput_budget(&self) -> bool {
        self.ticks_per_second >= 1_000.0
    }
}

/// Runs the bench: drives `ticks` simulation steps on the Alpha content,
/// sampling each tick's wall-clock cost.
pub fn run_bench(cli: &BenchCli) -> Result<BenchReport> {
    let bundle = ContentBundle::load_dir(&cli.content)
        .with_context(|| format!("loading content from {}", cli.content.display()))?;
    let setup = MatchSetup {
        seed: cli.seed,
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
    let world = bundle.world();
    let host = if cli.no_ai {
        MatchHost::new(&world, setup)
    } else {
        let controllers: Vec<(PlayerId, Box<dyn pandemonium_ai::Controller>)> = vec![(
            PlayerId(1),
            Box::new(alpha_controller(&bundle, &world, PlayerId(1), setup.seed)),
        )];
        MatchHost::with_controllers(&world, setup, controllers)
    };
    let mut host = host;
    let mut samples: Vec<Sample> = Vec::with_capacity(cli.ticks as usize);
    let start = Instant::now();
    for _ in 0..cli.ticks {
        let tick_start = Instant::now();
        host.step_once();
        samples.push(Sample {
            us: tick_start.elapsed().as_micros() as u64,
        });
    }
    let wall = start.elapsed();
    let end_entities = host.render_snapshot().entities.len();
    Ok(aggregates(samples, wall, end_entities))
}

/// Reduces the per-tick samples into the bench report.
fn aggregates(samples: Vec<Sample>, wall: Duration, end_entities: usize) -> BenchReport {
    let ticks = samples.len() as u32;
    if samples.is_empty() {
        return BenchReport {
            ticks: 0,
            wall_seconds: wall.as_secs_f64(),
            end_entities,
            ..BenchReport::default()
        };
    }
    let total_us: u64 = samples.iter().map(|s| s.us).sum();
    let avg_us = total_us as f64 / samples.len() as f64;
    let mut sorted: Vec<u64> = samples.iter().map(|s| s.us).collect();
    sorted.sort_unstable();
    let pick = |q: f64| -> f64 {
        let idx = ((samples.len() as f64 - 1.0) * q).round() as usize;
        sorted[idx.min(sorted.len() - 1)] as f64
    };
    let p50_us = pick(0.50);
    let p95_us = pick(0.95);
    let p99_us = pick(0.99);
    let max_us = *sorted.last().unwrap_or(&0);
    let wall_seconds = wall.as_secs_f64();
    let ticks_per_second = if wall_seconds > 0.0 {
        ticks as f64 / wall_seconds
    } else {
        0.0
    };
    BenchReport {
        ticks,
        avg_us,
        p50_us,
        p95_us,
        p99_us,
        max_us,
        wall_seconds,
        ticks_per_second,
        end_entities,
    }
}

/// Prints the bench report against plan §15's budgets.
pub fn print_report(report: &BenchReport) {
    println!("pandemonium bench — per-tick cost (plan §15 baselines)");
    println!("  ticks sampled:  {}", report.ticks);
    println!("  end entities:   {} (plan §15 Alpha budget: ≤ 200)", report.end_entities);
    println!(
        "  avg tick:        {:.1} µs  ({:.2} ms)  — {} plan §15 (≤ 1 ms avg)",
        report.avg_us,
        report.avg_us / 1000.0,
        if report.meets_alpha_avg_budget() { "MEETS" } else { "MISSES" },
    );
    println!(
        "  p50 tick:        {:.1} µs  ({:.2} ms)",
        report.p50_us,
        report.p50_us / 1000.0,
    );
    println!(
        "  p95 tick:        {:.1} µs  ({:.2} ms)",
        report.p95_us,
        report.p95_us / 1000.0,
    );
    println!(
        "  p99 tick:        {:.1} µs  ({:.2} ms)  — {} plan §15 (≤ 4 ms p99)",
        report.p99_us,
        report.p99_us / 1000.0,
        if report.meets_alpha_p99_budget() { "MEETS" } else { "MISSES" },
    );
    println!(
        "  max tick:        {} µs  ({:.2} ms)",
        report.max_us,
        report.max_us as f64 / 1000.0,
    );
    println!(
        "  throughput:      {:.0} ticks/s  — {} plan §15 (≥ 1000 t/s)",
        report.ticks_per_second,
        if report.meets_throughput_budget() {
            "MEETS"
        } else {
            "MISSES"
        },
    );
    println!(
        "  wall time:       {:.2}s for {} ticks",
        report.wall_seconds, report.ticks
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_handle_a_constant_load() {
        // 100 ticks of exactly 500 µs each — avg, p50, p95, p99, max all 500.
        let samples: Vec<Sample> = (0..100).map(|_| Sample { us: 500 }).collect();
        let report = aggregates(samples, Duration::from_micros(50_000), 12);
        assert_eq!(report.ticks, 100);
        assert!((report.avg_us - 500.0).abs() < 1e-3);
        assert!((report.p50_us - 500.0).abs() < 1e-3);
        assert!((report.p95_us - 500.0).abs() < 1e-3);
        assert!((report.p99_us - 500.0).abs() < 1e-3);
        assert_eq!(report.max_us, 500);
        assert_eq!(report.end_entities, 12);
    }

    #[test]
    fn aggregates_handle_a_spike_at_the_tail() {
        // 90 ticks at 100 µs, 10 ticks at 10_000 µs — p99 captures the spike
        // (with 100 samples, p99 lands at index 98, which is in the spike range
        // starting at index 90). The p50 stays at 100; the max is the spike.
        let mut samples: Vec<Sample> = (0..90).map(|_| Sample { us: 100 }).collect();
        samples.extend((0..10).map(|_| Sample { us: 10_000 }));
        let report = aggregates(samples, Duration::from_micros(100_900), 7);
        assert!(
            (report.avg_us - ((90 * 100 + 10 * 10_000) as f64 / 100.0)).abs() < 1e-3,
            "avg = {}",
            report.avg_us
        );
        assert!((report.p50_us - 100.0).abs() < 1e-3, "p50 stays at 100: {}", report.p50_us);
        assert!(
            (report.p99_us - 10_000.0).abs() < 1e-3,
            "p99 captures the spike: {}",
            report.p99_us
        );
        assert_eq!(report.max_us, 10_000);
    }

    #[test]
    fn budget_gates_match_plan_thresholds() {
        let mut report = BenchReport {
            ticks: 100,
            avg_us: 800.0,
            p99_us: 3_500.0,
            ticks_per_second: 1_200.0,
            ..BenchReport::default()
        };
        assert!(report.meets_alpha_avg_budget());
        assert!(report.meets_alpha_p99_budget());
        assert!(report.meets_throughput_budget());
        // Just over the line on each:
        report.avg_us = 1_200.0;
        report.p99_us = 4_500.0;
        report.ticks_per_second = 950.0;
        assert!(!report.meets_alpha_avg_budget());
        assert!(!report.meets_alpha_p99_budget());
        assert!(!report.meets_throughput_budget());
    }
}
