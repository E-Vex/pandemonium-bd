//! Pandemonium developer tools (plan §12): headless runner, soak runner, replay
//! verifier, content validator, and benchmarks. M1 ships `headless` (a scripted
//! trivial-world match that prints its final hash) and `replay-verify`
//! (re-simulation with checkpoint comparison — acceptance A2). M2 adds
//! `content-validate` (strict validation of a content directory, plan §14).
//! The remaining subcommands arrive with their milestones (the AI runners in
//! M7, soak in nightly CI, bench in M10).

use std::path::PathBuf;

use anyhow::{bail, Context};
use clap::{Parser, Subcommand};

mod demo;
mod validate_content;
mod verify;

use demo::record_replay;
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
    /// Run the scripted trivial-world match headlessly; print checkpoint hashes.
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
    },
    /// Re-simulate a replay file and compare every checkpoint hash (A2).
    ReplayVerify {
        /// The replay file to verify.
        file: PathBuf,
    },
    /// Validate a content directory (strict mode) and print its identity (M2).
    ContentValidate {
        /// The content directory (defaults to ./content — run from the repo root).
        #[arg(default_value = "content")]
        path: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Headless {
            seed,
            ticks,
            record,
        } => {
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
        Command::ReplayVerify { file } => {
            let bytes = std::fs::read(&file)
                .with_context(|| format!("reading replay {}", file.display()))?;
            let replay = pandemonium_replay::ReplayFile::decode(&bytes)
                .with_context(|| format!("decoding replay {}", file.display()))?;
            let world = demo::demo_world();
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
    }
    Ok(())
}
