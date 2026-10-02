# Content Guide

Status: **live as of milestone M2** (content pipeline, plan §14). This is the
designer's manual for the `content/` tree: what each file kind contains, the
units every field uses, how to add a unit, faction, or map without touching
engine code, and how to read validation errors. The authoritative spec is
[`plan.md`](../plan.md) §10 (boundary table, authoring units, Alpha manifest,
map requirements, add-a-unit tests) and [ADR-0001](adr/0001-3d-presentation.md)
(the display-only heightmap).

## The law in one paragraph

Stats, costs, requirements, maps, and factions are **versioned data files**
(FD-4); tick order, command semantics, and capability behavior are code. Data
only selects and parameterizes — if a rule needs logic, it becomes a capability
or a hook in code (plan §10.1). Every file starts with `schema_version`. Every
field is an integer, string, or bool — no floating-point values anywhere (FD-5).
Unknown fields are **errors** (strict mode is the only mode), so a typo can
never silently no-op.

## Layout

```text
content/
├─ rules/      exactly one ruleset: the resource registry, base population
│              cap, victory parameters
├─ entities/   one file per entity kind; the file name must equal the id
├─ factions/   one file per faction: roster, production lists, starting forces
└─ maps/       one file per map: grid, terrain legend, starts, ore, heightmap
```

- Files are sorted by name when loading, canonical collections end up sorted by
  id, and `KindId`s are indices into the entities sorted by id — loading is a
  pure function of the directory (tested).
- A file's declared `id` must equal its file-name stem (`entities/worker.ron`
  declares `id: "worker"`); a mismatch is a precise error.
- Extra non-`.ron` files (design notes) are ignored; subdirectories are errors.

## Authoring units (plan §10.2 — integers only)

| Quantity | Unit | Notes |
|---|---|---|
| Durations | milliseconds (`u32`) | converted at load to ticks: `(ms * 30 + 999) / 1000` (round up, never round away) |
| Speeds | milli-tiles per second (`i32`) | converted to fixed-point per tick at spawn |
| Ranges / radii | milli-tiles (`i32`) | 5000 = 5 tiles |
| Costs, hp, damage, population, ore amounts | plain integers | |

## Entities (`content/entities/<id>.ron`, schema_version 1)

See `content/entities/rifleman.ron` for a fully commented example. Fields:

| Field | Type | Meaning |
|---|---|---|
| `schema_version` | `u32` | must be `1` |
| `id` | string | must equal the file-name stem |
| `display_name` | string | shown in UI |
| `capabilities` | list | capability blocks, in order (below) |
| `cost.ore` | `i64` | Ore to build/train |
| `build_time_ms` | `u32` | build (structure) or train (unit) time |
| `population` | `i32` | population cost when fielded |
| `requires` | list of ids | entity ids that must exist first (one shared checker, plan §9.4) |

Capability blocks (the Alpha vocabulary — plan §7.4; the simulation consumes
all of them except Attack, which waits for combat in M6):

```ron
Health(( max: 60 ))                                   // optional regen_per_tick: 0
Move(( speed_milli_tiles_per_s: 2400, radius_milli_tiles: 350 ))
Attack(( damage: 8, range_milli_tiles: 5000, cooldown_ms: 1000, acquire_range_milli_tiles: 7000 ))
Gather(( carry_amount: 10, gather_time_ms: 2000 ))
Build(())
Produce(())
Storage(( resources: [ "ore" ] ))
ProvidesPopulation(( amount: 10 ))
Resource(( resource: "ore", amount: 1500 ))
Vision(( radius_milli_tiles: 8000 ))
Footprint(( w: 4, h: 4 ))
```

Rules the validators enforce: no capability twice on one entity; `Health.max`,
`Attack.damage`, ranges, radii, gather amounts, and footprints positive;
`acquire_range ≥ range`; resource references must exist in the rules registry.

## Factions (`content/factions/<id>.ron`, schema_version 1)

A faction is a bundle of entity references (plan §10.1) plus what its producers
train and the forces it starts with:

```ron
roster: [ "worker", "rifleman", ... ]        // every member must exist
production: { "barracks": [ "rifleman", ... ] }  // producers need the Produce capability
starting_forces: [                            // placed relative to each map start anchor
    ( entity: "command_center", offset: ( x: 0, y: 0 ) ),
    ( entity: "worker", offset: ( x: -2, y: 4 ) ),
]
```

`starting_forces` offsets are **tiles relative to the start anchor**. Structures
(footprint entities) must land on buildable, unoccupied ground; units (movers)
on passable ground outside every static footprint — the loader checks each
(map × faction) pair. Resource nodes may not be starting forces; maps place
nodes.

## Maps (`content/maps/<id>.ron`, schema_version 2)

- `width`/`height` in tiles (1..=4096; the Alpha map is 64×64).
- `terrain`: the legend — single-character codes with `passable` and
  `buildable` flags (passability classes as data, plan §10.5; the terrain grid
  becomes real input for M4's navigator).
- `grid`: one string per row, one legend character per tile.
- `starts`: `(player, anchor)` — the anchor is the top-left tile of the anchor
  structure; players unique, anchors in bounds, footprints must fit.
- `ore_nodes`: `(kind, tile)` — the kind must carry `Resource` + `Footprint`
  (the amount lives in the entity's Resource capability).
- `symmetric: true` opts the map into the 180-degree-rotation symmetry
  validator: grid, ore-node tile layout, and start anchors (mirrored through
  the anchor structure's footprint) must each map onto themselves.
- `heightmap: Some(( rows: [...] ))` — **display-only** (ADR-0001): one row of
  `width` integer heights per map row. The renderer displaces terrain vertices
  with it; the simulation never receives it, and it affects no gameplay. A map
  without a heightmap renders flat. (It does participate in the content hash:
  content identity covers the whole bundle.)

Version history: **v1** is the pre-ADR-0001 shape (no `heightmap` field) — the
loader migrates v1 forward by treating it as flat (FD-10: old content keeps
loading). **v2** adds the optional heightmap.

## How to add a unit (data-only — acceptance A3)

1. Copy the closest existing entity file to `content/entities/<new_id>.ron`,
   change `id` (must match the file name), stats, and capabilities.
2. Reference it from the faction: add to `roster`, to the right producer's
   `production` list, and (if you want it on the field immediately) to
   `starting_forces`.
3. Run `cargo run -p pandemonium-tools -- content-validate content`.

That is the whole procedure — no engine, sim, or tool file changes. The
acceptance test `tests/content_pipeline.rs::add_a_unit_is_data_only` does
exactly this in a temp copy and additionally proves the full loop — the kind
loads, spawns, obeys a Move order, and is **trained from the barracks**
(gather, Build, Train) — while nothing under `crates/sim/` mentions the new
kind; the git-diff half of A3 (zero changes under `crates/sim/` in the fixture
commit) is part of the commit review.

## How to add a map (data-only — acceptance A4)

1. Write `content/maps/<new_id>.ron` (schema_version 2). Declare
   `symmetric: true` only if the layout really is 180-degree symmetric.
2. Place ore so every start can reach it (the loader BFS-checks reachability
   from every start to every node, treating footprints as blocked).
3. Validate with `tools content-validate`. Build a match on it with
   `ContentTree::load_dir(...)?.bundle("<new_id>")`.

## Reading errors

Every rejection names its file (`entities/worker.ron`), the entity/map and
player it concerns, coordinates or indices where relevant, and the exact rule
violated — for example:

```text
maps/crossroads_64.ron: map 'crossroads_64': start for player 0 is illegal:
'command_center' footprint tile (13,14) sits on terrain 'rock' which is not buildable
maps/crossroads_64.ron: map 'crossroads_64': ore node 'ore_node' at (29,29) is
unreachable from player 1's start
```

Parse errors carry the decoder's line:column. Run the validator early and
often; it is the fastest feedback loop in the project.

## The bundle and the simulation seam

`ContentBundle::load_dir("content")` produces the validated bundle;
`bundle.world()` is the plain `TrivialWorld` the simulation receives
(`Sim::new(&bundle.world(), setup)`), and `bundle.content_hash()` /
`bundle.map_id()` are the identities a replay records (plan §6.5). Initial
entity ids follow the documented spawn order: starts in authored order, each
start's forces in faction order, then ore nodes in map order — deterministic
and part of the match contract.

The Alpha's starter values live in plan §10.4 and are pinned, cell by cell, by
`tests/content_pipeline.rs::alpha_manifest_loads_and_pins_the_plan_10_4_stat_table`.
