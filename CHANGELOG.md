# Changelog

All notable changes to Pandemonium: Build & Destroy are recorded here, one
entry per milestone (the project ships milestone-by-milestone; see
`plan.md` §14 and `AI-Handoff.md` §6 for the live status board). The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); this project
does not use SemVer yet (the Alpha is undeclared), so each entry is keyed to
its milestone tag.

## [Unreleased] — pre-M10 cleanup

A review pass over M0–M9.1 in preparation for M10 (Stabilization &
declaration). No frozen decision changed; no golden hash moved; no
dependency bumped.

- **ci**: `Swatinem/rust-cache@v2` added to every CI job, keyed on
  `Cargo.lock` — cuts typical CI time roughly in half on a cache hit.
- **ci**: a separate `.github/workflows/audit.yml` runs `cargo audit` on
  every `Cargo.lock` change and nightly.
- **docs**: refreshed `docs/ARCHITECTURE.md`'s status header from M8 to
  M9.1; added `tests/alpha_loop.rs` to its test list.
- **docs**: per-constant doc comments on the scripted-opponent tuning
  block in `crates/ai/src/scripted.rs`.
- **community**: added `CHANGELOG.md`, `SECURITY.md`, `.editorconfig`,
  `.github/PULL_REQUEST_TEMPLATE.md`, and
  `.github/ISSUE_TEMPLATE/bug_report.md`.
- **refactor**: `crates/content/src/bundle.rs` — `map_passability` and
  `map_buildability` now share one helper; output bytes unchanged.
- **fix**: `crates/engine/src/host.rs` — the A15 "checkpoint trail"
  assertion was a no-op (`assert_eq!(x.len(), x.len())` plus an empty
  loop); replaced with a real `assert_eq!(oa.hashes, ob.hashes)`.
- **test**: `crates/fx/tests/properties.rs` — added property tests for
  `Vec2Fx::dist` ↔ `(a - b).len()`, `Fnv1a64` integer-write LE encoding,
  and `Fx::abs`/`Fx::square` contracts.

## [M9.1] — input hotfix (DEBT-008 human pass findings)

The first human playtest of the windowed client (the project owner, on a
real display) reported every input-layer defect the machine pass could
not see. The simulation itself was machine-verified through M9 and
untouched.

- Camera pan axes had been mirrored since M3 — D panned left, A right, W
  retreated, S advanced. Corrected, with direction-pinning tests at yaw 0
  and 90°.
- Mouse could not move the camera at all — plan §11.3's edge pan and
  zoom-toward-cursor were unimplemented, and the wheel direction was
  inverted. All wired, with frame-rate-independent pan speed.
- Right-click only ever issued `Move` — the client had no path to order an
  attack. Added `client::orders` context resolution: enemy → Attack,
  node → Gather (for workers), ground → Move.
- 'A' fired attack-move on key-down *and* on every OS key repeat while
  also being a pan key. Now 'A' arms attack-move for the next left-click,
  with key-repeat guards; Esc/right-click cancels.
- Rejected orders were silent — `CommandRejected` events were dropped.
  Now refused orders flash a red square at the click point plus a HUD
  reason line; the smoke run pins the wiring with a deliberately invalid
  order.
- The opening camera framed the map center at 58 tiles out — the starting
  force read as specks. Now focused on the player's start at 26 tiles,
  re-framed on restart, with map-bounds clamping.
- `frames_presented` only advanced in `--frames` verification mode, so
  M9's flashes and pings never expired and accumulated forever in normal
  play. The feedback clock now advances every presented frame.

Zero golden movement — the hotfix is 100% presentation/input. 19 new
tests (7 camera, 8 context-resolution, 4 refusal/bracket). DEBT-008
narrowed to the human *re-verification* of the fixed build.

## [M9] — Alpha content & feel pass

The M8 gap closed: the loop resolves, the AI presses, and the client
feels like a game.

- **fix(sim)**: a chase order whose commanded target died no longer
  froze both armies — combat stage 1 pops it now (the "frozen-army"
  bug that kept AI matches from resolving). Two unit tests pin the
  behavior.
- **ai**: the wave machine re-marches on the pressure cadence while a
  live wave is out (a wave never parks) and focuses the sighted enemy
  command center. Waves of ten / army cap sixteen — matches resolve
  in 9.1k–23.3k ticks across a 32-seed sweep.
- **client**: the feel pass — hit flashes (entities flash toward hot
  white when hit), health bars over damaged units, command-acknowledgment
  pings at the clicked ground.
- **engine**: the `AudioSink` seam — a pure event → cue mapping, a null
  counter sink, the placeholder cue set (plan §11.5). No audible backend
  (DEBT-011).
- **test**: `tests/alpha_loop.rs` — the M9 exit suite: resolution within
  the fifteen-minute budget across seeds, end-to-end resolution
  determinism, and the windowed host's vs-AI loop closing naturally.

The demo hash is untouched (its lone Attack command is gate-refused on
the trivial world, so the chase-order pop never fires there). The
AI-vs-AI flagship re-pinned twice with written reasons (the chase-order
pop, then the AI tuning).

## [M8] — Match rules & full loop (P5)

Stage 10 (plan §6.3.10) lands: defeat = zero owned Footprint structures
OR resigned; victory = one survivor; `MatchEnded` fires exactly once
(idempotent — `Sim::outcome` caches the result).

- **sim**: `match_rules.rs` — the defeat check only fires in
  structure-bearing matches (A-067) so the M1 spine-test fixture and the
  M3 headless smoke stay green. The outcome is derived state (A-066: not
  part of the canonical hash) — the M7 goldens stay green by
  construction. No `STATE_ENCODING_VERSION` bump.
- **engine**: `MatchHost` extended with optional AI controllers
  (`with_controllers`), a command log (`log()`), and the match outcome
  (`outcome()` / `is_finished()`).
- **client**: hosts the vs-AI opponent through the same seam, renders the
  end-screen panel (VICTORY/DEFEAT/MUTUAL DESTRUCTION), supports restart
  (R), control groups (1-9), and Stop/AttackMove hotkeys (S/A).
- **tools**: the `--p1 ai --p2 ai` runner prints a `match ended:` line.
- **test**: `tests/match_rules.rs` — A15 restart cleanliness (two fresh
  hosts from the same seed → identical hashes; with AI controllers →
  identical logs too); stage 10 is hash-neutral (the M7 golden holds
  with match rules active).

## [M7] — AI through commands (P4)

The scripted Alpha opponent. Parity is structural, not a policy.

- **ai**: the `Controller` trait (plan §9.6) + `ScriptedController` —
  workers → depot → barracks → mixed army; idle-worker gather
  management; attack waves on size + timer; base defense; pending /
  build bookkeeping verified by sight (never a rejection channel).
- **sim**: `run_command_log` — the one canonical headless re-simulation
  of a command log. Tools' recorder/verifier and the acceptance suites
  all drive through it (DEBT-005 repaid).
- **engine**: `AiMatchHost` — controllers hosted on the tick boundary,
  ascending slot order, every fed command recorded (rejections
  included). Plus `alpha_plan`/`alpha_controller` (capability-shaped
  kind resolution, costs through the Ore seam, ring-scanned build
  ground clear of static claims and node doorsteps).
- **test**: `tests/ai.rs` — A5 (structural audits + ledger identity),
  A11 (mirrored battery + label-blindness + proptest fuzz), AI-vs-AI
  completion + determinism, the golden hash, and the log-alone replay.

## [M6] — Combat & vision (P2)

The immediate-hit attack pipeline and the three-state fog model.

- **sim**: `combat.rs` — acquire → validate → wind_up → hit → mitigate
  → apply → death-via-stage-8 → credit. Cooldowns in ticks; squared-
  distance range checks; acquisition priority (commanded > units-
  attacking-friendly > other units > structures; ties to lower id).
- **sim**: `vision.rs` — per-player per-tile Hidden/Explored/Visible,
  maintained incrementally each tick. `FogState` is derived (NOT hashed
  — A-059). DEBT-004 repaid.
- **sim**: Attack/AttackMove/Stop semantics; the turret kind (no Move,
  Attack+Footprint). DEBT-006/007/010 retired. Encoding v4.
- **content**: the seam maps `Attack` into `CapTemplate::Attack` and
  the runtime `CapabilityData::Attack` block.
- **test**: `tests/combat.rs` (composition + position matter, bit-
  identical run-to-run, turret, A12 combat invariants, legibility) and
  `tests/vision.rs` (A10 fog integrity, targeting both directions,
  three-state transition, FD-8).

## [M5] — Economy & production (P3)

The worker gather loop, production queues, and construction lifecycle.

- **sim**: `economy.rs` (travel → gather timer → extraction capped by
  capacity and node remaining → carry → deposit → repeat; depletion
  removes nodes with same-tick auto-seek; ghost BuildAt orders
  hygiene-cleaned) and `production.rs` (ordered queues, costs paid on
  enqueue and refunded verbatim on cancel, only the head progresses,
  completed items hold at the queue front until population headroom
  returns, ring-tile spawns with rally orders, construction with a
  committed builder completing into Active).
- **sim**: `invariants.rs` — the A12 checker (debug assertions over the
  whole state at the end of every step in debug builds).
- **content**: the world definition carries the buildability grid, the
  faction production lists, the base population cap, and each kind's
  economy stats.
- **test**: `tests/economy.rs` — divergent scripted openings produce
  measurably different timelines; scripted-match determinism; the A12
  economy soak (3 openings × 2 seeds × 900 ticks of both-player soak,
  every invariant held — caught and fixed a real spawn-position bug).

## [M4] — Movement (P1)

The three-layer movement contract: nav grid + A* with deterministic
tie-breaks, waypoint steering with a terrain guard, spatial-hash
push-apart, stuck detection with `MoveFailed`. DEBT-003 repaid.

- **sim**: `nav.rs` (NavGrid + per-tile static-body claim count;
  8-connected A* with no corner cutting, integer milli-tile costs, the
  octile heuristic, the (f, h, tile) total-order heap key) and
  `movement.rs` (the three layers).
- **test**: `tests/movement.rs` — 50 units respond within 2 ticks under
  spam-clicked orders; no permanent stuck units (every order resolves —
  arrival, crowded arrival, or `MoveFailed`); the real-map detour
  (4 workers cross Crossroads around the rock walls, never on blocked
  terrain); push-apart separates stacks and never pushes into terrain.

## [M3] — Engine shell

The windowed 3D client. ADR-0001 (3D presentation over the 2D logical
ground plane) accepted.

- **engine**: `FixedTimestep` (30 Hz, 5-tick catch-up cap), `Interpolator`
  (snapshot blending, spawn/death handling, Q16.16 ↔ ground-plane
  boundary conversions), `MatchHost` (owns `Sim` privately; `submit` +
  `advance` are the only mutation paths), `RtsCamera` (perspective
  orbit, ground picking, box select), terrain mesh, `Renderer` trait +
  `NullRenderer`.
- **client**: winit 0.30 + wgpu 26 windowed 3D renderer (depth buffer,
  terrain mesh with heightmap displacement, instanced placeholder entity
  boxes), selection + right-click Move, fontdue text atlas + HUD/debug
  overlay, `--frames N` windowed smoke, headless fallback for CI.
- **test**: windowed path machine-verified on Xvfb + llvmpipe (selection
  + Move commands provably work). HUD/overlay slice landed (DEBT-009
  repaid). The human visual pass remains open as DEBT-008.

## [M2] — Content pipeline

RON schemas, versioned loaders, validators, content hash.

- **content**: strict RON schema with versioned loaders and forward
  migration (map v1 → v2), precise validators at file and placement
  level, `ContentBundle` with canonical content hash and map id,
  `world()` seam producing the plain `TrivialWorld` the simulation
  receives.
- **tools**: `content-validate` subcommand.
- **test**: `tests/content_pipeline.rs` — §10.4 stat pin, loaded-bundle
  determinism, the A3 add-a-unit scaffold (data-only load + spawn +
  Move; the §10.6 built-and-fights extension is DEBT-007 for M5/M6).

## [M1] — Simulation core

The tick pipeline, entity and capability stores, the command gate, the
state hash, the replay codec, and the determinism proofs.

- **fx**: Q16.16 fixed-point math with 64-bit intermediates, round-
  toward-zero and saturating contracts, exact integer square root over
  the full u64 range, PCG32 RNG with canonical seeding, FNV-1a 64-bit
  hashing — all property-tested.
- **sim_api**: the boundary vocabulary (Command, Event, Snapshot,
  PlayerView, Reject, MatchSetup).
- **sim**: `step()` runs plan §6.3's eleven stages; entity + capability
  stores kept in ascending-ID order by construction; the shared command
  gate (stable (issuer, seq) sort, tick check, per-kind existence /
  ownership / capability / target checks); canonical little-endian
  state hash through `fx::Fnv1a64`. Snapshots and `PlayerView`s project
  entities ascending by id.
- **replay**: a checksummed canonical little-endian codec (record, load,
  validate) with typed errors. No serde — the dependency law forbids it
  here.
- **test**: `tests/determinism.rs` (A1/A2 with pinned golden hashes),
  `tests/architecture_law.rs` (the dependency graph + the source-level
  determinism bans).

## [M0] — Skeleton & guardrails

The workspace, the toolchain pin, the guardrails, and the `fx` crate.

- Workspace (virtual `[workspace]`, resolver 2), shared package
  metadata, `rust-toolchain.toml` pinning 1.98.1.
- `clippy.toml` disallowing unordered-map types workspace-wide.
- CI authored: fmt, clippy `-D warnings`, tests × {Linux, Windows,
  macOS} × {dev, release}, plus the replay round-trip.
- `tests/architecture_law.rs` parses every member manifest on every test
  run, checks the internal edge allow-list and the per-crate external
  allow-list, fails on the forbidden-crate list, and scans the
  determinism crates' sources for banned types. Breaking the
  architecture is a build failure, not a conversation.
