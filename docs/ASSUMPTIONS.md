# Assumptions Log

Decisions that had to be guessed, logged per the operating contract (plan §0): prefer
a small, reversible guess plus an entry here over stopping to ask — except for
anything touching the frozen decisions of plan §2 (those require an ADR instead).
Each entry cites the plan section it interprets. Strike an entry out when the human
confirms or rejects it.

- **A-001 (§3.3 platform).** Carried over from the plan, awaiting human confirmation:
  desktop PC (Windows/Linux/macOS), 2D top-down presentation, single-player Alpha,
  mouse + keyboard.
- **A-002 (§4 layout).** The root manifest stays a pure virtual `[workspace]` (as the
  plan's tree shows), and `tests/` is itself a workspace member package
  (`pandemonium-tests`) so that the plan's `tests/*.rs` acceptance tests actually
  compile — cargo needs a package to build test targets. Each acceptance test is
  declared as an explicit `[[test]]` entry in `tests/Cargo.toml`.
- **A-003 (§4 diagram).** The dependency diagram is read as *non-exhaustive*. Added
  edges, all downward: `engine → sim` (the engine's game loop owns stepping the
  simulation, §11.1), `engine → content` (loads content bundles to construct
  matches), `engine → ai` (hosts controllers in the tick loop), `client → sim`
  (constructs the match; mutates only through step inputs, per the M3 exit
  criterion), `client → ai` and `client → replay`.
- **A-004 (§4, §3.2).** `content` depends on `sim` (not the reverse): the loader
  converts RON into the simulation's plain structs, which keeps serde/ron out of the
  sim's dependency tree — matching "content produces plain Rust structs that sim
  receives" and the sim's dependency whitelist.
- **A-005 (§3.2).** `sim_api` depends on `fx`: commands and views carry `Vec2Fx`.
  The "core/alloc/std + thiserror" rule is read as governing *external* crates; `fx`
  is our own pure-integer math crate at the very bottom of the graph.
- **A-006 (§5).** clippy has no per-crate configuration file, so the unordered-map
  ban in `clippy.toml` applies workspace-wide — stricter than the plan's letter
  (which scopes it to fx/sim/sim_api/ai). Order-independent uses elsewhere require an
  explicit `#[allow]` with a justification comment.
- **A-007 (§5).** Wall-clock reads and floating-point types are banned in the
  determinism crates (fx, sim, sim_api, ai, content) by a line-by-line source scan
  in `tests/architecture_law.rs`, complementing clippy — because those bans must NOT
  apply to engine/client (rendering interpolation and real-time timing live there).
- **A-008 (§4).** The architecture-law test parses member manifests directly instead
  of shelling out to `cargo metadata`: hermetic (no nested cargo during `cargo
  test`), offline-safe, and equivalent in enforcement. Dev-dependencies are exempt
  from the direction law (test-only code never ships); the forbidden-crate list
  still applies to them.
- **A-009 (§5).** PCG32 constants and algorithm are transcribed from O'Neill's
  `pcg_basic.c` (64/32, XSH-RR output). No golden-vector test against the C
  reference exists yet — structural determinism tests only. Adding reference
  vectors later is cheap and reversible.
- **A-010 (§14 M0 exit).** "CI green on 3 OSes" cannot be verified in this
  environment (no GitHub runner). The workflow is authored and committed
  (`.github/workflows/ci.yml`); the local equivalents of every gate (fmt, clippy
  `-D warnings`, tests in dev and release) ran green instead. Per the declaration
  rule (§13), the 3-OS claim stands only once the repo actually runs on GitHub.
- **A-011 (§3.2).** Toolchain pinned to Rust 1.98.1 (the version installed and
  verified in this environment). Edition 2021 per §3.3.
- **A-012 (§4).** `fx` ships no lookup tables yet (the plan mentions them). Nothing
  needs them at M0 — `isqrt` is exact; facing is a direction vector, not an angle
  (§5). Revisit if a hot fixed-point conversion demands one.
