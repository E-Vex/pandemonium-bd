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
- **A-013 (§14 M1, handoff §8).** M1's world is `sim::fixture::TrivialWorld` — the
  in-code bundle the handoff asked for (no RON before M2). Its `content_hash()`
  canonicalizes every fixture field, standing in for the M2 `ContentBundle`'s hash.
  When M2 lands, loaded content becomes the real path for matches; the fixture
  stays as the minimal spine-test world.
- **A-014 (§8.2).** The M1 validation gate implements the structural checks —
  tick match, issuer existence, duplicate `(issuer, seq)` within a tick
  (second command refused, first wins — deterministic), referenced-entity
  existence, ownership, capability presence, target existence and visibility.
  The economy checks (affordability, population, placement, requirements) become
  reachable when economy data exists (M5). Commands whose required capability
  variant does not exist yet (Attack, Gather, Build, Produce) run the structural
  checks and then fail capability presence — semantically correct for a world
  where no entity carries them; the checks become store lookups as the variants
  land (M5/M6). Empty unit lists are valid no-ops.
- **A-015 (§6.3 stage 8, §14 M1 exit).** `HealthDef.regen_per_tick` (a data
  parameter, possibly negative) is the M1 death path — the only way to exercise
  death & cleanup and the id-never-reused exit test without combat (M6). Health
  advance runs within stage 8, immediately before death removal; combat damage
  becomes the primary health mover in M6. Regeneration clamps to `0..=max_hp`.
- **A-016 (§6.3 stage 3, §14 M1 exit).** The fixture's `scheduled_spawns` stand in
  for production spawning, exercising the same entity-store append + `Spawned`
  events the real production path will use; they stay useful for spine tests after
  M5 makes production the real source. Processed in `(tick, fixture order)` with
  the RNG advanced once per jittered spawn (x draw then y draw; zero jitter
  consumes no draws).
- **A-017 (§6.4).** The canonical state hash encoding is version 1 and
  entity-major: identity/position/facing/order-queue plus a capability presence
  bitmask over the fixed order (Health, Move, Vision) followed by the present
  blocks. Per-tile visibility bitsets join when fog state exists (M6). Events are
  outputs and are not hashed. Ids allocate from 1, so `EntityId(0)` remains a null
  value (`EntityId::NONE`).
- **A-018 (§9.5, §9.6).** M1 `player_view` visibility is the circular-radius
  filter, recomputed on demand (full scan, ascending id) — own entities always
  visible, others within any friendly Vision radius (squared-distance compare).
  The incremental three-state fog model is M6; a slot outside the match yields an
  empty view rather than an error.
- **A-019 (§6.5).** The replay command log records the exact stream fed to
  `Sim::step` at each command's declared tick (chronological order preserved);
  rejections are recorded too, so re-simulation reproduces them. Stale-tick
  rejections are a live-client phenomenon — a replay cannot produce them, so they
  are pinned by sim unit tests and the determinism suite instead. The recorder
  omits script entries targeting ticks past the run length (never fed ⇒ not
  recorded), and forces a final checkpoint at the end tick so
  `final_hash == last checkpoint hash` holds by construction.
- **A-020 (§3.2, §12).** `.github/workflows/ci.yml` was absent from the inherited
  repository even though A-010 and the handoff described it as committed; it is
  authored as part of M1 (the A2 exit needs replay-verify in CI). It runs
  fmt + clippy, the test matrix (Linux/Windows/macOS × dev/release), the replay
  round-trip, and the runnability of every binary. As with A-010, the 3-OS claim
  stands only once the repo actually runs on GitHub.
- **A-021 (§4).** The replay re-simulation driver is duplicated between
  `pandemonium-tools` and `tests/determinism.rs` (~40 lines each) because the
  `replay` crate must not depend on `sim`. Consolidation would need a new crate
  both may depend on; revisit if the driver grows (see DEBT-005).
- **A-022 (§8.1).** M1 treats `AttackMove` like `Move` for validation and movement
  (it additionally requires the Attack capability when combat lands, M6); the
  engage-en-route semantics are M6. `Stop` is valid for any owned entity (a
  no-op for entities without orders or a mover).
- **A-023 (§3.2).** `thiserror` v2 is used where the plan's table allows
  `thiserror` (replay's decode/validate errors). `clap` 4 (derive) and `anyhow`
  are used in `tools`, as the plan's table allows; `sim_api` needs no error types
  and stays dependency-minimal apart from `fx` (A-005). `sim_api` re-exports
  `Vec2Fx` since commands, events, and views carry positions in their public
  shape.
- **A-024 (§14 M2, §4).** The loaded-bundle path into the simulation is
  `Sim::new(&bundle.world(), setup)`: `ContentBundle::world()` produces the
  simulation's own plain `TrivialWorld` (kinds, resources, initial spawns) and
  `Sim::new` is unchanged. The dependency law (content depends on sim, never the
  reverse) makes any sim-side "bundle" parameter impossible, so this IS the
  loaded-bundle path the handoff §8 describes; the fixture stays for spine tests.
  M2 therefore landed with zero diffs under `crates/{sim,sim_api,fx,ai}`.
- **A-025 (§10.3/§10.4, §7.4).** The content schema carries the full Alpha
  capability vocabulary (Attack, Gather, Build, Produce, Storage,
  ProvidesPopulation, Resource, Footprint) even though the simulation only
  consumes Health/Move/Vision until M4-M6 — data arrives first, systems catch up
  (see DEBT-006). Gathering parameters live on the worker's Gather capability
  (§10.4's "workers carry 10 Ore per trip, 2000 ms per gather" reads as
  per-worker data), not in the rules file.
- **A-026 (§10.2, FD-10, ADR-0001).** Map schema versioning: v2 is the
  ADR-0001 shape (optional display-only `heightmap`); v1 is the pre-pivot shape
  and migrates forward as a flat v2 map (the migration is one explicit typed
  function per step, `migrate_map_v1_to_v2`, tested). Entities, factions, and
  rules are at v1. A version newer than the loader is a precise error, never a
  guess; version 0 is invalid. Strict mode (unknown fields are errors) is the
  only mode — a lenient mode is unimplemented until a need appears.
- **A-027 (§10.5).** The optional symmetry validator runs exactly when a map
  declares `symmetric: true` and checks 180-degree rotational symmetry of: the
  terrain grid, the ore-node footprint tile multiset, and the start anchors
  (mirrored through the anchor structure's footprint). Faction starting-force
  *positions* are not symmetry-checked (they are faction data with shared
  offsets, not map data); ore reachability is validated per start against every
  node (stricter than "some ore" — catches authoring pockets); map dimensions
  are capped at 4096 as a sanity bound.
- **A-028 (§9.7, §10.4).** Starting forces (1 Command Center + 4 Workers) are
  faction data placed at tile offsets from each map start anchor; ore nodes are
  map data referencing node entity kinds (the amount lives in the entity's
  Resource capability). Initial spawn order — and therefore entity id allocation
  — is: map starts in authored order, each start's forces in faction order, then
  ore nodes in map order; positions are footprint centers in fixed-point tile
  units. `spawn_jitter_milli` is 0 for loaded content (exact placement; the
  fixture keeps jitter to exercise the RNG).
- **A-029 (§10.6, A3).** The M2 A3 scaffold proves: a new kind defined purely by
  a new data file + faction edits loads, spawns, and obeys a Move order, and
  nothing under `crates/sim/` mentions it (source scan inside the test). The full
  §10.6 test — trained from a production list and fighting — extends the
  scaffold when production (M5) and combat (M6) land (DEBT-007). The git-diff
  half of A3 ("no file in crates/sim/ changed in the fixture commit") is the
  commit review protocol, not a runtime check.
- **A-030 (§5, §10.2).** Determinism of loading: the directory reader sorts each
  category's files by name (OS order is not deterministic), canonical
  collections are sorted by id, `KindId`s index the entities sorted by id (so
  adding an entity can shift later kind ids — bundles are self-consistent and
  content-hash-pinned, and nothing persists kind ids across bundles in the
  Alpha), and serde maps deserialize into ordered maps. Loading is a pure
  function of the directory (tested both in-crate and in the acceptance suite).
- **A-031 (ADR-0001, §6.5).** The display-only heightmap participates in the
  content hash and the map id: content identity covers the whole bundle, so any
  content edit (visual or mechanical) is a different match for replay matching —
  stricter than simulation-equality requires, chosen so a replay always implies
  an exact content tree. Extra §10.4 stat values the table leaves open were
  authored as: collision radii worker 300 / rifleman 350 / raider 300 /
  guardian 500 milli-tiles (rifleman's 350 is plan §10.3's example), acquire
  ranges rifleman 7000 (§10.3) / raider 5000 / guardian 9000 / turret 8000, and
  requirements worker←command_center, combat units←barracks,
  barracks/depot/turret←command_center (CC requires nothing).
- **A-032 (§3.3, ADR-0001).** A-001's "2D top-down presentation" clause is
  superseded by ADR-0001 (accepted 2026-10-01): 3D perspective presentation over
  the 2D logical ground plane; `glam` is allowed in client/engine only. The rest
  of A-001 (desktop, single-player, mouse+keyboard) stands.
- **A-033 (§14 M3, §11.1/§11.3).** M3 engine/client shape: `MatchHost` owns the
  `Sim` privately so "the client never touches Sim mutably except via step
  inputs" is a compile-time property (submit re-stamps the command tick to the
  next step's tick, so a submitted command is never rejected as stale). The
  camera picks on the y=0 logical ground plane (`world.x = sim.x`, `world.z =
  sim.y`); conversions cross the boundary as fixed point. Selection is
  client-local and (placeholder rule until M6 combat) picks own units only.
  Entity placeholder art is instanced boxes, team-colored, brightened when
  selected, slightly smaller while moving. Terrain colors are flat per class
  (passable green / blocked gray); height scale 0.02 tiles per heightmap unit.
- **A-034 (§14 M3 exit, §13).** This environment has no display (no
  X11/Wayland), so the windowed M3 exit criterion cannot be verified here. The
  client binary detects the missing display, prints the finding, and runs a
  headless smoke pass over the real content (load, 180 frames, state + content
  hashes) exiting 0 — CI's "every binary starts" check stays green and the
  windowed verification remains an explicit open item (DEBT-008), never waved
  through. Everything CI-able is tested: the clock, interpolation, MatchHost
  command flow, camera math (projection, picking, box select), the terrain
  mesh, and the null renderer.
