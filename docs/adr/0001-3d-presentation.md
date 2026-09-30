# ADR-0001: 3D presentation over the 2D logical plane

- **Date:** 2026-10-01
- **Status:** accepted
- **Frozen decision affected:** plan §3.3 (platform assumption: "2D top-down
  presentation"), plan §11.2/§11.3 (renderer and input specification), plan §3.2
  (dependency table: `glam` allowance). No FD-1..FD-10 decision changes: the
  simulation's 2D fixed-point ground plane (FD-5, plan §5) is untouched.
- **Proposed by:** E-Vex (project owner) — direction issued to the implementing
  agent on 2026-10-01.

## Context

Plan §3.3 pinned the presentation as "2D top-down" and §11.2 specified a 2D
instanced-quad renderer accordingly. The owner has directed that the game be
presented in 3D instead. The pressure is a product judgment, not a measurement:
perspective projection, a displaced terrain mesh, and volumetric-feeling
placeholder primitives are expected to carry the Alpha's legibility bar
(acceptance A14: "spectators can tell who is winning within seconds") further
than flat quads, and primitive 3D shapes (boxes, capsules) read as "intentional
placeholder" more readily than flat colored rectangles.

The frozen simulation contract is not in question and must not move: entities
live on a 2D logical ground plane with Q16.16 fixed-point coordinates
(FD-5, plan §5); all determinism rules, golden hashes, and replays depend on
that plane staying bit-identical. Presentation floats were already legal above
the sim boundary — plan §5 scopes the float ban to sim, sim_api, ai, and
content structs, and `docs/ASSUMPTIONS.md` A-007 records that engine/client
are deliberately exempt (rendering interpolation and real-time timing live
there).

## Proposal

1. The game is presented in **3D**: a wgpu renderer with a depth buffer, a
   terrain mesh built from the map grid and an optional **display-only
   heightmap**, and primitive placeholder models (team-colored boxes/capsules)
   for entities.
2. The **simulation stays exactly as designed**: a 2D logical ground plane,
   fixed-point math, deterministic, no floats. 3D is a presentation-layer
   concern only. `crates/sim`, `crates/sim_api`, `crates/fx`, and `crates/ai`
   are not modified by this decision.
3. Camera and input become 3D-native (plan §11.3): a **perspective RTS
   camera** (pan, zoom, optional rotation), **ground-plane ray picking**
   (cursor ray intersected with the logical plane, converted to fixed point at
   the boundary), and **screen-space box selection** (project entity
   positions to screen space, rectangle test).
4. The map content schema gains an **optional, display-only heightmap**:
   integer per-tile heights consumed exclusively by the renderer. It never
   enters the simulation, never affects passability, pathing, vision, or
   combat, and a map without a heightmap renders flat. The map schema moves
   to `schema_version: 2` (the pre-pivot format, version 1, migrates forward
   by inserting "no heightmap", honoring FD-10).
5. The dependency table (plan §3.2) gains **`glam`, used by client and engine
   only**, for presentation-side float math (transforms, camera, picking).
   Floats remain banned in the sim's tree exactly as before; the
   architecture-law test's per-crate allow-lists are updated to encode this.

## Evidence

- The direction itself is an owner decision (the project's authority for
  product questions); this ADR records it with its technical consequences
  rather than deriving it from a benchmark.
- Feasibility evidence, verifiable in the repo at the commit that lands this
  ADR: the full green gate (`cargo fmt --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace` in dev and release)
  passes with **zero diffs under `crates/sim`, `crates/sim_api`, `crates/fx`,
  and `crates/ai`** — the 3D change is provably presentation-only. The pinned
  golden hashes in `tests/determinism.rs` and the replay round-trip are
  unchanged by the pivot.
- Cost evidence: the added engine-side work is bounded — depth buffer +
  terrain mesh + perspective camera + ray picking are standard, and the plan's
  Renderer-trait + null-renderer boundary (§11.2) already isolates wgpu from
  the sim-facing engine code, so no architectural seam moves.

## Consequences

- **Cheaper / better:** depth cues and scale support the legibility bar (A14);
  primitive placeholder models look acceptable earlier than 2D sprite art
  would; camera work (zoom toward cursor, rotation) improves scouting feel at
  data-boundary cost only.
- **More expensive:** the renderer needs a depth buffer, terrain mesh
  generation (grid + heightmap), perspective camera math, and cursor ray
  picking instead of a flat quad batch; `glam` joins the dependency set
  (client/engine only); the map schema version bumps to 2 with a migration.
- **Tests / hashes:** none break. The architecture-law test gains `glam` in
  the engine and client external allow-lists (the allow-lists encode the full
  plan §3.2 table, so this is the table change made law, not a silent edit).
  Golden hashes, replays, and every sim test are untouched because no
  simulation file changes.
- **Heightmap discipline:** the heightmap is display-only by construction —
  the simulation never receives it (it is not part of the world definition the
  content loader hands to `Sim::new`). It is included in the content hash
  (content identity covers the whole bundle), so a heightmap edit is a content
  change for replay matching purposes even though it cannot alter any
  simulated outcome.

## Alternatives considered

- **Do nothing (stay 2D top-down).** Rejected: the owner explicitly directed
  3D; staying 2D contradicts the direction this ADR records.
- **3D with gameplay-affecting elevation** (height altering pathing, vision,
  or combat). Rejected: it would move the frozen 2D fixed-point logical plane
  (FD-5, plan §5) into the simulation, risking every determinism guarantee
  and invalidating golden hashes and replays for a presentation goal. If
  elevation-as-mechanic is ever wanted, it needs its own ADR with evidence.
- **2.5D billboard sprites in a 3D scene.** Rejected: more art effort than
  primitive 3D models (placeholder art only — plan §0) while giving up the
  depth and scale cues that motivated the change.

## Decision

Accepted 2026-10-01: 3D presentation is the direction; the simulation's 2D
fixed-point plane, determinism rules, and all frozen decisions are unchanged;
`glam` is allowed in client/engine only; the map schema gains an optional
display-only heightmap at version 2 with forward migration from version 1.
