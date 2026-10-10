//! A-002 Part 2.2: stamp the git short SHA into the binary so `--version`
//! and the F3 overlay can say exactly which commit a player ran (the A14
//! bug-report promise: "name the build" is now automatic).
//!
//! Best effort by design: a build from a git checkout gets the real SHA; a
//! build from an exported tarball (no `.git`) falls back to `unknown` — an
//! honest string, never a build failure (release CI always checks out the
//! full repo, so shipped archives always carry the real SHA).

use std::path::Path;
use std::process::Command;

fn main() {
    // Re-run when HEAD moves so the stamped SHA never goes stale across
    // commits built in the same target dir.
    if Path::new(".git/HEAD").exists() {
        println!("cargo:rerun-if-changed=.git/HEAD");
    }
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|sha| !sha.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=PANDEMONIUM_GIT_SHA={sha}");
}
