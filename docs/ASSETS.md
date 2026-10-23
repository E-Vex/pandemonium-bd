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

The two stray `war-*.wav` files at the repo root belong in
`crates/client/assets/audio/` (gitignored) or should be removed; they must not
be `git add`-ed.

## Adding visuals (no logic changes)

1. Place the asset in a gitignored directory under `crates/client/assets/`
   (or in the separate asset repository).
2. Declare it in the **client-side pack manifest**, keyed by the entity's
   `kind id` from `content/` — for example `"rifleman"` →
   `models/units/rifleman.glb`. Never write asset paths into `content/*.ron`;
   those files stay hashable and shareable between peers.
3. The client renderer resolves kind → asset at draw time. The sim draws
   nothing, so improving or replacing art never changes match results.

## Adding a new army or soldier (data-only, zero engine/sim changes)

1. `content/entities/<id>.ron` — stats + capabilities (`docs/CONTENT_GUIDE.md`).
2. `content/factions/<id>.ron` — roster + production lists.
3. Optional: pack-manifest entry for its visuals (gitignored).
4. `cargo run -p pandemonium-tools -- content-validate content`

That matches plan FD-4 and acceptance A3/A10: new content is data, not code.

## CI

`cargo test --workspace`, `content-validate`, and the replay round-trip run
without any art assets — the client draws placeholders. If a PR touches
`crates/sim` or `content/`, pinned golden hashes must not move; that is the
logic check, and visuals are exempt from it.
