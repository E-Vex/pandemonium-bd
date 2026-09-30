# Pandemonium — RTS Foundation & Alpha Plan (Rust + Custom Engine)

Audience: an autonomous AI coding agent (and the human reviewing its work). Status: implementation-ready. Supersedes v1.0 (Sept 2026). Stack decision (final): Rust, with our own engine (own game loop, own entity store, own renderer layer on top of low-level crates). No off-the-shelf game engine or ECS framework.

## 0. Operating Contract for the Agent

Read this section first and re-read it at the start of every milestone.

- You are building a foundation, not a game. The Alpha is judged by system properties (Section 13), not content volume. If a task tempts you to add content, features, or polish that no acceptance test requires, do not.
- Work milestone by milestone (Section 14). Never start milestone N+1 until every exit test of milestone N is green in CI. The game must build and run at the end of every milestone.
- Every task ends with: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. Commit only when green. One logical change per commit.
- Foundational decisions (Section 2) are frozen. If you believe one is wrong, stop and write an ADR (docs/adr/NNNN-title.md) proposing the change, with evidence. Do not change it silently.
- Keep a debt register at docs/DEBT.md. Every simplification you make gets a row: what, why, category, replacement plan, trigger for repayment.
- Log decisions you had to guess in docs/ASSUMPTIONS.md. Prefer a small, reversible guess plus a log entry over stopping to ask, except for anything touching Section 2.
- Prove, don't assert. Any claim ("deterministic", "AI has parity", "add-a-unit is data-only") must be backed by an automated test that exists in the repo.
- Placeholder art only. Colored shapes and simple sprites. No time on graphics.

## 1. Vision

### 1.1 The Governing Question

Are we building a game, or a foundation that can become a much larger game? The second. Architecture comes first; content is a consequence.

### 1.2 The Long Arc

Core Foundation → Small Alpha → Continuous Updates → New Units → New Armies → New Buildings → New Maps → New Mechanics → Better AI → Better Balance → Better Graphics → Expanded Multiplayer → Mature RTS. Each arrow must be cheap: grow without rebuilding.

### 1.3 Design Pillars

- Systems are DNA; content is expression. Systems know how to move, fight, gather, produce, win. Units, buildings, factions, maps are data using those systems.
- Spend care where change is expensive. Rigor on the simulation, commands, entity identity, and data boundary. Stay lightweight on visuals, UI layout, and balance numbers.
- Small, honest, always playable. Every build runs; every system is testable in isolation.

### 1.4 The Minecraft Standard

Early Minecraft had few features, but its block/world abstraction absorbed years of growth. Our "blocks" are: the simulation loop, the entity model, the command system, and the data boundary. The Alpha is the smallest world exercising all four together.

### 1.5 What the Alpha Must Prove (eleven questions)

(v1 said "ten" but listed eleven; corrected.)

1. Is the core loop (economy, production, movement, combat, territory) fun in primitive form?
2. Does commanding units feel good?
3. Is intent-to-visible-response short enough to feel instant?
4. Is movement reliable for single units and groups of dozens?
5. Is combat meaningful: do build/position/timing choices change outcomes?
6. Can a player read the battlefield without a tutorial?
7. Does the economy allow genuinely different strategies?
8. Does the AI use exactly the same systems/commands as the player?
9. Can a new unit be added without touching core code?
10. Can a new faction be added without rebuilding a major system?
11. Can a new mechanic be added later without destabilizing existing ones?

### 1.6 Anti-Goals (the Alpha is NOT judged on)

Number of units/buildings/factions/maps; graphics quality; campaign/story; any online feature; balance depth. Any proposal that mainly improves these is out of scope.

## 2. Frozen Foundational Decisions

| ID | Decision | Consequence |
|----|----------|-------------|
| FD-1 | Deterministic fixed-tick simulation, 30 ticks/s, decoupled from rendering. | A match = seed + content hash + ordered command log. Replays, headless soak tests, and lockstep multiplayer follow. |
| FD-2 | All intent enters through validated Command records applied at tick boundaries. Players, AI, replays, and (later) network all use the same door. | No code path may mutate the simulation except the tick function. |
| FD-3 | Unified entity model: base record + composable capability blocks. IDs are 64-bit, monotonic, never reused. | New entity types are data. Systems iterate by capability, never by type name. |
| FD-4 | Strict data/code boundary. Stats, costs, requirements, maps, factions = versioned data files. Simulation tick order, command semantics, capability behavior = code. | "Add-a-unit" and "add-a-map" require zero engine changes. |
| FD-5 | No floating point in the simulation or in content files. Fixed-point / integer math only. | Bit-identical results across OS, CPU, and compiler versions. |
| FD-6 | Simulation depends on nothing above it (no renderer, window, audio, clock, filesystem, or OS randomness). Enforced by crate boundaries. | Renderer/UI can be replaced without touching rules. |
| FD-7 | AI touches the game only through commands and a fog-filtered PlayerView. | Parity is structural, not a policy. |
| FD-8 | Fog filters information; it never alters the simulation. | Replays and AI stay honest. |
| FD-9 | Events flow outward (sim → presentation/audio/telemetry). Presentation never polls sim internals. | Feedback layer can grow without sim changes. |
| FD-10 | Content schema only moves forward: versioned, validated, old content keeps loading. | Content survives years of growth. |

## 3. Technology & the "Own Engine" Definition

### 3.1 What "our own engine" means here

We write, ourselves: the game loop and timing, the entity/capability store, the simulation, the fixed-point math library, the deterministic RNG, pathfinding and steering, the command/replay system, the content loader/validator, the renderer abstraction (2D batch renderer), the camera, the input mapping, the UI toolkit (rects, text, buttons, minimap, panels), and dev tooling.

We use only low-level crates for OS/GPU access and utilities.

### 3.2 Dependency policy

Allowed (with purpose):

| Crate | Used by | Purpose |
|-------|---------|---------|
| winit | client only | Window and input events |
| wgpu (+ pollster, bytemuck) | client only | GPU rendering |
| glam | client, engine | Presentation-only float math (camera, transforms, picking). Never in the sim's tree (ADR-0001). |
| fontdue or equivalent | client only | Glyph rasterization for our own text renderer |
| image (png only) | client, tools | Loading placeholder sprites |
| cpal (+ own mixer) | client, post-alpha | Audio output (stub in Alpha) |
| serde, ron | content | Content files (RON) |
| thiserror, anyhow | libs / bins | Errors |
| clap | tools | CLI |
| proptest, criterion | dev-deps | Property tests, benchmarks |
| egui (optional) | client, feature debug-ui only | Developer overlays, never player UI |

Forbidden: bevy*, macroquad, ggez, fyrox, godot-rust, any ECS crate (hecs, legion, specs, shipyard, flecs), physics or pathfinding crates (rapier, pathfinding, etc.), and any crate that pulls floats or non-determinism into sim.

Rule for sim and sim_api crates: dependencies limited to core/alloc/std collections that have deterministic iteration, plus thiserror. No serde in sim (snapshots use our own canonical byte encoding, see 6.4). content produces plain Rust structs that sim receives.

### 3.3 Platform assumptions (log in docs/ASSUMPTIONS.md, adjust if the human says otherwise)

Desktop PC (Windows, Linux, macOS), **3D perspective presentation over the 2D logical ground plane** (ADR-0001: the simulation stays 2D fixed-point; 3D is a presentation-layer concern only), single-player Alpha, mouse + keyboard. Rust stable, edition 2021 or newer, pinned by rust-toolchain.toml.

## 4. Workspace Layout & Dependency Law

```text
pandemonium/
├─ Cargo.toml                (workspace)
├─ rust-toolchain.toml
├─ crates/
│  ├─ fx/          fixed-point math (Fx, Vec2Fx, isqrt, lookup tables), deterministic PRNG, canonical hasher
│  ├─ sim/         world state, entity store, capabilities, tick pipeline, systems, events, state hash
│  ├─ sim_api/     public types shared outward: EntityId, PlayerId, Command, CommandKind, Event, PlayerView, Reject
│  ├─ content/     RON schema, versioned loaders, validators, content bundle + content hash → plain structs
│  ├─ ai/          AI controllers (depend on sim_api only)
│  ├─ replay/      replay file format: record, load, verify
│  ├─ engine/      game loop, timing, snapshot interpolation, input→command mapping, camera, UI toolkit, render abstraction
│  ├─ client/      the playable binary: wgpu renderer, winit window, screens, HUD, audio stub
│  └─ tools/       bins: headless runner, replay verifier, content validator, soak runner, bench
├─ content/        RON data: factions/, entities/, maps/, rules/
├─ docs/           adr/, DEBT.md, ASSUMPTIONS.md, ARCHITECTURE.md, CONTENT_GUIDE.md
└─ tests/          cross-crate acceptance tests
```

Dependency law (dependencies point downward only; enforced by an automated test that parses cargo metadata):

```text
client ─► engine ─► sim_api        tools ─► sim, content, ai, replay
   │        │           ▲            ai ─► sim_api        replay ─► sim_api
   └────────┴──► content ─► (plain data) ─►  sim ─► fx, sim_api
```

- sim must NOT depend on: engine, client, ai, replay, winit, wgpu, any I/O crate.
- ai must NOT depend on sim (only sim_api). This makes "AI reaches into game state" a compile error.
- client never mutates sim; it submits Commands and reads snapshots/events.

## 5. Determinism Rules (Rust-specific, mandatory)

Enforced by lints, tests, and code review. Put `#![deny(clippy::float_arithmetic, clippy::disallowed_types, clippy::disallowed_methods)]` in fx, sim, sim_api, ai, and configure clippy.toml.

1. No floating-point types in sim, sim_api, ai, content structs, or content files. Authored speeds, ranges, and durations are integers (see 10.2).
2. No unordered-map / unordered-set types in sim state or in any iteration that affects state. Use ordered maps, dense Vecs, or our own stable stores. (clippy.toml disallows them in those crates.)
3. Iteration order is always ascending EntityId (or ascending player slot), documented per system and tested.
4. All randomness comes from fx::Rng (PCG32 or xoshiro128**), seeded from the match seed, owned by the sim state, advanced in a fixed order. No rand crate, no OS entropy, no wall-clock time in sim.
5. No usize, isize, or pointer-width-dependent values in hashed or serialized state. Use u32/u64/i32/i64. Convert at boundaries.
6. Integer overflow is defined: use checked_*, saturating_*, or wrapping_* explicitly in the sim; build tests in both debug and release so overflow behavior can't diverge.
7. No threads inside the sim tick in the Alpha. (Parallel soak runs are separate processes/matches.) Any future parallelism must be proven bit-identical by the hash test first.
8. No trigonometry, sqrt of floats, or division by non-constant floats. Use squared-distance comparisons; isqrt (integer) where a length is needed; facing is a direction vector, not an angle.
9. Sorting must be stable and use total-order keys (no float keys, no partial comparisons). Tie-break by EntityId.
10. State hash is computed over a canonical byte encoding (little-endian, fixed field order) with our own hasher (e.g. xxHash64/FNV-1a implemented in fx), never over Debug output or memory layout.

## 6. Simulation Core

### 6.1 Fixed-point math (fx crate)

- Fx(i32) Q16.16 for positions/distances in tile units; i64 intermediates for multiply/divide with defined rounding (round toward zero, documented).
- Vec2Fx { x: Fx, y: Fx } with add/sub/scale/dot, len_sq(), isqrt-based len(), normalized() using integer math.
- Property tests: associativity where expected, round-trip of conversions, no panics on extremes (proptest).

### 6.2 State and tick

```rust
pub const TICKS_PER_SECOND: u32 = 30;

pub struct Sim {
    tick: Tick,
    rng: fx::Rng,
    world: World,          // map grid, entity store, capability stores, players
    next_entity_id: u64,   // monotonic, never reused
    events: Vec<Event>,    // drained by the caller each tick
}

impl Sim {
    pub fn new(content: &ContentBundle, setup: MatchSetup) -> Self;
    /// The ONLY way state advances. Commands must all be for `self.tick`.
    pub fn step(&mut self, commands: &[Command]) -> StepOutput; // events + optional hash
    pub fn state_hash(&self) -> u64;
    pub fn snapshot(&self) -> Snapshot;               // read-only, for rendering
    pub fn player_view(&self, p: PlayerId) -> PlayerView; // fog-filtered, for AI/UI
}
```

### 6.3 Fixed update order inside step (never varies per content item)

1. Apply commands — sort by (issuer slot, seq); validate; apply valid ones; emit CommandRejected for invalid.
2. Orders — advance each entity's order queue (resolve current order into movement/attack/gather/build intents).
3. Production & construction — advance queues and construction progress; spawn/complete.
4. Economy — gathering timers, deliveries, storage, spending effects.
5. Target acquisition — attackers with no valid target select one by acquisition rules (visible + targetable only).
6. Movement — path requests → path following → steering → collision push-apart (entities processed in ID order).
7. Combat — resolve attacks through the pipeline (9.2), apply damage, mark deaths.
8. Death & cleanup — fire lifecycle events, remove dead entities (IDs are never reused).
9. Vision — incremental per-player visibility update.
10. Match rules — evaluate defeat/victory conditions.
11. Finalize — increment tick, flush events, compute periodic hash (every 30 ticks and on demand).

### 6.4 Snapshots and hashing

Snapshot is a plain read-only copy of what presentation needs (entity id, owner, kind id, position, facing vector, hp fraction, state, footprint, visibility flags). The client keeps the previous and current snapshot and interpolates for smooth rendering at any frame rate.

state_hash() encodes: tick, RNG state, next_entity_id, all entities in ID order with all capability data, player resources/pop, visibility bitsets, in canonical byte form.

### 6.5 Command log, replay, and lockstep readiness

Every command carries tick (target tick). In single-player, the client submits for current_tick + 1. This same field is what lockstep will use for input delay later.

Replay file: { format_version, content_hash, map_id, seed, player_setup (controller kinds), commands[], checkpoints[(tick, hash)], final_hash }.

tools replay-verify <file> re-simulates headless and compares every checkpoint hash.

## 7. Entity Model

### 7.1 Everything that exists is an entity

Units, structures, resource nodes, (future) projectiles, neutrals, objectives.

### 7.2 The seven aspects

| Aspect | Alpha implementation | Must tolerate later |
|--------|----------------------|---------------------|
| Identity | EntityId(u64), monotonic, never reused | replays, networking, save/load, mods |
| Ownership | PlayerId(u8) slot; Neutral sentinel | capture, teams, shared control |
| Position | tile footprint (structures), Vec2Fx (units), facing vector | irregular footprints, air layer |
| State | small explicit enum per system; no ad hoc bool flags | status effects, transforms |
| Capabilities | composable data blocks (7.3) | new capability types without touching the spine |
| Interaction | targetable (by ownership + vision), damage reception | capture, repair, transports |
| Lifecycle | Spawn → Active → Dead → Cleaned, with events at each step | construction sites, wreckage, revival |

### 7.3 Capabilities (code defines behavior, data defines parameters)

```rust
pub struct Entity { pub id: EntityId, pub owner: PlayerId, pub kind: KindId,
                    pub pos: Vec2Fx, pub facing: Vec2Fx, pub life: Lifecycle }

pub enum CapabilityData {          // one variant per capability type
    Health(HealthDef), Move(MoveDef), Attack(AttackDef), Gather(GatherDef),
    Build(BuildDef), Produce(ProduceDef), Storage(StorageDef),
    Resource(ResourceDef), Vision(VisionDef), ProvidesPopulation(PopDef),
    Footprint(FootprintDef), // ...extend by adding a variant + a system
}
```

Runtime storage: one store per capability type, keyed by EntityId, iterated in ascending ID order. Implementation choice is yours (sorted sparse set, generational-free dense array + id→index map, etc.) as long as (a) iteration order is deterministic and tested, (b) removal doesn't reorder survivors unpredictably, and (c) lookup by ID is O(1) or O(log n).

Systems query by capability, never by kind name. A grep test forbids string/enum matches on unit kind names inside crates/sim/src/systems/.

Litmus test: adding a new entity type must require touching only its own content file. If not, the model has failed and must be fixed before more content is added.

### 7.4 Alpha entities as capability compositions

```text
Worker         = Base + Health + Move + Gather + Build + Vision
Rifleman       = Base + Health + Move + Attack + Vision
Raider         = Base + Health + Move + Attack + Vision   (fast, short range)
Guardian       = Base + Health + Move + Attack + Vision   (slow, heavy, long range)
Command Center = Base + Health + Footprint + Produce(Worker) + Storage(Ore) + ProvidesPopulation + Vision
Barracks       = Base + Health + Footprint + Produce(Rifleman|Raider|Guardian) + Vision
Supply Depot   = Base + Health + Footprint + ProvidesPopulation + Vision
Turret         = Base + Health + Footprint + Attack + Vision      (no Move)
Ore Node       = Base + Footprint + Resource(Ore, finite)
```

## 8. Command System

### 8.1 Types (in sim_api)

```rust
pub struct Command {
    pub issuer: PlayerId,
    pub tick: Tick,        // tick at which it is applied
    pub seq: u32,          // per-issuer sequence for stable ordering
    pub queue: bool,       // append to order queue instead of replacing
    pub kind: CommandKind,
}

pub enum CommandKind {
    Move        { units: Vec<EntityId>, target: Vec2Fx },
    Attack      { units: Vec<EntityId>, target: EntityId },
    AttackMove  { units: Vec<EntityId>, target: Vec2Fx },
    Stop        { units: Vec<EntityId> },
    Gather      { units: Vec<EntityId>, node: EntityId },
    Build       { worker: EntityId, structure: KindId, at: TilePos },
    // Added in v2 (v1 described queues, cancellation and rally but had no commands to drive them):
    Train       { producer: EntityId, unit: KindId },
    CancelQueueItem { producer: EntityId, index: u16 },
    SetRally    { producer: EntityId, target: Vec2Fx },
    Resign      { },
}
```

### 8.2 Flow

Issuer → Command record → validation gate → tick-boundary queue → apply → events. Validation is one function shared by everyone: checks ownership of every referenced entity, target legality (exists, targetable, visible to issuer), affordability (resources, population), placement legality (footprint free, buildable tile), capability presence (a unit without Attack can't be ordered to attack), and requirement lists. Invalid commands produce Reject { reason } events and change no state.

### 8.3 Extension rule

A new command = new enum variant + payload validation + handler. It must not require changes to input handling, the validation gate structure, or command application order.

### 8.4 Feel budget

A valid command produces visible feedback within one tick (33 ms): selection flash, facing change, or acknowledgement cue. Client-side, click feedback (ping/marker) is instant and does not wait for the sim.

## 9. System Specifications

### 9.1 Movement (three layers with strict contracts)

1. Navigation layer — grid pathfinding: A* on an 8-connected tile grid, no corner cutting, straight-line fast path when unobstructed, path cache keyed by (start region, goal). Deterministic tie-breaks: (f, h, tile index) in a total-order heap key. Swappable later for flow fields / hierarchical pathing behind the same trait (Navigator).
2. Path execution layer — steering along waypoints per tick.
3. Collision layer — deterministic local push-apart using a spatial hash grid (cell lists ordered by ID); units have integer radii from data; structures block tiles.

Budgets (tested): visible motion within 2 ticks of a valid Move; no unit stuck permanently (blocked > threshold → repath escalation → give up with MoveFailed event); groups of 50 spread without thrashing.

### 9.2 Combat (fixed pipeline; the shape must absorb years of content)

Acquire target → Validate (range, visible, targetable) → Wind-up (0 ticks in Alpha) → Hit → Mitigate (identity in Alpha) → Apply damage → Death → Credit. Alpha: instant-hit with tracer event (presentation draws the tracer), integer damage, cooldown in ticks, acquisition range with priority: commanded target > units that attack > other units > structures. Every stage is a named function so future armor, projectiles, and abilities insert at a stage.

Legibility (tested by human playtest checklist): a spectator can tell which side is winning within seconds; team colors, health bars, hit/death events produce clear cues.

### 9.3 Economy

Resource registry keyed by ResourceId (Alpha has one: Ore) so a second resource is a data addition. Worker loop: go to node → gather (timer) → carry → return to nearest storage → deposit. Nodes deplete; depletion emits an event and workers auto-seek the nearest node. Per-player resource ledger with can_afford / spend / refund.

### 9.4 Production & construction (one model, two surfaces)

Any entity with Produce owns an ordered queue of {producible, cost_paid, progress_ticks, ...}. Costs are paid on enqueue and refunded on cancel. Population headroom is checked at enqueue and at spawn. Construction is the same progress model with a builder committed; a structure is an entity in a UnderConstruction lifecycle state until complete. Requirements are a list evaluated by one checker (e.g. "requires structure X"), used by both surfaces and by population.

### 9.5 Vision & fog

Per player per tile: Hidden / Explored / Visible. Circular vision radius (from Vision capability) in Alpha; updated incrementally as vision-carrying entities move/spawn/die; full recompute only at match start. Targeting rejects entities not visible to the issuer. Fog never alters simulation (FD-8): hidden entities behave identically.

### 9.6 AI (ai crate)

```rust
pub trait Controller {
    fn think(&mut self, view: &PlayerView, tick: Tick, out: &mut Vec<Command>);
}
```

Three stages with clean seams: Perceive (read PlayerView), Decide (state machine), Act (emit commands).

Alpha behavior: scripted build order (workers → depot → barracks → mixed army), expansion-free, attack waves on timers and army-size thresholds, defend when its base is attacked.

- The AI runs on the same tick boundary as the player and its commands go through the same validation. Its economy obeys the same costs/times.
- Any exception to parity (e.g. full-map info for testing) must be a named, documented, toggled option in docs/ADR, defaulting off.
- AI is deterministic: seeded RNG passed in, never its own entropy.

### 9.7 Match rules

Start conditions from map data; defeat = a player has zero structures (data-defined condition consulted by code); victory = all opponents defeated or resigned; restart = new Sim from the same setup with a new seed, with no leaked state (tested by hash equality of two fresh sims).

### 9.8 Events (sim → outside)

Typed enum: Spawned, Moved?, AttackHit, Died, ProductionStarted/Completed, ConstructionStarted/Completed, ResourceDelivered, NodeDepleted, CommandRejected, MoveFailed, MatchEnded, .... UI, audio, telemetry, and replay tooling consume events; nothing polls sim internals.

## 10. Content & Data

### 10.1 Boundary table (what lives where)

| Item | Lives in | Note |
|------|----------|------|
| Unit/building stats, costs, build times, requirements | Data | designer-editable |
| Faction as a bundle of entity references | Data | second faction = duplication |
| Maps: grid, terrain, spawns, resource nodes | Data | map = file, nothing else |
| Match conditions parameters | Data | victory evaluation is code |
| Tick order, command semantics, capability behavior | Code | never varies per item |
| Pathfinding/steering internals | Code | swappable layer |

Do not turn content into a scripting language. If a rule needs logic, add a capability or a hook in code; data only selects and parameterizes.

### 10.2 Authoring units (no floats anywhere)

- Durations: milliseconds (u32), converted at load to ticks with integer math: ticks = (ms * 30 + 999) / 1000.
- Speeds: milli-tiles per second (i32), converted to fixed-point per tick at load.
- Ranges/radii: milli-tiles.
- Costs, hp, damage, population: plain integers.
- Every file starts with schema_version: N. Loaders migrate old versions forward; unknown fields are errors in strict validation mode.

### 10.3 Example content record (RON)

```ron
(
  schema_version: 1,
  id: "rifleman",
  display_name: "Rifleman",
  capabilities: [
    Health(( max: 60 )),
    Move(( speed_milli_tiles_per_s: 2400, radius_milli_tiles: 350 )),
    Attack(( damage: 8, range_milli_tiles: 5000, cooldown_ms: 1000, acquire_range_milli_tiles: 7000 )),
    Vision(( radius_milli_tiles: 8000 )),
  ],
  cost: ( ore: 75 ),
  build_time_ms: 10000,
  population: 1,
  requires: [ "barracks" ],
)
```

### 10.4 Alpha manifest (starter values — placeholders, tunable as data only)

| Category | Count | Purpose |
|----------|-------|---------|
| Faction | 1 | faction-as-bundle concept |
| Map | 1 (64×64 tiles) | map-as-data, every strategic pressure below |
| Resource | 1 (Ore) | full economy loop |
| Buildings | 4 | distinct structural concepts |
| Units | 4 | spans movement/combat/economy parameter space |
| AI opponent | 1 | command-parity proof |
| Victory | Elimination + resignation | simplest rule that resolves the loop |

| Entity | HP | Cost (Ore) | Build ms | Speed (mt/s) | Range (mt) | Dmg | Cooldown ms | Vision (mt) | Pop |
|--------|----|-----------|----------|--------------|------------|-----|-------------|--------------|-----|
| Worker | 40 | 50 | 12000 | 2600 | — | — | — | 7000 | 1 |
| Rifleman | 60 | 75 | 10000 | 2400 | 5000 | 8 | 1000 | 8000 | 1 |
| Raider | 45 | 60 | 8000 | 4000 | 1500 | 6 | 600 | 8000 | 1 |
| Guardian | 220 | 200 | 25000 | 1600 | 8000 | 30 | 2000 | 9000 | 3 |
| Command Center (4×4) | 1000 | 400 | 40000 | — | — | — | — | 10000 | +10 cap |
| Barracks (3×3) | 600 | 150 | 25000 | — | — | — | — | 8000 | — |
| Supply Depot (2×2) | 300 | 100 | 15000 | — | — | — | — | 6000 | +10 cap |
| Turret (2×2) | 350 | 125 | 20000 | — | 7000 | 10 | 1000 | 9000 | — |
| Ore Node (2×2) | — | — | — | — | — | — | — | — | 1500 Ore |

Gathering: workers carry 10 Ore per trip, 2000 ms per gather. Start: 1 Command Center, 4 Workers, 200 Ore per player. Population cap starts at 0 + provided by structures.

### 10.5 Alpha map requirements (data file)

64×64 grid; two symmetric start positions; a chokepoint; one contested expansion with ore; hazard-free but with obstacles forcing pathfinding decisions; passability classes as data. Loader validates dimensions, spawn legality, ore reachability from each start, and symmetry check (as an optional validator).

### 10.6 Add-a-unit / add-a-map tests

An automated test copies a unit RON file, changes stats and id, adds it to a faction's production list, and runs a headless match where it gets built and fights. This test must pass with zero changes under crates/sim. Same for a new map file.

## 11. Engine & Client Layer

### 11.1 Game loop (engine)

Classic fixed-timestep accumulator: real-time delta accumulates; the sim steps at 30 Hz (cap catch-up at e.g. 5 ticks per frame to avoid spiral of death); rendering runs at display rate (vsync), interpolating between the previous and current snapshots by alpha = accumulator / tick_duration. Time comes from the client/engine, never from sim.

### 11.2 Renderer (3D presentation — ADR-0001)

wgpu renderer with a **depth buffer**, presenting the 2D logical world in 3D: a **terrain mesh** built from the map grid (one quad per tile, vertically displaced by the optional display-only heightmap — ADR-0001; the heightmap never affects gameplay), an entity layer of **primitive placeholder models** (team-colored boxes/capsules, instanced), an overlay layer (selection circles, health bars, tracers, fog visualization, build placement ghost — ground-projected or camera-facing geometry), and a UI layer. Render from Snapshot, never from Sim. A Renderer trait boundary keeps the sim-facing engine code independent of wgpu so a headless "null renderer" exists for tests and soak runs. Float math (glam) lives in client/engine only; the simulation stays on its 2D fixed-point plane.

### 11.3 Input & selection (3D — ADR-0001)

**Perspective RTS camera** (pan by edge + keys, zoom toward the cursor, optional rotation), **ground-plane ray picking** (cursor ray intersected with the 2D logical ground plane; converted to fixed-point at the boundary so the sim never sees a float), and **screen-space box selection** (project entity positions to screen space and rectangle-test). Drag-box selection, click-select, control groups, shift-queue, right-click context command (move/attack/gather resolved from what's under the cursor), attack-move hotkey, stop hotkey, build placement mode with legality preview. Input produces only Commands and client-local state (selection, camera, hotkeys). Selection state lives in the client, not the sim.

### 11.4 UI shell (own toolkit)

Resource display, population, selection panel, command card (buttons bound to commands), production queue display with cancel, minimap with fog and click-to-move-camera, match end screen with restart. Simple rect + text widgets; layout via anchors/flow. UI reads snapshots/events only.

### 11.5 Audio & feedback

Alpha: an AudioSink trait fed by events, with a null implementation and optionally a few placeholder cues. Visual feedback (hit flashes, death fade, tracer lines, click pings) driven by events.

### 11.6 Debug tooling (dev builds)

Overlays for: entity IDs, paths, collision radii, vision, tile passability, tick time graph, state hash of the current tick; pause / single-step tick; replay scrubbing; a deterministic "seed + tick" bug-report dump.

## 12. Tools (tools crate)

| Command | Purpose |
|---------|---------|
| tools headless --seed N --p1 ai --p2 ai | Run a match without rendering; print result + final hash |
| tools soak --matches 1000 | Many seeded AI-vs-AI matches in parallel processes; detect crashes, stalls, invariant violations; emit win-rate telemetry |
| tools replay-verify FILE | Re-simulate and compare checkpoint hashes |
| tools content-validate [PATH] | Validate all content + maps; strict mode |
| tools bench | Criterion benchmarks for tick cost, pathfinding, vision |

## 13. Acceptance Criteria (verified by automated tests where possible)

Minimum viable loop (manual + scripted): start match → build base structures → gather Ore with workers → train units → scout under fog → engage the AI → destroy all enemy structures (or resign) → see end screen → restart cleanly. No special-case code for any step.

| # | Criterion | How it's verified |
|---|-----------|-------------------|
| A1 | Determinism: same seed + log → byte-identical hash | tests/determinism.rs: run twice, compare every checkpoint; runs on Linux, Windows, macOS in CI against a committed golden hash |
| A2 | Replay: a recorded match replays to identical final hash | tools replay-verify in CI |
| A3 | Add-a-unit is data-only | Test in 10.6, plus a git-diff check that no file in crates/sim/ changed in the test's fixture commit |
| A4 | Add-a-map is data-only | Same pattern |
| A5 | AI parity: AI acts only through Commands | Compile-time (ai depends only on sim_api) + audit test that the sim exposes no AI-specific entry point; AI economy verified against player rules |
| A6 | Isolation: each system testable alone | Per-system unit tests constructing minimal worlds (no renderer, no other systems' fixtures) |
| A7 | Headless soak: 1000 seeded AI-vs-AI matches complete with no panics, no stuck matches (> 20 min game time), no invariant violations | tools soak in nightly CI |
| A8 | Responsiveness: valid command → visible feedback ≤ 1 tick; motion ≤ 2 ticks | Sim-level test on tick counts; client-level manual check |
| A9 | Movement quality: 50-unit group orders complete, no permanent stuck units | Scenario test with spam-click stress |
| A10 | Fog integrity: hidden entities behave identically; targeting rejects unseen | Test: run same scenario with fog logic on and off in observer mode, hashes equal; targeting tests |
| A11 | Command parity for all issuers: identical validation results | Property test: same command from "player" and "AI" issuer produces same outcome |
| A12 | Invariants hold every tick (debug builds): no negative resources, pop ≤ cap, no entity on blocked tile, no ID reuse, hp ≥ 0, queue costs consistent | debug_assert-based invariant checker run in soak |
| A13 | Architecture law: dependency graph and float/unordered-map bans hold | CI test parsing cargo metadata; clippy lints |
| A14 | Design bar: first-time players complete the loop unaided; spectators can tell who is winning | Human playtest checklist with ≥ 5 testers (record results in docs/PLAYTEST.md) |
| A15 | Restart cleanliness: two consecutive matches from fresh sims with the same seed have identical hashes | Automated test |

Declaration rule: the Alpha is declared only when every criterion is verified. A criterion that cannot be met honestly is recorded as a finding, not waved through.

## 14. Milestones (each ends with green exit tests; do not skip ahead)

Calendar reference from v1: ~26 weeks for a small human team. For an agent, exit criteria are the truth; dates are not.

**M0 — Skeleton & guardrails (v1 Phase 0)**

Tasks: workspace + crates per Section 4; rust-toolchain.toml; CI (fmt, clippy, tests, 3-OS matrix); clippy.toml bans; architecture-law test; docs skeleton (ARCHITECTURE, DEBT, ASSUMPTIONS, ADR template); fx crate with Fx, Vec2Fx, Rng, hasher.

Exit: fx property tests green; architecture-law test green; CI green on 3 OSes.

**M1 — Simulation core (Phase 1)**

Tasks: Sim, step, tick counter, entity store with monotonic IDs, capability stores, command queue + ordered application, event buffer, state hash, snapshot, replay record/verify.

Exit: A1/A2 on a trivial world (spawn N entities, apply dummy commands): two runs give identical hashes on all 3 OSes; ID never reused test; iteration-order test.

**M2 — Content pipeline**

Tasks: RON schemas + versioning + validator; content bundle + content hash; map loader with validation; entities spawn from data; tools content-validate.

Exit: all Alpha content and the map load; malformed files produce precise errors; A3-style test scaffold in place (data-only spawn of a new kind).

**M3 — Engine shell**

Tasks: window, wgpu 3D renderer (depth buffer, terrain mesh from map data + display-only heightmap, primitive placeholder models — ADR-0001), perspective RTS camera with ground-plane ray picking and screen-space box selection, snapshot interpolation, fixed-timestep loop, null renderer, input → commands, selection, minimal HUD, debug overlays.

Exit: a windowed build shows the map and entities from a live sim; selecting and issuing Move commands works; client never touches Sim mutably except via step inputs.

**M4 — Movement (Prototype P1: "Do multiple units move responsibly?")**

Tasks: nav grid, A* with deterministic tie-breaks, path execution, collision push-apart, stuck detection.

Exit (P1): 50 units respond within 2 ticks under spam-clicked orders; no permanent stuck units; hash check still green. Fail → revisit grid granularity/steering before moving on.

**M5 — Economy, production, construction (Prototype P3: "Do economic openings diverge?")**

Tasks: resource registry, gather/deliver/deplete, storage, ledger, queue model, Train/Cancel/SetRally, Build + construction lifecycle, population cap, requirements checker.

Exit (P3): scripted openings (e.g. worker-heavy vs early Raider) produce measurably different timelines; invariants (A12) green under soak of economy-only matches.

**M6 — Combat & vision (Prototype P2: "Is combat legible and meaningful?")**

Tasks: pipeline stages, acquisition, Attack/AttackMove/Stop, Turret, death lifecycle, events → visual cues, fog three-state model, targeting filters, minimap fog.

Exit (P2): scripted skirmishes show composition/position mattering; legibility checklist passes; A10 fog integrity green.

**M7 — AI through commands (Prototype P4: "Is AI parity real?")**

Tasks: Controller trait, PlayerView, scripted controller, parity audit.

Exit (P4): A5 + A11 green; AI-vs-AI headless matches complete.

**M8 — Match rules & full loop (Prototype P5: "Do all systems work together?")**

Tasks: victory/defeat/resign, end screen, restart, UI depth (control groups, hotkeys, queue UI, minimap polish).

Exit (P5): a 10–15 minute match vs the AI completes and restarts cleanly (A15).

**M9 — Alpha content & feel pass (Phase 4)**

Tasks: finalize manifest values, tune AI script, feel pass (feedback cues, responsiveness), placeholder audio cues.

Exit: minimum viable loop fully playable end to end against the AI.

**M10 — Stabilization & declaration (Phase 5)**

Tasks: full acceptance suite A1–A15, nightly soak, benchmark baselines, documentation of boundaries (CONTENT_GUIDE, ARCHITECTURE), debt register review, add-a-unit / add-a-map acceptance demonstration.

Exit: every criterion in Section 13 verified; soak green; written declaration in docs/ALPHA_DECLARATION.md citing evidence for each criterion.

## 15. Performance Budgets (dev-machine reference, release build; set real baselines in M10)

| Metric | Target |
|--------|--------|
| Sim tick, Alpha-size match (≤ 200 entities) | ≤ 1 ms average, ≤ 4 ms p99 |
| Sim tick at 10× scale test (≈ 2000 entities) | ≤ 8 ms average (proves growth path; may require spatial partitioning inside systems) |
| Headless throughput | ≥ 1000 ticks/s (≈ 33× real time) for Alpha matches |
| Render | 60 FPS at 1080p with placeholder art |
| Pathfinding request (64×64 map) | ≤ 0.5 ms typical |
| Memory | < 200 MB for an Alpha match |

Performance work never changes semantics: optimizations must keep golden hashes identical.

## 16. Risks & Mitigations (v1 left this section empty)

| Risk | Impact | Mitigation |
|------|--------|------------|
| Hidden nondeterminism (map iteration, uninitialized state, OS differences) | Breaks replays and future multiplayer | Section 5 rules, lint bans, golden hashes on 3 OSes in CI from M1, hash check on every PR |
| Fixed-point bugs (overflow, rounding, precision) | Subtle desyncs or physics glitches | Property tests on fx; explicit overflow policy; invariant checks; conservative ranges |
| Building the engine eats the schedule | Never reach gameplay | Strict "engine only as needed by a milestone" rule; placeholder art; use wgpu/winit; no custom features beyond the tasks listed |
| Movement feels bad or units get stuck | Fails core feel | P1 gate at M4; stuck detection; debug path overlays; layer swap ready |
| Scope creep from content/features | Foundation starves | Anti-goals, manifest annotation rule, postponed-features table |
| Data boundary leaks (logic sneaks into content or kind-name matches in systems) | Add-a-unit stops being data-only | A3/A4 tests, grep test for kind-name matching, review checklist |
| AI cheating creeps in | Parity destroyed | ai crate depends only on sim_api; parity audit at M7 |
| Rendering/platform issues (GPU drivers, wgpu backends) | Client instability | Renderer trait + null renderer; sim and tests independent of GPU; CI runs headless |
| Agent drift or accumulating unlogged debt | Foundation rots quietly | Operating contract (Section 0), DEBT.md, milestone gates, ADR for frozen-decision changes |

## 17. Explicitly Postponed (with the hook that keeps each cheap later)

| Feature | Hook already present |
|---------|---------------------|
| Multiple factions | Faction-as-data bundle |
| Tech trees, upgrades | Requirement lists, modifier concept via ProvidesPopulation pattern |
| Abilities | Command machinery, validation gate, cooldown-ready pipeline |
| Formations | Movement layer contracts; formation-less spread in Alpha |
| Air units | Position aspect supports movement layers; navigation trait |
| Second resource, trade | Resource registry, player-slot model |
| Multiplayer / netcode | Deterministic command-log sim, Command.tick, hashed checkpoints (desync detection) |
| Replay browser UI | Replay recording already exists |
| Better graphics | Renderer trait + snapshot/event boundary |
| Flow-field pathing | Navigator trait |
| Campaigns, cosmetics, ranked, mods, live service | Only after the core proves it needs them |

## 18. Technical Debt Strategy

Safe temporary simplifications (allowed, must be logged in docs/DEBT.md): placeholder art; single map; A* per request instead of flow fields; instant-hit combat; scripted AI; one resource; circular vision only; formation-less movement; no audio beyond stubs.

Dangerous shortcuts (forbidden): AI reading or mutating sim state directly; hardcoded unit/building stats or kind-name checks in systems; direct click→state mutation bypassing commands; unseeded randomness or wall-clock time in sim; floating-point types in sim or content; reusing entity IDs; presentation code polling sim internals; unversioned content files; special-casing an issuer in validation; skipping the hash check "just this once."

Debt register row format: `| id | what | category | why now | replacement plan | repay-when trigger | status |`. Reviewed at every milestone exit.

## 19. Post-Alpha Evolution (the next year, in philosophy only)

Invariants that never move: command interface admits no issuer-specific behavior; content schema moves only forward; determinism is never traded for convenience; AI touches the game only through commands and declared perception; the acceptance suite runs green on every build.

Expansion test sequence (used to audit the foundation): Update 1 new unit, 2 new building, 3 new map, 4 stat rebalance, 5 second faction (all data-only) → 6 ability system (first designed extension) → 7 tech tree / upgrades → 8 evaluative AI above the command interface.

Rhythm: monthly internal playable that passes the acceptance suite; quarterly external playtest once content volume justifies it; a debt-register review after every release.

Rewritten on purpose over time: renderer, UI skin, audio, navigation implementation. These are isolated by design and their replacement is not a failure.

## Appendix A — What Changed From v1.0

- Implementation language and engine strategy specified (Rust, fully custom engine, dependency policy, crate layout, dependency law).
- Determinism made concrete for Rust (lint bans, integer authoring units, canonical hashing, iteration-order rules).
- Fixed tick pipeline order written out step by step.
- Command set completed: v1 required queues, cancellation, rally, and resignation but defined no commands for them. Added Train, CancelQueueItem, SetRally, Resign.
- Content numbers provided (starter stats, map size, start conditions) so the Alpha is buildable without guesswork.
- Acceptance criteria made testable (A1–A15, each with a verification method).
- Milestones rewritten as agent-executable work packages with tasks and exit tests, preserving v1's phases and prototype gates P1–P5.
- Risks section filled in (v1 §24.3 was empty).
- Small consistency fixes: "ten questions" → eleven; Table 1 expanded into full criteria list.
- Added the agent operating contract (Section 0), assumptions log, ADR process, and debt register mechanics.



### by E-Vex
