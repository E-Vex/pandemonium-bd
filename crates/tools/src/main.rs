//! Pandemonium developer tools (plan §12): headless runner, soak runner, replay
//! verifier, content validator, and benchmarks. M1 ships `headless` (a scripted
//! trivial-world match that prints its final hash) and `replay-verify`
//! (re-simulation with checkpoint comparison — acceptance A2). M2 adds
//! `content-validate` (strict validation of a content directory, plan §14).
//! M7 makes `headless` drive real content with AI controllers (`--p1 ai --p2
//! ai`, plan §12's own shape) and teaches `replay-verify` to resolve either
//! world by content identity. The remaining subcommands arrive with their
//! milestones (`soak` and `bench` land as M10 prep — `soak` runs N seeded
//! AI-vs-AI matches for A7, `bench` measures per-tick cost against plan §15's
//! perf budgets).

use std::path::PathBuf;

use anyhow::{bail, Context};
use clap::{Parser, Subcommand};
use pandemonium_sim_api::PlayerId;

mod ai_match;
mod bench;
mod demo;
mod soak;
mod validate_content;
mod verify;

use ai_match::{load_content, resolve_replay_world, run_ai_match, Slot};
use bench::{print_report as print_bench_report, run_bench, BenchCli};
use demo::record_replay;
use soak::{print_report, run_soak, SoakCli};
use validate_content::validate_content;
use verify::{verify_replay, Verification};

#[derive(Parser)]
#[command(
    name = "pandemonium-tools",
    about = "Pandemonium developer tools (plan.md §12)",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a headless match and print its checkpoint hashes.
    Headless {
        /// The match seed (plan §6.5: a match is seed + content + command log).
        #[arg(long, default_value_t = 7)]
        seed: u64,
        /// How many ticks to simulate.
        #[arg(long, default_value_t = 300)]
        ticks: u32,
        /// Also write a replay file for `replay-verify`.
        #[arg(long)]
        record: Option<PathBuf>,
        /// What drives the first map start's slot (plan §12: `--p1 ai`).
        #[arg(long, default_value = "demo")]
        p1: Slot,
        /// What drives the second map start's slot (plan §12: `--p2 ai`).
        #[arg(long, default_value = "demo")]
        p2: Slot,
        /// The content directory AI matches load (ignored by the demo).
        #[arg(long, default_value = "content")]
        content: PathBuf,
    },
    /// Re-simulate a replay file and compare every checkpoint hash (A2).
    ReplayVerify {
        /// The replay file to verify.
        file: PathBuf,
        /// The content directory candidate for world resolution.
        #[arg(long, default_value = "content")]
        content: PathBuf,
    },
    /// Validate a content directory (strict mode) and print its identity (M2).
    ContentValidate {
        /// The content directory (defaults to ./content — run from the repo root).
        #[arg(default_value = "content")]
        path: PathBuf,
    },
    /// Run many seeded AI-vs-AI matches and report win/stuck/crash telemetry
    /// (M10 prep, A7 acceptance). Sequential — nightly CI that wants
    /// parallelism spawns N `headless --seed N --p1 ai --p2 ai`
    /// subprocesses.
    Soak(SoakCli),
    /// Measure per-tick cost on the Alpha content and report against plan
    /// §15's perf budgets (M10 prep). Samples each tick with `Instant::now`
    /// (presentation-only telemetry — the sim never sees it, FD-6).
    Bench(BenchCli),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Headless {
            seed,
            ticks,
            record,
            p1,
            p2,
            content,
        } => match (p1, p2) {
            (Slot::Demo, Slot::Demo) => {
                // The M1 scripted demo over the trivial world — unchanged,
                // including its pinned final hash.
                let replay = record_replay(seed, ticks);
                println!("pandemonium headless — scripted trivial-world match");
                println!("  seed:        {}", replay.seed);
                println!("  ticks:       {ticks}");
                println!("  content:     {:#018x}", replay.content_hash);
                println!("  map id:      {}", replay.map_id);
                println!(
                    "  commands:    {} (rejections included, all deterministic)",
                    replay.commands.len()
                );
                println!("  checkpoints:");
                for cp in &replay.checkpoints {
                    println!("    tick {:>4}: {:#018x}", cp.tick, cp.hash);
                }
                println!("  final hash:  {:#018x}", replay.final_hash);
                if let Some(path) = record {
                    let bytes = replay.encode();
                    std::fs::write(&path, &bytes)
                        .with_context(|| format!("writing replay to {}", path.display()))?;
                    println!(
                        "  replay file: {} ({} bytes, checksummed)",
                        path.display(),
                        bytes.len()
                    );
                }
            }
            (p1, p2) => {
                if p1 == Slot::Demo || p2 == Slot::Demo {
                    bail!("`demo` mixes with nothing: choose it for both slots or neither");
                }
                let bundle = load_content(&content)?;
                let slot_name = |slot: Slot| match slot {
                    Slot::Ai => "ai",
                    Slot::Idle => "idle",
                    Slot::Demo => "demo",
                };
                let match_result = run_ai_match(&bundle, seed, ticks, p1, p2)?;
                println!("pandemonium headless — controller-driven match");
                println!("  seed:        {}", seed);
                println!("  ticks:       {ticks}");
                println!("  slots:       {} vs {}", slot_name(p1), slot_name(p2));
                println!("  content:     {:#018x}", match_result.replay.content_hash);
                println!("  map id:      {}", match_result.replay.map_id);
                println!(
                    "  commands:    {} (rejections included, all deterministic)",
                    match_result.replay.commands.len()
                );
                let summary = &match_result.summary;
                for (index, player) in match_result.replay.player_setup.iter().enumerate() {
                    println!(
                        "  player {}:    deliveries {}, trained {}, built {}, hits {}, deaths {}",
                        player.player.0,
                        summary.delivered.get(index).copied().unwrap_or(0),
                        summary.trained.get(index).copied().unwrap_or(0),
                        summary.built.get(index).copied().unwrap_or(0),
                        summary.attack_hits.get(index).copied().unwrap_or(0),
                        summary.deaths.get(index).copied().unwrap_or(0),
                    );
                }
                println!("  checkpoints:");
                for cp in &match_result.replay.checkpoints {
                    println!("    tick {:>4}: {:#018x}", cp.tick, cp.hash);
                }
                println!("  final hash:  {:#018x}", match_result.replay.final_hash);
                match match_result.summary.winner {
                    Some(winner) if winner == PlayerId::NEUTRAL => {
                        println!("  match ended: mutual destruction (no winner)");
                    }
                    Some(winner) => {
                        println!("  match ended: player {} wins", winner.0);
                    }
                    None => {
                        println!("  match ended: unresolved (ran the tick budget)");
                    }
                }
                if let Some(path) = record {
                    let bytes = match_result.replay.encode();
                    std::fs::write(&path, &bytes)
                        .with_context(|| format!("writing replay to {}", path.display()))?;
                    println!(
                        "  replay file: {} ({} bytes, checksummed)",
                        path.display(),
                        bytes.len()
                    );
                }
            }
        },
        Command::ReplayVerify { file, content } => {
            let bytes = std::fs::read(&file)
                .with_context(|| format!("reading replay {}", file.display()))?;
            let replay = pandemonium_replay::ReplayFile::decode(&bytes)
                .with_context(|| format!("decoding replay {}", file.display()))?;
            let world = resolve_replay_world(&content, &replay)?;
            match verify_replay(&world, &replay)? {
                Verification::Passed {
                    checkpoints,
                    final_hash,
                } => {
                    println!(
                        "replay-verify: PASS — {} checkpoints reproduced",
                        checkpoints.len()
                    );
                    println!("  final hash: {final_hash:#018x}");
                }
                Verification::Diverged { detail } => {
                    println!("replay-verify: FAIL — {detail}");
                    bail!("replay diverged from the re-simulation: {detail}");
                }
            }
        }
        Command::ContentValidate { path } => {
            validate_content(&path)?;
        }
        Command::Soak(cli) => {
            let report = run_soak(&cli)?;
            print_report(&report);
            // A7's "no stuck matches" gate: a non-zero `unresolved` count
            // at this scale is a finding worth surfacing as a non-zero exit
            // (the run still completes — the report is the evidence).
            if report.crashed > 0 {
                bail!(
                    "soak reported {} crashed match(es) — A7 crash signal",
                    report.crashed
                );
            }
        }
        Command::Bench(cli) => {
            let report = run_bench(&cli)?;
            print_bench_report(&report);
        }
    }
    Ok(())
}
