# ADR-0200: Authoritative vs presentation data — the boundary, the identity, and the content classes

- **Date:** 2026-10-13
- **Status:** accepted
- **Frozen decision affected:** **FD-1** (plan §2, plan §6.5 — what the content hash
  covers): *clarified* — the content identity is defined as covering authoritative
  content only, and the display-only heightmap is classified as presentation data
  whose participation in the hash is an interim state ended at the R2 re-baseline
  (§2 below). **FD-4** (plan §2 — the data/code boundary): *extended* — the
  "versioned data files" side of the boundary splits into authoritative content and
  presentation data, with the content classes (§3), the map format and discovery
  rules (§5), and the manifest contract (§7) as the extension. **FD-10** (plan §2 —
  content schema moves only forward): *extended* — the forward-only rule covers the
  map declaration fields (§5) and the manifest contract (§7) when they are added.
  **FD-5, FD-6, FD-9 are unchanged** and bind everything this ADR touches: no
  floats in sim or in any generation path (§6), the sim depends on nothing above it
  (the generator, packs, and discovery all live above the content seam), and events
  flow outward with presentation never polling sim internals.
- **Proposed by:** Agent B (brief B-008; recording the owner's long-term
  engine/asset boundary direction — the "brain vs body" split — relayed by the PM)

## Context

The owner's long-term vision is a strict split between the **brain** (engine,
simulation, rules — this repo's sim-side crates) and the **body** (models,
textures, audio, shaders, UI resources — assets that will live in their own
repository and be assembled with the engine at release time). Assets, maps, mods,
and manifests are versioned; community maps and packs load without changing the
executable; a launcher, a World Builder, and a seed-based World Generator come
later. Within that vision the owner has fixed five rules this ADR must encode:
cosmetic data stays **outside** the authoritative content hash while gameplay data
is validated; gameplay mods follow session compatibility rules (initially
disabled, isolated, or explicitly governed); resource and shader packs stay
independently manageable; cosmetic differences never change simulation results,
stats, hitboxes, or selection areas; generated worlds are deterministic and
exportable to the standard map format. Maps are data, never code. Nothing in the
vision is a build request: no launcher, no builder, no generator, and no mod
runtime is built now, and no abstraction is added without a concrete need. This
ADR only fixes the boundaries.

The current repo is almost entirely authoritative already, and the boundary is
implicit rather than declared. The one piece of presentation data that exists
today — the map's display-only heightmap (ADR-0001) — sits **inside** the map file
and **inside** both content identities: the canonical content hash covers rules,
entities, factions, and the whole chosen map including that heightmap
(`crates/content/src/bundle.rs:255-263`, the heightmap encoded at
`bundle.rs:781-793`, the contract documented at `bundle.rs:285-296`), and the map
id is a domain-separated hash over the same full map section
(`bundle.rs:264-269`, `bundle.rs:298-303`). ADR-0001 recorded that participation
as deliberate ("content identity covers the whole bundle, so a heightmap edit is a
content change for replay-matching purposes even though it cannot alter any
simulated outcome" — `docs/adr/0001-3d-presentation.md:90-95`). Everything else
visual is code-generated placeholder: primitive instanced boxes for entities
(`docs/ARCHITECTURE.md:337-342`), terrain class colors in the mesh builder
(`crates/engine/src/mesh.rs:93-101`), synthesized PCM audio behind the `AudioSink`
seam (ADR-0002), and a rect-and-text UI toolkit with an embedded font
(`crates/client/src/text.rs`). There are no model, texture, audio, shader, or icon
assets as data anywhere in the tree.

The pressure on the frozen decisions is therefore not a defect in what exists —
the gate is green (576 dev / 571 release at the session-start baseline), the
goldens are stable, and the heightmap's inclusion has costs no one has paid yet —
but the fact that the boundary is about to stop being free. The moment a second
asset repository, a presentation pack, or a community map channel exists, the
question "what exactly is a match's identity a hash of, and what may two
installations differ in while still playing the same match?" must have a written
answer, because the answer determines the replay format's semantics, the future
lockstep handshake's comparison set, and what a map file must declare to be
loadable. Deciding it later, after asset volume grows, would mean deciding it
under pressure with goldens in the blast radius; deciding it now costs one ADR and
moves nothing.

## Proposal

### 1. Terms: authoritative content vs presentation data

**Authoritative content** is every byte the simulation's outcome can depend on:
the ruleset, the entity kinds (stats, capabilities, costs, build times,
population, requirements), the factions (rosters, production lists, starting
forces), and the map's simulation-relevant fields (dimensions, terrain classes
with their passability/buildable flags, the grid, start positions, ore node
placements, the symmetry declaration). It is validated at load, hashed into the
match's content identity, and agreed identically by every participant in a match.

**Presentation data** is everything the player sees or hears that the simulation
cannot observe: the heightmap (today), and later models, textures, audio,
shaders, icons, and UI resources. It never enters the world definition the
simulation receives (`crates/content/src/bundle.rs:333-473` — `world()` emits the
passability and buildability grids, kinds, resources, production lists, base
population cap, and initial spawns, and no heightmap), never affects passability,
vision, combat, or production, and is never exchanged between peers.

Current location of every item (all citations at `5af55a3`; every file named
below is blob-identical between `5af55a3` and this branch's base `b8473e5`,
verified by blob hash in the session log):

| Item | Class today | Current location | Hashed into the content identity? | Reaches the simulation? |
|---|---|---|---|---|
| Ruleset (resource registry, base pop cap, victory params) | authoritative | `content/rules/alpha.ron` → `RulesDef` (`loader.rs:161-205`) | yes — `encode_rules` (`bundle.rs:621-635`) | yes — starting balances, base cap (`bundle.rs:391-400`, `bundle.rs:468`) |
| Entity kinds (stats, capabilities, costs, requirements) | authoritative | `content/entities/*.ron` → `EntityDef` (`loader.rs:210-247`) | yes — `encode_entities` (`bundle.rs:637-654`) | yes — capability templates + economy (`bundle.rs:353-385`) |
| Factions (rosters, production, starting forces) | authoritative | `content/factions/legion.ron` → `FactionDef` (`loader.rs:293-329`) | yes — `encode_factions` (`bundle.rs:726-750`) | yes — production lists + spawns (`bundle.rs:402-443`) |
| Map simulation fields (grid, terrain classes, starts, ore, symmetry) | authoritative | `content/maps/crossroads_64.ron` → `MapDef` (`loader.rs:331-383`) | yes — `encode_map` (`bundle.rs:752-780`) | yes — grids + spawns (`bundle.rs:387-389`, `bundle.rs:423-457`) |
| Map heightmap | presentation (by ADR-0001) | inline in the map file → `MapDef.heightmap` (`schema.rs:386-388`, `schema.rs:391-399`) | **yes, today** (`bundle.rs:781-793`) — the one presentation item inside the identity | **never** — not in `world()` (`bundle.rs:333-473`); its only consumer is the render-side terrain mesh (`engine/src/mesh.rs:90-101`) |
| `display_name` strings (rules, entities, factions, maps) | authoritative for identity, presentation in consumption | inline in each content file | yes (each `encode_*` writes them, e.g. `bundle.rs:622-623`, `bundle.rs:753-754`) | no (UI reads them above the seam) |
| Entity models | presentation | none as data — placeholder instanced primitives generated in client code (`docs/ARCHITECTURE.md:337-342`) | no | no |
| Textures / terrain art | presentation | none — class colors computed in the mesh builder (`engine/src/mesh.rs:93-101`) | no | no |
| Audio | presentation | none as data — synthesized PCM in client code (ADR-0002) | no | no |
| Shaders | presentation | WGSL embedded in the client's render modules (`docs/ARCHITECTURE.md:360-368`) | no | no |
| Icons / UI resources | presentation | none — rect+text toolkit, embedded font (`crates/client/src/text.rs`) | no | no |

The table's headline: **the repo's presentation surface is code, not data, except
for the heightmap** — and the heightmap is the one presentation item that today
sits inside the authoritative identity. The boundary this ADR fixes is therefore
narrow, concrete, and mostly a matter of writing down what already holds, plus one
deliberate, deferred correction (§2).

### 2. The hash boundary

What the identity is today (all at `5af55a3`): the content hash is `XxHash64`
over `CONTENT_ENCODING_VERSION` (u32, =1, `bundle.rs:30-32`), then the canonical
encodings of rules, entities, factions, and the whole map — including the
display-only heightmap (`bundle.rs:255-263`; `encode_map` at `bundle.rs:752-794`
writes the heightmap at `bundle.rs:781-793`; the contract is documented at
`bundle.rs:285-296`). The map id is a domain-separated `XxHash64` — first block
`MAP_ID_DOMAIN = 0x004D_4150` (`bundle.rs:34-36`) — over the same full map
section alone (`bundle.rs:264-269`, documented at `bundle.rs:298-303`), echoed
into the world definition (`bundle.rs:459-460`) and the replay header. **No state
hash folds either of them in**: `hash_state` encodes only the encoding version,
tick, RNG state, id allocator, entities with capability data, and players
(`crates/sim/src/hash.rs:33-171`, `STATE_ENCODING_VERSION = 4` at
`hash.rs:21-30`) — the map shapes state through its derived grids and spawns, but
neither identity value is ever hashed into a checkpoint.

Three options for where the boundary should sit:

**(a) Keep as is** — the content hash and map id continue to cover the whole map
including the heightmap.
*Goldens:* none move; the registered content hash `0x9bc18c521107b262` and map id
`0xd38136401ab02ff1` (`AI-Handoff.md:32`) stay exact. *Replay impact:* none —
`ReplayFile` carries `content_hash` + `map_id` (`crates/replay/src/lib.rs:49-69`)
and `verify_replay` compares both against the freshly loaded world
(`crates/tools/src/verify.rs:42-54`); nothing changes. *Migration:* none.
*Cost:* the owner's rule "cosmetic data stays outside the authoritative content
hash" is permanently unimplementable — a heightmap tweak (pure presentation, zero
simulated effect, provable: the heightmap never reaches `world()`) invalidates
every replay of that map and changes the `--version` identity line
(`crates/client/src/app/mod.rs:339-346`); worse, once presentation packs exist, a
peer with a different local pack could not share a session identity with a peer
without it, contradicting §3's "presentation packs are local, never hashed, never
exchanged".

**(b) Split the identity** — the authoritative content hash covers rules,
entities, factions, and the map's simulation-relevant fields only; the heightmap
leaves both hashes; a separate **presentation id** (same canonical-encoding
discipline, its own domain-separation tag) covers the heightmap today and the
presentation-pack set later.
*Goldens that move:* the content hash `0x9bc18c521107b262` and the map id
`0xd38136401ab02ff1` both move (the heightmap leaves the hashed field set of
each; `CONTENT_ENCODING_VERSION` bumps to 2 so the change is review-visible per
`bundle.rs:30-32`'s own rule). The demo golden `0xb6fff6659cfb7709` and flagship
golden `0x6e9a18bd7c5f699f` do **not** move — they are state hashes, and
`hash_state` never encodes either identity (`hash.rs:33-171`); the fixture
goldens in `tests/determinism.rs` (e.g. `0x5A467269B78F6B93` at
`tests/determinism.rs:503`) do not move (fixture worlds, not content bundles).
*Replay impact:* every existing replay's `content_hash`/`map_id` header fields
were computed under the old rule and fail `verify_replay`'s identity check
(`verify.rs:42-54`) under the new one. *Migration:* cheap by construction — no
replay files are committed to the repo (the A2 round-trip job records and
verifies within one CI run, `.github/workflows/ci.yml:107`;
`--record` outputs are per-session artifacts), so migration is: re-record the
evidence, bump the pinned values in the release smoke gates
(`.github/workflows/release.yml` — Lane A's file, cross-lane request), and
update `AI-Handoff.md:32`'s goldens row in the same PM sync. Per standing
orders §7 the moves land in **one dedicated commit with the reason in the
message**.

**(c) Move the display data out of the map file** — the map schema drops the
heightmap entirely; heightmaps live in presentation packs keyed by map id.
*Goldens:* the same two move as in (b). *Replay impact:* same as (b). *Migration:*
far larger: map schema v3 loses a field, the v1→v2→v3 migration chain must decide
where inline heightmaps go (FD-10 — old content keeps loading, so v2 maps with
inline heightmaps must keep working, which means the loader splits them at load…
which is (b)'s computation anyway), the renderer's terrain mesh source changes
(`engine/src/mesh.rs:93-101` reads `MapDef.heightmap` — engine and client are
Lane A's crates), and a presentation-pack discovery/loading channel must exist
before any heightmap can ship — the very machinery the owner deferred ("add no
abstraction without a concrete need").

**Recommendation: (b), adopted at the R2 re-baseline — not now.** No golden is
authorized to move by this ADR; this brief touches docs only, and the current
identity rule stays in force until a brief explicitly bundles the golden moves.
The R2 re-baseline is the moment: one coordinated change (encoding bump, pinned
values, re-recorded evidence, the AI-Handoff sync), one dedicated commit, with
the presentation id arriving alongside. Until then, the current rule of
`bundle.rs:285-296` — the heightmap participates in the identity even though it
cannot alter any simulated outcome — remains the operative clarification of FD-1,
and this ADR is the written record that it is an interim state, not the design.

### 3. Content classes and the multiplayer compatibility model

Three classes, matching the `kind` taxonomy of §7:

1. **Base content** — the shipped content tree (rules, entities, factions, the
   map set). Authoritative: validated at load, hashed into the content identity,
   identical on every peer. This is everything the repo has today.
2. **Gameplay mods** (`kind: rules-mod`) — additional or overriding authoritative
   content: new entity kinds, stat changes, new factions, rule parameter changes.
   Authoritative by definition (they change bytes the simulation can observe), so
   **part of the identity**: a mod set hashes into the same content identity as
   base content — two peers with different mod sets have different content hashes
   and cannot enter the same session. Session rules (the owner's): agreed per
   session by every participant before the match starts, **disabled by default in
   matches** (a match runs base content unless every peer explicitly opts into a
   named mod set), and initially isolated or explicitly governed — a mod
   declares its dependencies and schema ranges (§7) and loads only through the
   same validators as base content (strict parse, reference checks, placement
   checks per `(map, faction)` — `bundle.rs:100-106`); there is no mod API, no
   scripting, and no code execution (plan §10.1: do not turn content into a
   scripting language, `plan.md:360-371`).
3. **Presentation packs** (`kind: resource-pack`, `kind: shader-pack`) — local
   cosmetic data. **Never hashed into the session identity, never exchanged, never
   validated for gameplay.** A peer's packs are invisible to the protocol: two
   players with different texture packs, shader packs, or audio packs play the
   same match, produce identical state hashes, and verify each other's replays.

What each artifact carries and compares:

| Artifact | Carries | Compares |
|---|---|---|
| Match record (today `MatchSetup`: seed + player slots, `crates/sim_api/src/lib.rs:94-101`) | the seed, the controller kinds — plus, when multiplayer arrives: the content identity, the map id, the (empty-by-default) ordered mod list | nothing yet (single-player); the future lockstep handshake compares all of the left column against every peer |
| Replay (today `ReplayFile`, `crates/replay/src/lib.rs:49-69`) | `format_version`, `content_hash`, `map_id`, `seed`, `player_setup`, the exact command stream, checkpoints, `final_hash` | on verify: `content_hash` and `map_id` against the loaded world **first**, before a tick is spent (`crates/tools/src/verify.rs:34-54`), then every checkpoint hash by re-simulation |
| Lockstep handshake (future; not built) | engine/protocol version, content identity (which folds in the mod set), map id, seed, player setup | every peer must agree on all of them before tick 0; per-tick checkpoint hashes then detect desync mid-match (plan §17's hook: "Deterministic command-log sim, Command.tick, hashed checkpoints", `plan.md:602`) |

The mod list is deliberately *not* a separate field: the content identity is a
hash over the loaded authoritative content as a whole, so a mod set changes the
identity by construction and no separate comparison can be forgotten. The
replay's header already carries exactly the two identity values a mod-aware
future needs.

**The cosmetics rule list** (the owner's "cosmetic differences never change
simulation results" made checkable) — presentation data may not affect:

- **stats** — capability parameters come from entity content only
  (`bundle.rs:353-385`); no presentation source can reach them;
- **footprints** — structure and node occupancy comes from the `Footprint`
  capability in content data (`schema.rs:237-245`, `bundle.rs:543`);
- **hitboxes** — combat ranges and acquisition radii are `Attack` capability
  fields in content (`schema.rs:169-180`); collision radii are `Move` capability
  fields (`schema.rs:158-166`); nothing visual feeds them;
- **selection areas** — pick and hover reach is computed from the content
  footprint, not model bounds: the hover pick's extra radius is derived from
  `def.footprint()` of the entity's content definition
  (`crates/client/src/app/mod.rs:1137-1148` feeding
  `crates/client/src/input/orders.rs:142-161`), and box selection projects entity
  positions, not model geometry (plan §11.3, `plan.md:447`). **Selection sizes
  must always come from rules data, never from model bounds** — when real models
  arrive, this is the rule that keeps a fancy mesh from silently widening what a
  player can click;
- **pathing** — the nav grid is built from the map's terrain classes
  (`bundle.rs:566-605`), never from visual collision geometry.

### 4. Information fairness

In lockstep multiplayer every peer necessarily holds and advances the full
simulation state — that is what FD-1's deterministic command-log design means —
so information fairness is a property of what a client *presents*, not what it
computes. FD-7 covers the AI ("AI touches the game only through commands and a
fog-filtered PlayerView", `plan.md:68`); FD-8 ("fog filters information; it never
alters the simulation") and FD-9 ("events flow outward; presentation never polls
sim internals") apply to every consumer above the seam.

What the client reads today, at `5af55a3` (the post-A-006 layout):

- **The render path is fog-filtered.** The App owns a dedicated interpolator over
  the *human's* view, documented as "the human sees what the human's player can
  see (FD-8 from the rendering side)" (`crates/client/src/app/mod.rs:178-182`).
  Each frame it pulls `host.player_view(HUMAN)`, pushes only that view's entity
  set into the view interpolator, and draws everything through
  `view_snapshot()` — "every drawn pixel and every screen query (selection,
  context orders) reads through this" (`app/mod.rs:507-516`, the frame body at
  `app/mod.rs:1052-1081`). The fog texture comes from `view.fog`
  (`app/mod.rs:1067-1076`); the minimap markers come from the same filtered
  entity set (`app/mod.rs:1301`); the renderer's instance, shadow, and overlay
  passes all consume `frame.snapshot`, which is the filtered blend
  (`crates/client/src/render/mod.rs:373-376`, `:470`, `:495-502`). The HUD reads
  `hud_state`, which is itself built from `player_view`
  (`crates/engine/src/host.rs:240-252`). The command card reads `latest_view`
  (`app/mod.rs:194-196`, set at `app/mod.rs:1080`).
- **Full-state reads that remain, and where they live:** the host's own
  full-snapshot interpolator (`host.rs:66-70`, fed at `host.rs:119-120` and
  `host.rs:199`, exposed as `render_snapshot()` at `host.rs:276-278` and
  `snapshot()` at `host.rs:289-292`) — consumed today by the headless smoke and
  the client's tests (`crates/client/src/main.rs:1120`, `:1322`, `:1330`), not by
  the match render path; and `hud_state_with_hash` (`host.rs:264-270`), which
  surfaces the full-world canonical hash for the F3 debug overlay only
  (`app/mod.rs:1180-1184`).

**Recommendation: the renderer consumes a fog-filtered view — keep it, and treat
it as the rule.** The current architecture already answers the brief's question
the right way; this ADR records it as a named invariant (the presentation layer
renders and hit-tests through per-player views in match play, never through raw
snapshots) rather than a coincidence of the current code. A competitive mode
could additionally restrict: disable the debug overlay's hash read in ranked
play (it is a full-state read, even if only a digest), gate replay file access
until a match ends, and confine spectator/observer views to explicit
participants. What a competitive mode *cannot* do, within FD-1's lockstep
design, is hide state from a hostile client — every peer computes everything,
so a modified client can always read the full world; protocol-level information
hiding would require a server-authoritative or state-minimal architecture, which
would trade away the frozen determinism design and is out of scope (§8).

### 5. Map format and discovery

The map format rules, all of which hold today and become the fixed contract:

- **Self-contained**: one file carries the whole map — dimensions, terrain
  legend, grid, starts, ore nodes, symmetry flag, optional heightmap
  (`schema.rs:360-389`). The only cross-file references are ore-node *kind* ids,
  resolved and placement-validated against the entity set in the same tree
  (`loader.rs:331-383`, `bundle.rs:100-106`) — a map plus its referenced entity
  kinds is a closed set.
- **Versioned**: `schema_version: 2` today (`schema.rs:22-24`); v1 migrates
  forward one explicit typed step (`version.rs:101-121`); newer-than-loader is a
  precise error (`version.rs:31-40`) — FD-10's forward-only rule.
- **Data-only**: RON with `deny_unknown_fields` on every struct
  (`schema.rs:10-12`); no scripting, no logic, no code execution — maps are
  data, never code.
- **Discovery**: a designated maps directory scanned at startup. Today that
  directory is `content/maps/` inside the single content tree — sorted file
  order, `.ron` only, subdirectories refused loudly (`loader.rs:63-75`,
  `loader.rs:107-149`) — and the client already lists every map the tree
  contains for the New Match screen (`app/mod.rs:960-968`) and bundles the
  chosen one on demand (`app/mod.rs:379-394` via `tree.bundle(map_id)`,
  `bundle.rs:133-152`). FD-4's "add-a-map requires zero engine changes"
  (`plan.md:65`, tested by A4's data-only pattern, `plan.md:431-433`) therefore
  already holds at the tree level: a new `.ron` in `content/maps/` appears in
  the picker with no code change.

**Reconciling with `load_dir`'s one-map shape:** `ContentBundle::load_dir` is the
Alpha convenience that bundles *the directory's only map* and errors otherwise
(`bundle.rs:154-171`, `bundle.rs:280-283`) — it is **not** an engine limitation.
The client never uses it to start a match: it loads the full tree
(`ContentTree::load_dir`, any map count) and picks a map explicitly
(`crates/client/src/main.rs:132-142` picks the first map only for the default
backdrop bundle; `begin_match` bundles whatever the player chose). The one-map
error is `single_map_bundle`'s guard for the `--version` and default-bundle
paths, removable the day a second map ships.

**What a map declares** — today: `id`, `display_name`, dimensions, terrain,
grid, starts, ore nodes, `symmetric`, optional heightmap (`schema.rs:365-389`).
The contract adds three declaration fields at map schema v3 (specification only;
not added by this ADR): `author` (free text), `licence` (SPDX id or named
licence with a pointer to text), and a **content compatibility range** — the
referenced-content declaration: either the explicit entity ids the map places
(today implicit in `ore_nodes[].kind`, already resolved exactly by validation)
or a content-schema range the map is known to load against. Exact id resolution
stays the validator's rule; the range exists so a community map can declare "I
was built for entity schema 1" and load forward under FD-10 without re-authoring.

**The minimal loader change this requires (stated, not made):** a second scan
root for maps — a user maps directory (for example `maps/` beside the
executable or a per-OS user-data path) scanned with the same sorted-`.ron` rules
after `content/maps/`, its files validated against the base tree's entities,
factions, and rules, their ids merged into the tree's map list (duplicate ids
across roots rejected with both sources, exactly as `bundle.rs:174-192` does
within one root). That is a content-crate change only — no engine, no sim, no
client change — and it is what makes "community maps loadable without changing
the executable" true in the packaging sense (a player drops a file into a
directory; the executable is untouched). Map schema v3's declaration fields
arrive in the same change with the v1→v2→v3 migration chain extended one step.

### 6. World generation constraints

The seed-based World Generator is future work; these are the constraints it will
be built under, fixed now so the design cannot drift:

- **Integer-only** (FD-5, `plan.md:66`, `plan.md:140-153`): all generation math
  is integer or `fx` fixed-point; no floats anywhere in the generation path —
  floats would break the cross-OS bit-identity the generator exists to provide.
- **`fx` PCG32 randomness**: all generation randomness flows from `fx::Rng`
  (PCG32, `docs/ARCHITECTURE.md:16`, `:60`), seeded from the world seed, owned
  by the generator, advanced in a fixed order — the same discipline plan §5.4
  imposes on the sim.
- **Versioned algorithm and config**: the generator carries an explicit version
  word (the `CONTENT_ENCODING_VERSION` / `STATE_ENCODING_VERSION` pattern —
  `bundle.rs:30-32`, `hash.rs:21-30`) and its configuration (size class, terrain
  parameters, resource density, symmetry mode) is versioned data; changing
  either changes outputs and must be a review-visible version bump, never a
  silent shift.
- **Seed code = (generator version, config, seed)**: the shareable token a
  player hands another player is a pure encoding of those three; a code pins the
  generator that produced it. No code is ambiguous about what it will grow.
- **Output = a standard map file**: the generator emits exactly the map schema
  of §5 (data-only, self-contained, versioned), passing the same validators —
  not a special runtime format. Generated worlds are deterministic and
  exportable to the standard map format (the owner's rule), which this satisfies
  by construction: the export *is* the output.
- **Generate-then-load**: the simulation only ever sees the finished map through
  the normal content pipeline (`ContentBundle::world()` → `TrivialWorld`); the
  generator has no presence inside the sim, no hooks, and no state the sim
  observes — FD-6's "simulation depends on nothing above it" holds with the
  generator above the content seam like any other tool.

**Multiplayer distribution — two models:** (i) the **host distributes the map
data** (the generated map file/section) and every peer **verifies `map_id`** (and
the content identity) against what it received; (ii) **each peer regenerates**
the map from the seed code locally and the handshake compares the resulting
`map_id`s. **Recommendation: (i) host-distributes with `map_id` verification.**
The verification step is mandatory in both models — regenerating without
comparing would be trust without checking, and the comparison is the existing
`verify_replay` discipline (`verify.rs:42-54`) — so model (ii) adds a constraint
(every peer must run the same generator version at match time, and must run one
at all) to save one transfer that model (i) performs once per match.
Distribution also works uniformly for hand-authored maps, edited maps, and
generated maps, which regeneration does not. Peer regeneration remains available
as an offline/lobby convenience (a peer who has the same generator can regrow
the map instead of downloading it); the protocol still compares `map_id`.

**Cross-OS golden requirement (recorded for when the generator is built):** for a
fixed seed code, the generator's output must be bit-identical across OSes —
pinned by the same cross-OS pattern CI already runs for the demo and flagship
goldens (the `cross-os` matrix and `cross-OS golden gate (A1)` comparing
`GOLDEN_DEMO`/`GOLDEN_FLAGSHIP`, `.github/workflows/ci.yml:163-242`). A
generated-map golden joins the registered goldens at that time; integer-only math
is what makes the requirement achievable rather than aspirational.

### 7. The manifest contract (specification only)

No loader, no schema change, no parsing — this section is the specification the
mod/pack era will implement, written now so the taxonomy is fixed before the
first non-base content exists. Every distributable content item (mod, map,
pack) carries a manifest with common fields:

| Field | Meaning |
|---|---|
| `id` | stable, unique, the identity other items reference (the file-stem discipline content already enforces, `loader.rs:170-176`) |
| `version` | the item's own version, independent of any schema version |
| `kind` | one of `rules-mod`, `map`, `resource-pack`, `shader-pack` |
| `engine-api` range | min..max engine/protocol API the item supports |
| `content-schema` range | which content schema versions the item targets (FD-10 ranges, not single pins, so items load forward) |
| `dependencies` | ordered list of `(id, version-range)` — loadable only when all resolve |
| `licence` | SPDX id or named licence + pointer to the text (the repo itself is MIT) |
| `author` | free text; optional contact |

**This ADR owns the authoritative-side fields and the `kind` taxonomy**: what a
`rules-mod` may carry (authoritative content only — entities, factions, rules
parameters; §3's session rules apply), what a `map` is (§5's format — its
declaration fields are the map-manifest subset: `id`, `schema_version`, name,
author, licence, referenced content), and the fact that `resource-pack` and
`shader-pack` are presentation classes (§3: local, never hashed, never
exchanged). **The internals of presentation packs** — directory layout, asset
naming and override rules, versioning of pack assets, integrity/signing if any —
belong to a later Lane A ADR (crates/client and the presentation stack are Agent
A's lane per STANDING_ORDERS §3); listed as a cross-lane request in the B-008
report. Maps today already carry `id` + `schema_version` + name inline
(`schema.rs:365-389`) — the manifest generalizes a discipline the map format
already practices; nothing parses manifests until the loader work of §5/§7 is
briefed.

### 8. Non-goals and triggers

Nothing in this ADR is a build request. The following are explicitly **not
designed now**, each with the concrete need that would trigger its design (the
owner's "no abstraction without a concrete need" as a table):

| Not designed now | Would be triggered by |
|---|---|
| **Launcher** (install, update, pack management UI) | a published release that real users install and update at enough scale that manual install friction is reported by the playtest |
| **Hub** (matchmaking, server browser, community channel) | shipped lockstep multiplayer plus players needing to find each other beyond direct connection |
| **World Builder** (in-game map editor) | map authors outgrowing hand-authored RON after the map format is stable at v3 and the community asks for tooling |
| **Mod runtime** (loading gameplay mods into live matches) | the multiplayer session-compatibility rules of §3 being implemented (the handshake compares identities) **and** a first party-quality rules-mod proving the pipeline — until both, mods stay disabled-by-default and testable headless only |
| **Scripting / data-as-logic** | a concrete gameplay need that capabilities and code hooks cannot express — and even then it needs its own ADR touching FD-4's core (plan §10.1's "do not turn content into a scripting language" is the standing refusal, `plan.md:360-371`) |
| **Network protocol** (lockstep wire format, handshake encoding) | the owner green-lighting multiplayer after the Alpha declaration (plan §17's hook is already present: the command log, `Command.tick`, hashed checkpoints, `plan.md:602`) |
| **The generator, the pack channel, the user maps directory** (§5/§6/§7's loader-side work) | the first concrete consumer: the World Builder/generator brief, the first presentation pack, or the first community map — each arrives as its own brief touching the content crate only |

## Evidence

Every current-behavior claim above cites `file:line` at `5af55a3`
(`5af55a303883e5bf919b2576b3c1acc7a9e478c3`). Verification performed in the
B-008 session:

- **Blob-identity**: all 27 files cited (the content crate's `bundle.rs`,
  `schema.rs`, `version.rs`, `loader.rs`, `defs.rs`; `sim/src/hash.rs`;
  `sim_api/src/lib.rs`; `engine/src/{host,renderer,mesh}.rs`; the client's
  `app/mod.rs`, `main.rs`, `input/orders.rs`, `render/mod.rs`, `report.rs`,
  `text.rs`; `replay/src/lib.rs`; `tools/src/{verify,main}.rs`;
  `tests/{determinism,content_pipeline}.rs`; `plan.md`; `docs/ARCHITECTURE.md`;
  `docs/CONTENT_GUIDE.md`; `docs/adr/TEMPLATE.md`; ADR-0001; ADR-0100;
  `AI-Handoff.md`; `content/maps/crossroads_64.ron`) are blob-identical
  between `5af55a3` and this branch's base `b8473e5` (per-file
  `git rev-parse <sha>:<path>` equality, scripted) — every citation is valid at
  both commits, so the ADR reads true against the brief's named head and
  against the branch it rides on.
- **The identity values are live**: `tools content-validate content` on this
  tree prints `content hash: 0x9bc18c521107b262, map id: 0xd38136401ab02ff1`
  and `heightmap: yes (display-only, ADR-0001)` — the registered goldens
  (`AI-Handoff.md:32`), re-derived, not trusted.
- **The state hash is identity-free**: `hash_state`'s encoding enumerated field
  by field at `crates/sim/src/hash.rs:33-171` — version word, tick, RNG state,
  id allocator, entities, players; no content hash, no map id, no fog bitsets
  (fog is derived and unhashed, A-059) — which is why §2 can state that the
  demo and flagship goldens cannot move under any hash-boundary option.
- **The gate**: the full four-step gate on the untouched-master baseline tree
  (this branch's base, `b8473e5`, whose tree equals master's): fmt clean,
  clippy `-D warnings` clean, **576 dev / 571 release, 0 failed, 2 ignored** —
  matching the brief's expectation and the AI-Handoff §2 record; the branch's
  diff is two Markdown files, so the counts hold on the final head by
  construction.
- **No code, test, content, or workflow change is needed for this ADR to be
  true**: the diff is `docs/adr/0200-authoritative-vs-presentation-data.md`
  (this file) and `docs/pm/reports/B-008.md` — the acceptance check of the
  brief, satisfied by construction.

## Consequences

- **Cheaper / possible if accepted:** the brain/body split has a written
  boundary before asset volume exists; the identity's scope (FD-1 clarified) is
  pinned, so the future asset repository, pack channel, and mod support inherit
  a fixed contract instead of inventing one under pressure; the heightmap's
  current participation in the identity is recorded as interim with a named
  exit (R2 re-baseline) rather than discovered during the first presentation
  pack; the cosmetics rule list (§3) gives every future presentation feature a
  checkable definition of "cannot affect the simulation"; the generator's
  constraints (§6) make the World Generator's determinism obligations part of
  the accepted design rather than a future argument.
- **More expensive:** the R2 re-baseline must budget the two golden moves, the
  encoding bump, the release-smoke pin updates (Lane A's `release.yml`), and a
  re-record of the replay evidence — a known, bounded cost, deliberately
  deferred; the fog-filtered-render invariant (§4) constrains future debug and
  spectator tooling to keep full-state reads out of match play; the manifest
  contract (§7) commits future content tooling to the four-kind taxonomy.
- **What breaks if accepted: nothing, now.** No golden moves in this brief
  (standing orders §7: none authorized); no test, replay, or workflow changes —
  the diff is two Markdown files and the gate counts are unchanged. The next
  golden-touching change is the R2 re-baseline itself, which must bundle §2(b)'s
  moves in one dedicated commit with the reason in the message.
- **FD-1's clarification is honest about the interim**: until R2, a heightmap
  edit remains a content-identity change for replay matching (the current rule,
  `bundle.rs:285-296`); this ADR does not silently reinterpret that rule, it
  schedules its replacement.

## Alternatives considered

- **Do nothing (no ADR; keep the boundary implicit).** Rejected: the boundary is
  about to stop being free — the owner's asset-repository and pack direction
  makes "what does the identity cover?" a load-bearing question for replays,
  the future handshake, and map loading, and deciding it after asset volume
  grows means deciding it with goldens and content in the blast radius. The
  plan's own operating contract (plan §0: frozen decisions change only through
  an ADR) names this document as the mechanism.
- **Hash boundary (§2): keep the heightmap inside the identity forever
  (option a).** Rejected as the long-term rule — it makes the owner's
  "cosmetics stay outside the authoritative hash" unimplementable and makes
  local presentation packs impossible to reconcile with a shared session
  identity. **Move the heightmap out of the map file now (option c).** Rejected
  for now — it spends the pack/discovery machinery this ADR deliberately
  defers (owner's "no abstraction without a concrete need") and drags Lane A's
  engine/client code into a change that option (b) achieves inside the content
  crate alone. **Split now rather than at R2.** Rejected — no golden move is
  authorized by this brief; an unforced golden move without a re-baseline
  window violates standing orders §7's spirit.
- **Content classes (§3): a single content class (no mod taxonomy).** Rejected:
  the owner's rules already distinguish gameplay mods from presentation packs,
  and the distinction is exactly what the identity semantics and the session
  rules need; leaving it unnamed guarantees the first mod support invents the
  wrong split. **Hash presentation packs into a per-peer identity and exchange
  them.** Rejected: contradicts the owner's "packs are local and independently
  manageable" and would make cosmetic differences session-visible.
- **Renderer fog view (§4): render from the raw snapshot (the pre-observer-seam
  state) and draw fog as an overlay only.** Rejected — it would make the honest
  client display state the player should not see and violate the recorded
  intent of the view interpolator (`app/mod.rs:178-182`); the fog-filtered
  path exists and is tested. **Protocol-level information hiding.** Rejected:
  impossible in FD-1's lockstep (every peer computes full state) without
  trading away the frozen determinism design; out of scope by §8.
- **Map discovery (§5): keep `content/maps/` as the only map source.** Rejected
  as the end state — "community maps loadable without changing the executable"
  requires a user-writable scan root; retained as the interim state. **A
  container/collection map format (many maps per file).** Rejected: the
  self-contained single-map file is loadable, validatable, and shareable
  independently — the smallest thing that works.
- **World generation distribution (§6): peer regeneration as the primary
  model.** Rejected: verification is mandatory in both models, so regeneration
  only adds a same-generator-version constraint to save one transfer, and
  fails for hand-authored or edited maps. **Generator inside the sim (grow the
  world during the match).** Rejected: violates FD-6 (the sim would depend on a
  generator) and breaks generate-then-load's clean seam.
- **Manifest (§7): defer the contract until the first pack ships.** Rejected:
  the `kind` taxonomy and the authoritative-side fields are what make the
  first pack brief writable without re-litigating the classes; the cost of
  writing the spec now is this section. **Implement the manifest loader now.**
  Rejected: no consumer exists — the owner's "no abstraction without a concrete
  need".

## Decision

Accepted by the owner, 2026-10-13. The operative rules are the current ones
(the heightmap stays inside the identity; every deferred item in §8 stays
unbuilt).
