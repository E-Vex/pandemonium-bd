# Assets — the "body" side of the brain/body split

## The rule

The **brain** (`crates/sim`, `crates/sim_api`, `crates/fx`, `crates/ai`) is the
deterministic simulation: entity ids, stats, positions, commands, state hashes.
It never sees a model path, texture, or sound file, and
`tests/architecture_law.rs` fails the build if anything above it leaks down
(plan §4, FD-6).

The **body** (`crates/client`, `crates/engine`) is presentation: models,
textures, shaders, audio, icons. It renders from fog-filtered snapshots the
sim pushes out — `MatchHost::player_view()` — and the sim never polls
presentation internals (FD-8, FD-9, docs/ARCHITECTURE.md §"The boundary").

Presentation data is **local, never hashed, never exchanged** between peers
(docs/adr/0200-authoritative-vs-presentation-data.md §3): two players on the
same server can load different packs, see different visuals, and still produce
identical state hashes because the pack never enters the simulation.

## Where files live

| Location | Tracked in Git? | What goes there |
|---|---|---|
| `content/**` | **Yes** | Authoritative `.ron` data: rules, entities, factions, maps. Every peer in a match must have byte-identical content (this is the match's content identity). |
| **`E-Vex/pandemonium-assets`** (separate repo) | **Yes — in its own repo** | The official presentation packs. `default/` is the shipped pack (`pack_id: "tactical_field"`). Never committed here. |
| `crates/client/assets/packs/` | **No** (gitignored) | The local install area the fetch script and the release archives fill in — see "The official pack" below. |
| `crates/client/assets/{models,textures,audio,shaders}/` | **No** (gitignored) | Local presentation data only. A fresh clone runs without them — colored boxes and synthesized audio are already in client code. |
| `/user-maps/`, `/user-packs/`, `/mods/` | **No** | User content discovered beside the executable at runtime; no engine change needed (ADR-0200 §5). |

The stray `war-*.wav` files at the repo root are gitignored by the `*.wav`
rule and must not be `git add`-ed.

### The assets repo's two zones (Standing Orders v1.4)

The assets repo carries two kinds of directories with two different entry
rules:

- **`design/` — Designer reference material** (art bibles, palettes, concept
  boards, role notes). The Owner commits it directly; no Programmer brief,
  no client load — nothing in `design/` ever ships or loads.
- **Pack directories — `default/` and any other pack** (the manifests and
  files the client actually loads). These enter **only through a Programmer
  brief and a verified client load**: the Designer delivers to the Owner,
  the Owner relays a brief, the Programmer commits, and the load is proven
  (the release pipeline's packaged smoke or the dev run's
  `asset pack '...' loaded` line). Reference material that wants to ship
  moves from `design/` into a pack through that same path.

## Asset pack system (implemented)

The client can load an optional, local **asset pack** at startup. The pack is
presentation-only: it maps entity kind ids (from `content/entities/*.ron`) and
terrain class ids to local asset file paths. It is never hashed, never sent
to peers, and never reaches the simulation.

### The official pack (A-014)

The official art lives in its own repository,
**[E-Vex/pandemonium-assets](https://github.com/E-Vex/pandemonium-assets)**,
not in this repo — art history stays out of the game's history, and the
binary clones stay small. The repo holds the packs; `default/` is the one
that ships (its manifest keeps `pack_id: "tactical_field"` — the id names
the art style, `default/` names the slot it loads into).

Three delivery paths, one pack:

1. **Dev**: `scripts/fetch-assets.sh` — one command, idempotent. It clones
   or updates the assets repo (shallow) under
   `crates/client/assets/packs/` (gitignored) and copies `default/` to
   `crates/client/assets/packs/default/`, the compile-time dev path.
   Overrides for forks: `PANDEMONIUM_ASSETS_REPO` / `PANDEMONIUM_ASSETS_REF`.
2. **Release**: the `release` workflow checks the assets repo out and
   packages `default/` into every archive at `assets/packs/default/` —
   next to the binary, discovery order #2 below — so a player unpacks and
   sees the pack with zero setup. The packaged smoke FAILS if the manifest
   is not the `tactical_field` pack, if any of the 8 entity textures is
   missing, or if the headless client run does not print the pack-loaded
   line (`asset pack 'tactical_field' (schema 1) loaded ...`).
3. **Players/artists**: drop any pack under `<exe_dir>/assets/packs/<name>/`
   or point `--pack <dir>` at it — always local, never hashed.

The Designer never edits `content/` (Standing Orders v1.4): art enters a
pack only through a Programmer brief and a verified client load, delivered
via the Owner; Designer reference material lives in the assets repo's
`design/` and the Owner commits it directly.

### Pack directory discovery

The client searches for a pack directory in this order:

1. The `--pack <dir>` command-line flag (the player's explicit choice).
2. `<exe_dir>/assets/packs/default/` (the release archive layout: binary and
   assets side by side).
3. The compile-time dev path
   `crates/client/assets/packs/default/` (the `cargo run` dev layout).

A missing pack directory is **not an error** — the client falls back to
placeholder rendering (colored boxes, synthesized audio).

### Manifest format (`manifest.ron` at the pack root)

The current schema is **2** (M12, ADR-0101 §4); **v1 manifests keep loading
unchanged** and migrate forward at load — a `.png` path becomes the kind's
`skin`, a `.glb`/`.gltf` path becomes its `model`, every other field takes
its default (`crates/client/src/pack/mod.rs`, `build_v2_entry` over the
migrated shape). The official `tactical_field` pack is still schema 1 — its
release smoke line (`asset pack 'tactical_field' (schema 1) loaded`,
`.github/workflows/release.yml`) stays true until that pack itself bumps.

A v2 manifest (the shape of ADR-0101 §4's example, which parses verbatim
against the M12 loader):

```ron
(
    pack_id: "default",
    schema_version: 2,
    entities: {
        "rifleman": (
            model: Some("models/units/rifleman.glb"),      // optional
            skin:  Some("textures/units/rifleman.png"),    // optional
            scale: 1.0,            // optional, default 1.0, 0.05..=10.0
            rotation_deg: 0.0,     // optional, default 0.0, -360.0..=360.0
            ground_offset: 0.0,    // optional, meters, default 0.0
        ),
    },
    terrain: {
        "grass": "textures/terrain/grass.png",
        "rock":  "textures/terrain/rock.png",
    },
)
```

- `pack_id` (string) and `schema_version` (1 or 2) are required; a
  `schema_version` above 2 is the precise `UnsupportedVersion` error
  before any field is read.
- `entities` keys are entity kind ids — they must match the `id` field in
  `content/entities/<id>.ron`. A v2 entry's fields are all optional:
  `model` (a `.glb` path — the model tier) and `skin` (a `.png` path —
  the skin tier) are the two renderable references; `icon` (`.png`-only)
  and `sounds` (cue name → audio path, cue names checked against the
  engine's cue vocabulary) are **validated but not consumed in M12**; and
  `scale`/`rotation_deg`/`ground_offset` are the per-pack placement fixes
  the model upload bakes in (see "The model tier (M12)" below). A kind
  that declares neither a model nor a skin renders the placeholder.
- `terrain` maps a terrain class id to a relative asset path. Entries are
  validated; their consumption is the terrain pass's single whole-map
  tint (see "Terrain textures" under "Current limitations").
- All paths are relative to the pack root directory.
- Unknown manifest fields are rejected (strict mode).

### Asset resolution and security

All declared asset paths are validated before use:

- Absolute paths are rejected.
- Path traversal (`..` components) is rejected.
- Resolved paths are canonicalized (symlinks resolved) and must remain inside
  the pack root.
- Unsupported schema versions are rejected.
- Malformed RON is rejected.

Non-fatal issues (a referenced asset file missing at manifest load) do not
abort the pack. A v2 entity reference that cannot be read records a
deferred note (`PackNote`) that the renderer's startup resolution folds
into that kind's single diagnostic line; a v1 entity path or a terrain
class keeps its own direct one-line print, exactly as it always printed
it. The kind then renders on the best tier the fallback chain allows —
model, skin, or placeholder (see "Fallback behavior" below); a file the
renderer cannot parse (a non-GLB "model", an undecodable PNG) fails the
same way, one tier down, never a crash.

### Supported asset formats

The pack system recognizes these extensions:

| Extension | Kind | Renderer support |
|---|---|---|
| `.png` | Texture | **Loaded and rendered** — the kind's skin tier: the PNG wraps the box silhouette (see "PNG texture rendering"). Terrain-class PNGs are consumed as the terrain pass's single whole-map tint (see "Current limitations") — per-class sampling is still not implemented. |
| `.glb` | Model | **Loaded and rendered** — the kind's model tier: the static mesh draws in place of the boxes (see "The model tier (M12)"). |
| `.gltf` | Model (recognized) | Recognized by the pack system (a v1 `.gltf` entry migrates to a `model` reference) but refused at load — model entries are glTF 2.0 binary only (ADR-0101 §1). One diagnostic, and the fallback chain walks. |

Other extensions are not recognized by `AssetPack::kind_of()` and fall back to
placeholders. Which tier draws a kind — model, skinned box, or placeholder
silhouette — is the fallback chain's decision; see "Fallback behavior" below.

### PNG texture rendering (implemented — the skin tier)

An `entities` entry with a `skin` (a `.png`; every v1 `.png` path migrates
to one) is genuinely loaded and rendered — as the *middle* tier of the
fallback chain: a kind whose model loads never decodes its skin, and a
kind whose model failed falls back to it:

1. At startup — once, never per frame — the renderer resolves the kind's
   pack entry, decodes the PNG to RGBA8, and uploads it as one sRGB GPU
   texture with its own bind group (a "skin"). Each load prints
   `entity skin '<kind>' loaded (...)`.
2. The entity shader samples the skin with box-mapped UVs: every face of
   the kind's silhouette boxes shows the whole image, upright on the four
   side faces. The texel multiplies the lit team color (so ownership still
   reads), and texels with alpha < 0.5 are cut away (alpha cutout — the
   entity pass writes opaque pixels).
3. Per frame the renderer groups the instance stream by skin and issues
   one instanced draw per texture — no PNG is decoded, uploaded, or given a
   bind group during rendering, and untextured kinds are unaffected.
4. Placeholder draws sample a built-in 1×1 white skin: the multiply is
   exactly 1.0, so a pack-less client renders byte-identically to the
   pure-silhouette renderer.

Limits on PNG files (checked before anything large is read or decoded):

- PNG only, in any color type (normalized to RGBA8 on load).
- At most `2048 × 2048` pixels (the header is checked before decoding — a
  tiny file claiming a huge bitmap is refused, not allocated).
- At most 64 MiB on disk.
- A kind may map to one PNG; sharing one file between kinds is fine (each
  kind gets its own GPU copy).

### The model tier (M12)

An `entities` entry with a `model` (a `.glb`) is genuinely loaded and
rendered as a **static mesh**:

1. At startup — once, never per frame — the loader (`render/model.rs`)
   parses the file under the ART_SPEC subset: one self-contained `.glb`
   (a single embedded BIN buffer, embedded PNG images — any external
   `uri` is refused), `TRIANGLES` primitives with `POSITION` required
   (`NORMAL`/`TEXCOORD_0` optional — flat normals are computed per
   triangle when absent), `OPAQUE` alpha only, and the 32 MiB pre-parse
   cap (`MAX_GLB_BYTES`) checked before a single byte is parsed. Every
   rejection is a precise `ModelLoadError`, never a panic.
2. Animation data is counted and ignored — `animations`, `skins`, and
   morph targets render as nothing (ADR-0101 §2); the load line carries
   one `N ignored animation blocks` note when the file carried any.
3. Node-hierarchy transforms bake into the vertices at load; the
   manifest's `scale`/`rotation_deg`/`ground_offset` bake once at GPU
   upload (meters divided by `METERS_PER_TILE = 3.0`, the one named
   constant in `render/entities/mod.rs`); the fixed glTF-to-entity
   orientation seam `(x, y, z) → (−z, y, x)` with the correct-handed yaw
   is pinned by direction-pinned tests (ADR-0101 §6, assumption A-156).
4. A material named exactly `team_color` (the first wins when duplicated)
   multiplies the owning player's color — the skin path's team-tint
   discipline carried over. Other materials render their embedded PNG or
   plain `baseColorFactor`; embedded PNGs decode under the same guards as
   skin files (the 2048×2048 per-side cap is checked on the header before
   any pixel decodes — `decode_embedded_png` in `render/mod.rs`).
5. Each kind's model draws instanced, per-primitive per-kind, from the
   same fog-filtered snapshot the box silhouettes use (ADR-0200 §4's
   invariant), and prints the positive evidence line the machine passes
   grep: `entity model '<kind>' loaded (N primitives, M triangles, from
   <path>)`.

The look itself — scale against the terrain, rotation feel — is the
owner's visual call (Standing Orders §8); the machine proves the load,
the draws, and the facing math.

### Fallback behavior

The game continues to launch and run when:

- No custom pack exists (placeholder rendering for every kind).
- The manifest is missing or malformed (placeholder rendering + diagnostic).
- An entity kind has no asset mapping (placeholder used).

Within a kind, the presentation is decided once at startup by the fallback
chain **model → skin → placeholder** (ADR-0101 §5;
`AssetPack::resolve_presentation` in `crates/client/src/pack/mod.rs`, pure
and unit-pinned over on-disk fixtures): the model tier when the `.glb`
loads, else the PNG-skinned box when the kind declares a skin and it
decodes, else the placeholder silhouette. The walk is lazy — a kind whose
model wins never decodes its skin — and a bad model never crashes the
client; it lands one tier down.

- A v1 entity path that does not exist skips the whole entry with its own
  direct one-line print (v1's pinned behavior; a kind with one reference
  can never double).
- A v2 reference that cannot be read (model, skin, icon, sound) records a
  deferred note folded into the kind's single line at the renderer's
  startup resolution; a terrain class keeps its direct one-line print.
- A PNG fails to decode, is larger than the limits above, or is rejected
  by the GPU: the kind falls to the next tier down — the placeholder when
  no model stands above it (every other kind's assets still render).
- **One diagnostic per failing kind per run** (the A-146 pin): every
  failure the walk passed folds into a single line naming the tier the
  kind landed on — `pandemonium pack: entity '<kind>' — <failures>;
  rendering the <tier> tier` (`presentation_diagnostic` in
  `pack/mod.rs`) — printed once at startup, never per frame. The positive
  side carries its own evidence lines (`entity model '<kind>' loaded …`,
  `entity skin '<kind>' loaded …`).

A missing or invalid custom pack never makes the simulation unusable.

## Adding visuals (no logic changes)

1. Art lives in the separate asset repository
   (`E-Vex/pandemonium-assets`); the Programmer lands it there through a
   brief, and `scripts/fetch-assets.sh` installs it locally as the default
   dev pack `crates/client/assets/packs/default/` (gitignored — nothing is
   committed to this repo). For private experiments, pass `--pack <dir>`
   to point at any local folder.
2. Declare it in the pack manifest, keyed by the entity's kind id from
   `content/`. Never write asset paths into `content/*.ron` — those files stay
   hashable and shareable between peers.
3. The client resolves kind → asset at startup through the pack manifest
   (`AssetPack::asset_for()`, exposed on the renderer as
   `WgpuRenderer::asset_for()`) and walks each kind through the fallback
   chain: a declared `.glb` uploads as the kind's model, otherwise a
   declared `.png` uploads into the renderer's skin library, otherwise
   the placeholder draws. The sim draws nothing, so improving or
   replacing art never changes match results.

For a real-model trial without touching the official pack,
`scripts/make-pilot-pack.sh` builds a **schema-2 pilot pack outside the
repository** (default `/tmp/pandemonium-pilot-pack`, `PILOT_OUT` to move
it) from the assets repo's three staged D-002 `.glb` models — `rifleman`
and `barracks` with their official skins as the fallback tier, `ore_node`
model-only so the placeholder silhouette is its third tier — plus every
official skin the pack ships (eight; `ore_node` has none) and the four
terrain classes, all read-only: the official pack is never modified and
stays schema 1. A kinds filter builds a one-model variant
(`make-pilot-pack.sh rifleman`). Point the client at it:

```bash
scripts/make-pilot-pack.sh
cargo run -p pandemonium-client -- --pack /tmp/pandemonium-pilot-pack
```

A minimal example — put `rifleman.png` (any PNG up to 2048×2048) into
`crates/client/assets/packs/default/textures/units/`, then:

```ron
(
    pack_id: "default",
    schema_version: 1,
    entities: {
        "rifleman": "textures/units/rifleman.png",
    },
)
```

`cargo run -p pandemonium-client` picks the pack up from the dev path and
prints the load line; riflemen now wear the image on every silhouette box.
Deleting the file (or the manifest) restores placeholder rendering with a
diagnostic — no code change, no content change, no match-identity change.

## Adding a new army or soldier (data-only, zero engine/sim changes)

1. `content/entities/<id>.ron` — stats + capabilities (`docs/CONTENT_GUIDE.md`).
2. `content/factions/<id>.ron` — roster + production lists.
3. Optional: pack-manifest entry for its visuals (gitignored).
4. `cargo run -p pandemonium-tools -- content-validate content`

That matches plan FD-4 and acceptance A3/A10: new content is data, not code.

## Current limitations

The renderer (wgpu) draws each entity kind on whichever tier the fallback
chain gives it: the static mesh when a model loads, the instanced box
silhouettes otherwise (keyed by kind id through `silhouette::named()` with
a capability-shape fallback), wearing the kind's PNG skin when one
uploaded (see "PNG texture rendering" above). Still not implemented:

- **Skeletal animation**: models are static meshes — `animations`,
  `skins`, and morph targets in a `.glb` are counted and ignored (one
  load-time note; ADR-0101 §2). Any motion beyond the entity's yaw and
  placement is a future ADR's business.
- **PBR materials and `.gltf` files**: one albedo per material (an
  embedded PNG or a plain `baseColorFactor`), `OPAQUE` alpha only, and
  the container must be a self-contained `.glb` — a `.gltf` is refused
  at load and the chain walks (see "Supported asset formats").
- **Per-terrain-class textures**: the terrain pass samples **one** PNG
  from the pack — the first terrain-class entry in class-id order (or
  the first entity entry when the manifest declares no terrain), mapped
  over the whole map as a multiply tint (`load_pack_texture` in
  `render/pipeline/mod.rs`, bound at the terrain pipeline's group 1; a
  1×1 white fallback keeps the no-pack look unchanged). Each class
  wearing its own texture where it is authored — per-class sampling
  through `terrain_asset_for()` — is still not implemented, and the
  procedural material branches remain the terrain's visual authority.
- **Icons and sounds**: the manifest's `icon` and `sounds` fields are
  schema-validated (`.png`-only icons; cue names against the engine's
  vocabulary) but consumed by nothing in M12 — they ride the manifest
  for the follow-up briefs that consume them.
- **Mipmapping**: skins (and the whole-map terrain tint) upload a single
  mip level, so very small on-screen entities sample without mip
  filtering.

The asset pack system provides:

- The manifest schemas (v1 with its forward migration, and v2) and the
  loader (`crates/client/src/pack/`).
- Path validation and security (absolute paths, traversal, pack-root escape).
- Asset resolution by kind id (`AssetPack::asset_for()`, exposed on the
  renderer as `WgpuRenderer::asset_for()`).
- The per-kind fallback chain (model → skin → placeholder) under the
  one-diagnostic-per-kind rule (`resolve_presentation` +
  `presentation_diagnostic` in `pack/mod.rs`).
- GLB loading as static meshes (`crates/client/src/render/model.rs`, the
  ART_SPEC subset) with GPU upload through the model pass
  (`render/mod.rs::build_model_pass`).
- PNG decode + GPU upload for entity skins (`crates/client/src/render/texture.rs`).
- Graceful fallback to placeholders when a pack is absent or unusable,
  and a pilot-pack builder for real-model trials
  (`scripts/make-pilot-pack.sh`).

The smallest logical follow-up task is the terrain pass sampling each
terrain class's own already-validated PNG through the same skin plumbing
(`terrain_asset_for()` exists; the single whole-map tint described above
is the current iteration), keeping the procedural terrain as the
fallback.

## CI

`cargo test --workspace`, `content-validate`, and the replay round-trip run
without any art assets — the client draws placeholders. If a PR touches
`crates/sim` or `content/`, pinned golden hashes must not move; that is the
logic check, and visuals are exempt from it.

The release channel is the exception: since A-014 it ships the pack and
gates on it. The `release` workflow checks `E-Vex/pandemonium-assets` out,
packages `default/` at `assets/packs/default/` in every archive, and the
packaged smoke fails the leg unless the tactical_field manifest, all 8
entity textures, and the headless pack-loaded line are all present
(see "The official pack" above).
