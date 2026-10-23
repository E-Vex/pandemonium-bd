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
| `crates/client/assets/{models,textures,audio,shaders,packs}/` | **No** (gitignored) | Local presentation data only. A fresh clone runs without them — colored boxes and synthesized audio are already in client code. |
| `/user-maps/`, `/user-packs/`, `/mods/` | **No** | User content discovered beside the executable at runtime; no engine change needed (ADR-0200 §5). |

The stray `war-*.wav` files at the repo root are gitignored by the `*.wav`
rule and must not be `git add`-ed.

## Asset pack system (implemented)

The client can load an optional, local **asset pack** at startup. The pack is
presentation-only: it maps entity kind ids (from `content/entities/*.ron`) and
terrain class ids to local asset file paths. It is never hashed, never sent
to peers, and never reaches the simulation.

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
        "rifleman": "models/units/rifleman.glb",
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
- ` terrain` maps a terrain class id to a relative asset path.
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

### Supported asset formats (declared, not yet rendered)

The pack system recognizes these extensions:

| Extension | Kind | Renderer support |
|---|---|---|
| `.glb`, `.gltf` | Model | Not yet — placeholder used |
| `.png` | Texture | Not yet — placeholder used |

Other extensions are not recognized by `AssetPack::kind_of()` and fall back to
placeholders. The renderer currently draws instanced box silhouettes; it
cannot yet consume external model or texture files. See "Current limitations"
below.

### Fallback behavior

The game continues to launch and run when:

- No custom pack exists (placeholder rendering).
- The manifest is missing or malformed (placeholder rendering + diagnostic).
- A manifest references an asset that does not exist (that entry skipped,
  placeholder used + diagnostic).
- An entity kind has no asset mapping (placeholder used).
- A referenced asset format is not supported (that entry skipped,
  placeholder used + diagnostic).

A missing or invalid custom pack never makes the simulation unusable.

## Adding visuals (no logic changes)

1. Place the asset in a gitignored directory under
   `crates/client/assets/` (or in the separate asset repository).
2. Declare it in the pack manifest, keyed by the entity's kind id from
   `content/`. Never write asset paths into `content/*.ron` — those files stay
   hashable and shareable between peers.
3. The client resolves kind → asset at draw time through `WgpuRenderer::asset_for()`.
   The sim draws nothing, so improving or replacing art never changes match results.

## Adding a new army or soldier (data-only, zero engine/sim changes)

1. `content/entities/<id>.ron` — stats + capabilities (`docs/CONTENT_GUIDE.md`).
2. `content/factions/<id>.ron` — roster + production lists.
3. Optional: pack-manifest entry for its visuals (gitignored).
4. `cargo run -p pandemonium-tools -- content-validate content`

That matches plan FD-4 and acceptance A3/A10: new content is data, not code.

## Current limitations

The renderer (wgpu) currently renders entities as instanced box silhouettes
(keyed by kind id through `silhouette::named()` with a capability-shape
fallback). It does **not** yet load external models, textures, or audio files
from disk. The asset pack system provides:

- The manifest schema and loader (`crates/client/src/pack/`).
- Path validation and security (absolute paths, traversal, pack-root escape).
- Asset resolution by kind id (`AssetPack::asset_for()`, exposed on the
  renderer as `WgpuRenderer::asset_for()`).
- Graceful fallback to placeholders when a pack is absent or unusable.

The smallest logical follow-up task is to teach the renderer to load and
sample a supported texture (e.g. `.png`) for a kind, using the resolved path
from `asset_for()`, while keeping the silhouette path as the fallback.

## CI

`cargo test --workspace`, `content-validate`, and the replay round-trip run
without any art assets — the client draws placeholders. If a PR touches
`crates/sim` or `content/`, pinned golden hashes must not move; that is the
logic check, and visuals are exempt from it.
