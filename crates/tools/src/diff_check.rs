//! The B-006 differential check (plan §15 scale work, DEBT-202): the
//! golden-preservation instrument for optimization commits.
//!
//! Runs many seeded matches — the default is the real-content AI-vs-AI
//! configuration (the flagship golden's shape) — collects each match's FULL
//! checkpoint hash trail (tick 0, every `CHECKPOINT_INTERVAL` boundary, and
//! the forced final), and prints ONE digest line per seed. The digest is
//! `fx::XxHash64` over the seed, the checkpoint count, and every
//! (tick, hash) pair in trail order, so any state divergence at any
//! checkpoint — even one that cancels out by the final hash — moves that
//! seed's line. An optimization branch re-runs the mode over the same seed
//! range and diffs the output against the committed baseline: an empty diff
//! is the bit-for-bit evidence BRIEF B-006 requires per commit.
//!
//! The `--scale N` tier runs the proving-ground scenario (B-002 Part 3's
//! `content_scale/` world, scripted waves, no AI) and digests the same
//! trail shape, covering the tier the scale budgets are about.
//!
//! Determinism posture: this mode is read-only telemetry. It constructs the
//! same matches `headless --p1 ai --p2 ai` (and `bench --scale`) construct,
//! through the same hosting, and feeds nothing back into the sim. Two runs
//! over the same range print byte-identical lines — that re-runnability is
//! itself pinned by a unit test over a reduced seed range.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use pandemonium_content::ContentBundle;
use pandemonium_fx::XxHash64;
use pandemonium_sim::Sim;
use pandemonium_sim_api::{Command, Tick};

use crate::ai_match::{run_ai_match, Slot};
use crate::scale::{kinds_of, scaled_world, scenario_commands, scripted_setup, SECOND_WAVE_TICK};

/// The diff-check subcommand's CLI shape.
#[derive(Args, Clone, Debug)]
pub struct DiffCheckCli {
    /// How many seeded matches to run. BRIEF B-006's differential floor is
    /// 200; smaller values are fine for smoke checks.
    #[arg(long, default_value_t = 200)]
    pub matches: u32,
    /// The seed of the first match; subsequent matches use seed_base + i.
    #[arg(long, default_value_t = 0)]
    pub seed_base: u64,
    /// Per-match tick budget for the AI-vs-AI tier. 1500 ticks (50 s of
    /// game time) crosses the opening, the first engagements, and the
    /// mid-game economy churn while keeping a 200-match sweep runnable
    /// after every commit.
    #[arg(long, default_value_t = 1_500)]
    pub ticks: u32,
    /// Run the proving-ground scale scenario (B-002 Part 3) instead of the
    /// AI-vs-AI match: `--scale 1` the Alpha-size tier, `--scale 10` the
    /// plan §15 10x tier. Loads `content_scale` (a sibling of --content).
    #[arg(long)]
    pub scale: Option<u32>,
    /// The content directory to load (AI matches); the scale tier loads
    /// `content_scale` next to it.
    #[arg(long, default_value = "content")]
    pub content: PathBuf,
}

/// One seed's differential line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SeedDigest {
    /// The match seed.
    pub seed: u64,
    /// The digest over the full checkpoint hash trail.
    pub digest: u64,
    /// How many checkpoints the trail carried.
    pub checkpoints: usize,
}

/// Digests one match's checkpoint trail: the seed, the checkpoint count,
/// and every (tick, hash) pair in trail order. Pure function of the trail —
/// two identical trails digest identically, any divergence moves the value.
pub fn digest_trail(seed: u64, trail: &[(Tick, u64)]) -> u64 {
    let mut h = XxHash64::new();
    h.write_u64(seed);
    h.write_u64(trail.len() as u64);
    for (tick, hash) in trail {
        h.write_u32(*tick);
        h.write_u64(*hash);
    }
    h.finish()
}

/// Runs the differential sweep and returns one digest per seed, ascending.
pub fn run_diff_check(cli: &DiffCheckCli) -> Result<Vec<SeedDigest>> {
    if let Some(scale) = cli.scale {
        run_scale_tier(cli, scale)
    } else {
        run_ai_tier(cli)
    }
}

/// The AI-vs-AI tier: the real content, both slots scripted controllers,
/// the same match `headless --p1 ai --p2 ai` runs.
fn run_ai_tier(cli: &DiffCheckCli) -> Result<Vec<SeedDigest>> {
    let bundle = ContentBundle::load_dir(&cli.content)
        .with_context(|| format!("loading content from {}", cli.content.display()))?;
    let mut out = Vec::with_capacity(cli.matches as usize);
    for index in 0..cli.matches {
        let seed = cli.seed_base + index as u64;
        let match_result = run_ai_match(&bundle, seed, cli.ticks, Slot::Ai, Slot::Ai)?;
        let trail: Vec<(Tick, u64)> = match_result
            .replay
            .checkpoints
            .iter()
            .map(|cp| (cp.tick, cp.hash))
            .collect();
        // The final hash is folded in even when the last checkpoint already
        // sits at the final tick (the trail then carries it twice — always
        // the same way, so the digest stays a pure function of the run).
        let mut trailed = trail;
        trailed.push((cli.ticks, match_result.replay.final_hash));
        out.push(SeedDigest {
            seed,
            digest: digest_trail(seed, &trailed),
            checkpoints: trailed.len(),
        });
    }
    Ok(out)
}

/// The scale tier: the proving-ground scenario, scripted waves, no AI —
/// the tier the plan §15 budgets measure.
fn run_scale_tier(cli: &DiffCheckCli, scale: u32) -> Result<Vec<SeedDigest>> {
    let bundle = load_scale_content(&cli.content)?;
    let kinds = kinds_of(&bundle);
    let world = scaled_world(&bundle, scale);
    let mut out = Vec::with_capacity(cli.matches as usize);
    for index in 0..cli.matches {
        let seed = cli.seed_base + index as u64;
        let mut sim = Sim::new(&world, scripted_setup(seed));
        let mut seq = [0u32; 2];
        let mut trail: Vec<(Tick, u64)> = vec![(0, sim.state_hash())];
        for _ in 0..cli.ticks {
            let wave = match sim.tick() {
                0 => Some(scenario_commands(&sim, &kinds, 0, &mut seq)),
                SECOND_WAVE_TICK => Some(scenario_commands(&sim, &kinds, 1, &mut seq)),
                _ => None,
            };
            let commands: Vec<Command> = wave.unwrap_or_default();
            let step = sim.step(&commands);
            if let Some(hash) = step.hash {
                trail.push((sim.tick(), hash));
            }
        }
        let final_hash = sim.state_hash();
        if trail.last().map(|(tick, _)| *tick) != Some(sim.tick()) {
            trail.push((sim.tick(), final_hash));
        }
        out.push(SeedDigest {
            seed,
            digest: digest_trail(seed, &trail),
            checkpoints: trail.len(),
        });
    }
    Ok(out)
}

/// Loads the proving-ground bundle: `content_scale` next to the given
/// content directory (the same sibling layout `bench --scale` resolves).
fn load_scale_content(content: &Path) -> Result<ContentBundle> {
    let scale_dir = content
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("content_scale");
    ContentBundle::load_dir(&scale_dir)
        .with_context(|| format!("loading scale content from {}", scale_dir.display()))
}

/// Prints the differential report: one line per seed, then the aggregate.
pub fn print_report(digests: &[SeedDigest]) {
    println!("pandemonium diff-check — per-seed checkpoint-trail digests");
    for line in digests {
        println!(
            "seed {:>6}  checkpoints {:>4}  digest {:#018x}",
            line.seed, line.checkpoints, line.digest
        );
    }
    let mut aggregate = XxHash64::new();
    aggregate.write_u64(digests.len() as u64);
    for line in digests {
        aggregate.write_u64(line.seed);
        aggregate.write_u64(line.digest);
    }
    println!(
        "total: {} seeds, aggregate digest {:#018x}",
        digests.len(),
        aggregate.finish()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The digest is a pure function of the trail: same trail, same value;
    /// any single checkpoint change moves it.
    #[test]
    fn digest_is_pure_and_sensitive() {
        let trail = vec![(0u32, 1u64), (30, 2), (60, 3)];
        let a = digest_trail(7, &trail);
        let b = digest_trail(7, &trail);
        assert_eq!(a, b, "same trail digests identically");
        let mut moved = trail.clone();
        moved[1] = (30, 2 + 1);
        assert_ne!(
            a,
            digest_trail(7, &moved),
            "one moved checkpoint moves the digest"
        );
        // Length and seed are folded in too.
        assert_ne!(a, digest_trail(8, &trail));
        let short = &trail[..2];
        assert_ne!(a, digest_trail(7, short));
    }

    /// A reduced AI-vs-AI sweep is re-runnable bit-for-bit: two runs over
    /// the same seeds print identical digests (the mode's contract).
    #[test]
    fn the_ai_tier_is_reproducible() {
        // The compiled-in workspace layout the other tools use
        // (CARGO_MANIFEST_DIR = crates/tools, content two levels up).
        let content = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .map(|root| root.join("content"));
        let Some(content) = content else {
            return; // no layout to resolve — the unit tier is skipped
        };
        if !content.is_dir() {
            return; // no content in this environment — the unit tier is skipped
        }
        let cli = |base: u64| DiffCheckCli {
            matches: 3,
            seed_base: base,
            ticks: 240,
            scale: None,
            content: content.clone(),
        };
        let first = run_diff_check(&cli(11)).expect("the reduced sweep runs");
        let second = run_diff_check(&cli(11)).expect("the re-run runs");
        assert_eq!(first, second, "two runs over the same seeds are identical");
        assert_eq!(first.len(), 3);
        let other = run_diff_check(&cli(12)).expect("the shifted sweep runs");
        assert_ne!(first, other, "different seeds produce different digests");
    }
}
