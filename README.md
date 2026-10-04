<div align="center">

# Pandemonium: Build & Destroy

**A deterministic real-time strategy foundation, written from scratch in Rust.**
No game engine, no ECS framework, no off-the-shelf simulation — the loop, the
entity store, the fixed-point math, the replay codec: all of it is ours.

The name promises chaos. The simulation declines to deliver it. A match is a
seed, a content hash, and an ordered command log — replay it anywhere and every
checksum must match, tick for tick. Units may scatter. The simulation does not.

[![CI](https://github.com/E-Vex/pandemonium-bd/actions/workflows/ci.yml/badge.svg)](https://github.com/E-Vex/pandemonium-bd/actions/workflows/ci.yml)
![toolchain](https://img.shields.io/badge/toolchain-1.98.1_pinned-9E6A03?labelColor=21262D)
![stage](https://img.shields.io/badge/stage-M8_match_rules_&_full_loop_done-9E6A03?labelColor=21262D)
![sim floats](https://img.shields.io/badge/sim_floats-0_%28enforced%29-9E6A03?labelColor=21262D)

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/hero-dark.svg">
  <img src="assets/hero-light.svg" alt="Pandemonium theater display rendered in 3D: on the left, scattered debris and an amber strike reticle labeled CONTACTS — UNSORTED; in the centre, three tanks follow dashed movement paths to a hostile target diamond while an F-16-style fighter jet with a lit afterburner and vapor trails fires a missile that locks onto it (TGT LOCK); on the right, build-order footprints and a neat row of contacts labeled CONTACTS — SORTED. A tick ruler along the bottom runs to T+300 with a checkpoint every 30 ticks. Instrument labels read simulation feed live, replay verify pass, seed 7, 30 ticks per second, and Q16.16 fixed-point, no floats." width="100%">
</picture>

</div>

---

## What is Pandemonium

Pandemonium is not yet a game — it is the foundation a game grows from. The
repository holds a nine-crate Rust workspace implementing a deterministic
fixed-tick simulation (30 ticks per second), an intent-only command interface, a
checksummed replay codec, and the developer tooling to prove all three. The
content pipeline, the windowed client, and the AI opponent are specified and
scheduled, and are built in that order. Nothing here is aspirational hand-waving:
the operating contract says "prove, don't assert," and the technical claims on
this page are backed by tests that exist in the repository.

"Own engine" is meant literally. We write the game loop and timing, the
entity and capability stores, the simulation core, the fixed-point math library,
the deterministic RNG, pathfinding and steering, the command and replay systems,
the content loader and validators, the renderer abstraction, the camera, the
input mapping, and the UI toolkit. Only low-level crates for OS and GPU access
(winit, wgpu) are used at the edges. Game engines, ECS frameworks, physics and
pathfinding crates, and the `rand` ecosystem are not merely avoided — they are
forbidden by name, and adding one fails the build.

The design rests on four blocks, chosen so that everything later grows through
them instead of around them:

- **The simulation loop** — a fixed 30 Hz tick that is the only way state advances.
- **The entity model** — one base record plus composable capabilities, 64-bit IDs
  that are never reused, systems that iterate by capability rather than by name.
- **The command system** — every player, AI, and replay action enters as a
  validated `Command` applied at a tick boundary, through one shared gate.
- **The data boundary** — stats, costs, maps, and factions are versioned data
  files; tick order and command semantics are code, and never vary per item.

## Why this exists

The governing question (plan §1): *are we building a game, or a foundation that
can become a much larger game?* The second. Early Minecraft had few features,
but its block abstraction absorbed years of growth; Pandemonium's equivalent
abstractions are the four blocks above. The long arc — new units, new armies,
new maps, new mechanics, better AI, expanded multiplayer — only stays cheap if
each arrow passes through the architecture instead of fighting it.

Rigor is spent where change is expensive: the simulation, the commands, entity
identity, and the data boundary. Visuals, UI layout, and balance numbers stay
deliberately lightweight — placeholder art is policy, not poverty. The Alpha is
explicitly *not* judged on unit count, graphics, campaign, online features, or
balance depth; a proposal that mostly improves those is out of scope by rule,
not by mood.

## Frozen decisions

Ten foundational decisions are frozen (plan §2). Changing one requires a written
ADR with evidence in `docs/adr/` — never a silent edit. Together they are the
difference between "deterministic" as a buzzword and determinism as a property
you can test.

| | Decision | Consequence |
|---|---|---|
| **FD-1** | Deterministic fixed-tick simulation, 30 ticks/s, decoupled from rendering | A match = seed + content hash + ordered command log; replays, soak tests, and lockstep follow |
| **FD-2** | All intent enters as validated `Command` records at tick boundaries | Only the tick function may mutate simulation state |
| **FD-3** | Unified entity model: base record + composable capabilities; monotonic 64-bit IDs | New entity types are data; systems iterate by capability |
| **FD-4** | Strict data/code boundary; versioned, validated content files | Add-a-unit and add-a-map require zero engine changes |
| **FD-5** | No floating point in the simulation or in content | Bit-identical results across OS, CPU, and compiler |
| **FD-6** | The simulation depends on nothing above it — no renderer, clock, or I/O | The renderer and UI can be replaced without touching rules |
| **FD-7** | The AI plays through commands and a fog-filtered view, like everyone else | Parity is structural — cheating is a compile error, not a policy |
| **FD-8** | Fog filters information; it never alters the simulation | Replays and AI stay honest; hidden entities behave identically |
| **FD-9** | Events flow outward; presentation never polls simulation internals | The feedback layer grows without simulation changes |
| **FD-10** | Content schema only moves forward | Content survives years of growth |

Determinism is enforced mechanically, not ceremonially: no floating-point types,
no unordered maps in state or state-affecting iteration, no wall-clock reads, no
OS entropy, no pointer-width values in hashed state — each banned by clippy
lints *and* a line-by-line source scan that runs with the test suite. Randomness
is allowed, but it is seeded, owned by the simulation, and advanced in a fixed
order. No entropy without paperwork.

## Architecture

Dependencies point downward only. This is not a convention enforced by review —
`tests/architecture_law.rs` parses every member manifest on every test run,
checks the internal edge allow-list and the per-crate external allow-list, fails
on the forbidden-crate list, and scans the determinism crates' sources for
banned types. Breaking the architecture is a build failure, not a conversation.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/architecture-dark.svg">
  <img src="assets/architecture-light.svg" alt="Layered dependency diagram: client, engine, content, ai, replay, sim, sim_api, fx, tools, and tests. Solid arrows show dependencies pointing downward; a dashed red crossed arrow marks that ai may never depend on sim; milestone status tags mark sim, sim_api, replay, fx, and tools as complete at M1. A legend states that the full edge list is machine-checked by tests/architecture_law.rs." width="100%">
</picture>

The shape the whole design hangs on is FD-2: `Sim::step(commands)` is the only
mutator of simulation state, and the simulation knows nothing above itself.
Everything the outside world learns — snapshots, events, fog-filtered
`PlayerView`s — flows out through typed boundaries. The client never touches the
simulation directly; it submits commands and renders snapshots. Because every
issuer goes through the same gate, the player, the AI, and a replay share one
code path by construction — and so would a future network peer.

### The shape of time

The simulation is a clock, not a frame counter. It advances at exactly 30 Hz;
rendering runs at display rate and interpolates between the previous and current
snapshots. Time originates in the engine and the client — never inside the
simulation.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/timestep-dark.svg">
  <img src="assets/timestep-light.svg" alt="Two timelines: the simulation track ticks at a fixed 30 Hz with 33.33 ms intervals; the render track runs denser at display rate. A highlighted band shows the client interpolating between two snapshots by alpha = accumulator / tick duration. A note marks the catch-up cap of five ticks per frame." width="100%">
</picture>

### Inside a tick

The fixed update order inside `step()` never varies per content item (plan §6.3).
At M8 all eleven stages are live and the systems reflect their milestone status:

| # | Stage | State at M8 |
|---|---|---|
| 1 | Apply commands — sort by (issuer, seq), validate, apply or reject | **live** |
| 2 | Orders — resolve current orders into system intents | **live** (M4/M5/M6) |
| 3 | Production & construction — queues, progress, spawns | **live** (M5) |
| 4 | Economy — gathering, delivery, storage, spending | **live** (M5) |
| 5 | Target acquisition — combat auto-acquire within `acquire_range` | **live** (M6) |
| 6 | Movement — path requests, steering, collision | **live** (M4) |
| 7 | Combat — immediate-hit pipeline: acquire → validate → hit → mitigate → apply → credit | **live** (M6) |
| 8 | Death & cleanup — health advance, lifecycle events, removal, clear dead targets | **live** |
| 9 | Vision — per-player per-tile three-state fog (Hidden/Explored/Visible) | **live** (M6) |
| 10 | Match rules — defeat/victory/resignation evaluation; `MatchEnded` fires once (idempotent) | **live** (M8) |
| 11 | Finalize — tick++, flush events, periodic hash every 30 ticks | **live** |

## Current status

**Milestones M0 through M8 are complete. M9 (Alpha content & feel pass) is next.** The live status board is
[`AI-Handoff.md`](AI-Handoff.md); an out-of-date handoff is treated as a bug.

Built and verified through M8:

- **`fx`** — Q16.16 fixed-point math with 64-bit intermediates, round-toward-zero
  and saturating contracts (identical in debug and release), exact integer square
  root over the full u64 range, PCG32 RNG with canonical seeding, FNV-1a 64-bit
  hashing — all property-tested.
- **`sim`** — the tick pipeline (all 11 stages live, including M8's match rules);
  entity and capability stores kept in ascending-ID order by construction
  (12 capability types); the shared command gate covering all command kinds
  including Attack/AttackMove/Stop with rejection events and zero state change
  on refusal; canonical little-endian state hash (v4); snapshots and
  fog-filtered player views backed by the per-tick three-state fog cache;
  `outcome()` / `is_finished()` surfacing stage 10's `MatchEnded` (M8 — derived
  state, not hashed, so the M7 goldens stay pinned).
- **`content`** — strict RON schema with versioned loaders and forward migration
  (map v1→v2), precise validators at file and placement level, `ContentBundle`
  with canonical content hash and map id, `world()` seam producing the plain
  `TrivialWorld` the simulation receives with **zero changes under sim/sim_api/fx/ai**
  (all 12 capabilities mapped).
- **`replay`** — a checksummed canonical little-endian codec (record, load,
  validate) with typed errors. No serde: the dependency law forbids it here.
- **`engine`** — `FixedTimestep` (30 Hz, 5-tick catch-up cap), `Interpolator`
  (snapshot blending, spawn/death handling), `MatchHost` (owns `Sim` privately,
  `submit` + `advance` are the only mutation paths; M8 adds `with_controllers`
  for AI hosting, `log()` for the command log, `outcome()` / `is_finished()`
  for the end screen), `AiMatchHost` (headless controller hosting), `RtsCamera`
  (perspective orbit, ground picking, box select), terrain mesh, `Renderer`
  trait + `NullRenderer`.
- **`client`** — winit 0.30 + wgpu 26 windowed 3D renderer (depth buffer, terrain
  mesh with heightmap displacement, instanced placeholder entity boxes), selection
  + right-click Move, fontdue text atlas + HUD/debug overlay, `--frames N`
  windowed smoke, headless fallback for CI. **M8**: hosts the AI opponent through
  `MatchHost::with_controllers`, renders the end-screen panel (VICTORY/DEFEAT/
  MUTUAL DESTRUCTION), supports restart (R), control groups (1-9), and
  Stop/AttackMove hotkeys (S/A).
- **`tools`** — headless runner (demo + `--p1/--p2 ai` controller matches; M8
  adds a `match ended:` line printing the winner), `replay-verify`,
  `content-validate` subcommands.
- **`tests/`** — determinism acceptance A1/A2 with pinned golden hashes, movement
  acceptance (M4), economy acceptance (M5: divergent openings, soak, invariants),
  combat acceptance (M6: composition + position matter, bit-identical, turret,
  A12 soak, legibility), vision acceptance (M6: A10 fog integrity, targeting both
  directions, three-state transition, FD-8), content pipeline acceptance (M2 +
  M5 train extension + M6 fight half — the data-defined kind now trains AND fights),
  AI acceptance (M7: A5 parity, A11 issuer-blindness, AI-vs-AI determinism,
  golden pin, log-alone replay), match-rules acceptance (M8: `MatchEnded` fires
  on resignation and surfaces through the host; A15 restart cleanliness — two
  fresh hosts from the same seed produce identical hashes, and with AI
  controllers identical logs too; stage 10 is hash-neutral vs the M7 golden),
  architecture law (A13), plus proofs that IDs are never reused, iteration order
  is strictly ascending, invalid commands change no state, and the dependency
  law holds.
- **CI** — fmt, clippy with `-D warnings`, and the test suite in dev *and*
  release on Linux, Windows, and macOS, plus the replay round-trip and a
  binaries-run check. **320 tests green in dev, 315 in release** (5 should-panic
  invariant-checker tests are debug-only by nature).

Not built yet — on purpose, in milestone order:

- M9 (Alpha content & feel pass): finalize manifest values, tune the AI script
  (the scripted Alpha vs Alpha match at 7200 ticks doesn't resolve yet — the
  opponent isn't aggressive enough to eliminate the other's command center;
  M9's tuning makes a 10-15 minute match resolve reliably), feel pass
  (feedback cues, responsiveness), placeholder audio cues.
- No multiplayer: the hooks are designed in (`Command.tick`, hashed checkpoints
  for desync detection), the netcode is not.

## Repository structure

```text
pandemonium-bd/
├─ crates/
│  ├─ fx/         fixed-point math (Fx, Vec2Fx, isqrt), PCG32 RNG, FNV-1a hasher
│  ├─ sim_api/    boundary vocabulary: Command, Event, Snapshot, PlayerView, Reject
│  ├─ sim/        the simulation: tick pipeline, stores, command gate, state hash
│  ├─ content/    RON schema, versioned loaders, validators, ContentBundle
│  ├─ ai/         the scripted Alpha opponent (Controller trait)
│  ├─ replay/     checksummed replay codec: record, load, verify
│  ├─ engine/     game loop, interpolation, MatchHost, camera, mesh, renderer
│  ├─ client/     the playable binary: wgpu 26 + winit, 3D renderer, HUD
│  └─ tools/      headless runner (demo + AI matches), replay verifier, content validator
├─ tests/         cross-crate acceptance tests (A1–A15)
├─ content/       data-only game content: rules, entities, factions, maps
├─ docs/          ARCHITECTURE · DEBT · ASSUMPTIONS · CONTENT_GUIDE · adr/
├─ plan.md        the authoritative specification (v2.0)
└─ AI-Handoff.md  live status board and resume protocol
```

## Quick start

The toolchain is pinned to an exact Rust version in
[`rust-toolchain.toml`](rust-toolchain.toml); rustup installs it automatically on
first use.

```bash
# run the scripted demo match headlessly — prints every checkpoint and the final hash
cargo run -p pandemonium-tools -- headless --seed 7 --ticks 300

# record a replay, then re-simulate it and compare every checkpoint
cargo run -p pandemonium-tools -- headless --seed 7 --ticks 300 --record demo.pdrp
cargo run -p pandemonium-tools -- replay-verify demo.pdrp

# validate the content bundle (prints content hash + map id, exits 0 on PASS)
cargo run -p pandemonium-tools -- content-validate content

# the windowed 3D client — on a desktop opens a window; headless runs a smoke pass
cargo run -p pandemonium-client
# windowed smoke: auto-exit after N frames with evidence summary
cargo run -p pandemonium-client -- --frames 900
```

The full verification gate — the same one CI runs:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace             # dev profile
cargo test --workspace --release   # determinism must hold in release too
```

Same seed, same ticks — same hashes, on any machine, in either profile. If those
hashes ever differ between two runs, that is not a feature. File it.

## Development workflow

Every change passes the green gate before commit: formatting, clippy with zero
tolerance, and the full test suite in both profiles. One logical change per
commit. Milestones are never skipped, and M(N+1) does not start until every exit
test of M(N) is green in CI. Deliberate simplifications are logged in
[`docs/DEBT.md`](docs/DEBT.md) with a repayment trigger; guessed decisions are
logged in [`docs/ASSUMPTIONS.md`](docs/ASSUMPTIONS.md); challenges to frozen
decisions go through an ADR in `docs/adr/`.

The process is strict because the product is trust. A deterministic engine is a
promise, and promises are kept by boring discipline — the operating contract in
[plan.md §0](plan.md) is short, and worth reading before the first commit.

## Documentation

| Document | Role |
|---|---|
| [`plan.md`](plan.md) | the authoritative specification: frozen decisions, determinism rules, milestones M0–M10, acceptance criteria A1–A15 |
| [`AI-Handoff.md`](AI-Handoff.md) | live status board, verification commands, and hard-won gotchas — start here to pick up development |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | the working map of the code as built |
| [`docs/DEBT.md`](docs/DEBT.md) | every deliberate simplification, each with a repayment trigger |
| [`docs/ASSUMPTIONS.md`](docs/ASSUMPTIONS.md) | every interpretation that had to be guessed, numbered and cited |
| [`docs/CONTENT_GUIDE.md`](docs/CONTENT_GUIDE.md) | content authoring reference (expands in M2; plan §10 governs meanwhile) |
| [`docs/adr/`](docs/adr/README.md) | the process for proposing changes to frozen decisions |

## Roadmap

Milestones are gates, not dates — the calendar (~26 weeks for a small human team)
is a reference, the exit criteria are the truth. Each prototype gate P1–P5 asks
one question and refuses to move on until it is answered.

| Milestone | Delivers | Exit gate | Status |
|---|---|---|---|
| **M0** — Skeleton & guardrails | Workspace, toolchain pin, guardrails, `fx` crate | fmt, clippy, fx property tests, architecture law | ✔ **Complete** |
| **M1** — Simulation core | Tick pipeline, entity/capability stores, command gate, state hash, replay, determinism proofs | A1/A2 green, pinned golden hashes, replay round-trip | ✔ **Complete** |
| **M2** — Content pipeline | RON schemas, versioned loaders, validators, map loader, content hash, `content-validate` tool | All Alpha content loads from data; add-a-unit scaffold proves the boundary | ✔ **Complete** |
| **M3** — Engine shell | Window, wgpu 26 3D renderer, camera, snapshot interpolation, fixed loop, input → commands, HUD/debug overlay | Windowed build shows live simulation; Move commands work; `--frames` smoke pass | ✔ **Complete** |
| **M4** — Movement *(P1: do multiple units move responsibly?)* | Nav grid, deterministic A*, path execution, collision, push-apart, stuck detection | 50 units respond within 2 ticks under spam-clicked orders; nobody permanently stuck | ✔ **Complete** |
| **M5** — Economy & production *(P3: do openings diverge?)* | Resource ledger, gather loop (depletion + auto-seek), production queues (Train/Cancel/SetRally), construction lifecycle, population cap, requirements, footprints blocking tiles, A12 invariant checker | Scripted openings produce measurably different timelines; economy soak holds all invariants | ✔ **Complete** |
| **M6** — Combat & vision *(P2: is combat legible and meaningful?)* | Immediate-hit attack pipeline (acquire → validate → hit → mitigate → apply → credit), Attack/AttackMove/Stop semantics, three-state fog (Hidden/Explored/Visible per player per tile), turret (no Move, Attack+Footprint), A12 combat invariants | Composition and position matter; fog integrity proven (A10); bit-identical run-to-run | ✔ **Complete** |
| **M7** — AI through commands *(P4: is parity real?)* | `Controller` trait, scripted opponent, parity audit | AI-vs-AI headless matches complete; parity is compile-time | ✔ **Complete** |
| **M8** — Match rules *(P5: do all systems work together?)* | Stage 10 defeat/victory/resignation evaluation, `MatchEnded` event (idempotent), `MatchHost` AI hosting + command log + outcome, windowed client end screen + restart (R) + control groups + Stop/AttackMove hotkeys | A15 restart cleanliness (two fresh hosts, same seed → identical hashes; with AI → identical logs too); stage 10 is hash-neutral (M7 golden holds) | ✔ **Complete** |
| **M9** — Alpha content & feel | Manifest tuning, feedback pass, placeholder audio | Minimum viable loop playable end-to-end | ⬜ Pending |
| **M10** — Stabilization & declaration | Full acceptance suite A1–A15, nightly soak, benchmark baselines | Every criterion verified with evidence, in writing | ⬜ Pending |

Beyond the Alpha, the expansion sequence is already audited against the
architecture: new unit, new building, new map, stat rebalance, second faction —
all data-only — then abilities, tech tree, and evaluative AI above the command
interface. Multiplayer rides on hooks that exist from day one: `Command.tick` is
the input-delay field for lockstep, and hashed checkpoints are the desync alarm.
The Alpha is declared only when every criterion is verified; one that cannot be
met honestly is recorded as a finding, never waved through.

## Contributing

Contributions follow the same law as the code. Read
[plan.md §0](plan.md) (the operating contract) and
[`AI-Handoff.md`](AI-Handoff.md) first, then work within a milestone: one logical
change per commit, the green gate before every commit, and automated proof for
any claim the code makes. Simplifications go to the debt register; guesses go to
the assumptions log; frozen decisions change only through an ADR.

Gameplay content is not accepted yet — the content pipeline is **complete** (M2), but the Alpha content manifest is locked per the plan; content is data, so the door opens when the data boundary exists and the Alpha declares. Bug reports, however,
are welcome now, and this project makes them unusually actionable: a report that
includes the seed and the tick is a reproducible incident. Anything else is a war
story.

## License

No license has been published yet. Without a LICENSE file, standard copyright
defaults apply: you are welcome to read, study, and link to the code, but formal
reuse and redistribution terms have not been granted. This section will be
updated the moment that changes.

---

<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/seal-dark.svg">
  <img src="assets/seal-light.svg" alt="Circular seal: a dial of thirty tick marks, PANDEMONIUM inscribed above, BUILD & DESTROY below, and a hostile diamond overlaying a dashed divide between scattered strokes and an ordered grid of squares." width="150">
</picture>

**Pandemonium: Build & Destroy** — by [E-Vex](https://github.com/E-Vex)

*Determinism under fire.*

</div>
