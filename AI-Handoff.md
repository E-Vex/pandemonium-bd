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
| Repo | `github.com/E-VEx/pandemonium-bd` (git, branch `master`; local clone at `/home/z/my-project/pandemonium-bd`) |
| Toolchain | Rust 1.98.1, edition 2021, pinned by `rust-toolchain.toml` |
| Presentation | **3D perspective over the 2D logical ground plane** — the simulation stays 2D fixed-point; 3D is presentation-only ([ADR-0001](docs/adr/0001-3d-presentation.md), 2026-10-01) |
| Status | **M5 (Economy/production/construction, P3) COMPLETE** — resource ledger, worker gather loop with depletion + auto-seek, production queues (Train/Cancel/SetRally) with paid-on-enqueue costs, construction lifecycle (Build + committed builder), population cap, shared requirements checker, footprints blocking tiles, A12 invariant checker; divergent-opening and economy-soak exit tests green (244/244 dev, 239 release — 5 should-panic checker tests are debug-only). **Next: M6 (Combat & vision)** |
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

Run from the repo root (`/home/z/my-project/pandemonium-bd`):

```bash
cargo fmt --all -- --check                 # formatting
cargo clippy --workspace --all-targets -- -D warnings   # lints, zero tolerance
cargo test --workspace                      # all tests (debug)
cargo test --workspace --release            # determinism must hold in release too
cargo run -p pandemonium-tools -- headless --seed 7 --ticks 300
cargo run -p pandemonium-client             # windowed 3D client; headless smoke pass without a display
cargo run -p pandemonium-client -- --frames 900   # windowed smoke: auto-exit + evidence summary
```

Expected at this handoff: all commands succeed; 244 tests pass in dev and
239 in release (the 5 should-panic invariant-checker tests are debug-only by
nature — `debug_assert` compiles out in release): 39 fx unit tests, 15 fx
property tests, 82 sim unit tests (economy/production/construction/invariants
among them), 4 sim_api unit tests, 8 replay codec tests, 12 tools tests,
33 content unit tests, 9 determinism acceptance tests, 4 content-pipeline
acceptance tests (the A3 scaffold now trains the data-defined kind),
3 movement acceptance tests, 4 economy acceptance tests (the M5 exit suite),
28 engine unit tests + 2 mesh tests, 5 client text-atlas tests,
2 architecture-law tests, 1 sim constant test; the headless demo's final hash
is 0xbc86a622e357252d (seed 7, 300 ticks — moved by M5's encoding v3, see
A-050). `tools content-validate content` prints the bundle identity and PASS
(content hash 0x249b69f0ee343a10, map id 0xbc0970c3cf14e9cf — moved by the
contested-node relocation, A-052); `cargo run -p pandemonium-client` prints
the no-display finding and runs the headless smoke pass (on a desktop it
opens the window); with `--frames N` the windowed run exits after N frames
and prints the evidence summary. On a headless machine the windowed path is
verified on Xvfb + llvmpipe per DEBT-008/A-037 (selection + Move commands
provably work; the human visual pass remains).
CI additionally runs fmt + clippy + tests on Linux/Windows/macOS in dev and
release, plus the replay round-trip, when pushed to GitHub
(`.github/workflows/ci.yml`) — see A-010/A-020 below.

## 5. Workspace map (as built; see `docs/ARCHITECTURE.md` for detail)

```text
crates/fx        DONE    Q16.16 fixed-point math, isqrt, PCG32 Rng, FNV-1a hasher
crates/sim_api   DONE    vocabulary: ids, Command/CommandKind, Event, Reject, Snapshot, PlayerView, MatchSetup
crates/sim       DONE    Sim spine + M4 movement + M5 economy: step pipeline, entity + capability stores (11 capability types), command gate w/ economy checks, state hash (v3), trivial-world fixture w/ terrain grids + production lists, nav grid + A* + footprint occupancy, mover, gather loop, production queues, construction, A12 invariant checker
crates/content   DONE    RON schema (strict), versioned loaders + map v1->v2 migration, precise validators, ContentBundle + content hash, world() seam
crates/ai        STUB    empty lib, role documented (lands in M7)
crates/replay    DONE    canonical LE byte codec + checksummed replay record/validate (re-sim driver lives in tools/tests)
crates/engine    DONE*   FixedTimestep, Interpolator, MatchHost (structural FD-2) + pause/single-step, RtsCamera (glam) + picking/box-select, terrain mesh, Renderer trait + null renderer, HudState on the Frame boundary
                   (*engine core)
crates/client    DONE*   winit window + wgpu 26 3D renderer (depth buffer, terrain mesh + heightmap, instanced placeholder boxes), selection + right-click Move, fontdue text atlas + HUD/debug overlay pass, --frames windowed smoke, headless smoke fallback (*DEBT-008 keeps the human visual pass)
crates/tools     DONE    headless, replay-verify, content-validate subcommands (clap CLI)
tests/           ACTIVE  pandemonium-tests: architecture_law.rs (A13) + determinism.rs (A1/A2 + spine proofs) + content_pipeline.rs (M2/A3 + the M5 train extension) + movement.rs (M4/P1) + economy.rs (M5/P3)
content/         DONE    rules/, entities/ (9 kinds), factions/ (Legion), maps/ (Crossroads 64x64, symmetric, heightmap)
docs/            ACTIVE  ARCHITECTURE, DEBT, ASSUMPTIONS, CONTENT_GUIDE, adr/ (ADR-0001 accepted)
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
| M1 | Simulation core | ✅ **complete** | A1/A2 green on the trivial world (two in-process runs + pinned golden hashes; tools replay-verify round-trip PASS; CI matrix makes the 3-OS claim real once it runs, A-020); ID-never-reused test ✓; iteration-order test ✓ |
| M2 | Content pipeline | ✅ **complete** | all Alpha content + the map load (`tools content-validate` PASS; the repo tree is test-pinned); malformed files produce precise errors (every validator has a test); A3 scaffold in place (data-only add-a-unit: load + spawn + Move, sim sources scanned clean; the §10.6 built-and-fights extension is DEBT-007 for M5/M6); map schema v2 with display-only heightmap per ADR-0001, v1 migrates forward; content hash + map id canonical (FNV-1a, plan §5.10) |
| M3 | Engine shell | ✅ **complete** | engine + client implemented and green (183 tests): FixedTimestep loop w/ catch-up cap, interpolation, MatchHost (the step-only mutation guarantee is structural) + pause/single-step, RtsCamera + ground picking + box select, terrain mesh + null renderer, wgpu 26 windowed client w/ depth buffer, terrain + heightmap, instanced placeholder boxes, selection + right-click Move, fontdue text atlas + HUD/debug overlay pass, `--frames` windowed smoke. **Windowed path machine-verified on Xvfb + llvmpipe** (DEBT-008/A-037: 900 frames presented, XTEST drag-box selection of the 5 start entities, 2 Move commands, visible motion); the *human* visual pass remains open in DEBT-008. HUD/overlay slice landed (DEBT-009 repaid) |
| M4 | Movement (P1) | ✅ **complete** | 50 units respond within 2 ticks under spam-clicked orders (per-unit motion check, tests/movement.rs); no permanent stuck units (every order resolves — arrival, crowded arrival, or MoveFailed; unreachable/zero-speed fail immediately); hashes green (A1 + regenerated goldens, encoding v2); real-map detour: 4 workers cross Crossroads around the rock walls, never on blocked terrain; push-apart separates stacks and never pushes into terrain (DEBT-003 repaid) |
| M5 | Economy/production/construction (P3) | ✅ **complete** | divergent scripted openings produce measurably different timelines (worker-heavy vs early-Raider: worker counts, barracks, Raiders fielded, income, balances, hashes branch ≤ tick 60); scripted matches bit-identical run-to-run; A12 invariant checker fires every tick in debug and 3 openings × 2 seeds × 900 ticks of both-player economy soak hold every invariant; the soak caught and fixed a real spawn-position bug |
| M6 | Combat & vision (P2) | ⬜ pending | composition/position matter; legibility checklist; A10 fog integrity green |
| M7 | AI through commands (P4) | ⬜ pending | A5 + A11 green; AI-vs-AI headless matches complete |
| M8 | Match rules & full loop (P5) | ⬜ pending | 10–15 min match vs AI completes and restarts cleanly (A15) |
| M9 | Alpha content & feel pass | ⬜ pending | minimum viable loop playable end-to-end vs the AI |
| M10 | Stabilization & declaration | ⬜ pending | every A1–A15 criterion verified; soak green; docs/ALPHA_DECLARATION.md with evidence |

**The plan was broken into parts along these milestones.** Parts 1–5 (M0–M3,
M4 movement, and M5 economy/production/construction) are complete (M3's
windowed path machine-verified; the human visual pass stays open as
DEBT-008's narrowed scope); parts 6–11 (M6–M10) remain, in strict order.

## 7. M0–M3 inventory — what exists today, concretely

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

**M1 — the simulation core (plan §14, citing §6, §7, §8)**

- `crates/sim_api` — the full boundary vocabulary: `Command`/`CommandKind` (§8.1,
  all ten variants including the v2 queue commands), `Event` (§9.8, the planned
  variant set — M1 emits Spawned/Died/CommandRejected), `Reject`/`RejectReason`
  (§8.2), `MatchSetup`/`PlayerSetup`/`ControllerKind` (§6.5), `Snapshot`/`EntityView`/
  `MoveState` (§6.4), `PlayerView`/`ViewResource` (§9.6), `TilePos`, `EntityId::NONE`.
  Re-exports `Vec2Fx` (A-023).
- `crates/sim` — `Sim::new(world, setup)`, `step(&[Command])` as the only mutator
  (FD-2), running plan §6.3's eleven stages verbatim (unimplemented stages are
  documented no-ops with milestone pointers); monotonic id allocation from 1
  (`next_entity_id()` exposes the watermark); entity + capability stores (Health,
  Move, Vision) as Vecs kept ascending by id — deterministic iteration by
  construction, removal preserves survivor order, binary-search lookups; the
  shared command gate (stable (issuer, seq) sort, tick/issuer/duplicate-sequence/
  existence/ownership/capability/target checks, rejection events, zero state
  change on refusal); per-entity order queues with queue/replace semantics and a
  placeholder straight-line mover (DEBT-003); health advance + death & cleanup
  (regen is the M1 death path, A-015); scheduled spawns as the production
  stand-in (A-016); `state_hash()` — canonical LE, entity-major, capability
  presence bitmask, RNG state, allocator, players, `STATE_ENCODING_VERSION = 1`;
  periodic hash every 30 ticks in `StepOutput`; `snapshot()` and `player_view(p)`
  (circular-radius fog filter, A-018); `TrivialWorld` fixture with `content_hash()`
  (A-013) and §10.2 authoring-unit conversion at load.
- `crates/replay` — `ReplayFile`/`Checkpoint` matching plan §6.5's field list;
  own canonical LE codec (`bytes.rs`) with a trailing FNV-1a checksum (no serde —
  dependency law); `encode`/`decode`/`validate` with typed `ReplayError`s; the log
  records exactly the fed command stream, rejections included (A-019).
- `crates/tools` (clap CLI) — `headless [--seed N --ticks T --record FILE]`: runs
  the scripted demo match, prints every checkpoint + the final hash, optionally
  writes a replay; `replay-verify FILE`: decodes, validates, checks the content
  hash, re-simulates, compares every checkpoint (A2).
- `tests/determinism.rs` — 9 acceptance tests: A1 (two runs, full equality of
  checkpoints/final/events/snapshots/allocator; different seed diverges), pinned
  golden hashes, A2 (codec round-trip, checksum corruption detection, full
  re-simulation equality, content-hash check), id-never-reused (monotonic
  allocation, dead id never returns, post-death ids higher), iteration order
  (snapshots and player views strictly ascending every tick), invalid-commands-
  change-no-state (bit-identical hashes vs a quiet run), motion within one tick
  (A8's sim side), queue replace/append semantics.
- 30 sim unit tests, 4 sim_api unit tests, 8 replay codec tests, 6 tools tests.
- `.github/workflows/ci.yml` — authored during M1 because the file was missing
  from the inherited repo despite A-010 (see A-020): fmt + clippy, the
  3-OS × dev/release test matrix, the replay round-trip, and binary runnability.

**M2 — the content pipeline (plan §14, citing §10; see `docs/ARCHITECTURE.md` "The content pipeline" and `docs/CONTENT_GUIDE.md`)**

- `crates/content` — the RON schema (strict: every struct denies unknown fields;
  unknown fields are errors, plan §10.2) for rules/entities/factions/maps;
  schema-version gates with forward migration (map v1 — the pre-ADR-0001 shape —
  migrates to v2 as a flat map; newer-than-loader versions are precise errors,
  FD-10); precise typed errors (`ContentError`: file + entity/map + coordinates
  + the violated rule); validators at file level (stat ranges, duplicate
  capabilities, cross-references, grid shape, bounds, heightmap shape) and
  placement level per (map × faction): spawn legality, ore reachability
  (8-connected BFS per start), and the declared 180-degree symmetry check
  (grid + ore tile multiset + mirrored anchors). serde + ron live here and only
  here (plan §3.2, enforced by the architecture law).
- `ContentTree` / `ContentBundle` — the bundle carries the canonical content
  hash and map id (FNV-1a over an explicit little-endian encoding, plan §5.10;
  the display-only heightmap participates — content identity covers the whole
  bundle) and `world()` produces the plain `TrivialWorld` the simulation
  receives: `Sim::new(&bundle.world(), setup)` is the loaded-bundle path with
  **zero changes under crates/{sim,sim_api,fx,ai}** (A-024). Initial spawn
  order (therefore entity ids): map starts in authored order → each start's
  forces in faction order → ore nodes in map order; positions are footprint
  centers; jitter 0 for loaded content (A-028).
- `content/` — the Alpha manifest as data (plan §10.4 exact, test-pinned cell
  by cell): rules (Ore, 200 start, elimination + resignation), nine entities
  (worker/rifleman/raider/guardian, CC 4×4, barracks 3×3, supply depot, turret,
  ore node 2×2/1500), the Legion (roster, CC+barracks production lists, 1 CC +
  4 workers starting forces), and Crossroads (64×64, symmetric + verified,
  central crossroads choke, 4 ore per start + 4 contested center nodes,
  display-only heightmap — ADR-0001).
- `tools content-validate [PATH]` — strict validation + the bundle identity
  report; the repo's own tree is validated by a tools unit test.
- `tests/content_pipeline.rs` — the M2 acceptance suite: §10.4 stat pin,
  loaded-content starting state (22 entities, 200 Ore per player),
  loading/match determinism, and the A3 add-a-unit scaffold (a new kind defined
  purely by data files loads, spawns, and obeys a Move order; nothing under
  `crates/sim/` mentions it — source-scanned in the test).
- 31 content unit tests, 2 tools tests, 4 acceptance tests (150 total at the M2
  exit; dev and release identical).

**M5 — economy, production, construction (plan §14, citing §9.3, §9.4; see
`crates/sim/src/economy.rs`, `production.rs`, `invariants.rs`,
`tests/economy.rs`, and A-041..A-055 for the semantics)**

- `economy.rs` — stage 4: the worker gather loop as a phase machine (travel to
  the node through stage 6's shared travel targets, gather timer, extraction
  capped by capacity *and* node remaining, carry, deposit at the nearest
  completed storage with `ResourceDelivered`, repeat). Depleted nodes are
  removed (`NodeDepleted`) with their tiles released and affected workers
  auto-seeking the nearest same-resource node the same tick; no nodes left
  drops the order. Ghost `BuildAt` orders are hygiene-cleaned. The approach
  test (nearest footprint-rect point within 1500 milli + body radius) is
  deliberately generous so economy travel needs no crowd machinery (A-043).
- `production.rs` — stage 3: producers' ordered queues (cost paid on enqueue,
  refunded verbatim on cancel, only the head progresses, completed items hold
  at the front when population headroom is missing), spawns on the first free
  ring tile with a rally `MoveTo`, construction progress with a committed
  builder completing into `Lifecycle::Active` (builders' orders popped), the
  population usage/cap recompute (match start, end of stage 3, after deaths),
  and the one requirements checker (completed entities of the required kind,
  owned) shared by both surfaces.
- `command.rs` — the economy gate: Train/Cancel/SetRally/Build validation
  reading the fixture through the seam (capability, kind, faction production
  list, requirements → afford → population; placement = in-bounds + buildable
  + unclaimed + no mover standing), with Build spawning the site inline
  (occupying its tiles immediately, exact placement — no jitter). Rejection
  mappings per A-055.
- `invariants.rs` — the A12 checker: every invariant asserted at the end of
  every step in debug builds; five should-panic tests prove it fires; the
  strict `pop <= cap` reading is flagged for M6 (A-049, DEBT-010).
- The state encoding moved to **v3** and the fixture encoding to **v3** (the
  economy capability blocks, `GatherAt`/`BuildAt` orders, `UnderConstruction`
  lifecycle, kind economy, production lists, buildability, base cap) — the
  determinism goldens regenerated (A-050); the tools headless final hash
  moved to 0xbc86a622e357252d (seed 7, 300 ticks).
- Crossroads' four contested plaza nodes moved one tile outward (A-052): with
  footprints blocking tiles, the old doorstep positions sealed the only
  north-south crossing (independently verified by BFS); the content hash and
  map id moved accordingly.
- `tests/economy.rs` — the exit suite (divergent openings, determinism, the
  A12 soak, soak determinism); `tests/content_pipeline.rs` grew the A3/DEBT-007
  train-from-barracks half.

**M3 — the engine shell (plan §14, citing §11 as amended by ADR-0001; slices 1–2,
see DEBT-008/009 for what remains)**

- `crates/engine` — `FixedTimestep` (30 Hz accumulator, 5-tick catch-up cap,
  spiral-of-death backlog drop, `alpha()`); `Interpolator` (prev/current
  snapshot blending: lerp positions/facing, spawns appear without backward
  extrapolation, deaths drop, discrete fields from current; the Q16.16 ↔
  ground-plane boundary conversions); `MatchHost` — owns the `Sim` privately,
  so submit(commands) + advance(dt) are the only mutation paths (the M3 exit
  criterion as a compile-time property; commands are tick-stamped for the
  next step, so the gate never rejects them; the initial Spawned batch
  surfaces through the first advance); `RtsCamera` (glam: perspective orbit,
  pan/zoom/rotate with clamps, ground-plane ray picking via the inverse
  view-projection, NDC projection, screen-space box selection);
  `mesh::terrain_mesh` (one quad per tile, heightmap displacement, class
  colors — pure data, GPU-free, tested); `Renderer` trait + `NullRenderer`.
- `crates/client` — winit 0.30 window + wgpu 26 renderer with a depth buffer:
  terrain pipeline (the mesh), entity pipeline (36-vertex unit cube, instanced
  per entity: position/half-extent/color, team colors, selection brightening),
  the UI overlay pipeline (screen-space pixels → NDC, glyph atlas as R8Unorm,
  alpha blending, on top of the world), WASD pan + wheel zoom + left click /
  drag-box selection (own units) + right-click Move via ground picking
  converted to fixed point, Escape exits. F3 toggles the §11.6 debug overlay,
  P pauses, `.` single-steps; the HUD (resources by data-defined display name
  + POP) is always visible. `--frames N` exits after N frames with the
  evidence summary. Without a display the binary prints the finding and runs
  the headless smoke pass (real content, 180 frames, state + content hashes,
  exit 0) so CI's every-binary-starts check stays green.
- `crates/client/src/text.rs` — our own text renderer (plan §3.1/§3.2):
  fontdue rasterizes the embedded "Pandemonium Sans" (ASCII subset of DejaVu
  Sans, renamed per the Bitstream Vera license) once into a coverage atlas;
  layout is plain client-side math (5 unit tests pin the packing invariant,
  draw order, baseline conventions, and the fallback box).
- 28 engine tests; 5 client tests; workspace 183/183 in dev and release.

**M4 — movement (plan §14, citing §9.1; see `crates/sim/src/nav.rs` and
`movement.rs`, tests/movement.rs, and A-039/A-040 for the semantics)**

- `nav.rs` — the navigation layer: `NavGrid` over the world's passability
  (the content seam maps the map's terrain classes into it); 8-connected A*
  with no corner cutting, integer costs, the octile heuristic, and the
  total-order `(f, h, tile)` heap key (deterministic pops → deterministic
  paths); the straight beeline validated by an integer supercover of the
  exact segment (corner crossings checked like A* diagonals); tile-center
  routes prefixed with the start tile's center and suffixed with the exact
  goal; blocked/unreachable goals resolve to the nearest reachable tile,
  sealed starts to `None`.
- `movement.rs` — stage 6 as the three layers: path requests (immediate
  `MoveFailed` for unreachable goals and zero-speed distant orders), waypoint
  steering with a terrain guard, spatial-hash push-apart (one-tile cells,
  id-ordered pair resolution, symmetric terrain-guarded halves), stuck
  detection with repath escalation and a body-aware crowded arrival.
  `MoveDef` carries radius + path + counters (encoded in the state hash v2);
  replacing orders reset the runtime path.
- Exit tests (tests/movement.rs): 50-unit spam-click responsiveness, order
  resolution bounds, determinism, and the real-map detour; sim crate +21
  tests; workspace 208/208 in dev and release.

## 8. What is NOT built yet

M3, M4, and M5 are complete: the engine shell with its machine-verified
windowed client, the HUD/debug overlay slice (DEBT-009 repaid), the
three-layer movement system (DEBT-003 repaid), and the economy stack —
ledger, gather loop, production queues, construction lifecycle, population,
requirements, footprints blocking tiles, and the A12 checker (DEBT-006
narrowed to Attack-only, DEBT-007 narrowed to the fight half). What remains,
in order:

1. **The human visual pass of DEBT-008 (small)**: on a desktop display, eyeball
   `cargo run -p pandemonium-client` — the map + entities render from a live
   sim, selection and right-click Move work (both already machine-proven);
   confirm the visuals read well (colors, legibility) and close the row.
2. **M6 — Combat & vision (P2)**: the attack pipeline (acquire → validate →
   hit → mitigate → damage → death → credit), Attack/AttackMove/Stop
   semantics, the Turret, the three-state fog model, targeting filters, and
   the last carried-but-unmapped capability (Attack — DEBT-006's remainder).
   The A12 `pop <= cap` strictness needs its combat decision then (A-049,
   DEBT-010), and DEBT-007's fight half extends the A3 test.
3. Known limitations to carry forward honestly: the formation-less jam shape
   (A-040), path-smoothing-free staircases on detours, stalled construction
   sites when the builder dies (no reassignment command — A-045), gather
   targeting without fog filtering (A-048), and economy scripts that do not
   queue-cancel under pressure.

Nothing of the AI (M7) or match rules (M8) exists beyond stubs, and the
remaining M1 scopes are the deferred capabilities (DEBT-004, and the M6
halves of DEBT-006/007).

## 9. Sharp edges and gotchas discovered during M0–M3

- **fontdue reports y-up metrics**: `Metrics::ymin` counts upward from the
  baseline, so a y-down screen-space top edge is `baseline - (ymin + height)`
  (see `text.rs`'s `bearing_y`). Getting this backwards puts glyphs below
  their baseline — the descender test pins the convention.
- **A glyph-atlas coverage buffer must cover exactly `width × height` bytes**
  before `write_texture`: build it with `resize`, not `truncate` (truncate
  never grows; a one-row-short buffer fails wgpu's bounds validation at
  frame one — the first Xvfb run caught exactly this).
- **The wgpu GL backend on a headless box needs a userland EGL stack** (no
  root): `apt-get download libegl1 libegl-mesa0 libgles2 libxtst6
  libxkbcommon-x11-0 libxcb-xkb1` + `dpkg -x`, `LD_LIBRARY_PATH` at the
  extracted `usr/lib/x86_64-linux-gnu`, glvnd via
  `__EGL_VENDOR_LIBRARY_DIRS`, and *unversioned* `libxkbcommon*.so` symlinks
  (xkbcommon-dl dlopens the unversioned names). Then Xvfb +
  `WGPU_BACKEND=gl` + `LIBGL_ALWAYS_SOFTWARE=1` renders on llvmpipe, and
  XTEST (`libXtst`) injects real mouse input (see A-037).
- **Integer segment traversal is a sign-party**: comparing grid-line
  crossings by cross-multiplication is only order-correct when `dx` and `dy`
  share a sign — use step-signed distances (`dist_x * |dy|` vs `dist_y *
  |dx|`) instead. And a goal that sits *exactly on a lattice corner* makes a
  naive traversal step diagonally past the end tile: stop at any crossing at
  or past the goal. Both bugs shipped and were caught by tests
  (`beelines_work_in_every_quadrant_direction`, the Crossroads detour test).
- **Units funnel when lanes converge**: multiple movers sharing tile-center
  waypoints (same start tile, or walled funnels) jostle and can escalate to
  `MoveFailed`; the straight beeline's divergent lanes are also the crowd
  mitigation (A-040). Keep group-click tests on compact-block shapes.
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
- **Clippy 1.98 new-style lints bite old idioms** (all `-D warnings` failures):
  `x % n == 0` → `x.is_multiple_of(n)`; `map_or(true, …)` → `is_none_or(…)`;
  `[b'P', b'D', …]` → `*b"PD…"`. Write the new forms directly.
- **A command log must stay chronological** (weakly ascending tick order): a
  replay feeds every command at its *declared* tick, so a genuinely stale-tick
  command cannot exist in a recorded log — stale arrivals are a live-client
  phenomenon only. Keep testing TickMismatch by feeding `step()` directly.
- **Feed order within a tick matters for duplicate (issuer, seq)**: the gate's
  stable (issuer, seq) sort preserves feed order for exact ties, and the first
  command wins. Replay drivers must sort by tick *only* (stable) and never
  re-sort the whole log, or duplicate resolution diverges.
- **Borrow checker vs. per-system iteration**: iterating `&mut world.entities`
  while reading `world.movement` is fine (disjoint field paths) but NOT through
  `&World` methods. The mover collects `(id, speed)` pairs first, then mutates —
  keep that shape when adding systems.
- **Fx::from_milli rounds toward zero**, so per-tick speeds converted from
  milli-tiles/s are floor-ish and exact-arrival logic compares `dist <= speed`
  (snap to target) rather than assuming divisibility.
- **Events emitted during `Sim::new`** (the initial `Spawned` batch) sit in the
  buffer until the first `step` drains them — expected, tested, do not "fix".
- **The banned-token scan is a substring scan**: "**Instant**-hit" (plan §9.2's
  own wording!) trips the wall-clock `Instant` ban in the determinism crates.
  Write "immediate-hit"; check `crates/content` docs with the scan in mind
  (M2 hit this once).
- **RON strict-mode error wording** is "Unexpected field named `x` in `Struct`,
  expected one of …" — assert on "Unexpected field" + the field name, not on
  serde's classic "unknown field" phrasing.
- **Serde maps in content must deserialize into ordered maps** (`BTreeMap`) —
  unordered map types are banned workspace-wide (clippy.toml) and would also
  break load determinism.
- **Directory iteration order is OS-dependent** — the content loader sorts each
  category by file name before parsing; keep that invariant if you touch
  `loader.rs` (plan §5: iteration order is determinism).
- **Raw strings containing `"#"`** (terrain codes!) need `r##"…"##` delimiters —
  `r#"…"#` terminates at the first `"#`. Bit the map test factory once.
- **`CARGO_MANIFEST_DIR` depth differs per crate**: tests-package paths need one
  `.parent()` to reach the root, tools-package paths need `ancestors().nth(2)`.
  Wrong depth silently points at `crates/content` instead of `content/`.
- **anyhow's default Display shows only the outer context** — in tests, format
  with `{err:#}` to see the underlying `ContentError` chain.
- **`Fx::raw()` vs authored milli-tiles**: Q16.16 raw values are ×65536; assert
  positions via `Fx::from_milli(milli)` equality, never raw integers, unless you
  enjoy arithmetic slips.
- **Tile centers are `x * 1000 + 500` milli-tiles** — the soak caught a
  spawn-position helper that doubled the conversion (`x * 2000 + 1000`) and
  placed player 1's first trained worker at (95, 95) on a 64-tile map. The
  A12 checker is what flagged it; write tile math once and reuse it.
- **Economy orders are long-lived**: movement pops only `MoveTo` on path
  completion, and stuck detection skips empty-path (in-place work) movers.
  A new economy phase must clear the runtime path itself or the worker stands
  on a stale lane. Never compare a path's *terminus* to a blocked goal's
  center — paths legitimately end on the nearest reachable tile, and the
  mismatch loop (clear + rebuild every tick) freezes the worker on the
  start-waypoint no-op leg.
- **Debug-only `#[should_panic]` tests must be `#[cfg(debug_assertions)]`** —
  `debug_assert` compiles out in release, so un-gated should-panic tests fail
  there (the M5 checker tests carry the gate).
- **Same-script soak runs need player-relative build spots** — both players
  running one script with a hardcoded placement tile fight over the same
  ground; mirror the spot for player 1.
- **Footprint blocking reshapes old scenarios**: the M4 real-map exit test
  recalibrated its arrival bound (1100 → 2800 milli, the blocked-destination
  packing envelope) and gained a no-footprint-tile assertion; the map's
  contested nodes moved a tile because the old spots sealed the crossing.
  Content geometry + blocking semantics must be re-validated together.
- **Entity ids in tests follow the documented spawn order** — off-by-one kind
  or entity ids are the recurring test-authoring bug this milestone (kind
  lists reorder when a new kind inserts mid-vector; check `bundle.entities`
  positions, never hand-count).
- **The `is_none_or` / `is_multiple_of` new-style lints** now also bite test
  code (cooldown sentinels as `u32::MAX / 2` fight `saturating_add`; use
  `Option<u32>` + `is_none_or`).

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
| §6 simulation core | `crates/sim/src/` — `sim.rs` (pipeline), `world.rs` (stores), `hash.rs` (state hash), `fixture.rs` (trivial world) |
| §7 entity model | `crates/sim/src/world.rs` + capability stores; `crates/sim_api` ids |
| §8 commands | `crates/sim_api/src/lib.rs` (types) + `crates/sim/src/command.rs` (gate + application) |
| §9 systems | per-milestone; see status board §6 (M1 spine; M4 movement; M5: economy.rs + production.rs + invariants.rs) |
| §10 content | `docs/CONTENT_GUIDE.md`, `content/` (live), `crates/content/src/` (schema/version/loader/defs/validate/bundle) |
| §11 engine/client | `crates/engine` (clock, interpolate, host + pause/single-step, camera, mesh, renderer + HudState) + `crates/client` (wgpu renderer, UI overlay pass, input, text.rs) — 3D per ADR-0001; DEBT-008 keeps only the human visual pass |
| §12 tools | `crates/tools` — headless, replay-verify, content-validate live |
| §13 acceptance | `tests/` — A13 live; A1/A2 + spine proofs (M1); A3 scaffold + §10.4 pin (M2, `content_pipeline.rs`) |
| §14 milestones | this file §6 status board |
| §15–§19 budgets/risks/debt | `plan.md`; debt live in `docs/DEBT.md` |
