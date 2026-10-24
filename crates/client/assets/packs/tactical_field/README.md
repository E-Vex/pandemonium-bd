# Tactical Field Pack - Pandemonium Asset Pack

**pack_id:** `tactical_field`  
**schema_version:** 1  
**Style:** Worn military atlas / field manual

## Art Direction

Pandemonium's client renders terrain as vertex-colored geometry with procedural noise, then multiplies a single pack texture on top (`world.xz / camera.map_size` UVs, `Rgba8UnormSrgb`). This pack leans into that:

- **Palette:** Desaturated, high-value (240-255 base) so multiplication preserves vertex colors but adds tactile detail. Subtle warm/cool tints per terrain type.
- **Tiling:** UVs span the map once, so textures are designed with large-scale low-frequency variation (clouds, dunes, vignetting) plus fine grain, rather than high-frequency patterns that would pixelate when stretched.
- **Tactical grid:** Faint 64px grid lines (darken by 4-6) evoke a field map / command overlay without fighting the vertex colors. Top-down RTS readability first.
- **Box silhouettes:** Entities are still box silhouettes in current renderer, but entity textures are prepared for future use (canvas, olive camo, brushed steel, concrete).

## Pack Layout

```
tactical_field/
├── manifest.ron
├── README.md
├── textures/
│   ├── terrain/
│   │   ├── grass.png      # primary - light sage, paper fiber, 512x512
│   │   ├── gravel.png     # warm gray with pebble speckles
│   │   ├── rock.png       # cool gray with crack speckles
│   │   └── sand.png       # beige with dune waves
│   └── entities/
│       ├── worker.png
│       ├── rifleman.png   # light olive camo
│       ├── raider.png     # dusty streaks
│       ├── guardian.png   # brushed steel
│       ├── turret.png     # concrete + rivets
│       ├── command_center.png
│       ├── barracks.png   # corrugated metal
│       └── supply_depot.png # wood crate
```

All paths are relative, no `..`, forward slashes only, inside pack root - passes `pack/mod.rs` validation.

## Keys

### Terrain
Based on plausible content keys and default pack's `grass`:
- `grass` - primary, first alphabetically, so it is the texture actually bound by current `load_pack_texture()` (picks first terrain entry via BTreeMap order)
- `gravel`, `rock`, `sand` - variants for future biome support

BTreeMap is alphabetical, so `grass` < `gravel` < `rock` < `sand`. Primary remains `grass`.

### Entities
From `content/` roster (Legion faction):
- `worker`, `rifleman`, `raider`, `guardian`, `turret`, `command_center`, `barracks`, `supply_depot`
Lowercase, underscore style matching default pack's `rifleman`.

Models (`glb/gltf`) are declared in loader but not consumed yet - reserved for future.

## How Texture Multiplication Works

In `crates/client/src/render/terrain/mod.rs`, fragment:
```wgsl
final_color = vertex_color * texture_sample * procedural_detail
```
- White (255) = no change
- 230-250 = subtle darkening / tint
- Values below 180 start to heavily darken - avoided except for speckles/cracks

All our textures stay 180-255 for this reason. Grain intensity is ±6 to avoid banding.

## Validation

1. **Schema:** `cargo test -p pandemonium-client` - existing pack tests should still pass; new pack is isolated.
2. **Load test:** 
   ```bash
   cargo run -p pandemonium-client -- --pack crates/client/assets/packs/tactical_field
   # log: asset pack 'tactical_field' (schema 1) loaded from ...
   ```
3. **Visual sanity:** Terrain should show faint paper grain + grid modulating vertex colors. If too dark, lighten texture.
4. **Safety:** Loader rejects absolute paths and `..` via `canonicalize()` check. Our manifest uses only relative safe paths.
5. **Fallback:** Rename `manifest.ron` -> `manifest.ron.bak`, run without `--pack` or with missing manifest, client should fallback to 1x1 white.

## Design Notes

- 512x512 for terrain (power-of-two, reasonable size), 256x256 for entities
- RGB, `image::open` + `to_rgba8()` handles conversion
- No external dependencies
- Drop-in: copy this folder to `crates/client/assets/packs/tactical_field/` and use `--pack`

## Future

- When renderer consumes multiple terrain classes, `gravel`/`rock`/`sand` will map naturally
- When entity models are consumed, `textures/entities/` provides PBR-ish base
- Consider `models/` glb placeholders later

## Credits

Generated procedurally with Python (numpy + scipy + PIL) for determinism, no external art assets.
