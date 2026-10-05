# Pandemonium — Architecture

Status: milestone M9.1 complete (input hotfix on top of M9's Alpha content &
feel pass); M10 (Stabilization & declaration) is next. The authoritative
specification is [`plan.md`](../plan.md) — §2 frozen decisions, §4 workspace law,
§5 determinism rules, §10 content.
This file is the working map of how the code is actually laid out; update it when the
shape of the system changes, not for every feature. For current build status and what
exists versus what is pending, see [`AI-Handoff.md`](../AI-Handoff.md).

## Workspace

| Crate | Role (plan §4) | Depends on |
|-------|----------------|------------|
| `crates/fx` | Fixed-point math (`Fx`, `Vec2Fx`), integer sqrt, PCG32 RNG, FNV-1a hasher | nothing |
| `crates/sim_api` | Public vocabulary crossing the sim boundary: ids, commands, events, views | fx |
| `crates/sim` | World state, entity store, capabilities, tick pipeline, systems, state hash | fx, sim_api |
| `crates/content` | RON schema, versioned loaders, validators, content bundle + hash | fx, sim, sim_api + serde, ron, thiserror |
| `crates/ai` | The `Controller` trait (plan §9.6) + `ScriptedController`, the scripted Alpha opponent | fx, sim_api |
| `crates/replay` | Replay record/load/verify format | fx, sim_api |
| `crates/engine` | Game loop, timing, interpolation, input mapping, camera, UI toolkit, AI hosting on the tick boundary | fx, sim, sim_api, content, ai |
| `crates/client` | Playable binary: wgpu renderer, winit window, screens, HUD | fx, sim, sim_api, content, ai, replay, engine |
| `crates/tools` | Headless runner (demo + AI controller slots), soak, replay-verify, content-validate, bench | fx, sim, sim_api, content, ai, replay, engine |
| `tests` (`pandemonium-tests`) | Cross-crate acceptance suite (plan §13) | any — test host, exempt from the internal law |

The graph is enforced — not merely documented — by `tests/architecture_law.rs`, which
parses every member manifest and fails the build on any violation. The plan asks for
"a test that parses cargo metadata"; the test parses the member manifests directly
instead, which is hermetic (no nested cargo invocation, works offline, identical
enforcement). See [`ASSUMPTIONS.md`](ASSUMPTIONS.md).

## Hard rules encoded in the test

1. Dependencies point downward only, per the table above. Dev-dependencies are exempt
   (test-only code never ships), but the forbidden-crate list applies to them too.
2. External crates are allow-listed per member exactly as plan §3.2's table. The
   allow-lists encode the *whole* table from day zero, so adding wgpu to `client` in
   M3 is not a law change — it was always allowed there.
3. `bevy`, `macroquad`, `ggez`, `fyrox`, godot bindings, every ECS crate, physics and
   pathfinding crates, and `rand`/`getrandom` are forbidden workspace-wide.
4. `sim` may never depend on anything above it (engine, client, ai, replay, content,
   winit, wgpu, or any I/O) — FD-6.
5. `ai` may never depend on `sim` — AI parity is structural, a compile error rather
   than a policy (FD-7).
6. The determinism crates (`fx`, `sim`, `sim_api`, `ai`, `content`) may not contain
   unordered-map types, wall-clock reads, or floating-point types — scanned line by
   line by the same test (plan §5). The engine and client are *not* scanned: rendering
   interpolation and real-time timing legitimately use real clocks and float math
   there (plan §11.1); they never enter the simulation.

## Determinism digest (plan §5)

- All sim math is `fx` Q16.16 fixed-point. Authored content uses integer units
  (milliseconds, milli-tiles) converted at load (plan §10.2).
- `fx` arithmetic uses 64-bit intermediates, rounds toward zero, and saturates instead
  of panicking; the contracts are documented on the API and property-tested.
- No unordered maps anywhere simulation state or state-affecting iteration could
  touch. Iteration is ascending id order; sorts are stable with id tie-breaks.
- All randomness flows from `fx::Rng` (PCG32), seeded from the match seed and owned
  by the simulation state, advanced in a fixed order.
- State hashing is an explicit canonical little-endian byte encoding through
  `fx::Fnv1a64` — never `Debug` output or memory layout (plan §6.4).

## The boundary the whole design hangs on

FD-2/FD-6: `Sim::step(commands)` is the only mutator of simulation state, and the
simulation knows nothing above it. Everything the outside world learns arrives as
snapshots, events, and fog-filtered `PlayerView`s (FD-8/FD-9). Every later milestone
preserves this shape; if a task seems to require breaking it, write an ADR first
(plan §0, `docs/adr/`).

## The simulation core (M1)

```text
crates/sim/src/
  fixture.rs   TrivialWorld — the in-code world template (kinds as capability
               compositions with their economy stats, resource registry,
               production lists, base population cap, initial + scheduled
               spawns, spawn jitter, the terrain passability and buildability
               grids) and its canonical content hash (encoding v3). Real
               matches build it from the M2 ContentBundle; it remains the
               spine-test fixture.
  world.rs     World — entity store + one store per capability type (Health,
               Move, Vision, Gather, Build, Produce, Storage,
               ProvidesPopulation, Resource, Footprint, Construction) +
               player states with the ledger ops (can_afford/spend/refund/
               credit). Every store is a Vec kept sorted ascending by
               EntityId (an invariant): ids allocate monotonically, appends
               preserve order, removals shift without reordering, lookups
               binary-search. No ordered-map bookkeeping anywhere. Capability
               defs carry their runtime state beside their parameters — the
               hp-on-HealthDef pattern (Gather cargo/timer, Produce queue/
               rally, Construction progress).
  nav.rs       The navigation layer (plan §9.1.1, M4) plus the M5 occupancy
               overlay: NavGrid over the world's passability with a per-tile
               static-body claim count (structures and nodes block their
               footprint tiles; death and depletion release them); 8-connected
               A* with no corner cutting, integer milli-tile costs, the octile
               heuristic, and the total-order (f, h, tile) heap key —
               deterministic pops, so paths are a pure function of grid +
               endpoints. The straight beeline is validated by an integer
               supercover of the exact start→goal segment; other routes walk
               tile centers, prefixed with the start tile's center, suffixed
               with the exact goal. Blocked/unreachable/occupied goals resolve
               to the nearest reachable tile; sealed starts return None.
  movement.rs  Stage 6 as the three layers (plan §9.1, M4): path requests
               (unreachable and zero-speed distant orders fail immediately
               with MoveFailed; economy orders resolve travel targets through
               the economy system), waypoint steering with a terrain guard,
               spatial-hash push-apart (one-tile cells, id-ordered pairs,
               symmetric terrain-guarded halves), and stuck detection with
               repath escalation and a body-aware crowded arrival (MoveTo
               orders only; economy orders never pop on arrival — their own
               systems retire them).
  economy.rs   Stage 4 (plan §9.3, M5): the worker gather loop as a phase
               machine — travel, gather timer, extraction capped by capacity
               and node remaining, carry, deposit at the nearest completed
               storage (ResourceDelivered), repeat; depletion removes nodes
               (NodeDepleted) with same-tick auto-seek of affected workers;
               ghost BuildAt orders are hygiene-cleaned. The approach test
               (nearest footprint-rect point within 1500 milli + body radius)
               is shared with construction.
  production.rs Stage 3 (plan §9.4, M5): producers' ordered queues (costs paid
               on enqueue, refunded verbatim on cancel, only the head
               progresses, completed items hold when population headroom is
               missing), ring-tile spawns with rally orders, construction
               progress with a committed builder completing into Active, the
               population usage/cap recompute, and the one requirements
               checker shared by both surfaces.
  command.rs   The shared validation gate (plan §8.2) and application: stable
               (issuer, seq) sort, tick check, issuer check, duplicate-sequence
               check, per-kind existence/ownership/capability/target checks,
               and the M5 economy checks (faction production list,
               requirements, affordability, population, placement) reading the
               fixture through the seam; Build spawns sites inline with
               immediate tile claims. Invalid commands emit CommandRejected
               and change no state. visible_to() is the on-demand fog filter
               shared by the gate and player_view().
  hash.rs      The canonical state hash (plan §6.4): little-endian, fixed field
               order, entity-major with a capability presence bitmask (u16,
               eleven slots), through fx::Fnv1a64; carries
               STATE_ENCODING_VERSION = 4 (the economy capability blocks, the
               GatherAt/BuildAt orders, the UnderConstruction lifecycle, the
               Attack capability block + AttackUnit order — M6).
  combat.rs    Stage 7 (plan §9.2, M6): the immediate-hit attack pipeline —
               every stage a named function (acquire, validate, wind_up,
               hit, mitigate, apply, death-via-stage-8, credit). Cooldowns
               in ticks; squared-distance range checks; acquisition priority
               commanded > units-attacking-friendly > other units >
               structures; ties to lower id (A-057). `clear_dead_targets`
               is called by stage 8 after removal so attacker target slots
               pointing at the dead don't dangle.
  vision.rs    Stage 9 (plan §9.5, M6): the three-state fog model — per-
               player per-tile Hidden/Explored/Visible, maintained
               incrementally each tick. `FogState` is derived (NOT hashed —
               A-059); `player_view` consults the cache instead of the M1
               on-demand scan. DEBT-004 repaid.
  match_rules.rs Stage 10 (plan §6.3.10, §9.7, M8): defeat/victory
               evaluation. Idempotent — `Sim::outcome` caches the result so
               `MatchEnded` fires exactly once per match. The defeat check
               only fires when the match is "structure-bearing" (A-067: at
               least one player owns a Footprint structure) so the M1
               spine-test fixture (no Footprint kinds) stays green. The
               outcome is derived state (A-066: not part of the canonical
               hash — a pure function of the entity set and the players'
               `resigned` flags, both of which ARE hashed), so the M7
               goldens stay green by construction.
  runner.rs    `run_command_log` (M7): the one canonical headless
               re-simulation of a command log — tick-0 checkpoint, a
               stable tick-sort feed (within-tick order preserved for
               duplicate-sequence resolution), periodic hashes, a forced
               final, the allocator watermark. Tools' recorder and
               verifier and the acceptance suites all drive through it
               (DEBT-005 repaid; the `replay` crate must stay above
               `sim`, so it could never host the driver itself).
  invariants.rs The A12 checker (plan §13): debug assertions over the whole
               state at the end of every step in debug builds — ledger
               non-negativity, population cap + usage non-saturating (A-053:
               over-cap allowed from combat-destroyed structures), hp bounds,
               no mover on blocked or claimed tiles, store order + allocator
               ceilings, queue-cost consistency, construction budgets, live
               gather targets, combat invariants (cooldowns sane, target
               alive, no attack on own — M6).
  sim.rs       Sim — new/step/state_hash/snapshot/player_view/next_entity_id/
               outcome/is_finished. step() runs plan §6.3's eleven stages
               verbatim; stages whose systems arrive in later milestones are
               documented no-ops with milestone pointers. Stage contents
               today: command application (1), scheduled spawns + production
               + construction + population recompute (3), the gather loop
               (4), the three-layer mover (6), combat (7), health advance +
               death & cleanup + clear_dead_targets (8), the A12 checker
               (debug, end of tick), vision (9), match rules (10 — M8),
               finalize with the periodic hash every 30 ticks (11).
```

Events are outputs, never state: the buffer is drained by each `step` and is not
part of the hash. Snapshots and player views project entities ascending by id.

## The content pipeline (M2)

```text
crates/content/src/
  schema.rs    The RON-facing raw types, one per file kind plus the map's two
               historical versions (v2 = ADR-0001's optional display-only
               heightmap). Strict: every struct denies unknown fields.
  version.rs   schema_version gates + forward migration (map v1 -> v2 is one
               explicit typed function; newer-than-loader is a precise error).
  loader.rs    SourceMap (named sources; the in-memory input for tests) and the
               directory reader (files sorted by name — OS order is not
               deterministic), then per-file parse -> version gate -> strict
               parse -> id/stem check -> canonical conversion (ms -> ticks via
               plan §10.2's ceiling division).
  defs.rs      The canonical validated types (EntityDef, FactionDef, MapDef,
               RulesDef, CapabilityDef) — known-good, what everything
               downstream consumes.
  validate.rs  File-level checks (stats, duplicates, references, grid shape,
               bounds, heightmap shape) and placement-level checks per
               (map x faction): spawn legality (footprints on buildable,
               unoccupied ground), ore reachability (8-connected BFS from each
               start over free terrain), and the declared 180-degree symmetry
               check (grid + ore tile multiset + mirrored anchors).
  bundle.rs    ContentTree (everything in a directory) and ContentBundle (one
               match's content): the canonical content hash and map id (FNV-1a
               over an explicit little-endian encoding, plan §5.10) and world()
               — the plain TrivialWorld the simulation receives.
```

The seam (A-004/A-024): `content` depends on `sim`, so `Sim::new` gains its
loaded-bundle path as `Sim::new(&bundle.world(), setup)` — serde/ron stay
below the sim's dependency line. Since M4 the world definition also carries
the terrain passability grid; since M5 it carries the buildability grid, the
faction production lists, the base population cap, and each kind's economy
stats (per-resource cost, build time, population, requirements) — all pure
data the simulation consumes. The capability mapping is complete except for
Attack (combat, M6 — DEBT-006's remainder). The heightmap remains
display-only by construction: it is not part of the world definition the
simulation receives (ADR-0001). Authoring data lives in `content/` (see
[`CONTENT_GUIDE.md`](CONTENT_GUIDE.md)); `tools content-validate` is the
designer's front door. Determinism of loading is pinned by tests: sorted
iteration, id-sorted collections, pure-function loading, identical hashes
across loads.

## The engine and client (M3, ADR-0001)

```text
crates/engine/src/           presentation-layer crate: floats + glam are legal
                             here, never in the sim's tree (A-007, ADR-0001)
  clock.rs        FixedTimestep — the 30 Hz accumulator with a 5-tick catch-up
                  cap and spiral-of-death backlog drop; alpha() for rendering.
  interpolate.rs  Interpolator — prev/current snapshot blending (positions and
                  facing lerp, spawns appear, deaths drop, discrete fields from
                  current) + the Q16.16 <-> ground-plane boundary conversions
                  (fx_to_world / world_to_fx).
  host.rs         MatchHost — owns the Sim privately; submit() + advance() are
                  the only mutation paths (the M3 exit criterion as a
                  compile-time property, A-033). Commands are tick-stamped for
                  the next step, FrameOutcome carries events + checkpoint
                  hashes. Pause freezes the clock itself (no catch-up burst on
                  resume); step_once() is the §11.6 single-step debug action;
                  hud_state(player) gathers the HUD values through the
                  boundary. **M8 extensions**: with_controllers() hosts AI
                  controllers on the tick boundary (the same seam as
                  AiMatchHost — the client's vs-AI opponent); log() exposes
                  the command log (for replay recording); outcome() /
                  is_finished() surface stage 10's MatchEnded (for the end
                  screen). The Sim is still private (controllers receive an
                  immutable PlayerView, FD-7). Restart is by drop +
                  reconstruct (the client retains its ContentBundle +
                  MatchSetup; A15 constructs two fresh MatchHost instances).
  ai_host.rs      AiMatchHost (M7) — the headless shape of the same
                  ownership: one controller per driven slot, one tick per
                  advance, controllers invoked in ascending slot order on
                  their fog-filtered view of the tick being applied, every
                  fed command recorded (rejections included — a
                  controller-run match is a plain command log afterwards,
                  exactly what `sim::run_command_log` re-simulates). Also
                  `alpha_plan`/`alpha_controller` (the scripted opponent's
                  config derived from the content by capability shape —
                  production lists decide the roster — and `build_spots`,
                  the deterministic ring scan around each start that keeps
                  candidate placements clear of static claims and node
                  doorsteps). The M8 client will host its vs-AI opponent
                  through this same seam (A-063).
  camera.rs       RtsCamera — perspective orbit over the logical ground plane
                  (world.x = sim.x, world.z = sim.y); pan/zoom/rotate with
                  clamps; ground-plane ray picking (inverse view-projection ->
                  y=0); NDC projection; screen-space box selection.
  mesh.rs         terrain_mesh — the map grid + display-only heightmap as
                  vertex/index data (one quad per tile, class colors); pure,
                  GPU-free, tested.
  renderer.rs     the Renderer trait + Frame (snapshot, view-projection, eye,
                  selection, hud) + HudState (§11.4 resources/population,
                  §11.6 tick/hash/pause) + NullRenderer (headless sink for
                  tests/soak).

crates/client/src/
  main.rs         winit 0.30 window + input: WASD pan, wheel zoom, left click /
                  drag-box selection (own units), right-click Move via ground
                  picking (fixed point at the boundary), Escape exits; F3
                  toggles the §11.6 debug overlay, P pauses, '.' single-steps;
                  --frames N exits after N presented frames with the evidence
                  summary (the DEBT-008 windowed verification affordance,
                  A-036). Without a display: prints the finding and runs a
                  headless smoke pass over the real content, exiting 0 (A-034).
                  **M8**: hosts the AI opponent through
                  `MatchHost::with_controllers` (A-063 — the engine's hosting
                  loop is one loop, shared with the tools and tests); the
                  end-screen panel renders VICTORY/DEFEAT/MUTUAL DESTRUCTION
                  when `host.is_finished()`; 'R' restarts (drop + reconstruct
                  the host with fresh controllers from the same bundle + seed,
                  A15); control groups 1-9 (Ctrl+digit assigns, digit recalls);
                  'S' Stop and 'A' AttackMove hotkeys on the selection.
  text.rs         our own text renderer (plan §3.1/§3.2): fontdue rasterizes
                  the embedded "Pandemonium Sans" (ASCII subset of DejaVu
                  Sans, renamed per the Bitstream Vera license —
                  assets/fonts/LICENSE.txt) into a coverage atlas; layout is
                  plain client-side math (packing, order, baseline
                  conventions, and the fallback box are unit-tested).
  render.rs       WgpuRenderer (wgpu 26): depth buffer, terrain pipeline (the
                  engine mesh), entity pipeline (36-vertex unit cube, instanced
                  position/half-extent/color, team colors, selection
                  brightening), and the UI overlay pipeline (screen-space
                  pixels → NDC, glyph atlas as R8Unorm, alpha blending, drawn
                  on top of the world) — implements the engine Renderer trait.
```

The boundary the milestone hinges on: the client holds no `&mut Sim` and cannot
get one; the engine's `MatchHost` is the only owner, `FD-2`'s "step inputs only"
made structural. The windowed path is machine-verified on Xvfb + llvmpipe
(selection + right-click Move provably work — A-037); only the human visual
pass remains (DEBT-008, narrowed).

## The replay format (M1)

`crates/replay` encodes/decodes the plan §6.5 field list through its own canonical
little-endian codec (`bytes.rs`) with a trailing FNV-1a checksum — no serde, per the
dependency law and the canonical-encoding rule. The command log records exactly the
stream fed to `Sim::step` (rejections included), so re-simulation reproduces the same
hashes. The re-simulation driver lives in `tools` and in `tests/determinism.rs`
(the crate itself must not depend on `sim`).

## Tools and acceptance tests (M1 + M2 + M7 + M8)

`pandemonium-tools` (clap CLI) ships `headless` — the scripted demo match
printing per-checkpoint and final hashes, with `--record` to write a replay —
and, since M7, `--p1/--p2 ai|idle` controller slots over the real content (the
per-player evidence line: deliveries, trained, built, hits, deaths; M8 adds
the `winner` field from stage 10's `MatchEnded` event, with the CLI printing
one of "player N wins", "mutual destruction", or "unresolved (ran the tick
budget)") — plus `replay-verify` — re-simulation with checkpoint comparison
(A2), resolving the world by content identity (the demo world or the content
directory). M2 added `content-validate` — strict validation of a content
directory plus its content identity. The AI match driver and the
re-simulation both run through the engine's hosting and
`sim::run_command_log` respectively.

`tests/determinism.rs` carries the A1/A2 suite with pinned golden hashes
(encoding v4 — A-056); `tests/content_pipeline.rs` carries the M2 suite
(§10.4 stat pin, loaded-bundle determinism, the A3 add-a-unit scaffold now
extended to train the data-defined kind from the barracks — DEBT-007's M5
half); `tests/movement.rs` carries the M4 exit suite (50-unit spam-click
responsiveness, order resolution, determinism, the real-map detour with the
blocked-destination arrival envelope); `tests/economy.rs` carries the M5 exit
suite (divergent scripted openings, scripted-match determinism, the A12
economy soak, soak determinism); `tests/architecture_law.rs` still pins the
dependency graph and the source-level determinism bans; `tests/ai.rs` carries
the M7 exit suite — the A5 structural audits (ai reaches only fx + sim_api;
the sim never references the ai crate), the A5 ledger identity (the AI pays
the data-defined prices; every entity traces to an accepted command), the
A11 mirrored battery and label-blindness, the A11 proptest fuzz, AI-vs-AI
completion and determinism, the golden hash, and the log-alone replay;
**`tests/match_rules.rs` (M8) carries the match-rules exit suite** —
`MatchEnded` fires on resignation and surfaces through the host's outcome;
A15 restart cleanliness (two fresh hosts, same seed → identical hashes; with
AI controllers → identical logs too); stage 10 is hash-neutral (the M7
golden holds with match rules active). **`tests/alpha_loop.rs` (M9) carries
the Alpha-loop exit suite** — AI-vs-AI matches resolve inside the
fifteen-minute budget across seeds (9.1k–23.3k ticks over a 32-seed sweep),
end-to-end resolution is deterministic (same seed → same winner, end tick,
log, and final hash), and the windowed host's vs-AI loop closes naturally
(an idle human's base falls to the AI; `outcome()` surfaces through the
end-screen boundary). CI (`.github/workflows/ci.yml`) runs the whole gate on
Linux/Windows/macOS in dev and release, plus the replay round-trip; a
separate `.github/workflows/audit.yml` runs `cargo audit` over `Cargo.lock`
on every lockfile change and nightly.
