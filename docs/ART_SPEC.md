# ART_SPEC — Pandemonium Model Technical Specification (D-002)

> **What this is.** The Designer's technical spec for 3D models: axes, scale,
> origin, poly budgets, texture limits, naming, material rules, and file-size
> caps. Every rule below is **checkable** — a file either passes or fails
> each line, and the delivery checklist (§11) is the pre-delivery self-test.
> This document fills the D-002 slot the art bible reserved ("the technical
> asset spec (model format, poly budget, axes, export scale) arrives in
> D-002 once the model pipeline exists" — art bible §9) and is the companion
> of [ADR-0101](adr/0101-model-pipeline.md) (status: proposed).
>
> **What this is not.** Gameplay numbers do not live here and never will:
> footprints, collision radii, selection sizes, vision, and stats come from
> `content/` data only (ADR-0200 §3's cosmetics rule list — "selection sizes
> must always come from rules data, never from model bounds"). A model can
> never change how the game plays; it changes only what the player sees.
>
> **Style authority.** Direction, palette, lighting language, silhouette
> rules, and faction languages are the art bible's (D-001,
> `pandemonium-assets/design/art-bible/art_bible.md`), cross-referenced
> below as **AB §n**. This spec never overrides the bible on style — it adds
> the technical layer the bible deliberately left out.

---

## 1. File format

| Rule | Value |
|---|---|
| Format | glTF 2.0 **binary** — `.glb` only |
| Files per asset | One self-contained `.glb` per entity kind (buffers and textures embedded) |
| `.gltf` (JSON + external files) | **Not accepted.** The entry is skipped with one diagnostic; the fallback renders |
| Scene | One scene; every node in it is drawn |
| Compression extensions (Draco, Meshopt, KTX2) | **Not accepted in M12.** Uncompressed geometry and PNG-embedded textures only |
| Animations / skins / morph targets | **Ignored** (M12 is static — ADR-0101 §2). A file that carries them loads as static geometry and logs one note. Do not rig for M12 |

Blender export preset that satisfies this: *File → Export → glTF 2.0*, format
**glTF Binary (.glb)**, *Apply Modifiers* on, *Y up* (default), *Unit:
Meters* (default), compression off, animations off.

## 2. Units, axes, orientation

| Rule | Value |
|---|---|
| Unit | **1 glTF unit = 1 meter** (the glTF spec's unit — never author in tile units) |
| Up axis | **+Y** (glTF standard — do not re-export with axis conversion) |
| Forward at rest | **−Z** (glTF standard: the model's nose points at −Z) |
| Handedness | Right-handed (glTF standard) |
| Tile conversion | **1 tile = 3 m** (AB §6). The client converts at load (`METERS_PER_TILE = 3.0`, ADR-0101 §6) — you never do |
| Camera the art must survive | High-angle RTS, default pitch ≈ 57°, ~55° per AB §1 — **design top surfaces first** (AB §1) |

Example: a `command_center` occupies a 4×4-tile footprint → its base is
12 m × 12 m in the export; its height budget is 3 tiles → 9 m (§4).

## 3. Origin and ground contact

| Rule | Value |
|---|---|
| Origin XZ | Center of the entity's footprint (the point the game places) |
| Origin Y | **0.0 = the ground plane.** Wheels, treads, and foundation plates touch Y = 0; nothing meaningful floats |
| Geometry below Y = 0 | Not allowed (it sinks through the terrain). `ground_offset` in the manifest is the pack-side lift, default 0 — do not rely on it to fix a mis-origin'd export |
| Buildings | The base plate's XZ bounding box must sit **inside** the footprint rectangle (§4's tile dims × 3 m), centered on the origin |
| Units | The visual ground width (XZ bounding box at rest) must be **0.9–1.1 ×** the collision diameter in §4 — a slight overhang reads; a big one lies |

## 4. Scale per kind (the checkable table)

Heights and widths follow AB §6's ladder; **the footprint and collision
columns are content data's, re-derived 2026-10-25 from
`content/entities/*.ron` — where AB §6 and content disagree, content wins**
(ADR-0101 §6; the two bible rows pending correction are noted in report
A-016 §9 P1).

| Kind | Size class | Footprint (content) | Collision diameter | Visual ground width | Height budget |
|---|---|---|---|---|---|
| `worker` | S | — (unit) | 0.6 tile = 1.8 m | 1.62–1.98 m | ≤ 0.5 tile = 1.5 m |
| `rifleman` | S | — (unit) | 0.7 tile = 2.1 m | 1.89–2.31 m | ≤ 0.7 tile = 2.1 m |
| `raider` | M | — (unit) | 0.6 tile = 1.8 m | 1.62–1.98 m | ≤ 0.4 tile = 1.2 m |
| `guardian` | L | — (unit) | 1.0 tile = 3.0 m | 2.7–3.3 m | ≤ 1.2 tile = 3.6 m |
| `command_center` | XL | 4×4 tiles | 12 m × 12 m block | within 12×12 m | ≤ 3.0 tiles = 9.0 m |
| `barracks` | L | 3×3 tiles | 9 m × 9 m block | within 9×9 m | ≤ 1.2 tiles = 3.6 m |
| `supply_depot` | M | 2×2 tiles | 6 m × 6 m block | within 6×6 m | ≤ 0.9 tile = 2.7 m |
| `turret` | M | 2×2 tiles | 6 m × 6 m block | within 6×6 m | ≤ 1.5 tiles = 4.5 m |
| `ore_node` | M | 2×2 tiles | 6 m × 6 m block | within 6×6 m | ≤ 0.8 tile = 2.4 m |

Ladder rules (AB §6): workers shortest; combat units 0.5–1 tile; the
turret's barrel tip ≤ 1.5 tiles; **nothing buildable exceeds the command
center's 3 tiles**. New kinds inherit their size class from their footprint
or collision radius in content data, then take that class's budgets below.

## 5. Poly budget per size class

Triangles counted across **all** meshes in the file (the exporter's final,
post-modifier count):

| Size class | Triangle budget | Primitives (meshes) per model |
|---|---|---|
| S | ≤ 600 | ≤ 4 |
| M | ≤ 1,500 | ≤ 4 |
| L | ≤ 3,000 | ≤ 4 |
| XL | ≤ 6,000 | ≤ 4 |

Why: the client draws entities instanced per primitive per kind (the
PNG-skin pipeline's "one instanced draw per texture" discipline, generalized
— `docs/ASSETS.md` "PNG texture rendering"); fewer primitives mean fewer
instanced calls. A whole Alpha-scale battlefield (200 entities, mostly S/M)
fits inside ~200k triangles at these caps; the 10× scale test stays inside
2M.

Primitive rules: mode `TRIANGLES` (4) only — points/lines/strips/fans are
skipped with one diagnostic and the fallback renders; indexed or
non-indexed both accepted; attributes `POSITION` + `NORMAL` + `TEXCOORD_0`
(POSITION required, the other two strongly recommended; missing NORMAL
falls back to a computed flat normal, missing TEXCOORD_0 makes the material
render as its base-color factor).

## 6. Materials and textures

| Rule | Value |
|---|---|
| Material model | glTF PBR **metallic-roughness** (the default) |
| Materials per model | ≤ 3 total, one of which may be `team_color` (§7) |
| Texture format | **PNG, embedded** in the `.glb`. JPEG/WebP/KTX2/basisu are **rejected** — the file is skipped with one diagnostic and the fallback renders (the client decodes PNG only) |
| Color space | sRGB (exporter default for albedo) |
| Texture dimensions | Power of two; ≤ 512×512 for S/M/L; ≤ 1024×1024 for XL |
| Textures per material | One albedo (`baseColorTexture`) **or** a plain `baseColorFactor` |
| `metallicRoughnessTexture` / `normalTexture` / `occlusionTexture` / `emissiveTexture` | **Not consumed** (ignored at load). Author flat albedo; shading comes from the runtime key light (AB §4's single NW key is produced by the renderer, not baked into your textures — do not pre-shade albedo with directional light) |
| `metallic` / `roughness` factors | Ignored in M12 — set `metallic: 0.0`, `roughness: 1.0` for forward compatibility. Style stays matte per AB §3 regardless |
| Alpha mode | `OPAQUE` only in M12. `MASK`/`BLEND` entries are skipped with one diagnostic (AB §3 forbids alpha-decal clutter anyway) |
| Double-sided | Allowed (costs fill rate) — off unless the part is a flag/tarp/cloth slab |
| Extensions on materials | None in M12 (unlit, transmission, volume, … all ignored) |

Palette rule (AB §2): bodies stay desaturated neutrals — 60% body neutrals,
25% secondary materials, 10–15% team color, < 5% accents; the 12 base
colors come from `palette.json`, never free-picked hex.

## 7. The `team_color` material

Ownership reads through one named material (ADR-0101 §3):

| Rule | Value |
|---|---|
| Name | Exactly `team_color` — case-sensitive, no prefix/suffix |
| Count | ≤ 1 per model |
| Who must carry one | Every player-ownable kind (all units, all buildings) |
| Who must **not** | `ore_node` (neutral — it renders amber per AB §2) |
| Base color | `baseColorFactor` of white `(1.0, 1.0, 1.0, 1.0)`, **or** an albedo texture whose minimum channel ≥ 0.75 (near-white, so the per-player tint multiplies predictably) |
| Coverage | AB §2's zone table: one flat primary zone on the upper mass (shoulders / roof / mantlet — the per-kind zones are listed there), plus at most one secondary trim |
| Forbidden placements | Thin parts that alias away (antennas, barrels, struts), scattered specks (AB §2) |
| Sanity check before delivery | Set the material's color to mid-gray `#808080`: the zone should render as an obvious darker patch under every team color, not just bright ones |

A player-ownable model without a `team_color` material still renders
(untinted) and logs one diagnostic at load — ownership then reads only
through the ground ring / team band. Fix it before delivery; the checklist
(§11) catches it.

## 8. Naming, paths, and the manifest

| Rule | Value |
|---|---|
| File name | `<kind_id>.glb` — **snake_case, exactly the content kind id** (`rifleman.glb`, never `Rifleman_v2.glb`; AB §9: no version suffixes) |
| Directory | `models/units/`, `models/buildings/`, `models/nodes/` inside the pack |
| Manifest paths | Relative to the pack root, forward slashes, no `..`, inside the root (loader-validated) |
| Icons (schema-only in M12) | `icons/<kind_id>.png` — 128×128, RGBA, transparent background (AB §9); referenced by the manifest `icon` field |
| Sounds (schema-only in M12) | `audio/<kind_id>/<cue>.<ext>` — cue names come from the engine's existing cue vocabulary; referenced by the manifest `sounds` map; playback arrives in a later brief |
| Skins | `textures/entities/<kind_id>.png` — the PNG-skin path keeps working unchanged; a kind can carry both a model and a skin (the skin is the model's fallback tier) |

The manifest entry that ships a model (schema 2, ADR-0101 §4):

```ron
entities: {
    "rifleman": (
        model: Some("models/units/rifleman.glb"),
        skin:  Some("textures/entities/rifleman.png"),
        scale: 1.0,            // optional, default 1.0, range 0.05..=10.0
        rotation_deg: 0.0,     // optional, default 0.0, range -360.0..=360.0
        ground_offset: 0.0,    // optional, meters, default 0.0
        icon: Some("icons/rifleman.png"),
    ),
},
```

`scale` / `rotation_deg` / `ground_offset` are pack-side presentation
fix-ups (a 5% size nudge, a re-oriented forward axis, a lifted hover) —
they exist so nobody re-exports for a nudge. They are **not** a license to
ship a mis-scaled or mis-facing export: origin, orientation, and scale per
§§2–4 still gate delivery.

## 9. File-size caps

| Artifact | Cap |
|---|---|
| `.glb`, size classes S/M | ≤ 2 MiB per file |
| `.glb`, size class L | ≤ 4 MiB per file |
| `.glb`, size class XL | ≤ 8 MiB per file |
| Client hard refuse (pre-parse) | 32 MiB — a larger file is never parsed; one diagnostic, fallback renders |
| Icon PNG | ≤ 256 KiB, 128×128 |
| Whole pack (all files) | ≤ 48 MiB — release archives ship the pack beside the binary |

Budget honesty: the nine current kinds at their class caps sum to ≤ 28 MiB
of models — the pack cap leaves room for icons, skins, and terrain without
touching the archive budget that matters (A-014: the pack added ~2.5 MB to
a ~8.1 MB archive; keep it in that spirit).

## 10. What the pipeline does with your file (debugging map)

Load order, and what each failure does — one diagnostic per failing kind
per run, always at startup, never per frame (ADR-0101 §5):

1. **Path + security** (unchanged from the current pack system): absolute
   paths, `..`, and pack-root escapes are rejected before anything opens.
2. **Pre-parse size check** (§9's hard refuse).
3. **Parse** the glTF JSON + BIN chunks. Malformed or truncated → the model
   tier is abandoned (diagnostic).
4. **Subset validation** (§§1, 5, 6): non-`.glb`, unsupported primitive
   modes, non-embedded-PNG textures, non-OPAQUE alpha → skipped
   (diagnostic).
5. **Upload** meshes/materials; resolve `team_color` (§7). A missing
   `team_color` on an ownable kind: renders untinted + one diagnostic.
6. **Fallback tiers**: model → `skin` PNG box → placeholder silhouette. A
   kind with both a broken model and a valid skin lands on the skin; both
   broken lands on the placeholder. The game never crashes on any byte
   sequence — that property is fuzz-tested, not promised.

## 11. Delivery checklist (run before every delivery)

A model is deliverable when every box is checked — each maps to a rule
above:

- [ ] One `.glb`, named `<kind_id>.glb`, under `models/{units|buildings|nodes}/`
- [ ] No animations/skins/morph data in the file (or you accept the one ignored-animations note)
- [ ] 1 unit = 1 m; +Y up; nose at −Z; origin at footprint center, Y = 0 ground contact
- [ ] Footprint block inside the §4 tile rectangle (buildings) / visual width 0.9–1.1 × collision diameter (units)
- [ ] Height within the §4 budget; nothing exceeds the command center
- [ ] Triangles within the §5 class budget; ≤ 4 primitives; TRIANGLES mode
- [ ] ≤ 3 materials; textures embedded PNG, power-of-two, within §6 dims; no normal/ORM/emissive textures; OPAQUE alpha; no pre-shaded albedo
- [ ] `team_color` present (ownable kinds) / absent (`ore_node`); near-white base; one upper-mass zone per AB §2; gray-tint sanity check done
- [ ] File size within §9's class cap
- [ ] Manifest entry written (schema 2) with correct relative path; `scale`/`rotation_deg`/`ground_offset` left at defaults unless a fix-up was agreed

Delivery path (Standing Orders v1.4 §3): files go to the Owner; the Owner
relays a brief; the Programmer commits to a pack directory in
`pandemonium-assets`; the verified client load (the dev pack-loaded line or
the packaged smoke) is the evidence. Nothing in `design/` ever ships or
loads. The Designer never edits `content/` or this repo's code.

## Appendix — one-line-per-kind quick reference

| Kind | Class | File | Ground | Height ≤ | Tris ≤ | team_color |
|---|---|---|---|---|---|---|
| worker | S | `models/units/worker.glb` | 1.62–1.98 m | 1.5 m | 600 | required (hard hat + backpack shell) |
| rifleman | S | `models/units/rifleman.glb` | 1.89–2.31 m | 2.1 m | 600 | required (torso vest + helmet band) |
| raider | M | `models/units/raider.glb` | 1.62–1.98 m | 1.2 m | 1,500 | required (engine cowl) |
| guardian | L | `models/units/guardian.glb` | 2.7–3.3 m | 3.6 m | 3,000 | required (shoulder mantlet plates) |
| command_center | XL | `models/buildings/command_center.glb` | 12×12 m | 9.0 m | 6,000 | required (roof disc + door frame) |
| barracks | L | `models/buildings/barracks.glb` | 9×9 m | 3.6 m | 3,000 | required (gable roof panels) |
| supply_depot | M | `models/buildings/supply_depot.glb` | 6×6 m | 2.7 m | 1,500 | required (silo dome caps) |
| turret | M | `models/buildings/turret.glb` | 6×6 m | 4.5 m | 1,500 | required (mantlet ring) |
| ore_node | M | `models/nodes/ore_node.glb` | 6×6 m | 2.4 m | 1,500 | **forbidden** (neutral amber) |

(team-color zones quoted from AB §2's table.)
