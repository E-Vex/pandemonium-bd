# Security Policy

## Reporting a vulnerability

A deterministic engine is a security-sensitive artifact: a desync is a
bug, full stop. If you have found a way to make the same seed + content
hash + ordered command log produce divergent state across runs, hosts,
profiles, or platforms, **report it privately** — please do not open a
public issue.

- **Preferred**: open a [GitHub Private Security
  Advisory](https://github.com/E-Vex/pandemonium-bd/security/advisories/new)
  against this repository.
- **Fallback**: email the maintainer directly (the email is on the GitHub
  profile).

Include, if you can:

- The exact `cargo run -p pandemonium-tools -- headless ...` command
  that reproduces it.
- The seed (`--seed N`), tick budget (`--ticks N`), and slots
  (`--p1 ai --p2 ai`).
- The final hash from each divergent run.
- The platform(s) and toolchain version (`rustc --version`).

A report with the seed and the tick is a reproducible incident; anything
else is a war story.

## Threat model

The Alpha is a single-process, single-machine RTS foundation. There is
no network code, no untrusted input parsing, and no plugin surface in
the Alpha scope. The assets the engine consumes are the workspace's own
checked-in content (`content/`); user-supplied content is not yet a
supported configuration (the Alpha content manifest is locked per the
plan).

The meaningful threat surface is **determinism correctness**: a
silent divergence between two runs of the same seed + content + command
log would invalidate every replay, every AI-vs-AI comparison, and the
"prove, don't assert" operating contract. That is the threat the
guardrails exist to prevent, and the threat a security report should
target.

## Mechanical guardrails already in place

These are enforced by the build, not by review:

- **Dependency law** (`tests/architecture_law.rs`): dependencies point
  downward only (fx → sim_api → sim → content → ai/replay → engine →
  client/tools). The `sim` crate may not depend on anything above it
  (FD-6); the `ai` crate may not depend on `sim` (FD-7).
- **Forbidden-crate list**: `bevy`, `macroquad`, `ggez`, `fyrox`,
  godot bindings, every ECS crate, physics and pathfinding crates,
  and `rand` / `getrandom` are forbidden workspace-wide.
- **Determinism source bans** (`tests/architecture_law.rs`): the
  determinism crates (`fx`, `sim`, `sim_api`, `ai`, `content`) are
  scanned line by line for `HashMap`, `HashSet`, `Instant`,
  `SystemTime`, and floating-point type names. The engine and client
  are NOT scanned — rendering interpolation and real-time timing
  legitimately use real clocks and float math there, and never enter
  the simulation (plan §11.1).
- **Pinned toolchain** (`rust-toolchain.toml`): the exact Rust version
  is fixed. A toolchain bump is a deliberate, reviewed change.
- **CI matrix**: tests run in dev *and* release on Linux, Windows, and
  macOS — a divergence across profile or OS is a build failure, not a
  footnote.
- **`cargo audit`** (`.github/workflows/audit.yml`): runs on every
  `Cargo.lock` change and nightly. Read-only — bumps nothing.

## Non-goals

- The windowed client's `Instant` and `f32` use is presentation-only
  and never enters the simulation. A report of "the renderer uses
  floats" is not a vulnerability — it is the architecture.
- The Alpha does not ship a network layer. Lockstep / desync-detection
  hooks exist (`Command.tick`, hashed checkpoints), but no netcode is
  in scope. A report of "no network authentication" is out of scope.
- Balance issues, AI weaknesses, and missing gameplay features are
  **not security issues**. File them as regular issues.

## Supported versions

Only the `master` branch is supported. The Alpha is undeclared; there
are no tagged releases to backport to.
