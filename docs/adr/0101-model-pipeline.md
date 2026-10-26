# ADR-0101: The model pipeline — glTF static meshes, team-color tinting, and pack manifest schema 2

- **Date:** 2026-10-25
- **Status:** accepted (owner sign-off, 2026-10-25 — brief A-017)
- **Frozen decision affected:** plan §3.2 / plan §4 dependency law — the `gltf`
  crate joins `pandemonium-client`'s external allow-list, following the
  ADR-0001 (`glam`) and ADR-0002 (`rodio`) precedent: client-only,
  presentation-only, never in the sim's tree. **FD-4 / FD-10** as extended by
  [ADR-0200](0200-authoritative-vs-presentation-data.md) §7 — the
  presentation-pack manifest contract gains `schema_version: 2` with v1
  loading unchanged; ADR-0200 §7 explicitly deferred "the internals of
  presentation packs — directory layout, asset naming and override rules,
  versioning of pack assets" to "a later Lane A ADR", and **this ADR is that
  record, scoped to the model slice**. No FD-1..FD-10 decision is weakened:
  the sim's tree, the content hash inputs, and the determinism rules are
  untouched (the diff of this ADR is documentation only).
- **Proposed by:** Programmer (brief A-016, PM-relayed; recording the PM's six
  pre-made decisions with their technical consequences, completing one gap in
  them, and challenging none as unsound — the challenge log is in §2 of the
  Proposal)

## Context

The client renders entities as multi-part instanced boxes
(`crates/client/src/silhouette.rs`), and since A-014 those boxes can wear a
PNG skin loaded from the official `tactical_field` pack
([`docs/ASSETS.md`](../ASSETS.md) "PNG texture rendering"). The pack system
already recognizes `.glb`/`.gltf` model entries
(`crates/client/src/pack/mod.rs:42-50` — the constant's own comment calls it
"forward-looking: the renderer cannot yet load external models"), validates
their paths, and then renders the placeholder silhouette anyway;
`docs/ASSETS.md` "Current limitations" states this plainly and adds "No GLB
loading is coming in the current milestone sequence" — a sentence this
milestone proposal retires.

Three things make the gap load-bearing now rather than later:

1. **The art direction assumes geometry.** The Designer's art bible
   (D-001, `pandemonium-assets` `design/art-bible/art_bible.md`) is written
   for readable stylized 3D — silhouette-first mass language per role (§5),
   a scale/footprint ladder in meters (§6, "1 tile = 3 m"), one baked key
   light (§4). Its §9 explicitly leaves "the technical asset spec (model
   format, poly budget, axes, export scale)" to **D-002** "once the model
   pipeline exists": the pipeline and its spec are one deliverable pair, and
   this ADR plus [`docs/ART_SPEC.md`](../ART_SPEC.md) are them.
2. **The boundary the pipeline must live inside is already law.** ADR-0200
   §3's cosmetics rule list forbids presentation data from affecting stats,
   footprints, hitboxes, selection areas, or pathing — "selection sizes must
   always come from rules data, never from model bounds: when real models
   arrive, this is the rule that keeps a fancy mesh from silently widening
   what a player can click". Real models are arriving; the rule they must
   obey is on the books. §7 left the pack internals to this ADR.
3. **The fallback discipline the pipeline must not regress already exists and
   is tested.** A missing pack, a malformed manifest, a missing or
   unsupported-format asset each degrade per-entry to placeholders with one
   diagnostic and the game keeps running (`pack/mod.rs:142-175`,
   `docs/ASSETS.md` "Fallback behavior"); the release smoke hard-requires the
   official pack to load (A-014). A model pipeline that could crash the
   client — or spam a diagnostic per frame — would be a regression against
   both.

The renderer's world units are **tiles** (terrain vertices at "tile corners
at integer x/z", `crates/engine/src/mesh.rs:16-19`; silhouette parts sized in
tile units, `silhouette.rs:52-68`), while the art bible authors in **meters**
(§6: 1 tile = 3 m). The pipeline therefore needs one fixed, documented
meters→tiles conversion, and the Designer needs to know which side of it they
author on.

## Proposal

The PM's six decisions are recorded verbatim in **bold**, each with its
technical consequence and the challenge result. Numbering continues through
the two additions this ADR makes (the field the list omitted, and the
dependency change it implies).

### 1. Format: glTF 2.0 binary (`.glb`)

**Decision (PM):** the model format is glTF 2.0 binary (`.glb`).

Consequence: glTF 2.0 is the Khronos runtime-transmission format whose spec
already matches this project's needs — meters as the unit, +Y up, −Z forward,
right-handed; embedded buffers and textures (one self-contained file per pack
entry, which the pack's single-path-per-entry, path-validated discipline
requires, `pack/mod.rs:190-226`); first-class export from every DCC the
Designer might use (Blender, Maya, 3ds Max, Substance). **Model entries in
the manifest accept `.glb` only; a `.gltf` model entry** (JSON + external
buffers — multiple files, or base64 data URIs) **is skipped with one
diagnostic and the fallback chain** — the `gltf` extension stays recognized
by the pack system but is not consumed as a model. Challenge result: sound;
the one-file rule is what keeps pack entries copy-safe and
canonicalization-checked.

### 2. Static meshes only in M12 — no skeletal animation

**Decision (PM):** M12 loads static meshes only; no skeletal animation.

Consequence: the M12 loader reads nodes, meshes, primitives, materials,
textures — and **ignores** `animations`, `skins`, and morph targets
carried in a file (a DCC that leaves a rig in the export still loads as
static geometry). Ignoring is deliberate, and logged: exactly one load-time
note per file that carries ignorable animation data ("animations present;
M12 pipeline is static — ignored"), so a Designer who expected a walk cycle
learns why they did not get one. Skeletal animation, if ever wanted, is its
own future ADR (it drags in interpolation policy, event sync, and a
determinism-adjacent feel pass — none of which M12 buys). Challenge result:
sound — static meshes cover the art bible's entire current scope (its §9
delivery list is still PNGs and concepts; no rig has been authored).

### 3. Team color: a material named `team_color`, tinted per player

**Decision (PM):** a material named `team_color`; the renderer tints it per
player.

Consequence: the tint multiplies the material's base color by the owning
player's team color — exactly the discipline the PNG skin path already uses
("the texel multiplies the lit team color (so ownership still reads)",
`docs/ASSETS.md` "PNG texture rendering"), generalized from texels to a
material. The rules, made checkable in
[`docs/ART_SPEC.md`](../ART_SPEC.md): the name is matched exactly and
case-sensitively (`team_color`; reading it needs the gltf crate's `names`
feature — see §7); at most one such material per model; player-owned kinds
(units and buildings) **must** carry one, the neutral `ore_node` **must
not**; its base color is near-white so the multiply is predictable; its
coverage follows the art bible §2 zone table (one flat zone on the upper
mass). A model that omits it still renders — untinted, with one diagnostic —
because a bad-or-incomplete model degrades, never crashes (§5). Ownership
additionally stays readable through the existing ground rings (units) and
team bands (structures), which do not depend on the model. Challenge result:
sound — a *named material* survives DCC re-export and is visible in any
glTF inspector, where vertex-color or texture-channel conventions do not
round-trip reliably.

### 4. Pack manifest `schema_version: 2`; v1 packs keep loading unchanged

**Decision (PM):** the pack manifest moves to `schema_version` 2; v1 packs
keep loading unchanged; a v2 entity entry carries `model`, `scale`,
`rotation_deg`, `ground_offset`, `icon`, and optional `sounds`.

Consequence — the v2 shape (RON, `deny_unknown_fields` stays on,
`pack/mod.rs:255-265`):

```ron
(
    pack_id: "tactical_field",
    schema_version: 2,
    entities: {
        "rifleman": (
            model: Some("models/units/rifleman.glb"),   // optional
            skin:  Some("textures/entities/rifleman.png"), // optional
            scale: 1.0,            // optional, default 1.0, 0.05..=10.0
            rotation_deg: 0.0,     // optional, default 0.0, -360.0..=360.0
            ground_offset: 0.0,    // optional, meters, default 0.0
            icon: Some("icons/rifleman.png"),            // optional
            sounds: { "SelectionClick": "audio/rifleman/select.ogg" }, // optional
        ),
    },
    terrain: { "grass": "textures/terrain/grass.png" },  // unchanged from v1
)
```

(RON/vocabulary note, A-153: map entries take the plain
`"key": value` form — ron 0.9 rejects the parenthesized `("key": value)`
form this example originally showed — and sound cue names come from the
engine's nine-cue vocabulary (`SelectionClick`, not `select`). The example
above parses verbatim against the M12 loader.)

- **One completion to the PM's field list (the challenge log):** the list
  names no field for the PNG skin, yet the PM's own fallback chain (§5) makes
  the "PNG-skinned box" its middle tier — a v2 pack must be able to address
  the skin **and** the model for the same kind so the chain has both tiers to
  walk. v2 therefore carries `skin` (the v1 string's new name) alongside
  `model`. Both are optional; a kind with neither keeps the placeholder.
- **v1 forward migration (FD-10's rule, the map-schema precedent of
  ADR-0001):** a v1 `entities: { "rifleman": "path" }` entry migrates forward
  one typed step at load — a `.png` becomes `skin`, a `.glb`/`.gltf` becomes
  `model`, every other field takes its default. The official
  `tactical_field` pack (v1, PNG-only, `pandemonium-assets/default/manifest.ron`)
  therefore loads with **byte-identical rendering behavior**; the only v1
  entries whose behavior changes are hypothetical `.glb` pointers, which
  render placeholders today and begin rendering when the M12 loader lands —
  that change is the milestone's purpose, not a break of the loading rules.
- `schema_version` greater than 2 remains the precise
  `UnsupportedVersion` error it is today (`pack/mod.rs:100-101`,
  `:131-136`); unknown fields remain rejected (strict mode,
  `pack/mod.rs:257`).
- `icon` and `sounds` are **schema-validated but not consumed in M12** —
  paths validated inside the pack root, icon `.png`-only, sound cue names
  checked against the engine's cue vocabulary (`crates/engine/src/audio.rs`),
  playback left to a future brief. This is the exact pattern the terrain
  PNGs rode in on (A-014 shipped them "validated and kept for the
  terrain-texture follow-up", which #50 then landed).
- `scale`, `rotation_deg`, `ground_offset` are floats in the **client's
  presentation tree only** — plan §5's float ban is scoped to the
  determinism crates (`tests/architecture_law.rs:398-401` scans fx, sim,
  sim_api, ai, content; engine and client are exempt), and the pack never
  crosses the seam. Semantics: `scale` multiplies the model uniformly in
  meters; `rotation_deg` yaws the model about its origin (degrees, applied
  before `scale`/`ground_offset`); `ground_offset` lifts (positive) or sinks
  (negative) it in meters. They are per-pack art fixes, range-validated at
  load, never gameplay inputs.

Challenge result: sound, with the `skin` completion above. Nothing here
touches the content hash: pack manifests are presentation data, "local,
never hashed, never exchanged" (ADR-0200 §3).

### 5. Fallback chain per kind: model → PNG-skinned box → placeholder

**Decision (PM):** per kind, the fallback chain is model → PNG-skinned box →
placeholder. A bad model never crashes the client; it logs one diagnostic
and falls back.

Consequence — the chain resolves **once, at startup load**, never per frame:
the loader builds a per-kind presentation plan (model data uploaded, or skin
texture, or placeholder flag) exactly as the skin path does today ("once,
never per frame", `docs/ASSETS.md` "PNG texture rendering" step 1); the
frame path only reads the plan. The failure surface is therefore entirely at
load: a file too large (pre-parse size check, the PNG 2048×2048
header-first precedent, `docs/ASSETS.md` "Limits on PNG files"), unparseable
JSON, truncated BIN chunk, primitives outside the accepted subset (§2 of
ART_SPEC), non-PNG-embedded textures — each returns an error, never a panic,
and the kind walks one tier down. **"One diagnostic" is pinned as: exactly
one log line per failing kind per run** (deduplicated; a kind that fails
model *and* skin logs once, naming the tier it landed on), in the existing
`pandemonium pack:` prefix style. The client's crash-freedom under any byte
sequence is a tested property, not a hope (M12 Phase 1's fuzz loop feeds
random slices through the parser; Phase 4 greps a full `--frames` run for
exactly one occurrence). Challenge result: sound — it is the existing
per-entry fallback discipline, extended one tier and hardened with a
dedup guarantee.

### 6. Units follow the art bible: 1 tile = 3 m; footprints per art bible §6

**Decision (PM):** units follow the art bible — 1 tile = 3 m, footprints from
art bible §6.

Consequence — split into the two different things the sentence touches:

- **Scale (art-side, presentation):** the fixed conversion
  `METERS_PER_TILE = 3.0` lives in the client as a named constant; models
  author 1 glTF unit = 1 m (the glTF spec's own unit) and the pipeline
  divides by 3 at load. The Designer never author-converts; ART_SPEC's
  per-kind table states every budget in both meters and tiles.
- **Footprints (gameplay-side, authoritative):** ADR-0200 §3 is explicit —
  footprints come from the `Footprint` capability in content data, radii
  from `Move`, selection sizes "from rules data, never from model bounds".
  Art bible §6 is the Designer's mirror of those numbers, not their source.
  Cross-checked today, the mirror has two smudges: `rifleman` (bible 0.6
  tile wide; content `radius_milli_tiles: 350` = 0.7 tile diameter,
  `content/entities/rifleman.ron:8`) and `raider` (bible 0.8; content
  `radius_milli_tiles: 300` = 0.6, `content/entities/raider.ron:8`). All
  five building footprints and worker/guardian widths match exactly.
  **Content wins** — it is hashed, tested, and balanced — so
  [`docs/ART_SPEC.md`](../ART_SPEC.md) encodes the content-derived widths,
  and the art bible's two rows are flagged for an Owner-side correction
  (report A-016 §9 P1; `design/` is the Owner's direct-commit zone per
  Standing Orders v1.4 §3).

Challenge result: sound once the split is written down; as one sentence it
conflated a presentation constant with a gameplay authority, and §6 of the
bible is the wrong source to cite for collision truth. ART_SPEC cites both,
correctly labeled.

Implementation note (M12 phase 3, A-018; recorded per that report's §9 P1
— no decision above changes): "−Z → forward" is pinned in code as
`bake_gltf_to_entity` in `render/entities/mod.rs` — the fixed basis bake
`(x, y, z) → (−z, y, x)`, a proper rotation (determinant +1, never a
mirror) that puts the model's glTF forward (−Z) on the yaw-0 facing (+X,
east) and its right (+X) on +Z, exactly where a box silhouette carries
right-hand parts. The per-instance yaw then uses the *correct-handed*
form (`c·x − s·z, s·x + c·z` — the transposed convention
`build_entity_instances` maps part offsets with), deliberately not the box
shader's mirrored form, because authored geometry must turn toward its
facing. The motivating finding (A-018 report §8): the renderer's world —
x-east, y-up, z-north, facing at `yaw = atan2(facing.z, facing.x)` —
reads rotations about +Y as the mirror of the glTF authoring convention,
which is why the box shader's form reads "mirrored" against `atan2` and
why the model yaw needed the transposed form. Both ends are
direction-pinned at yaw 0 (east) and yaw 90° (north), with the bake's
handedness and the right/up convention each carrying their own test —
assumption A-156 records the full math.

### 7. Dependency: the `gltf` crate joins the client allow-list (the exact change)

The pipeline's parser is the `gltf` crate ([gltf-rs/gltf](https://github.com/gltf-rs/gltf)),
the canonical Rust glTF 2.0 loader, checked 2026-10-25 on crates.io/docs.rs:

- **Version 1.4.1**, mature and slow-moving; MSRV 1.61 — well under the
  pinned 1.98.1 (`rust-toolchain.toml`).
- **Licence: MIT OR Apache-2.0** — both on `docs/product/deny.toml`'s
  allow-list (`deny.toml:29-40`), so the release pipeline's licence gate
  passes with **no exception** (unlike CC0-for-hexf-parse, which needed the
  PM's A-003 ruling).
- **Features:** default = `import`, `names`, `utils`. The client takes
  `default-features = false, features = ["names"]` — `names` is required to
  read material names (§3's `team_color` lookup); `import` (file-system
  loading that additionally pulls `base64`, `image`, and `urlencoding`) is
  **avoided**: the client already owns file reading and PNG decoding, and
  `Glb`/`Gltf` byte-level parsing is core. `utils` is unneeded. Transitive
  deps with this pin: `gltf-json` (MIT OR Apache-2.0), `serde`, `serde_json`
  — all licence-clean, none forbidden.
- Not on the forbidden list (`tests/architecture_law.rs:32-50` bans engines,
  ECS, physics, pathfinding, and OS randomness — a file-format parser is
  none of these), and not a determinism risk: it runs in the client, above
  the seam, on presentation data.

**The exact change an implementation brief makes** (Phase 0 of the M12 plan,
landed only after this ADR is accepted — the law is never edited silently,
STANDING_ORDERS §9):

1. `crates/client/Cargo.toml`:
   ```toml
   # glTF 2.0 binary parsing for the pack's static entity models
   # (ADR-0101): parse-only — no fs import (that would pull base64,
   # image, urlencoding), `names` for the team_color material lookup.
   gltf = { version = "1.4", default-features = false, features = ["names"] }
   ```
2. `tests/architecture_law.rs`, `allowed_external()`'s client entry
   (following the rodio/ADR-0002 comment precedent at
   `architecture_law.rs:135-137`), one line inside the existing array:
   ```rust
       // gltf joined the client's allow-list with ADR-0101 (M12 model
       // pipeline): glTF 2.0 binary parsing for the presentation pack's
       // static entity models — client-only, parse-only, never in the
       // sim's tree.
       "gltf",
   ```

Nothing else in the law moves: the forbidden list, the internal edges, and
the determinism scans are untouched; the change *strengthens* the law's
paper trail (a new external dep without its ADR line would fail
`dependency_law_holds`).

### 8. The M12 milestone (the implementation owner)

This ADR proposes milestone **M12 — the model pipeline**, continuing the
post-M10 handoff status-board convention (M10.2, M11, Mission Phase 1
preceded it; `plan.md` §14 itself still ends at M10). The full phased
implementation plan with per-phase acceptance tests is deliverable 4 of
brief A-016 and lives in
[`docs/pm/reports/A-016.md`](../pm/reports/A-016.md) §12; its shape: Phase 0
the dependency change above → Phase 1 the pure-data `.glb` loader with the
fuzz-proved never-panics property → Phase 2 manifest schema 2 with the v1
golden test → Phase 3 the renderer pass (fog-filtered, instanced,
direction-pinned yaw tests) → Phase 4 the fallback chain and one-diagnostic
guarantee → Phase 5 the docs truth pass (`docs/ASSETS.md`'s model rows, the
sentence this ADR retires) and the first real model through the Standing
Orders delivery path. Icon and sound consumption are deliberately **not**
M12 scope (§4).

## Evidence

- **The gap is pinned in the tree, not asserted:** `pack/mod.rs:42-50`
  (MODEL_EXTENSIONS "forward-looking" comment), `docs/ASSETS.md` "Supported
  asset formats" (`.glb`/`.gltf` → "Not yet — placeholder used") and
  "Current limitations" ("no mesh loading, skeletal animation, or PBR
  material system exists"), ADR-0200 §7's "the internals of presentation
  packs … belong to a later Lane A ADR".
- **Every discipline this pipeline reuses is already implemented and
  tested:** per-entry skip-with-diagnostic fallback (`pack/mod.rs:142-175`,
  the `missing_asset_file_is_non_fatal_with_diagnostic` test at
  `pack/mod.rs:412-429`), path security (`pack/mod.rs:190-226`, six
  dedicated tests), strict manifest parsing (`pack/mod.rs:257`,
  `malformed_manifest_is_rejected`), unsupported-version precision
  (`pack/mod.rs:340-355`), skin team-color multiply and per-texture
  instanced draws (`docs/ASSETS.md` "PNG texture rendering"), the
  fog-filtered render invariant (ADR-0200 §4, with its `file:line` map).
- **The unit mismatch is real and currently unpinned:** world units are
  tiles (`mesh.rs:16-19`, `silhouette.rs:52-68`); the art bible authors in
  meters (§6). One named constant fixes it; ART_SPEC publishes the
  conversion so the Designer never guesses.
- **The art-bible §6 ↔ content cross-check** (performed 2026-10-25 against
  `content/entities/*.ron`): footprints match 5/5 (command_center 4×4,
  barracks 3×3, supply_depot 2×2, turret 2×2, ore_node 2×2); widths match
  for worker (300 → 0.6 tile) and guardian (500 → 1.0); the two mismatches
  are rifleman (350 → 0.7 vs bible 0.6) and raider (300 → 0.6 vs bible
  0.8) — recorded in §6 above and flagged in report A-016 §9 P1.
- **Crate facts** (crates.io + docs.rs, checked 2026-10-25): `gltf` 1.4.1,
  MIT OR Apache-2.0, MSRV 1.61, features and their dependency edges as
  claimed in §7; the deny gate's allow-list covers both licences
  (`docs/product/deny.toml:29-40`). A maintained fork (`myth-gltf` 1.4.2,
  Meshopt-focused) exists as a live escape hatch if the canonical crate
  ever stalls — noted in Alternatives, not chosen.
- **The gate this ADR rides on:** master `2267e4a` (the A-013 remainder
  merge), whose last recorded gate is fmt PASS / clippy PASS / **851 dev,
  846 release, 0 failed** (`docs/pm/reports/A-013.md` §4). This ADR's diff
  is Markdown only; no Rust, content, or workflow file changes, so the
  gate state is master's by construction and no golden can move (the
  content hash covers `content/` only; the pack never enters it —
  ADR-0200 §2/§3).
- **What is honestly not yet proven:** any runtime behavior of the pipeline
  itself — this ADR is a *proposal* written before implementation, the same
  specification-first posture ADR-0200 took ("specification only; not added
  by this ADR"). Its runtime claims (never-panics, one-diagnostic, v1
  byte-identical) become test-pinned facts in M12 Phases 1-4, each with the
  named acceptance test in report A-016 §12.
- **The brief's STOP condition was checked:** no PM decision conflicts with
  ADR-0200 (this ADR *is* the §7-promised Lane A record; §3's cosmetics
  list is encoded into ART_SPEC's rules and the manifest's fields are
  presentation-only) or with the sim's determinism rules (zero sim-tree
  files touched; floats stay in the client above the seam; the
  `determinism_source_bans` scan set is unchanged). The two findings that
  surfaced (art-bible width rows) are data-doc smudges outside this repo's
  law, reported rather than stopped on.

## Consequences

- **Cheaper / possible if accepted:** real geometry under the existing
  instancing and fog-filter disciplines; the Designer gets a checkable
  technical spec (ART_SPEC fills the art bible's D-002 slot) and a delivery
  path that never touches code or content (FD-4's "add-a-unit is data-only"
  promise, extended to visuals: a model swap is a pack edit — no code
  change, no content change, no match-identity change); the pack manifest
  grows headroom (icons, sounds) that later briefs consume without another
  schema bump.
- **More expensive:** one external crate joins the client's allow-list
  (parse-only — no image, base64, or fs machinery); a model loader, a
  model shader path, and the v2 dual-version manifest loader are new client
  surface with their own tests; `docs/ASSETS.md` and ART_SPEC must stay
  synchronized with behavior (the Phase 5 truth pass); model review gains a
  checklist step (the ART_SPEC delivery checklist) in every art brief.
- **What breaks if accepted: nothing, now.** The diff is this ADR,
  ART_SPEC, the A-016 report, and the ASSUMPTIONS register rows (A-144…
  A-150). No test, golden, replay, or workflow changes; v1 packs (including
  the official `tactical_field` pack the release smoke gates on) load under
  identical rules; the pack-loaded smoke line is unchanged.
- **Deferred by this ADR (the hooks that keep each cheap):** icon/sound
  *consumption* (schema-only in M12 — the terrain-texture precedent);
  skeletal animation and morph targets (§2 — a future ADR if ever);
  `.gltf`+external-bin form (rejected by policy permanently); PBR shading
  (the entity pass's flat 3-value light stays; a BRDF can layer in later
  behind the same mesh data); pack-level overrides and the ADR-0200 §7
  common-manifest fields (`id`/`version`/`kind`/`licence`/…) — still
  deferred until the mod/pack era needs them.
- **Risks carried:** the `team_color` string coupling (mitigated: ART_SPEC
  checklist + the missing-material diagnostic + the rings/bands that
  already carry ownership); diagnostic spam (mitigated: the §5
  one-line-per-kind-per-run pin, test-grepped); licence-gate drift on a
  future `gltf` bump (mitigated: the deny gate runs per release; any new
  out-of-list licence fails the pipeline loudly, by the gate's own design).

## Alternatives considered

- **Do nothing (stay PNG-skinned boxes).** Rejected: the art direction
  (D-001) is written for geometry — silhouette language, a meters scale
  ladder, baked-light concept renders — and its §9 explicitly schedules
  this spec for the pipeline's arrival. The box ceiling was acceptable for
  the Alpha's legibility bar (ADR-0001's own justification), not for the
  product phase the project is in (STANDING_ORDERS §2's PRODUCT PHASE
  amendment).
- **`.gltf` (JSON + external `.bin`/textures) instead of `.glb`.** Rejected:
  multi-file assets break the pack's single-path-per-entry,
  canonicalize-and-contain discipline (`pack/mod.rs:190-226`), and inline
  base64 buffers would pull the `base64` crate through `import` and inflate
  files. `.glb` is the same format in the pack-friendly single-file form.
- **OBJ / FBX / USD.** Rejected: OBJ has no units, materials, or texture
  embedding discipline; FBX is proprietary with a chaotic Rust story; USD
  is a scene-description system, not a runtime asset format, and its Rust
  bindings are heavyweight. None matches a spec-first, DCC-neutral,
  single-file requirement the way glTF does.
- **Hand-rolled GLB parser (JSON chunk via `serde_json`, accessors by
  hand).** Rejected for M12: GLB looks small (a JSON chunk + a BIN chunk)
  but the spec's edge cases (sparse accessors, normalized attributes,
  image/bufferView stitching, malformed-input hardening) are exactly what a
  mature parser's test suite has already eaten; owning that surface would
  reproduce the `gltf` crate minus its tests. Revisit-able if the crate
  ever goes unmaintained — the `myth-gltf` fork's existence makes that
  escape hatch real rather than theoretical.
- **`myth-gltf` (the maintained fork) instead of canonical `gltf`.**
  Rejected: the fork's value-add is Meshopt compression and modern-engine
  support, none of which M12 needs; the canonical crate's maturity and
  ecosystem familiarity win. The fork stays the named fallback.
- **Full PBR (metallic-roughness BRDF) in the M12 shader.** Rejected: the
  art bible demands matte, 3-value, readability-first surfaces — flat
  albedo under the existing key light is the honest implementation of that
  direction and a small extension of the current entity pass. A BRDF can be
  added later behind the same loaded mesh data without touching the
  pipeline.
- **Team color via vertex colors or a reserved texture channel.** Rejected
  (§3): neither survives DCC round-trips as reliably as a named material,
  and neither is as cheap to inspect at review time.
- **No manifest `scale`/`rotation_deg`/`ground_offset` (bake everything at
  export).** Rejected: a 5% size error or a mis-oriented forward axis would
  then require a re-export round-trip through the Designer; the manifest
  fields are per-pack presentation fixes — cheap, range-validated, and
  visible in one place. (Their presence does not excuse bad exports:
  ART_SPEC's checklist still pins origin, orientation, and scale.)
- **Schema bump to a free-form "v3-later" manifest now (fold in ADR-0200
  §7's common fields: `id`, `version`, `kind`, `licence`, …).** Rejected:
  the §7 fields exist to govern *distributable* content identity (mods,
  packs-as-products) and nothing consumes them yet — adding them now is the
  abstraction-without-a-need ADR-0200 §8 itself refuses. v2 carries exactly
  what M12 consumes or pins.

## Decision

**Accepted by the owner, 2026-10-25** — the model pipeline as specified
is law: the `gltf` crate joins `pandemonium-client`'s external allow-list
(parse-only, `names` feature — §7), the pack manifest contract gains
`schema_version: 2` with v1 packs loading unchanged through the forward
migration (§4), and the fallback chain with its one-diagnostic guarantee
(§5) and the `METERS_PER_TILE = 3.0` seam (§6) bind every M12 phase. The
first implementation brief (A-017, M12 phases 0-2) landed only after this
verdict, exactly as §7's ordering requires: the allow-list line in
`tests/architecture_law.rs` follows the acceptance, never precedes it.
§1's current-state description is historical — it describes the tree
*before* M12 (the placeholder-era `pack/mod.rs`, the
"forward-looking" model extensions), not the tree at acceptance.
