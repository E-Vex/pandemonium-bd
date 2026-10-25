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

```ron
(
    pack_id: "default",
    schema_version: 1,
    entities: {
        "rifleman": "textures/units/rifleman.png",
        "command_center": "models/buildings/command_center.glb",
    },
    terrain: {
        "grass": "textures/terrain/grass.png",
        "rock":  "textures/terrain/rock.png",
    },
)
```

- `pack_id` (string) and `schema_version` (must be `1`) are required.
- `entities` maps an entity kind id (e.g. `"rifleman"`) to a relative asset
  path. The kind id must match the `id` field in `content/entities/<id>.ron`.
  A `.png` entry is loaded and rendered as the entity's skin (see "PNG
  texture rendering" below); a `.glb`/`.gltf` entry is accepted by the
  manifest but still renders as the placeholder silhouette.
- `terrain` maps a terrain class id to a relative asset path. Terrain
  textures are not yet consumed by the renderer — the entries are
  validated and kept for the terrain-texture follow-up.
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

Non-fatal issues (a referenced asset file missing, or an unsupported file
format) do not abort the pack: that entry is skipped with a diagnostic, and
the existing placeholder rendering is used for that kind.

### Supported asset formats

The pack system recognizes these extensions:

| Extension | Kind | Renderer support |
|---|---|---|
| `.png` | Texture | **Loaded and rendered** — entity kinds get their PNG as the box silhouette's skin (see "PNG texture rendering"). Terrain-class PNGs are not yet sampled. |
| `.glb`, `.gltf` | Model | Not yet — placeholder used |

Other extensions are not recognized by `AssetPack::kind_of()` and fall back to
placeholders. The renderer draws instanced box silhouettes; model files
cannot be consumed yet. See "Current limitations" below.

### PNG texture rendering (implemented)

An `entities` entry pointing at a `.png` is genuinely loaded and rendered:

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

### Fallback behavior

The game continues to launch and run when:

- No custom pack exists (placeholder rendering).
- The manifest is missing or malformed (placeholder rendering + diagnostic).
- A manifest references an asset that does not exist (that entry skipped,
  placeholder used + diagnostic).
- An entity kind has no asset mapping (placeholder used).
- A referenced asset format is not supported (that entry skipped,
  placeholder used + diagnostic).
- A `.glb`/`.gltf` is declared where a texture is needed (placeholder used).
- A PNG fails to decode, is larger than the limits above, or is rejected by
  the GPU (placeholder used for that kind + diagnostic; every other kind's
  textures still render).

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
   `WgpuRenderer::asset_for()`) and loads loadable PNGs into the renderer's
   skin library. The sim draws nothing, so improving or replacing art never
   changes match results.

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

The renderer (wgpu) renders entities as instanced box silhouettes
(keyed by kind id through `silhouette::named()` with a capability-shape
fallback). Entity **PNG textures are loaded and rendered** (see "PNG texture
rendering" above). Still not implemented:

- **Models**: `.glb`/`.gltf` entries are validated by the pack system but no
  mesh loading, skeletal animation, or PBR material system exists — those
  kinds keep the placeholder silhouette. No GLB loading is coming in the
  current milestone sequence.
- **Terrain textures**: terrain-class PNG entries are validated but the
  terrain pass does not sample them yet.
- **Mipmapping**: skins upload a single mip level, so very small on-screen
  entities sample without mip filtering.

The asset pack system provides:

- The manifest schema and loader (`crates/client/src/pack/`).
- Path validation and security (absolute paths, traversal, pack-root escape).
- Asset resolution by kind id (`AssetPack::asset_for()`, exposed on the
  renderer as `WgpuRenderer::asset_for()`).
- PNG decode + GPU upload for entity skins (`crates/client/src/render/texture.rs`),
  with per-kind fallback to placeholders.
- Graceful fallback to placeholders when a pack is absent or unusable.

The smallest logical follow-up task is the terrain pass sampling its
already-validated terrain-class PNGs through the same skin plumbing
(`terrain_asset_for()` exists; the terrain shader needs its own bind group),
keeping the procedural terrain as the fallback.

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
