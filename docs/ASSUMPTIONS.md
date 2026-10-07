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
- **A-035 (§3.2, §11.4).** The "own text renderer" is fontdue plus an embedded
  font: "Pandemonium Sans", an ASCII (U+0020..U+007E) subset of DejaVu Sans
  renamed as the Bitstream Vera license requires for modified versions (the
  license text ships beside it, `crates/client/assets/fonts/LICENSE.txt`;
  the rename also satisfies the no-"Bitstream"/"Vera" clause). One
  rasterization size (18 px) fits the placeholder HUD; multiple sizes, DPI
  scaling, and kerning wait for M8's UI-depth work. Regeneration is a
  fontTools subsetting step (recorded in the session worklog) — the binary
  never needs fonts from the host system.
- **A-036 (§14 M3 exit, §13).** The client accepts a presentation-only
  `--frames N` argument: after N presented frames it exits cleanly and prints
  the evidence summary (frames presented, tick, state hash, commands
  submitted, selection size). It exists so the windowed exit criterion is
  machine-verifiable without a human and changes no simulation behavior; it
  lives only in the client.
- **A-037 (§14 M3 exit, DEBT-008).** The windowed verification environment
  (no desktop, no root): Xvfb :99 + Mesa llvmpipe reached through
  userland-extracted `libEGL`/`libEGL_mesa`/`libGLESv2` (glvnd vendor dir via
  `__EGL_VENDOR_LIBRARY_DIRS`), `WGPU_BACKEND=gl`, and XTEST mouse injection
  via userland `libXtst`. Verified mechanically: window creation, 900
  presented frames, drag-box selection of the 5 start entities, two
  right-click Move commands, and the resulting worker motion visible in
  before/after screenshots. What this cannot verify — that the visuals *read
  well* to a human — stays open in DEBT-008.
- **A-038 (§6.4, §5.10, M4).** M4 deliberately changes both canonical
  encodings: `STATE_ENCODING_VERSION` 1→2 (the Move capability block gains
  radius, remaining path waypoints, and the stuck/repath counters) and
  `FIXTURE_ENCODING_VERSION` 1→2 (the world gains the passability grid; the
  Move template gains the radius). The determinism golden hashes were
  regenerated and the tools headless demo's final hash moved to
  `0x3904fff0c74ee4c4` (seed 7, 300 ticks) — review-visible value changes,
  not silent ones.
- **A-039 (§9.1, §14 M4).** Movement semantics choices: a goal whose tile is
  blocked or unreachable resolves as the *nearest reachable tile* (units walk
  as close as the terrain allows; only a fully sealed start or a zero-speed
  mover with a distant goal fails immediately with `MoveFailed`); stuck
  escalation (30 blocked ticks, 4 repaths) ends in `MoveFailed` or — when the
  mover is within one tile *plus its own radius* of the order target — a
  crowded arrival that completes the order; replacing Move orders reset the
  runtime path (a new destination invalidates the old lanes); steering
  refuses any landing on blocked terrain, so collision displacement can never
  shove a unit into rock. The straight beeline is validated by an integer
  supercover of the *exact* start→goal segment (lattice-corner crossings
  checked like A* diagonals); every other route walks tile centers prefixed
  by the start tile's center (a convex-safe first leg).
- **A-040 (§9.1, §17).** Formation-less movement has a known jam shape: a
  line of units ordered *across its own axis* into a single point funnels
  into shared lanes and the trailing units can end in `MoveFailed` (resolved,
  never stuck). Groups in compact-block shapes and spread destinations — the
  shapes the M4 exit tests and real play use — resolve cleanly. Formations
  and side-stepping avoidance remain post-Alpha hooks (plan §17); the crowd
  radius and head-on pass-through keep ordinary clicks working.
- **A-041 (§9.4, §10.4).** Which kinds a producer trains is *faction* data; it
  flows into the simulation through `TrivialWorld::production` (producer kind →
  trainable kinds, authored order) rather than parameters on the Produce
  capability, which stays parameterless. The command gate refuses Train of a
  kind outside the producer's list as `MissingCapability` (the producer "lacks
  the capability to produce that"). A second faction is still data-only: its
  lists ride the same seam.
- **A-042 (§9.4).** "Population headroom is checked at enqueue and at spawn" is
  read as: the enqueue check compares *live* usage + the kind's population
  against the cap (queued items reserve nothing), and the spawn re-checks
  against fresh usage — a completed item whose headroom vanished waits at the
  front of the queue until the cap grows (classic supply-block, resolved
  deterministically; pinned by the production tests).
- **A-043 (§9.3).** A worker is "at" a node / storage / site when the distance
  from its body to the nearest point of the footprint *rectangle* is at most
  1500 milli-tiles plus its collision radius. The generosity is deliberate:
  economy travel then needs none of the mover's crowded-arrival machinery, and
  arrivals under jostle still count. Extraction, deposits, and hammering all
  use the one shared test.
- **A-044 (§9.3).** Auto-seek on depletion (and for any dead gather target)
  rewrites the order to the nearest node — center distance, ties to the lower
  id — carrying the same resource as any partial cargo (any node when the
  hold is empty); with no node left the order is dropped and the worker idles
  (partial cargo rides along until a future order resolves it). Re-targeting
  happens on the tick the node is removed.
- **A-045 (§9.4).** A construction site whose builder dies stalls — the
  command vocabulary has no reassignment or construction-cancel, so the
  site (and its spent cost) waits indefinitely; site hp is full from spawn;
  costs are only refundable through production-queue cancellation. Revisit if
  combat (M6) makes dead builders common.
- **A-046 (§9.4).** Under-construction producers refuse Train with
  `MissingCapability` (a site is not open for business); `SetRally` and
  `CancelQueueItem` work on any producer carrying the capability regardless of
  lifecycle (setting an early rally is harmless and classic).
- **A-047 (§9.4).** The two surfaces stay disjoint by kind shape: Train of a
  footprint-carrying kind is refused `InvalidTarget` (structures are built,
  not trained) and Build of a kind without a Footprint is refused
  `InvalidTarget` (units are trained, not built).
- **A-048 (§9.5, §8.2).** Gather targeting does not fog-filter: nodes are
  static map features and the three-state fog model is M6's work, so the gate
  checks node existence and Resource-carriage only. The vision milestone
  revisits target legality for economy commands.
- **A-049 (§13 A12).** ~~`population <= cap` is asserted strictly. It holds
  through M5 (caps only grow; deaths only lower usage). M6 combat can destroy
  population-providing structures and legitimately open an over-cap window —
  the allowance (or its absence) is an explicit decision then; see DEBT-010.~~
  Superseded by A-053 (M6's first decision). Kept struck for the audit trail.
- **A-050 (§6.4, §5.10, §14 M5).** M5 bumps both canonical encodings:
  `STATE_ENCODING_VERSION` 2→3 (the Gather/Build/Produce/Storage/
  ProvidesPopulation/Resource/Footprint/Construction capability blocks, the
  `GatherAt`/`BuildAt` orders, the `UnderConstruction` lifecycle) and
  `FIXTURE_ENCODING_VERSION` 2→3 (buildability grid, production lists, base
  population cap, kind economy). The determinism goldens regenerated; the
  tools headless demo's final hash moved to 0xbc86a622e357252d (seed 7,
  300 ticks) — review-visible value changes, not silent ones.
- **A-051 (§9.3).** The nearest storage is chosen by center distance (ties to
  the lower id) among the player's *completed* storages that accept the
  carried resource; with no storage the full worker stands and waits (the
  order is kept). Center distance rather than rect distance keeps the choice
  simple and deterministic; the approach test is what actually governs the
  deposit.
- **A-052 (§10.5, §9.4).** Build placement legality is: every footprint tile
  in bounds, on buildable terrain (the new buildability grid), unclaimed by
  any static body, and free of movers standing on it. With footprints now
  blocking tiles, Crossroads' four contested plaza nodes on the gap's
  doorsteps sealed the only north-south crossing (independently verified by
  BFS: 907 reachable tiles, goal cut off) — they moved one tile outward
  ((28,29), (34,29), (28,33), (34,33); 180-degree symmetry kept), the content
  hash moved to 0x249b69f0ee343a10 and the map id to 0xbc0970c3cf14e9cf, and
  the M4 real-map exit test recalibrated its arrival bound to the
  blocked-destination packing envelope (2800 milli) while gaining a
  zero-MoveFailed assertion and a no-mover-on-footprint-tile check.
- **A-053 (§9.4).** Produced units spawn on the first free tile of an
  expanding row-major ring around the producer (≤ 3 tiles out, exact tile
  centers, producer position as the degenerate fallback) and carry a
  `MoveTo` rally order when one is set. Construction sites spawn at their
  authored placement with *no* jitter (a shifted site would misalign its
  footprint) and claim their tiles immediately.
- **A-054 (§6.3).** Stage 3's internal order is fixed as: the fixture's
  scheduled spawns, then production queues (producers ascending), then
  construction sites (ascending); population usage/cap recompute at match
  start, end of stage 3, and after stage 8's removals. Stage 4 runs the
  gather loop (workers ascending) and node removals with same-tick
  re-targeting.
- **A-055 (§8.2).** Rejection-reason mapping for the economy commands: a kind
  outside the producer's faction list → `MissingCapability`; an
  under-construction producer → `MissingCapability`; Train of a structure
  kind / Build of a unit kind → `InvalidTarget`; a cancel index outside the
  queue → `QueueIndexInvalid`; requirements unmet → `RequirementsUnmet`;
  insufficient funds → `CannotAfford`; no population headroom →
  `PopulationFull`; blocked placement → `PlacementBlocked` (terrain, claim,
  mover, or bounds).

- **A-053 (§13 A12, DEBT-010 retired).** `population <= cap` is the spawn-
  blocking rule, not an always-on invariant. Combat can destroy population-
  providing structures while units live, opening a temporary over-cap window:
  usage may then exceed cap, and the M5 production queue already holds
  completed items at the queue front until headroom returns (A-042), so no new
  spawns happen while over cap. The A12 checker is therefore relaxed to a
  cap-non-corrupt + usage-non-saturating assertion (catching only wild drift,
  not the legitimate combat-loss window). This decision touches A12 only;
  §9.4's production/construction semantics are unchanged. Pinned by
  `population_drift_far_past_cap_fires` (replaces the strict-over-cap test).
  The classic alternative — units die or decay when usage exceeds cap — was
  rejected as a feel regression with no compensating simplification.

- **A-056 (§6.4, §5.10, §14 M6).** M6 bumps both canonical encodings:
  `STATE_ENCODING_VERSION` 3→4 (the Attack capability block: damage, range,
  cooldown_ticks, cooldown_remaining, acquire_range, target tag + EntityId;
  the `AttackUnit` order) and `FIXTURE_ENCODING_VERSION` 3→4 (the
  `CapTemplate::Attack` variant — damage, range_milli_tiles, cooldown_ms,
  acquire_range_milli_tiles). The determinism goldens regenerated; the tools
  headless demo's final hash moved to 0x9d5ba9b565060336 (seed 7, 300 ticks).
  The content hash is unchanged (0x249b69f0ee343a10) — the bundle's
  `encode_capability` already covered Attack data, so the seam flip from
  DEBT-006's `None` to `Some(CapTemplate::Attack)` is invisible to the bundle
  identity by construction. Review-visible value changes, not silent ones.

- **A-057 (§9.2, M6).** Combat acquisition's tie-break: when two candidate
  targets are at the same squared distance from the attacker, the lower
  `EntityId` wins. The acquisition scan walks candidates ascending id and
  keeps the first at each distance bucket (`update_best` uses `<=` so a
  same-distance later candidate loses), so the ascending-id order is the
  tiebreak. Pinned by `auto_acquisition_picks_closest_enemy_unit_in_acquire_range`.

- **A-058 (§9.5, §9.2, M6).** Auto-acquired combat targets are by construction
  visible: the `acquire_range` for every authored unit is ≤ the `Vision`
  radius (raider 5000 ≤ 8000, rifleman 7000 ≤ 8000, guardian 9000 ≤ 9000,
  turret 8000 ≤ 9000 — verified by content validation). The combat pipeline's
  `validate` stage therefore does not re-check visibility for auto-acquired
  targets — the command gate already enforced `NotVisible` for commanded
  targets (plan §8.2), and auto-acquired targets within `acquire_range` are
  within vision. A unit whose `acquire_range` exceeds its `Vision` radius
  would be a content bug (a turret firing into fog); content validation
  catches this. Revisit if a future unit kind breaks the invariant.

- **A-059 (§9.5, §13 A10, M6).** Where the fog state lives: per-player
  per-tile visibility is **derived** state, not canonical hashed state. The
  state hash already includes every entity's position and the Vision
  capability radius, both of which fully determine the fog state at any
  tick. Adding the fog bitsets to the hash would be redundant (the same
  inputs always produce the same bitsets) and would couple the hash to a
  per-player derivative that A10 explicitly wants "hashes equal fog on/off".
  `FogState` is therefore carried on `Sim` as a non-hashed cache,
  recomputed each tick in stage 9. A10's "fog on/off, hashes equal" is then
  trivially true: the hash never includes the bitsets, so toggling observer
  mode (which only affects what `player_view` returns, not the canonical
  state) cannot change the hash. The targeted test
  (`a10_fog_integrity_hashes_equal`) pins this.

- **A-060 (§6.4, §13 A5/A11, M7).** Controller labels are hashed match
  identity, never behavior. `PlayerState` has carried `ControllerKind`
  since M1 and the state hash encodes it (`hash.rs`'s `controller_tag`),
  so the same command log re-simulated under different labels hashes
  differently — by design, the same way the seed does: a replay records
  `player_setup` and must re-simulate under those exact labels. Behavior
  is label-blind: the M7 battery runs identical mirrored commands under
  Human/Ai, Human/Human, and Ai/Ai setups and the outcome maps are
  identical (`a11_command_outcomes_are_issuer_blind`). The A5 audit's
  "no AI-specific entry point" is therefore structural (sim never
  references the ai crate) *and* behavioral (labels change nothing).

- **A-061 (§9.6, FD-7/FD-8, M7).** The AI knowledge boundary. Everything
  about *live entities* — positions, ownership, health, existence — flows
  to a controller only through the fog-filtered `PlayerView`. Everything
  else the plan carries (`AiPlan`) is public knowledge a player reads off
  the screen before the match: kind ids, costs, build times, start
  positions, and candidate build ground. Fog hides entities, never
  terrain or prices. The plan builder (`engine::ai_host::alpha_plan`)
  resolves kinds by capability shape — the production lists decide the
  roster (a producer whose list trains combat kinds is the barracks, the
  other trains workers), supply is the cheapest non-CC population
  structure, the node kind carries a Resource body — never by display
  name, and the controller itself never matches anything at all.

- **A-062 (§9.6, M7).** The controller acts blind and verifies by sight.
  The plan's `Controller` trait receives only a view — no events — so the
  script cannot see rejections, queues, or lifecycles. It verifies
  through the next views instead: pending trains retire when a new own
  entity of the kind appears, or expire at `2x build time + slack`
  (a rejection or a spawn-and-death between thinks); build attempts
  settle by the count of own entities of that kind (a successful `Build`
  spawns the site on the spot the moment the command applies, so a count
  increase is exact — an earlier spot-proximity check gave false
  successes on adjacent candidate spots and cycled placements every two
  ticks; the count is pure view arithmetic). Fixed cooldowns pace the
  retries.

- **A-063 (§4, M7).** The AI hosting loop lives in the engine
  (`engine::ai_host::AiMatchHost`): A-003 already declared `engine -> ai`
  for hosting controllers on the tick boundary, and one loop serves the
  tools' headless runner (plan §12), the acceptance tests, and — in M8 —
  the client's vs-AI matches. This adds the `tools -> engine` edge to
  the architecture law's allow-list, an A-003-style amendment (the plan's
  §4 diagram is non-exhaustive; the tools' headless AI-vs-AI runner is a
  match host exactly like the client). Recorded here rather than in an
  ADR because it changes no frozen decision — the law test's allow-list
  is the codification of A-003's reading.

- **A-064 (§12, M7).** `tools headless --p1/--p2` (`demo|ai|idle`) both
  default to `demo` — the M1 scripted match and its pinned final hash are
  unchanged (the CI replay job and the §4 verification command keep
  their meaning); `demo` mixes with nothing (a precise error says so).
  AI matches load `--content` (default `./content`) and print a
  per-player evidence line (deliveries, trained, built, hits, deaths) —
  the machine-visible "result" until match rules land in M8.
  `replay-verify` resolves the world by content identity (the demo world
  first, then the content directory), failing with every candidate's
  hash when nothing matches.

- **A-065 (§9.6, M7).** The scripted opponent's semantics, the reading
  of plan §9.6's "Alpha behavior". Worker management: `MoveState::Idle`
  means *no orders at all* (an empty order queue — auto-acquired combat
  shows Idle too), so re-tasking idle workers can never cancel gather,
  build, or march work in progress; idle workers go to the nodes nearest
  home, round-robin. Worker target 10; army cap 12 with round-robin
  composition over the barracks' production list; depot target 3 with a
  headroom-below-3 trigger; the barracks waits for six workers or a
  standing depot — the "workers -> depot -> barracks -> mixed army"
  order. Waves: a Massing/Attacking machine that fires on army size
  (six) or a 1500-tick pressure timer (minimum three), marches by
  `AttackMove` at the enemy start with a seeded jitter of ±600
  milli-tiles (the M4 funnel lesson), and rallies the barracks to the
  front; it regroups only when fewer than three survive — mid-fight
  idleness is not a regroup signal. Defense preempts waves: the nearest
  visible intruder within twelve tiles of home is `Attack`ed on a
  45-tick cooldown. The wave jitter is the controller's only
  randomness; its RNG seed derives from the match seed mixed by slot
  (`seed + (slot+1) * golden-ratio constant`), so pre-wave decision
  logs are seed-independent — the states still diverge through the
  hashed RNG stream (the sim's own), which the determinism test asserts
  instead of log inequality.

- **A-066 (§6.4, §5.10, §13 A15, M8).** The match outcome is derived
  state, not canonical hashed state — the same reasoning as fog (A-059).
  `MatchOutcome` is a pure function of the entity set (which determines
  structure counts) and the players' `resigned` flags, both of which ARE
  part of the canonical hash. Adding the outcome to the hash would be
  redundant (the same inputs always produce the same result) and would
  couple the hash to a derivative. The M7 goldens therefore stay green by
  construction: no `STATE_ENCODING_VERSION` bump, no golden regen. The
  `outcome` field on `Sim` is `Option<MatchOutcome>`, `None` while the
  match is ongoing, `Some` once stage 10 has fired `MatchEnded` (cached
  so the event is emitted exactly once). Toggling "evaluate match rules
  on/off" cannot change a checkpoint — the M8 invariant.

- **A-067 (§9.7, M8).** The defeat check only fires when the match is
  "structure-bearing" — at least one player in the match owns at least
  one structure (an entity carrying a `Footprint` capability, the
  data-defined "structure" marker). The M1 spine-test fixture
  (`TrivialWorld`) carries no Footprint kinds at all (it predates the
  economy milestone), so a strict "zero structures ⇒ defeated" rule would
  end every spine match at tick 0 with every player simultaneously
  defeated. The Alpha content (the M2+ real path) starts every player
  with a Command Center, so the rule fires only when a structure is
  actually destroyed — the intent of plan §9.7. A degenerate "no one ever
  had a structure" match simply continues; a real match ends the moment
  a side is eliminated. Ore nodes carry `Footprint` but are neutral
  (`PlayerId::NEUTRAL`), so they never count for a player's structure
  total.

- **A-068 (§9.7, M8).** The match ends when at least one player is
  defeated **and** at most one non-defeated player remains. The first
  clause ("at least one defeated") keeps a one-player smoke match (no
  opponents, no defeat) running — the M3 headless smoke and the client's
  no-AI test paths stay green. With two players, when one is defeated
  the match ends (one survivor, the winner); with both defeated
  simultaneously the match ends with `PlayerId::NEUTRAL` (mutual
  destruction — well-defined and deterministic, never expected in normal
  play). A one-player match where the single player is defeated ends
  with `NEUTRAL` (the single player lost; no winner).

- **A-069 (§4, §11.1, M8).** `MatchHost` extended with optional AI
  controllers (default empty) is the M8 client's hosting seam — the same
  shape as `AiMatchHost`, but with the real-time clock and interpolation
  the windowed client needs. A-003 already declared `engine -> ai` for
  hosting controllers on the tick boundary; M8 extends the same seam to
  the client (the plan's hosting loop is one loop, shared with the tools
  and tests). The M3 exit criterion holds: the `Sim` is still private,
  `submit` + `advance` are still the only mutation paths, and controllers
  receive an immutable `PlayerView` (FD-7) — they never see `&mut Sim`.
  `Clone`/`Debug` derives removed from `MatchHost` (`Box<dyn Controller>`
  carries neither); restart is by drop + reconstruct (the client retains
  its `ContentBundle` and `MatchSetup`, A15 constructs two fresh
  instances).

- **A-070 (§9.7, §13 A15, M8).** Restart cleanliness (A15) is tested by
  constructing two fresh `MatchHost` instances from the same setup and
  asserting identical state hashes at every checkpoint — not by a
  `restart()` method on `MatchHost`. The controller-reset problem (a
  deterministic controller's RNG state has advanced) makes an in-place
  `restart()` fragile; drop + reconstruct with fresh controllers (the
  client re-creates the `ScriptedController` from the same bundle + seed)
  is the clean path. The M8 acceptance test `a15_two_fresh_hosts_with_ai_
  produce_identical_logs_and_hashes` pins the stricter guarantee: with
  AI controllers, the deterministic controller RNG (seeded from the
  match seed) makes the command logs identical too — a same-seed restart
  reproduces the same match bit-for-bit. The windowed client's "Press R
  to restart" UX relies on this.

- **A-071 (§9.2, M9).** A chase order dies with its target. movement.rs
  documents "`AttackUnit` orders do NOT pop on path completion — the
  combat pipeline pops them when the target dies" — but no code ever
  popped them, and the M8 flagship's unresolved matches traced exactly
  to that hole: defense `Attack` orders outlived their dead intruders,
  the chasers froze reporting `Moving` (orders non-empty, path empty,
  no stuck detection for empty paths), and both armies parked forever.
  The M9 reading: stage 1 pops a head `AttackUnit` whose target is no
  longer in the world (covering every removal path — stage 8 deaths,
  node depletion — one tick after the removal), resets the runtime
  path, and the queue behind it advances; the unit falls back to its
  next order or pure auto-acquisition. Orders are canonical hashed
  state, so this is a behavior-visible fix (the demo golden is
  untouched: its lone Attack is gate-refused on the trivial world).

- **A-072 (§9.6, M9).** The wave machine's closing semantics — the
  M9 reading of "attack waves on timers and army-size thresholds".
  While a wave is alive (`Attacking`), the pressure timer re-issues
  the march for the whole living army at the cadence (WAVE_TIMER_TICKS,
  2400): a wave whose march orders drained (a `MoveFailed` choke, a
  completed march over a cleared target, a defense pull that brought
  the army home) still exists in the state machine, and "army ≥
  MIN_WAVE" is a liveness check, not an activity proof — a live wave
  must press or it parks. The march aims at the enemy's command center
  when it is in sight (`hunt_focus`, through the fog-filtered view —
  a hidden CC is not focused) and falls back to the enemy start. The
  mass tuning: waves of ten (from six) and an army cap of sixteen
  (from twelve), measured across a 32-seed sweep — every match
  resolves in 9.1k–23.3k ticks, inside the fifteen-minute budget.
  Waves of six never closed: they traded in low-count attrition
  cycles that never accumulated the damage to level a defended base.

- **A-073 (§14 M8/M9, §13).** "A 10–15 minute match resolves
  reliably" is read as a *ceiling*, not a floor: the match must end
  with a declared winner inside the fifteen-minute budget (27,000
  ticks at 30 Hz), after a real build phase (the resolution test also
  asserts the match ran past tick 3000 — no instant snowballs). The
  machine-verifiable form is `tests/alpha_loop.rs`: seeded AI-vs-AI
  matches (a typical and a slow-resolving seed) resolve within the
  budget; the same seed reproduces the winner, the end tick, and the
  bit-identical command log + final hash. AI-vs-AI resolution times
  (5–13 minutes in the sweep) are the machine's pace; a human's first
  vs-AI playthrough runs longer at human speed, which is the
  experience the window names.

- **A-074 (§11.5, M9).** The Alpha audio shape is the seam, not the
  sound: plan §11.5 says "an AudioSink trait fed by events, with a
  null implementation and optionally a few placeholder cues" — the
  M9 landing is exactly that (`engine::audio`: the trait, the pure
  `cue_for` event → cue mapping, the null counter sink; the client
  feeds it from the draw loop's event drain, FD-9 outward-only). The
  cues are named feedback moments (attack landed, unit lost, unit
  ready, structure done, delivery, match ended); spawn bookkeeping,
  rejections, path failures, and depletion carry no cue. An audible
  backend is DEBT-011 (post-Alpha, swapped behind the same trait) —
  "placeholder audio cues" means the cue vocabulary exists and flows,
  not that the Alpha ships sound.

- **A-075 (§11.2, §11.5, M9).** The feel pass's scope: the three cues
  a player needs to read the game through the placeholder art — hit
  flashes (an `AttackHit` victim pushes toward hot white for 18
  presented frames), health bars (the `hp_fraction_milli` field
  `RenderEntity` has carried since M6, drawn as backing + fill rects
  above the projected head, three color bands), and
  command-acknowledgment pings (a fading crosshair at the clicked
  ground position, the instant the command is issued). Tracer lines
  (§11.2's overlay list) are NOT included: `AttackHit` carries the
  target's position but not the attacker's, so an honest tracer needs
  either an event field addition or an attacker lookup the cue layer
  shouldn't do — deferred with the rest of the overlay layer (fog
  visualization, minimap) to the milestones that need them. All three
  cues are frame-indexed presentation state (DEBT-008's human pass
  judges whether they read well); the geometry is pure and
  unit-tested without a GPU.

- **A-076 (§10.4, M9).** Manifest finalization was a review, not a
  change: the M9 diagnosis attributed the unresolved matches to code
  (the frozen chase orders, the parked wave machine, the wave mass),
  and every §10.4 value survived the review unchanged — the content
  hash (0x249b69f0ee343a10) and the §10.4 pin in
  `tests/content_pipeline.rs` are unmoved by M9. The plan's
  "starter values — placeholders, tunable as data only" posture
  holds: tuning happened where the diagnosis said the defect lived.

- **A-077 (§11.3, M9.1).** The DEBT-008 human pass forced the input
  scheme's unresolved choices; these are the resolutions, all
  feel-judgments pending the human re-verification: WASD and the
  arrow keys pan (arrows alias the same key set); the pan speed is
  34 tiles/second scaled by the real frame delta (frame-rate
  independent; the M3 fixed per-frame speed was a refresh-rate
  property); edge scrolling runs at a 24-pixel margin, focused
  windows only; middle-drag is grab-the-ground panning (the world
  point under the press stays under the cursor); the wheel zooms
  toward the cursor with up = in; 'A' *arms* attack-move for the
  next left-click (SC2-style — firing on key-down fought the key's
  pan role and re-fired through OS key repeats, which are now
  filtered); Esc cancels an armed order before it ever quits the
  app; right-click always disarms. The plan's §11.3 letter ("pan by
  edge + keys, zoom toward the cursor, attack-move hotkey") is
  satisfied; the specific feels are the assumption.

- **A-078 (§11.3, M9.1).** Right-click context resolution order is
  enemy → resource node → open ground (plan §11.3's "move/attack/
  gather resolved from what's under the cursor"): an enemy under
  the cursor orders Attack for the whole selection; a node orders
  Gather for the selection's worker-kind units only (soldiers
  right-clicked onto a node walk there instead — the gather subset
  empty means the ground click wins); anything else (including
  clicking one's own units) orders Move to the picked ground
  point. Hit-testing uses the render snapshot (what is on screen)
  with the same 0.05 NDC radius as single-click selection; the
  command gate stays the sole authority — a fog-hidden target
  rejects NotVisible and the refusal cue surfaces it. Workers and
  nodes are identified by kind id through the engine's alpha-plan
  resolution (capability-shaped, never name-matched), reusing the
  seam the AI host already owns.

- **A-079 (§11.3, M9.1).** The opening camera frames the human
  player's start anchor (from the map's declared starts) at 26
  tiles distance, re-framed on restart — the M3 map-center default
  at 0.9× the map's diagonal (58 tiles on Crossroads) rendered the
  starting force as unreadable specks, which the human pass
  reported as "nothing appears on the screen". Panning clamps the
  orbit target to the map rectangle (zero margin) so the view can
  never wander off the battlefield. 26 tiles is a readability
  judgment (the base and its workers fill the lower-center of the
  frame at 16:9); the human re-verification judges it.

- **A-080 (§8.4, M9.1).** Rejected orders surface as a red hollow
  square at the click point plus one HUD line ("order refused:
  <reason>", a total, player-facing mapping of every RejectReason),
  both living 60 presented frames. The client remembers only the
  most recent order's click point, so a refusal pings where that
  order was clicked; any earlier order rejected in the same step
  still bumps the HUD line and the counter but draws no square.
  The M9.1 rationale: the plan's gate refuses illegal orders by
  design, and dropping those events on the floor read, to the first
  human player, as "the controls don't work".

- **A-081 (§9.6, §6.4, M10).** `PlayerView` grew two fields: the player's own
  per-tile fog row (`TileFog`, row-major in the map's tile order) and the
  player's own production queues (producer, items with per-item
  `progress_milli`, rally). Rationale: the boundary is "what that player may
  know", and a player knows their own fog state and their own queues; the
  presentation needs both (the fog decal + minimap, the §11.4 queue display),
  and parity holds structurally because both fields flow through the same
  view the AI reads — the AI need not use them. The fog row mirrors derived
  state (A-059) and is hashed nowhere; the state hash and every golden pin
  are unchanged.

- **A-082 (§11.2, M10).** The fog visualization is a terrain-geometry decal
  sampling a per-tile alpha texture (hidden 235/255, explored 130/255,
  visible 0) with linear sampling for soft edges. The same geometry gives the
  same depth values, so the fog hugs the hills without z-fighting. The alpha
  constants are readability judgments the human pass may retune.

- **A-083 (§11.3, M10).** The build-placement ghost's legality preview is a
  client-side approximation: bounds, buildable terrain, and static bodies the
  player can see — it does not check movers standing in the footprint (the
  sim's check does). The gate is the authority; a wrong guess surfaces as the
  standard refusal cue. Logged as DEBT-012.

- **A-084 (§11.4, M10).** The command card's train buttons dim when the
  player cannot afford the kind or a requirement kind is not among their
  visible entities. "Requirement visible" is an approximation (a
  still-under-construction requirement passes); the gate re-checks
  everything. Logged as DEBT-012.

- **A-085 (§11.4, M10).** The minimap composites its texture on the CPU each
  frame from sanctioned data (the terrain mesh's own palette, the PlayerView
  fog row, and the fog-filtered snapshot's entities), sized one texel per
  tile. The viewport indicator derives the frustum's ground footprint
  analytically from the camera (RtsCamera::ground_footprint_corners) with the
  shallow-pitch top-ray angle clamped so corners stay finite; the indicator
  clamps to the map, which reads honestly at pitches where the real frustum
  leaves the ground.

- **A-086 (§13 A7, M10).** "Stuck" is the plan's own threshold — 20 minutes
  of game time = 36000 ticks at 30 Hz — not the soak tool's original 18000
  default. The M9 sweep measured legitimate matches resolving as late as
  tick ~23300, so the smaller budget miscounted long-but-legal matches as
  stuck (an evidence run showed 4 of 32 "unresolved" that all resolve by
  36000). The default is now 36000 and the report's end-tick telemetry reads
  the tick `MatchEnded` fired.

- **A-087 (§6.4/§6.5, DEBT-001, M10.1).** The canonical hasher swapped
  FNV-1a → xxHash64 (DEBT-001 repaid) without bumping
  `STATE_ENCODING_VERSION` or the replay `format_version`: the canonical
  byte encodings (field set, order, little-endian form) are unchanged —
  only the digest algorithm moved, so an old reader's layout knowledge
  stays valid. No `*.pdrp` replay files existed anywhere at swap time
  (verified by search), so nothing in the wild can hold a stale digest.
  The replay file's trailing *integrity checksum* deliberately remains
  FNV-1a: it guards the file's bytes (a file-format concern), not the
  simulation's canonical state, and swapping it would move the format
  for no acceptance value. The swap is the pass's one hash-moving
  commit; every subsequent task re-verifies zero golden movement
  against the new pins.

- **A-088 (§11.6, M10.1).** The F8 bug-report dump's shape: a
  `pandemonium-report-<seed>-tick<tick>.pdrp` replay record (built exactly
  as the tools' recorder builds one, forced final at the dump's tick) plus
  a sidecar `-info.txt` carrying seed, tick, content hash, map id, player
  slot, frame count, selection size, and the controls line in effect
  (interpreted as the *input mode*: the armed-attack-move instruction, a
  showing refusal notice, or the standing controls summary — the state a
  reproducer needs). The sidecar's frame count is presented-frame state, so
  two dumps at the same tick but different frames differ in exactly that
  field; the "same tick → identical bytes" property is the pure-function
  determinism over identical state (pinned by `report.rs`'s unit test —
  no wall-clock, no paths, no environment), and the `.pdrp` itself is
  byte-identical across same-tick presses (verified windowed). The dump
  writes beside `--record`'s path when given, the working directory
  otherwise.

- **A-089 (§6.5, M10.1).** `--record`'s write points: at the match's end
  (first `is_finished()` — the log is complete there) and at clean exit
  (every exit path funnels through the winit `exiting` hook — window
  close, Escape, the `--frames` budget — writing only when the match never
  wrote its record). A restart starts a fresh segment: the checkpoint trail
  and the written-flag reset with the host, so the file on disk always
  describes the most recent completed or in-progress segment. Each write
  overwrites the path — the recording is "the run so far", not an archive.

- **A-090 (§12, M10.1).** The nightly soak's shard shape — 10 shards × 100
  matches — reads plan §12's "in parallel processes" at the process level
  (the soak binary itself stays sequential; the sequential runner is the
  tested artifact). 10×100 keeps every shard's runtime inside a 90-minute
  timeout at the measured ~800-830 ticks/s (~1.6M ticks per shard ≈ 33
  minutes), leaves headroom for a cold build (~15-20 min) and slower
  runners, and makes the seed intervals round ([BASE+100k, BASE+100k+100),
  union [BASE, BASE+1000) — the same 1000 consecutive seeds the sequential
  run covered). The smoke tier's seeds keep overlapping shard 0's first 32,
  exactly as they overlapped the unsharded tier before it. The optional
  `--json` summary flag was skipped: the tee'd text summary is the
  evidence, and the flag was explicitly allowed to be dropped.

- **A-091 (PLAN-M10.2 §1.1, M10.2).** The plan's "Click on empty ground with
  a selection and nothing else = nothing happens" is read literally: a plain
  left click on empty ground neither orders (left never orders — that part
  was already true) *nor clears the selection*. Generals ZH players may
  expect ground-click-to-deselect; the plan's sentence wins, and
  deselection stays available through Esc (the §1.3 Escape ladder) and an
  empty box-drag (a drag is "something else"). Logged because a future
  playtest may report "I can't deselect by clicking the ground" — that is
  this assumption, not a bug.

- **A-092 (PLAN-M10.2 §1.2, M10.2).** The right button's command/drag
  threshold is 6.0 px of Euclidean travel (the plan says "about 6"), and
  exactly 6.0 px counts as a command — the drag must be unambiguous. The
  threshold applies to the press→release travel; a drag that crossed it and
  returned to the press point is still a scroll (the release orders
  nothing), and a press whose ground point is off-screen (above the
  horizon) re-grabs at the crossing so a sky-started drag still scrolls.

- **A-093 (PLAN-M10.2 §1.2, M10.2).** Middle-drag rotation is
  horizontal-only, at a fixed 0.005 rad/px, direction pinned to match E
  ("orbit right"): dragging right increases yaw at every camera angle
  (pinned at yaw 0 and 90). Vertical middle-drag travel is inert — the
  plan says "rotates", and pitch already has Ctrl+wheel; vertical drag
  rotation would fight the wheel's muscle memory.

- **A-094 (PLAN-M10.2 §1.2, M10.2).** The edge-scroll band is 14 px (the
  plan says "about 12–16"), speed scales *linearly* with the cursor's depth
  into the band (the old M9.1 zone snapped to full speed at 24 px, which
  the owner read as broken), and depth is measured to the outermost pixel
  (`w-1` / `h-1`) so the last pixel row scrolls at full speed. The band is
  on by default; the *toggle* is PLAN §3/Phase 3's settings screen (the
  `edge_scroll_enabled` flag is wired now, DEBT-014 holds the UI).

- **A-095 (PLAN-M10.2 §1.3, M10.2).** The double-click and double-tap
  windows are both 350 ms (the plan names neither; Generals uses a similar
  few-hundred-millisecond window). A double click requires the second click
  to *pick* an entity — two quick clicks on empty ground are two
  nothing-happens clicks (A-091). A control-group *assign* (Ctrl+digit)
  breaks the double-tap chain: the next double tap must be two recalls.

- **A-096 (PLAN-M10.2 §1.3, M10.2).** "Double-click a unit selects all
  visible units of the same kind" means same kind AND same owner, drawn
  from the fog-filtered view snapshot (so "visible" is the fog's truth);
  enemy units never join a selection (selection itself stays own-only,
  the M6-era placeholder rule). A double-clicked *building* expands to
  all visible buildings of that kind too — the plan says "units", but
  kind-parity is the honest reading of the mechanism and it is harmless.

- **A-097 (PLAN-M10.2 §1.3, M10.2).** Space ("jump to the last event or
  base", optional) jumps to the most recent **death** — the event class a
  player most wants to look at — else the player's start anchor, keeping
  the current zoom distance. Attack hits were rejected as the tracked
  class: they fire many times per combat tick and the jump target would
  thrash. The command ping's position was rejected as a fallback: the
  player already knows where they clicked.

- **A-098 (PLAN-M10.2 §1.3, M10.2).** The cursor-feedback "invalid" state
  (the plan offers "Move / Attack / invalid") is *silence*: no marker when
  nothing is orderable (empty selection, placement mode owns the cursor,
  or no ground under the cursor). A permanent gray marker under an idle
  cursor would be noise, not feedback; the armed attack-move gets the
  crosshair *window cursor* as its mode cue, and its ground marker is
  orange (distinct from the red Attack preview).

- **A-099 (PLAN-M10.2 §1.2, M10.2).** Right-button release routing, per
  the state machine: a sub-threshold release over the bottom-bar buttons
  is swallowed (UI is not a world order — the old code only guarded the
  left button); a sub-threshold release over the minimap orders at the
  mapped ground point, as Move — or as AttackMove when attack-move was
  armed at the *press* (fixing the M9.1 dead branch the §1.1 audit found:
  the old code disarmed before the minimap check read the flag); a
  right-press that cancels a placement consumes the whole gesture (the
  release is a no-op, matching the old behavior).

- **A-100 (PLAN-M10.2 delivery, M10.2).** The owner's delivery answers for
  this pass (recorded where the plan leaves the choices open): the
  delivery covers **Phase 1 (Controls) only** — Phases 2–4 (visual
  legibility, menus/settings, audio) wait for the owner's re-test verdict;
  the Phase 4 audio backend, when that phase runs, is **rodio** (an
  external-crate allow-list amendment + ADR is required then — the
  allow-list is untouched by this pass); edge scrolling defaults to on
  with the 14 px band; and the patch's commits are authored as
  `elieaazzam-art <elieaazzam-art@users.noreply.github.com>` (the owner's
  account noreply form, so GitHub attributes them on import; the owner
  can `git am` and `--reset-author` if they prefer their local identity).

- **A-101 (PLAN-M10.2 kickoff, M10.2 Phase 2).** The owner's kickoff answers
  for the Phase 2 pass: the Phase 1 controls re-test is reported as
  **"passed with notes"** — the notes themselves arrive after this delivery
  (the owner re-tests on their own machine and reports exact locations of
  any issue), so `docs/PLAYTEST.md` carries the re-test row with that
  caveat. The scope is **Phase 2 only** (the owner's re-test gates Phase 3,
  as with Phase 1). Blocker policy: best effort + a log entry, never a
  silent skip.

- **A-102 (PLAN-M10.2 §2.2, M10.2 Phase 2).** Team palette: the owner chose
  **blue/orange** (the plan's colorblind-safe preference). The client
  resolves P1's content-red to orange — the mapping lives entirely in the
  presentation layer (`render::team_color` and the minimap dot table);
  content files and the content hash are untouched. P0 keeps a blue in the
  content's family and neutral keeps gold for ore.

- **A-103 (PLAN-M10.2 §2.1, M10.2 Phase 2).** The kind → silhouette mapping
  is keyed by the bundle's kind *name* (`EntityDef::id`) with a
  capability-shape fallback (Resource → amber cluster, Attack+Footprint
  without Move → turret, Footprint → building box with a team band, Move →
  unit box, else a plain unit box). The nine hand-tuned specs assume the
  authored footprints (CC 4×4, barracks 3×3, depot/turret 2×2); a content
  footprint change would need the client spec re-tuned (DEBT-016). All
  silhouettes are pure client data (`client/src/silhouette.rs`).

- **A-104 (PLAN-M10.2 Phase 2 exit, M10.2 Phase 2).** Evidence: the owner
  opted for a **text-only delivery** — no PNG screenshots are uploaded to
  the delivery folder. The Xvfb visual pass still runs (or its blocker is
  logged honestly) so the machine half of "a stranger identifies command
  center, worker, and tank" is exercised; the human verdict is the owner's
  re-test on their display.

- **A-105 (PLAN-M10.2 delivery, M10.2 Phase 2).** The delivery folder is
  `m10.2-phase2/` in the owner's delivery repo — the plan's generic
  "folder m10.2/" wording is refined to keep the Phase 1 delivery
  (already in `m10.2/`) and this one distinct. The patch, a README.txt
  with the base hash + apply command, and nothing else land there.

- **A-106 (PLAN-M10.2 §2.3, M10.2 Phase 2).** The single-selection info
  panel gains a production-queue summary (head item name + progress, count
  of the rest) next to the existing queue strip above the command card —
  the panel shows *what the selected building is doing*, the strip keeps
  the per-item cancel buttons. The hover tooltip shows the display name
  plus an owner tag (Own/Enemy/Neutral) near the cursor; it is suppressed
  over the bottom bar and the minimap, where the cursor means UI, not
  world.
