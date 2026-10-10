//! The plan §15 scale scenario (B-002 Part 3): a deterministic, data-only
//! 10x proving ground for the tick-cost budget.
//!
//! The scenario lives entirely on the data side (A4's proven path):
//! `content_scale/` carries a 128x128 symmetric map — Proving Grounds — and
//! a swarm faction whose starting forces are sized for the 10x tier (1000
//! forces per side, 2020 entities with the ore). Zero sim, engine, client,
//! or content-crate changes: the tree loads through the ordinary
//! `ContentBundle::load_dir` and validates with `content-validate`.
//!
//! `tools bench --scale N` runs the scenario:
//!
//! * **Tiering** — each side's starting forces are truncated to the first
//!   `100 * N` (the faction list leads with the command center, so every
//!   tier keeps it; ore nodes always travel along). `--scale 1` is the
//!   ~220-entity Alpha-size tier, `--scale 10` the ~2020-entity 10x tier.
//!   The truncation is a tools-authored scenario decision, like the scripted
//!   commands: it never edits content files, it selects from them.
//! * **The script** — tick 0: every worker gathers its nearest live node
//!   (squared distance on fixed-point positions, ties by lowest EntityId),
//!   every rifleman attack-moves to the map center. Tick 150: workers
//!   re-gather their nearest live node, riflemen re-target the enemy command
//!   center. No AI controllers participate — the workload is the tick
//!   pipeline over the full force count, driven by commands (FD-2).
//! * **The profile** — `Sim::step_observed` reports the eleven pipeline
//!   stage boundaries (plan §6.3) to a tools-side timing collector; the
//!   collector owns the clock, because clocks are legal in `tools` and
//!   banned in the determinism crates (plan §5.4). Per-tick wall samples
//!   give the §15 aggregates; `/proc/self/status` VmHWM gives the memory
//!   row. All timing is presentation-only telemetry (FD-6): the scenario's
//!   hash is deterministic, run to run and profile to profile.
//!
//! The same module measures plan §15's pathfinding row on the REAL 64x64
//! Crossroads map: one worker, one far `Move` per tick, the Movement stage's
//! observer cost minus an idle-tick baseline isolates the A* request.

use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use pandemonium_content::ContentBundle;
use pandemonium_sim::{Sim, TrivialWorld};
use pandemonium_sim_api::{
    Command, CommandKind, ControllerKind, EntityId, EntityView, MatchSetup, PlayerId, PlayerSetup,
    Stage, StageObserver, Tick, Vec2Fx,
};

/// Forces per side at `--scale 1` (so `--scale 10` = the plan §15 10x tier).
const FORCES_PER_SCALE: usize = 100;

/// The second command wave's tick (5 s of game time at 30 t/s): workers
/// re-gather the nearest live node, riflemen re-target the enemy base.
const SECOND_WAVE_TICK: Tick = 150;

/// The eleven pipeline stages in plan §6.3 order (the profile's rows).
const STAGES: [Stage; 11] = [
    Stage::Commands,
    Stage::Orders,
    Stage::Production,
    Stage::Economy,
    Stage::Acquisition,
    Stage::Movement,
    Stage::Combat,
    Stage::Cleanup,
    Stage::Vision,
    Stage::MatchRules,
    Stage::Finalize,
];

/// The stage's row index in [`STAGES`].
fn stage_index(stage: Stage) -> usize {
    STAGES
        .iter()
        .position(|s| *s == stage)
        .expect("the stage table is exhaustive")
}

/// The tools-side timing collector: a stage boundary closes the previous
/// stage (its cost is the elapsed between boundaries); the driver closes the
/// last stage when `step_observed` returns.
struct StageProfiler {
    totals: [u64; 11],
    samples: Vec<Vec<u64>>,
    last: Option<(usize, Instant)>,
}

impl StageProfiler {
    fn fresh() -> Self {
        Self {
            totals: [0; 11],
            samples: vec![Vec::new(); 11],
            last: None,
        }
    }

    /// Closes the open stage (if any) as of `now`.
    fn close(&mut self, now: Instant) {
        if let Some((index, since)) = self.last.take() {
            let us = now.duration_since(since).as_micros() as u64;
            self.totals[index] = self.totals[index].saturating_add(us);
            self.samples[index].push(us);
        }
    }
}

impl StageObserver for StageProfiler {
    fn stage(&mut self, stage: Stage) {
        let now = Instant::now();
        self.close(now);
        self.last = Some((stage_index(stage), now));
    }
}

/// The kind indices the scenario references, resolved from the bundle's
/// entity list (sorted by id — the same indices the sim's `KindId`s use).
struct Kinds {
    worker: u32,
    rifleman: u32,
    ore_node: u32,
    command_center: u32,
}

fn kinds_of(bundle: &ContentBundle) -> Kinds {
    let find = |id: &str| {
        bundle
            .entities
            .iter()
            .position(|entity| entity.id.as_str() == id)
            .unwrap_or_else(|| panic!("the '{id}' kind exists in the scale content")) as u32
    };
    Kinds {
        worker: find("worker"),
        rifleman: find("rifleman"),
        ore_node: find("ore_node"),
        command_center: find("command_center"),
    }
}

/// The proving-ground world with each side's forces truncated to
/// `100 * scale`. Spawn order (and therefore entity ids and the hash) is a
/// deterministic function of (content, scale): per side, the faction's
/// authored order, then the map's ore nodes.
fn scaled_world(bundle: &ContentBundle, scale: u32) -> TrivialWorld {
    let per_side = FORCES_PER_SCALE.saturating_mul(scale.max(1) as usize);
    let mut world = bundle.world();
    let mut kept = Vec::with_capacity(2 * per_side + world.initial_spawns.len() / 2);
    for player in [PlayerId(0), PlayerId(1)] {
        kept.extend(
            world
                .initial_spawns
                .iter()
                .filter(|spawn| spawn.owner == player)
                .take(per_side)
                .cloned(),
        );
    }
    kept.extend(
        world
            .initial_spawns
            .iter()
            .filter(|spawn| spawn.owner != PlayerId(0) && spawn.owner != PlayerId(1))
            .cloned(),
    );
    world.initial_spawns = kept;
    world
}

/// A two-player, all-Human setup (the scenario's commands are scripted; no
/// controller ever runs — the labels are match identity only).
fn scripted_setup(seed: u64) -> MatchSetup {
    MatchSetup {
        seed,
        players: vec![
            PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            },
            PlayerSetup {
                player: PlayerId(1),
                controller: ControllerKind::Human,
            },
        ],
    }
}

/// The scripted command wave (see the module docs). `phase` 0 is the opening
/// wave, 1 the re-issue wave; `seq` carries each player's running sequence
/// number across waves.
fn scenario_commands(sim: &Sim, kinds: &Kinds, phase: u8, seq: &mut [u32; 2]) -> Vec<Command> {
    let snapshot = sim.snapshot();
    let tick = sim.tick();
    let center = Vec2Fx::from_ints(64, 64);
    let mut commands = Vec::new();
    for player in [0u8, 1u8] {
        let own: Vec<&EntityView> = snapshot
            .entities
            .iter()
            .filter(|entity| entity.owner == PlayerId(player))
            .collect();
        let nodes: Vec<&EntityView> = snapshot
            .entities
            .iter()
            .filter(|entity| entity.kind.0 == kinds.ore_node)
            .collect();
        // Nearest live node by squared fixed-point distance, ties by id.
        let nearest_node = |from: Vec2Fx| -> Option<EntityId> {
            nodes
                .iter()
                .map(|node| ((node.pos - from).len_sq_raw(), node.id))
                .min()
                .map(|(_, id)| id)
        };
        for entity in own.iter().filter(|entity| entity.kind.0 == kinds.worker) {
            if let Some(node) = nearest_node(entity.pos) {
                seq[player as usize] += 1;
                commands.push(Command::new(
                    PlayerId(player),
                    tick,
                    seq[player as usize],
                    CommandKind::Gather {
                        units: vec![entity.id],
                        node,
                    },
                ));
            }
        }
        // Riflemen: to the center on the opening wave, at the enemy command
        // center on the re-issue wave (the map's fallback when it is gone).
        let enemy_center = snapshot
            .entities
            .iter()
            .find(|entity| {
                entity.owner == PlayerId(1 - player) && entity.kind.0 == kinds.command_center
            })
            .map(|entity| entity.pos)
            .unwrap_or(center);
        let target = if phase == 0 { center } else { enemy_center };
        for entity in own.iter().filter(|entity| entity.kind.0 == kinds.rifleman) {
            seq[player as usize] += 1;
            commands.push(Command::new(
                PlayerId(player),
                tick,
                seq[player as usize],
                CommandKind::AttackMove {
                    units: vec![entity.id],
                    target,
                },
            ));
        }
    }
    commands
}

/// One measured stage row.
pub struct StageRow {
    /// The pipeline stage (plan §6.3 order).
    pub stage: Stage,
    /// Average cost per tick, microseconds.
    pub avg_us: f64,
    /// p99 cost per tick, microseconds.
    pub p99_us: f64,
    /// Worst single tick, microseconds.
    pub max_us: u64,
    /// Share of the summed stage cost, percent.
    pub share_pct: f64,
}

/// Everything one scale run measured.
pub struct ScaleReport {
    /// The tier multiplier.
    pub scale: u32,
    /// Ticks sampled.
    pub ticks: u32,
    /// Entities alive at the end.
    pub end_entities: usize,
    /// Commands the script issued (both waves, both players).
    pub commands_issued: usize,
    /// Average tick cost, microseconds.
    pub avg_us: f64,
    /// p50 tick cost, microseconds.
    pub p50_us: f64,
    /// p95 tick cost, microseconds.
    pub p95_us: f64,
    /// p99 tick cost, microseconds.
    pub p99_us: f64,
    /// Worst tick, microseconds.
    pub max_us: u64,
    /// Wall-clock seconds for the whole run.
    pub wall_seconds: f64,
    /// Effective ticks per second.
    pub ticks_per_second: f64,
    /// The eleven stage rows (plan §6.3 order).
    pub stage_rows: Vec<StageRow>,
    /// Peak resident set size, MiB (None where /proc is unavailable).
    pub peak_rss_mib: Option<f64>,
    /// The scenario's final canonical hash (deterministic per content+scale+seed).
    pub final_hash: u64,
    /// The proving-ground bundle's content hash.
    pub content_hash: u64,
    /// The proving-ground map id.
    pub map_id: u64,
}

/// Runs the scale scenario: `ticks` steps over the truncated proving-ground
/// forces, sampling each tick's wall cost and each stage's observer cost.
pub fn run_scale_scenario(
    scale: u32,
    ticks: u32,
    content_dir: &Path,
    seed: u64,
) -> Result<ScaleReport> {
    let bundle = ContentBundle::load_dir(content_dir)
        .with_context(|| format!("loading scale content from {}", content_dir.display()))?;
    let kinds = kinds_of(&bundle);
    let world = scaled_world(&bundle, scale);
    let mut sim = Sim::new(&world, scripted_setup(seed));
    let mut profiler = StageProfiler::fresh();
    let mut seq = [0u32; 2];
    let mut commands_issued = 0usize;
    let mut samples: Vec<u64> = Vec::with_capacity(ticks as usize);
    let start = Instant::now();
    for _ in 0..ticks {
        let wave = match sim.tick() {
            0 => Some(scenario_commands(&sim, &kinds, 0, &mut seq)),
            SECOND_WAVE_TICK => Some(scenario_commands(&sim, &kinds, 1, &mut seq)),
            _ => None,
        };
        let commands = wave.unwrap_or_default();
        commands_issued += commands.len();
        let tick_start = Instant::now();
        let out = sim.step_observed(&commands, Some(&mut profiler));
        samples.push(tick_start.elapsed().as_micros() as u64);
        profiler.close(Instant::now());
        let _ = out;
    }
    let wall = start.elapsed();
    let final_hash = sim.state_hash();
    let end_entities = sim.snapshot().entities.len();

    // Per-tick aggregates.
    let ticks_run = samples.len();
    let avg_us = samples.iter().sum::<u64>() as f64 / ticks_run.max(1) as f64;
    let mut sorted = samples.clone();
    sorted.sort_unstable();
    let pick = |q: f64| -> f64 {
        let idx = ((ticks_run as f64 - 1.0) * q).round() as usize;
        sorted[idx.min(ticks_run.saturating_sub(1))] as f64
    };
    let p50_us = pick(0.50);
    let p95_us = pick(0.95);
    let p99_us = pick(0.99);
    let max_us = *sorted.last().unwrap_or(&0);
    let wall_seconds = wall.as_secs_f64();
    let ticks_per_second = if wall_seconds > 0.0 {
        ticks_run as f64 / wall_seconds
    } else {
        0.0
    };

    // Per-stage rows.
    let stage_total: u64 = profiler.totals.iter().sum();
    let stage_rows = STAGES
        .into_iter()
        .enumerate()
        .map(|(index, stage)| {
            let list = &profiler.samples[index];
            let mut stage_sorted = list.clone();
            stage_sorted.sort_unstable();
            let stage_p99 = stage_sorted
                .get(
                    stage_sorted
                        .len()
                        .saturating_sub(1)
                        .min(((list.len() as f64 - 1.0) * 0.99).round() as usize),
                )
                .copied()
                .unwrap_or(0);
            let count = list.len().max(1);
            StageRow {
                stage,
                avg_us: profiler.totals[index] as f64 / count as f64,
                p99_us: stage_p99 as f64,
                max_us: *stage_sorted.last().unwrap_or(&0),
                share_pct: if stage_total > 0 {
                    100.0 * profiler.totals[index] as f64 / stage_total as f64
                } else {
                    0.0
                },
            }
        })
        .collect();

    Ok(ScaleReport {
        scale,
        ticks: ticks_run as u32,
        end_entities,
        commands_issued,
        avg_us,
        p50_us,
        p95_us,
        p99_us,
        max_us,
        wall_seconds,
        ticks_per_second,
        stage_rows,
        peak_rss_mib: peak_rss_mib(),
        final_hash,
        content_hash: bundle.content_hash(),
        map_id: bundle.map_id(),
    })
}

/// Peak resident set size in MiB, from /proc (Linux). None elsewhere —
/// reported as "not measurable" rather than guessed.
fn peak_rss_mib() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
    let kb: f64 = line
        .trim_start_matches("VmHWM:")
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    Some(kb / 1024.0)
}

/// Plan §15's pathfinding row, measured on the real 64x64 Crossroads map:
/// one worker issues one far `Move` per tick; the Movement stage's observer
/// cost on those ticks, minus its idle-tick baseline, isolates the request.
pub struct PathCostReport {
    /// How many path requests were issued (one per measurement tick).
    pub requests: u32,
    /// Average marginal Movement-stage cost per request tick, microseconds.
    pub avg_us: f64,
    /// Median marginal cost, microseconds.
    pub p50_us: f64,
    /// Worst marginal cost, microseconds.
    pub max_us: u64,
    /// The idle-tick Movement-stage baseline average, microseconds.
    pub baseline_avg_us: f64,
}

/// Measures plan §15's pathfinding request row on the real map (see
/// [`PathCostReport`]). Wall-clock telemetry over a deterministic scenario.
pub fn measure_path_request_cost(content_dir: &Path) -> Result<PathCostReport> {
    let bundle = ContentBundle::load_dir(content_dir)
        .with_context(|| format!("loading content from {}", content_dir.display()))?;
    let kinds = kinds_of(&bundle);
    let world = bundle.world();
    let mut sim = Sim::new(&world, scripted_setup(7));
    let worker = sim
        .snapshot()
        .entities
        .iter()
        .find(|entity| entity.owner == PlayerId(0) && entity.kind.0 == kinds.worker)
        .map(|entity| entity.id)
        .expect("the real content starts with a worker");
    // Long-haul targets around the 64x64 map (all ground).
    let targets = [
        Vec2Fx::from_ints(2, 2),
        Vec2Fx::from_ints(61, 2),
        Vec2Fx::from_ints(2, 61),
        Vec2Fx::from_ints(61, 61),
        Vec2Fx::from_ints(31, 2),
        Vec2Fx::from_ints(2, 31),
        Vec2Fx::from_ints(61, 31),
        Vec2Fx::from_ints(31, 61),
    ];

    let movement_of_tick = |sim: &mut Sim, commands: &[Command]| -> u64 {
        let mut profiler = StageProfiler::fresh();
        sim.step_observed(commands, Some(&mut profiler));
        profiler.close(Instant::now());
        profiler.totals[stage_index(Stage::Movement)]
    };

    // Baseline: idle ticks (the worker stands; the movement scans still run).
    let baseline_ticks = 30;
    let mut baseline_total = 0u64;
    for _ in 0..baseline_ticks {
        baseline_total += movement_of_tick(&mut sim, &[]);
    }
    let baseline_avg_us = baseline_total as f64 / baseline_ticks as f64;

    // Measurement: one far Move per tick, replacing the previous order (each
    // issues a fresh A* request).
    let request_ticks: u32 = 40;
    let mut margins: Vec<u64> = Vec::with_capacity(request_ticks as usize);
    for tick in 0..request_ticks {
        let target = targets[(tick as usize) % targets.len()];
        let commands = [Command::new(
            PlayerId(0),
            tick + baseline_ticks,
            tick + 1,
            CommandKind::Move {
                units: vec![worker],
                target,
            },
        )];
        let cost = movement_of_tick(&mut sim, &commands);
        margins.push(cost.saturating_sub(baseline_avg_us as u64));
    }
    margins.sort_unstable();
    let avg_us = margins.iter().sum::<u64>() as f64 / margins.len() as f64;
    let p50_us = margins[margins.len() / 2] as f64;
    let max_us = *margins.last().unwrap_or(&0);
    Ok(PathCostReport {
        requests: request_ticks,
        avg_us,
        p50_us,
        max_us,
        baseline_avg_us,
    })
}

/// Runs the scale bench for `tools bench --scale` and prints the full report
/// (per-tick aggregates, the eleven stage rows, the memory row, and the
/// real-map pathfinding row). `content_dir` is the REAL content directory —
/// the proving ground is its sibling `content_scale`.
pub fn run_and_print(scale: u32, ticks: u32, content_dir: &Path, seed: u64) -> Result<()> {
    let scale_dir = content_dir
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("content_scale");
    let report = run_scale_scenario(scale, ticks, &scale_dir, seed)?;
    print_scale_report(&report);
    println!();
    match measure_path_request_cost(content_dir) {
        Ok(path) => print_path_report(&path),
        Err(error) => println!("pathfinding row: NOT MEASURED — {error:#}"),
    }
    Ok(())
}

/// Prints the scale report against plan §15's tier budgets.
pub fn print_scale_report(report: &ScaleReport) {
    let tier_avg_budget = if report.scale >= 10 { 8_000.0 } else { 1_000.0 };
    let tier = if report.scale >= 10 {
        "10x scale (plan §15: <= 8 ms avg)"
    } else {
        "Alpha-size (plan §15: <= 1 ms avg, <= 4 ms p99)"
    };
    println!("pandemonium bench — scale scenario (plan §15, B-002 Part 3)");
    println!(
        "  tier:            --scale {} ({tier}), {} ticks",
        report.scale, report.ticks
    );
    println!(
        "  content:         proving ground {:#018x} / map {:#018x}",
        report.content_hash, report.map_id
    );
    println!(
        "  end entities:    {} (script issued {} commands)",
        report.end_entities, report.commands_issued
    );
    println!(
        "  avg tick:        {:.1} us ({:.2} ms)  — {} the tier budget",
        report.avg_us,
        report.avg_us / 1000.0,
        verdict(report.avg_us <= tier_avg_budget)
    );
    println!(
        "  p50 tick:        {:.1} us ({:.2} ms)",
        report.p50_us,
        report.p50_us / 1000.0
    );
    println!(
        "  p95 tick:        {:.1} us ({:.2} ms)",
        report.p95_us,
        report.p95_us / 1000.0
    );
    println!(
        "  p99 tick:        {:.1} us ({:.2} ms){}",
        report.p99_us,
        report.p99_us / 1000.0,
        if report.scale < 10 {
            format!(
                "  — {} plan §15 (<= 4 ms p99)",
                verdict(report.p99_us <= 4_000.0)
            )
        } else {
            String::new()
        }
    );
    println!(
        "  max tick:        {} us ({:.2} ms)",
        report.max_us,
        report.max_us as f64 / 1000.0
    );
    println!(
        "  throughput:      {:.0} ticks/s{}",
        report.ticks_per_second,
        if report.scale < 10 {
            format!(
                "  — {} plan §15 (>= 1000 t/s)",
                verdict(report.ticks_per_second >= 1000.0)
            )
        } else {
            "  (informational at 10x; the budget row targets Alpha matches)".to_string()
        }
    );
    println!(
        "  wall time:       {:.2}s for {} ticks",
        report.wall_seconds, report.ticks
    );
    match report.peak_rss_mib {
        Some(rss) => println!(
            "  peak RSS:        {:.1} MiB — {} plan §15 (< 200 MB)",
            rss,
            verdict(rss < 200.0)
        ),
        None => println!("  peak RSS:        not measurable (no /proc on this platform)"),
    }
    println!(
        "  final hash:      {:#018x} (deterministic per content+scale+seed)",
        report.final_hash
    );
    println!("  stage profile (avg / p99 / max per tick, share of summed stage cost):");
    for row in &report.stage_rows {
        println!(
            "    {:<12} {:>9.1} us  {:>9.1} us  {:>9} us  {:>5.1}%",
            format!("{:?}", row.stage),
            row.avg_us,
            row.p99_us,
            row.max_us,
            row.share_pct
        );
    }
}

/// Prints the pathfinding row (real map).
pub fn print_path_report(report: &PathCostReport) {
    println!("pathfinding request row (real 64x64 Crossroads, plan §15: <= 0.5 ms typical):");
    println!(
        "  {} requests, one far Move per tick; Movement-stage marginal cost",
        report.requests
    );
    println!(
        "  avg {:.1} us ({:.3} ms)  p50 {:.1} us ({:.3} ms)  max {} us ({:.3} ms)  — {} the budget",
        report.avg_us,
        report.avg_us / 1000.0,
        report.p50_us,
        report.p50_us / 1000.0,
        report.max_us,
        report.max_us as f64 / 1000.0,
        verdict(report.p50_us <= 500.0)
    );
    println!(
        "  (baseline idle Movement stage: {:.1} us/tick; the row is wall-clock telemetry over a deterministic scenario)",
        report.baseline_avg_us
    );
}

/// The verdict word for a budget comparison.
fn verdict(passes: bool) -> &'static str {
    if passes {
        "MEETS"
    } else {
        "MISSES"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn repo_root() -> PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("the tools crate sits two levels under the workspace root")
            .to_path_buf()
    }

    fn scale_dir() -> PathBuf {
        repo_root().join("content_scale")
    }

    fn real_content() -> PathBuf {
        repo_root().join("content")
    }

    #[test]
    fn the_scale_content_tree_validates() {
        // The proving ground is a loadable, valid, symmetric bundle — the
        // same strict validation `content-validate content_scale` runs.
        crate::validate_content::validate_content(&scale_dir())
            .expect("the scale content directory must validate");
    }

    #[test]
    fn scale_entity_and_rule_files_match_the_real_content() {
        // The drift guard: the proving ground's entity and rule files are
        // byte-identical copies of the real content. If the real content
        // changes, this fails until the scale tree is re-copied — the two
        // trees must never silently diverge.
        let list = |dir: &PathBuf| -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(dir)
                .expect("the category directory is readable")
                .map(|entry| {
                    entry
                        .expect("a readable entry")
                        .file_name()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            names.sort();
            names
        };
        for category in ["entities", "rules"] {
            let scale = scale_dir().join(category);
            let real = real_content().join(category);
            assert_eq!(
                list(&scale),
                list(&real),
                "the {category} file sets must match"
            );
            for name in list(&scale) {
                let scale_bytes = std::fs::read(scale.join(&name)).expect("scale file readable");
                let real_bytes = std::fs::read(real.join(&name)).expect("real file readable");
                assert_eq!(scale_bytes, real_bytes, "{category}/{name} drifted");
            }
        }
    }

    #[test]
    fn tier_force_counts_are_exact() {
        // Scale 1 = 100 forces per side (the Alpha-size tier, ~220 entities
        // with the ore); scale 10 = the full proving ground (2020). Every
        // tier keeps its command center.
        let bundle = ContentBundle::load_dir(&scale_dir()).expect("the scale content loads");
        let kinds = kinds_of(&bundle);
        for (scale, per_side) in [(1u32, 100usize), (10, 1000)] {
            let world = scaled_world(&bundle, scale);
            let sim = Sim::new(&world, scripted_setup(7));
            let snapshot = sim.snapshot();
            assert_eq!(
                snapshot.entities.len(),
                2 * per_side + 20,
                "scale {scale}: 2x{per_side} forces + 20 ore nodes"
            );
            for player in [0u8, 1u8] {
                let centers = snapshot
                    .entities
                    .iter()
                    .filter(|entity| {
                        entity.owner == PlayerId(player) && entity.kind.0 == kinds.command_center
                    })
                    .count();
                assert_eq!(centers, 1, "scale {scale}: player {player} keeps one CC");
            }
        }
    }

    #[test]
    fn the_scale_scenario_is_deterministic() {
        // The acceptance test: same content + scale + seed -> the identical
        // command stream, entity count, and final canonical hash. The 1x
        // tier runs past the second wave's tick (150) so both command waves
        // and seven checkpoints are covered; the 10x tier is a shorter smoke
        // (31 ticks still spans a full checkpoint interval) because dev-
        // profile invariants over 2020 entities are the suite's slowest
        // single test by far.
        for (scale, ticks) in [(1u32, 210u32), (10, 31)] {
            let first = run_scale_scenario(scale, ticks, &scale_dir(), 7)
                .expect("the first scale run completes");
            let second = run_scale_scenario(scale, ticks, &scale_dir(), 7)
                .expect("the second scale run completes");
            assert_eq!(
                first.commands_issued, second.commands_issued,
                "scale {scale}: the script repeats"
            );
            assert_eq!(first.end_entities, second.end_entities, "scale {scale}");
            assert_eq!(
                first.final_hash, second.final_hash,
                "scale {scale}: the final hash repeats"
            );
            // The 1x tier must have exercised both waves: the opening wave
            // (2 * 99 orders) and the re-issue wave at tick 150.
            if scale == 1 {
                assert!(
                    first.commands_issued > 2 * 99,
                    "the second wave fired: {} commands total",
                    first.commands_issued
                );
            }
        }
    }

    #[test]
    fn the_opening_wave_actually_orders_everyone() {
        // The scenario's premise: at tick 0 every worker gets a Gather and
        // every rifleman an AttackMove (all validate — nodes need no line of
        // sight, positions need none either). A silent rejection storm would
        // hollow out the workload this bench exists to measure.
        let bundle = ContentBundle::load_dir(&scale_dir()).expect("the scale content loads");
        let kinds = kinds_of(&bundle);
        let world = scaled_world(&bundle, 1);
        let mut sim = Sim::new(&world, scripted_setup(7));
        let mut seq = [0u32; 2];
        let commands = scenario_commands(&sim, &kinds, 0, &mut seq);
        let snapshot = sim.snapshot();
        let workers = snapshot
            .entities
            .iter()
            .filter(|entity| entity.kind.0 == kinds.worker)
            .count();
        let riflemen = snapshot
            .entities
            .iter()
            .filter(|entity| entity.kind.0 == kinds.rifleman)
            .count();
        assert_eq!(
            commands.len(),
            workers + riflemen,
            "everyone is ordered in the opening wave"
        );
        let out = sim.step(&commands);
        let rejected = out
            .events
            .iter()
            .filter(|event| matches!(event, pandemonium_sim_api::Event::CommandRejected { .. }))
            .count();
        assert_eq!(
            rejected, 0,
            "the opening wave validates cleanly: {rejected} rejected"
        );
    }
}
