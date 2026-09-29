# Pandemonium — AI Handoff Document

> **Purpose.** Orient any agent (or human) resuming work on this project: what the
> project is, how to work on it, what is already built, and what to build next.
> This file is a living status map — it never replaces the authoritative plan.
>
> **If you are the next agent: read this file top to bottom, then read `plan.md`
> §0 (Operating Contract) and §14 (Milestones) before writing any code.**

---

## 1. Read this first (source-of-truth order)

1. `plan.md` — the authoritative specification (v2.0). Especially §0 (operating
   contract), §2 (frozen decisions FD-1..FD-10 — never change silently), §4
   (workspace + dependency law), §5 (determinism rules), §13 (acceptance criteria
   A1–A15), §14 (milestones M0–M10).
2. This file — current status and orientation.
3. `docs/ASSUMPTIONS.md` — every interpretation the implementing agent had to guess
   (numbered A-001…, each citing the plan section it interprets).
4. `docs/DEBT.md` — every deliberate simplification, with a repayment trigger.
5. `docs/ARCHITECTURE.md` — the working map of the code as built.
6. `docs/adr/` — the process for proposing changes to frozen decisions.

## 2. Project snapshot

| Field | Value |
|---|---|
| What | Pandemonium — an RTS **foundation** that grows into a game (architecture first, content is data) |
| Stack | Rust, fully custom engine (own loop, entity store, renderer on top of winit/wgpu — no game engine, no ECS framework) |
| Repo | `/home/z/my-project/pandemonium` (git, branch `main`, 3 commits at handoff) |
| Toolchain | Rust 1.98.1, edition 2021, pinned by `rust-toolchain.toml` |
| Status | **M0 (Skeleton & guardrails) COMPLETE.** Next: **M1 (Simulation core)** |
| Spirit | The Alpha is judged by system properties (plan §13), not content volume. Do not add what no acceptance test requires. |

## 3. Non-negotiable working rules (digest of plan §0)

- Work milestone by milestone. Never start M(N+1) until every exit test of M(N) is
  green. The game must build and run at the end of every milestone.
- Every task ends with the green gate (§4 below); commit only when green; one
  logical change per commit.
- Frozen decisions (plan §2) are frozen. To change one: stop, write an ADR in
  `docs/adr/` with evidence. Never silently.
- Log every guessed decision in `docs/ASSUMPTIONS.md`; log every simplification in
  `docs/DEBT.md`. Prefer a small reversible guess + a log entry over stopping to ask.
- Prove, don't assert: every claim ("deterministic", "AI parity", "add-a-unit is
  data-only") needs an automated test that exists in the repo.
- Placeholder art only. No time on graphics before M3.

## 4. How to verify the current state

Run from the repo root (`/home/z/my-project/pandemonium`):

```bash
cargo fmt --all -- --check                 # formatting
cargo clippy --workspace --all-targets -- -D warnings   # lints, zero tolerance
cargo test --workspace                      # all tests (debug)
cargo test --workspace --release            # determinism must hold in release too
cargo run -p pandemonium-tools              # prints a placeholder banner
cargo run -p pandemonium-client             # prints a placeholder banner
```

Expected at this handoff: all five commands succeed; 57 tests pass (39 fx unit
tests, 15 fx property tests, 1 sim constant test, 2 architecture-law tests).
CI additionally runs fmt + clippy + tests on Linux/Windows/macOS in dev and
release when pushed to GitHub (`.github/workflows/ci.yml`) — see A-010 below.

## 5. Workspace map (as built; see `docs/ARCHITECTURE.md` for detail)

```text
crates/fx        DONE    Q16.16 fixed-point math, isqrt, PCG32 Rng, FNV-1a hasher
crates/sim_api   STUB    identity vocabulary (EntityId, PlayerId, Tick, KindId, ResourceId)
crates/sim       STUB    TICKS_PER_SECOND = 30 only; everything else arrives in M1
crates/content   STUB    empty lib, role documented (lands in M2)
crates/ai        STUB    empty lib, role documented (lands in M7)
crates/replay    STUB    empty lib, role documented (lands in M1)
crates/engine    STUB    empty lib, role documented (lands in M3)
crates/client    STUB    placeholder binary banner (window arrives in M3)
crates/tools     STUB    placeholder binary banner (subcommands from M1)
tests/           ACTIVE  pandemonium-tests package: architecture_law.rs (A13 scaffold)
content/         EMPTY   factions/ entities/ maps/ rules/ — RON data lands in M2
docs/            ACTIVE  ARCHITECTURE, DEBT, ASSUMPTIONS, CONTENT_GUIDE, adr/
```

The dependency law is **enforced by tests, not by convention**: `tests/architecture_law.rs`
parses every member manifest, checks the internal edge allow-list, the per-crate
external allow-list (the full plan §3.2 table), the forbidden-crate list (bevy, ECS
crates, rapier, pathfinding, rand/getrandom, …), and scans the determinism crates
(fx, sim, sim_api, ai, content) line-by-line for unordered-map types, wall-clock
reads, and floating-point type names. Breaking the architecture fails CI.

## 6. Development stages — status board

| Stage | Title (plan §14) | Status | Exit criteria |
|-------|------------------|--------|---------------|
| M0 | Skeleton & guardrails | ✅ **complete** | fx property tests green ✓; architecture-law test green ✓; CI authored, 3-OS green pending a real GitHub run (A-010) |
| M1 | Simulation core | ⏭ **next** | A1/A2 on a trivial world (identical hashes on 3 OSes); ID-never-reused test; iteration-order test |
| M2 | Content pipeline | ⬜ pending | all Alpha content + map load; precise errors; A3-style data-only spawn scaffold |
| M3 | Engine shell | ⬜ pending | windowed build shows map + entities; Move commands work; client mutates sim only via step inputs |
| M4 | Movement (P1) | ⬜ pending | 50 units respond ≤ 2 ticks under spam-click; no permanent stuck units; hashes still green |
| M5 | Economy/production/construction (P3) | ⬜ pending | divergent scripted openings; A12 invariants green under economy soak |
| M6 | Combat & vision (P2) | ⬜ pending | composition/position matter; legibility checklist; A10 fog integrity green |
| M7 | AI through commands (P4) | ⬜ pending | A5 + A11 green; AI-vs-AI headless matches complete |
| M8 | Match rules & full loop (P5) | ⬜ pending | 10–15 min match vs AI completes and restarts cleanly (A15) |
| M9 | Alpha content & feel pass | ⬜ pending | minimum viable loop playable end-to-end vs the AI |
| M10 | Stabilization & declaration | ⬜ pending | every A1–A15 criterion verified; soak green; docs/ALPHA_DECLARATION.md with evidence |

**The plan was broken into parts along these milestones.** Part 1 (M0) is what is
implemented in this repo today; parts 2–11 (M1–M10) remain, in strict order.

## 7. M0 inventory — what exists today, concretely

**Workspace & guardrails**

- Virtual `[workspace]` with `members = ["crates/*", "tests"]`; shared package
  metadata; resolver 2.
- `rust-toolchain.toml` pinning 1.98.1 (+rustfmt, +clippy, minimal profile).
- `clippy.toml` disallowing unordered-map types workspace-wide (A-006 explains the
  scope choice).
- CI: fmt, clippy `-D warnings`, tests × {Linux, Windows, macOS} × {dev, release}.
- `tests/` is itself a workspace member package (`pandemonium-tests`) so the plan's
  `tests/*.rs` acceptance files compile (A-002). New acceptance tests need an
  explicit `[[test]]` entry in `tests/Cargo.toml`.
- Git history: `docs: add plan.md v2.0 …` → `feat: M0 skeleton — workspace,
  guardrails, and the fx math crate` → `docs: add AI-Handoff.md …`.

**The fx crate (the only implemented library; plan §6.1)**

- `Fx` — Q16.16 scalar, raw `i32`. Contracts: 64-bit intermediates; mul/div round
  **toward zero**; unrepresentable results **saturate** (never panic, never wrap —
  identical in debug and release); div-by-zero saturates by dividend sign;
  `checked_mul`/`checked_div` for exactness checks; `from_int`, `from_milli`
  (the plan §10.2 authoring-unit conversion), `floor_int`/`trunc_int`/`round_int`
  (halves round up); total ordering on the raw value (valid deterministic sort key).
- `Vec2Fx` — add/sub/neg/scale/dot; `len_sq_raw()` (u64, for comparisons, plan
  §5.8); `len()` via exact `isqrt`; `dist()`; `normalized()` — integer-only, zero
  maps to zero by contract, precision caveat for very short vectors (DEBT-002).
- `isqrt(n: u64) -> u64` — exact floor integer square root, full u64 range.
- `Rng` — PCG32 (XSH-RR 64/32, constants from O'Neill's pcg_basic.c, A-009).
  `new(seed, stream)` uses canonical pcg32 seeding; `seeded(seed)` on the default
  stream; `next_u32`, `next_u64` (fixed hi-then-lo composition), `bounded(n)`
  (rejection-sampled, unbiased); `state_parts()`/`from_state_parts()` for canonical
  hashing and exact restore (plan §6.4).
- `Fnv1a64` — incremental FNV-1a 64-bit canonical hasher; `write_u16/u32/u64/i32/i64`
  encode little-endian (the canonical byte form of plan §6.4); golden-tested
  against the published FNV vectors. xxHash64 swap is DEBT-001.

**Tests (57, all green dev + release)**

- 39 unit tests across the fx modules (golden values, contracts, extremes).
- 15 proptest properties: conversion round-trips; add commutes/associates in range;
  mul commutes always; mul-by-one identity everywhere; mul→div recovery within 2
  ulps (representable products); div-by-zero saturation; isqrt floor property over
  the whole u64 range; vector length dominance; normalization unit-length band;
  RNG reproducibility and bounded range; incremental-vs-one-shot hashing.
- 2 architecture-law tests (§5 above).
- 1 sim constant test pinning `TICKS_PER_SECOND = 30`.

## 8. What is NOT built yet — and exactly what M1 asks for

Nothing of the simulation, content, engine, client, AI, replay, or tools exists
beyond the stubs and vocabulary types listed above. **M1 — Simulation core
(plan §14, citing §6, §7, §8):**

1. `crates/sim`: `Sim::new(content, setup)`, `Sim::step(&[Command])` as the *only*
   mutator (FD-2), tick counter, entity store with monotonic never-reused IDs,
   capability stores iterated in ascending ID order, command queue with
   (issuer, seq) ordering, event buffer drained per tick, `state_hash()` over a
   canonical little-endian byte encoding through `fx::Fnv1a64`, `snapshot()`.
2. `crates/sim_api`: the full `Command`/`CommandKind` (§8.1), `Event` (§9.8),
   `Reject`, `PlayerView` types.
3. `crates/replay`: record/load/verify format `{format_version, content_hash,
   map_id, seed, player_setup, commands, checkpoints, final_hash}`.
4. `tests/determinism.rs` (+ its `[[test]]` entry): A1 — same seed + command log →
   identical checkpoint hashes across two runs; ID-never-reused; iteration-order
   test. Golden hashes get committed once green (then CI's 3-OS matrix makes the
   cross-OS claim real).
5. `tools` headless scaffold: run a trivial scripted match, print final hash.

M1 entry points: `crates/sim/src/lib.rs` (mostly empty, contract documented in its
module docs), `crates/sim_api/src/lib.rs` (identity vocabulary exists), and the fx
API in section 7 — that is the entire vocabulary M1 builds on. `ContentBundle` does
not exist yet (M2); M1's trivial world should hardcode a minimal in-code bundle or
take fixture structs, NOT jump ahead into RON loading.

## 9. Sharp edges and gotchas discovered during M0

- **proptest macro quirk**: `prop_assert!(x as T < (y + 1) * (y + 1))` fails to
  parse inside `proptest!` (cast-then-`<` breaks the expr fragment). Bind locals
  first; see `isqrt_is_the_floor_of_the_root` for the pattern.
- **Fx saturates on extremes** — including legitimate-seeming mul→div round trips
  when the intermediate product exceeds ±2^31 raw (a·b ≈ 2^47 raw²). The
  mul→div-recovery property is scoped to representable products for this reason;
  saturation itself is separately tested.
- **Clippy 1.98**: `#![cfg_attr(clippy, deny(...))]` triggers
  `clippy::no_mismatched_clippy_cfg`-style errors — use plain
  `#![deny(clippy::float_arithmetic, clippy::disallowed_types, clippy::disallowed_methods)]`
  (rustc accepts and ignores clippy tool lints).
- Inherent `Fx::mul`/`Fx::div` carry `#[allow(clippy::should_implement_trait)]` on
  purpose: the inherent methods are the canonical documented ops and the operator
  impls delegate to them (plan §5.6 favors greppable explicit arithmetic).
- **Docs in the determinism crates must avoid the literal banned tokens**
  (`HashMap`, `HashSet`, `Instant`, `SystemTime`, `f32`, `f64`) — the source scan
  checks comments and doc text too. Write "unordered-map types", "wall-clock
  reads", "floating-point types" instead.
- Rust 1.98.1 was installed via rustup in this environment; the exact-pin in
  `rust-toolchain.toml` auto-installs on first use (network needed once).

## 10. Maintenance protocol — every future agent, every milestone

1. Read plan.md §0 + §14, this file, `docs/DEBT.md`, `docs/ASSUMPTIONS.md`.
2. Run the §4 verification. Everything must be green **before** you start (else
   you are inheriting breakage — stop and investigate) and **after** you finish.
3. Implement exactly one milestone (or a logged sub-slice of one). Do not skip
   ahead; do not add unrequested features; do not weaken an exit test.
4. Log as you go: assumptions → `docs/ASSUMPTIONS.md`; simplifications →
   `docs/DEBT.md`; frozen-decision challenges → an ADR in `docs/adr/`.
5. Update this file: status board (§6), inventory (§7), gotchas (§9), and the
   snapshot (§2). An out-of-date handoff is a bug.
6. Commit green, one logical change per commit, messages in the existing style.
7. Honest declaration (plan §13): a criterion that cannot be met honestly is
   recorded as a finding, never waved through.

## 11. Pointer index (plan section → where it lives in the repo)

| Plan section | Where |
|---|---|
| §0–§1 contract & vision | `plan.md`; digested in this file §3 |
| §2 frozen decisions | `plan.md`; changes via `docs/adr/` |
| §3.2 dependency policy | `tests/architecture_law.rs` allow-lists (enforced) |
| §4 layout & law | `docs/ARCHITECTURE.md` + `tests/architecture_law.rs` |
| §5 determinism rules | fx API docs, `clippy.toml`, `tests/architecture_law.rs` scan |
| §6 simulation core | `crates/sim/src/lib.rs` module docs (contract) — code is M1 |
| §7 entity model | `crates/sim_api/src/lib.rs` (ids exist); rest is M1 |
| §8 commands | `crates/sim_api` (M1) |
| §9 systems | per-milestone; see status board §6 |
| §10 content | `docs/CONTENT_GUIDE.md`, `content/` (empty until M2) |
| §11 engine/client | `crates/engine`, `crates/client` stubs (M3) |
| §12 tools | `crates/tools` stub (M1 for replay-verify scaffold) |
| §13 acceptance | `tests/` (A13 scaffold live; A1 arrives with M1) |
| §14 milestones | this file §6 status board |
| §15–§19 budgets/risks/debt | `plan.md`; debt live in `docs/DEBT.md` |
