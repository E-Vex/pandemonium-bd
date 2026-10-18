//! Terrain mesh generation (plan §11.2 as amended by ADR-0001): the map grid +
//! the display-only heightmap become the vertex/index data the wgpu renderer
//! draws — one quad per tile, vertically displaced by the heightmap, colored by
//! terrain class, with a per-quad normal so the fragment pass can shade it.
//!
//! Pure data in, pure data out: this module knows nothing about wgpu, so the
//! mesh shape is unit-testable on headless CI (plan §11.2's Renderer-trait
//! spirit — the GPU layer stays in `client`). The heightmap is display-only by
//! construction: it displaces vertices here, in the presentation layer, and is
//! never part of the world definition the simulation receives.

use pandemonium_content::MapDef;

/// One terrain vertex: position (world units, Y up), albedo color, and the
/// quad's shading normal.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TerrainVertex {
    /// Position in ground-plane world space (tile corners at integer x/z,
    /// height from the heightmap).
    pub position: [f32; 3],
    /// Albedo color (RGB), chosen by terrain class with per-tile variation.
    pub color: [f32; 3],
    /// Face normal (flat shading — one normal per tile quad).
    pub normal: [f32; 3],
    /// Presentation material: grass, bare dirt, or stone. This never enters
    /// the simulation; the renderer uses it for procedural surface detail.
    pub material: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TerrainMaterial {
    Grass,
    Dirt,
    Stone,
}

impl TerrainMaterial {
    fn code(self) -> f32 {
        match self {
            Self::Grass => 0.0,
            Self::Dirt => 1.0,
            Self::Stone => 2.0,
        }
    }
}

/// The generated terrain: a triangle list with per-tile quad vertices.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct TerrainMesh {
    /// Four vertices per tile (flat shading — no corner sharing, so per-tile
    /// colors and normals stay trivial).
    pub vertices: Vec<TerrainVertex>,
    /// Six indices per tile (two triangles).
    pub indices: Vec<u32>,
}

/// The sun the whole scene is lit by (shaders share this direction). Tilted
/// enough to model volume, shallow enough that south faces stay readable.
pub const SUN_DIRECTION: [f32; 3] = [0.45, 0.85, 0.30];

/// Per-tile deterministic variation in `0.0..=1.0` from the tile's
/// coordinates — a coordinate hash, not an RNG: the same map always shades
/// the same way, on every machine, in every run.
fn tile_variation(x: u32, y: u32) -> f32 {
    let hash = x
        .wrapping_mul(7_385_609)
        .wrapping_add(y.wrapping_mul(19_349_663));
    ((hash >> 9) & 0xFF) as f32 / 255.0
}

/// Large-scale ground patchiness: bilinear value noise over the same tile
/// hash lattice at an 8-tile wavelength, with smoothed cell fractions so the
/// patches curve instead of kinking. Pure presentation math over
/// coordinates — deterministic like [`tile_variation`], but *smooth*: the
/// per-tile jitter alone reads as static, while these broad dry/lush patches
/// give the steppe an organic floor the eye reads as terrain, not tiling.
fn patch_noise(x: u32, y: u32) -> f32 {
    let (fx, fy) = (x as f32 / 8.0, y as f32 / 8.0);
    let (ix, iy) = (fx.floor() as u32, fy.floor() as u32);
    let (tx, ty) = (fx.fract(), fy.fract());
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let top = tile_variation(ix, iy) * (1.0 - sx) + tile_variation(ix + 1, iy) * sx;
    let bottom = tile_variation(ix, iy + 1) * (1.0 - sx) + tile_variation(ix + 1, iy + 1) * sx;
    top * (1.0 - sy) + bottom * sy
}

/// Terrain albedo by class: passable ground reads as calm, low-saturation
/// dry steppe — muted enough that the team-colored entities pop against it
/// (PLAN-M10.2 §2.5) — drifting between lush sage-green and dry straw
/// patches; blocked terrain as dark gray-brown rock, clearly darker than
/// the ground so walls and ridges read as obstacles. Height shading lives
/// at the corner level in [`terrain_mesh`] — per-vertex, so hills get a
/// smooth gradient instead of a per-tile step.
fn class_color(material: TerrainMaterial, variation: f32, patch: f32) -> [f32; 3] {
    // ±0.045 of jitter around the class base, plus a subtle two-tile checker
    // so flat areas keep a readable grain.
    let jitter = (variation - 0.5) * 0.09;
    let checker = if ((variation * 255.0) as u32).is_multiple_of(2) {
        0.015
    } else {
        -0.015
    };
    let mut color = match material {
        TerrainMaterial::Grass => {
            // Natural olive greens with a dry straw drift across broad patches.
            let lush = [0.29, 0.40, 0.23];
            let straw = [0.43, 0.39, 0.25];
            [
                lush[0] + (straw[0] - lush[0]) * patch + jitter,
                lush[1] + (straw[1] - lush[1]) * patch + jitter,
                lush[2] + (straw[2] - lush[2]) * patch + jitter,
            ]
        }
        TerrainMaterial::Dirt => [0.48 + jitter, 0.31 + jitter, 0.17 + jitter],
        TerrainMaterial::Stone => [0.25 + jitter, 0.23 + jitter, 0.21 + jitter],
    };
    for channel in &mut color {
        *channel = (*channel + checker).clamp(0.0, 1.0);
    }
    color
}

/// The per-corner brightness factor a terrain vertex carries: hilltops
/// brighten (rock ridges more than soil, so ridgelines read), lowlands sink
/// a touch cooler. The heightmap is display-only (ADR-0001), so this is
/// pure presentation math — and per-*corner*, so a slope shades as a
/// gradient across its quads rather than as flat per-tile steps.
fn corner_shade(passable: bool, height: f32) -> f32 {
    let up = (height / 3.0).clamp(0.0, 0.35);
    let low = 1.0 - (height / 2.0).clamp(0.0, 1.0);
    1.0 + if passable { up * 0.9 } else { up * 1.5 } - low * 0.05
}

/// Builds the terrain mesh for a map. `height_scale` converts heightmap units
/// (integer counts) to world tiles of vertical displacement; a map without a
/// heightmap renders flat regardless of the scale (ADR-0001).
pub fn terrain_mesh(map: &MapDef, height_scale: f32) -> TerrainMesh {
    let height = |x: u32, y: u32| -> f32 {
        map.heightmap
            .as_ref()
            .and_then(|heightmap| heightmap.rows.get(y as usize))
            .and_then(|row| row.get(x as usize))
            .map(|value| *value as f32 * height_scale)
            .unwrap_or(0.0)
    };
    let mut mesh = TerrainMesh::default();
    mesh.vertices
        .reserve((map.width as usize) * (map.height as usize) * 4);
    mesh.indices
        .reserve((map.width as usize) * (map.height as usize) * 6);
    for y in 0..map.height {
        for x in 0..map.width {
            let variation = tile_variation(x, y);
            let patch = patch_noise(x, y);
            let passable = map.passable(x as i32, y as i32);
            let material = map
                .terrain_at(x as i32, y as i32)
                .map(|class| {
                    if !class.passable {
                        TerrainMaterial::Stone
                    } else if ["dirt", "soil", "sand"]
                        .iter()
                        .any(|name| class.name.to_ascii_lowercase().contains(name))
                    {
                        TerrainMaterial::Dirt
                    } else {
                        TerrainMaterial::Grass
                    }
                })
                .unwrap_or(TerrainMaterial::Stone);
            let color = class_color(material, variation, patch);
            // Corners: a=(x,y) b=(x+1,y) c=(x,y+1) d=(x+1,y+1), Y up.
            let corner = |cx: u32, cy: u32| [cx as f32, height(cx, cy), cy as f32];
            let a = corner(x, y);
            let b = corner(x + 1, y);
            let c = corner(x, y + 1);
            let d = corner(x + 1, y + 1);
            // One vertex per corner, each shaded by its own height — the
            // per-corner gradient that makes hills read as slopes.
            let shade = |corner_position: [f32; 3]| {
                let f = corner_shade(passable, corner_position[1]);
                [color[0] * f, color[1] * f, color[2] * f]
            };
            let colors = [shade(a), shade(b), shade(c), shade(d)];
            // Estimate one normal at each grid corner from its neighboring
            // heights. Reusing this field across adjacent tiles removes the
            // per-tile lighting seams and lets slopes read as a continuous
            // surface. This affects lighting only; vertex positions remain
            // the authored heightmap values above.
            let corner_normal = |cx: u32, cy: u32| {
                let left_x = cx.saturating_sub(1);
                let right_x = (cx + 1).min(map.width);
                let near_y = cy.saturating_sub(1);
                let far_y = (cy + 1).min(map.height);
                let dx =
                    (height(right_x, cy) - height(left_x, cy)) / (right_x - left_x).max(1) as f32;
                let dz = (height(cx, far_y) - height(cx, near_y)) / (far_y - near_y).max(1) as f32;
                let length = (dx * dx + 1.0 + dz * dz).sqrt();
                [-dx / length, 1.0 / length, -dz / length]
            };
            let normals = [
                corner_normal(x, y),
                corner_normal(x + 1, y),
                corner_normal(x, y + 1),
                corner_normal(x + 1, y + 1),
            ];
            let base = mesh.vertices.len() as u32;
            for ((position, vertex_color), normal) in
                [a, b, c, d].into_iter().zip(colors).zip(normals)
            {
                mesh.vertices.push(TerrainVertex {
                    position,
                    color: vertex_color,
                    material: material.code(),
                    normal,
                });
            }
            // Two triangles per tile (culling is disabled in the pipeline, so
            // winding is not load-bearing for the placeholder terrain).
            mesh.indices.extend_from_slice(&[
                base,
                base + 1,
                base + 2,
                base + 1,
                base + 3,
                base + 2,
            ]);
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandemonium_content::{MapDef, OreNodeDef, StartDef, TerrainClass};

    fn flat_map(width: u32, height: u32) -> MapDef {
        MapDef {
            id: "test".to_string(),
            display_name: "Test".to_string(),
            width,
            height,
            terrain: vec![
                TerrainClass {
                    code: '.',
                    name: "ground".to_string(),
                    passable: true,
                    buildable: true,
                },
                TerrainClass {
                    code: '#',
                    name: "rock".to_string(),
                    passable: false,
                    buildable: false,
                },
            ],
            grid: (0..height)
                .map(|y| {
                    (0..width)
                        .map(|x| if x == 0 && y == 0 { '#' } else { '.' })
                        .collect()
                })
                .collect(),
            starts: vec![StartDef {
                player: 0,
                x: 1,
                y: 1,
            }],
            ore_nodes: vec![OreNodeDef {
                kind: "ore_node".to_string(),
                x: 2,
                y: 2,
            }],
            symmetric: false,
            heightmap: None,
            file: "maps/test.ron".to_string(),
        }
    }

    #[test]
    fn a_flat_map_produces_four_vertices_and_six_indices_per_tile() {
        let map = flat_map(3, 2);
        let mesh = terrain_mesh(&map, 0.02);
        assert_eq!(mesh.vertices.len(), 3 * 2 * 4);
        assert_eq!(mesh.indices.len(), 3 * 2 * 6);
        // Every vertex of a heightmap-less map sits on the plane.
        assert!(mesh.vertices.iter().all(|v| v.position[1] == 0.0));
        // Tile (0,0) is rock: its four vertices carry a rock-class albedo —
        // darker than ground and less green.
        let rock = mesh.vertices[0].color;
        let ground = mesh.vertices[4].color;
        assert!(
            rock[0] < 0.5 && rock[1] < 0.5,
            "rock reads as muted gray-brown"
        );
        assert!(
            ground[1] > ground[2] && ground[1] > rock[1] + 0.04,
            "ground reads as grass (green-dominant, greener than rock)"
        );
        // Every vertex of the flat map faces the sky.
        assert!(mesh
            .vertices
            .iter()
            .all(|v| (v.normal[1] - 1.0).abs() < 1e-4));
    }

    #[test]
    fn the_heightmap_displaces_vertices_and_only_vertices() {
        let mut map = flat_map(2, 2);
        map.heightmap = Some(pandemonium_content::Heightmap {
            rows: vec![vec![100, 0], vec![0, 0]],
        });
        let mesh = terrain_mesh(&map, 0.02);
        // Corner (0,0) is shared by tile (0,0)'s quad as its first vertex.
        assert!(
            (mesh.vertices[0].position[1] - 2.0).abs() < 1e-4,
            "100 * 0.02 = 2 tiles"
        );
        // A vertex not adjacent to (0,0) stays on the plane.
        assert_eq!(mesh.vertices[3].position[1], 0.0, "corner (1,1) is flat");
        // Tile (1,1)'s quad starts at vertex index 12 (three tiles earlier).
        assert_eq!(mesh.vertices[12].position[1], 0.0);
    }

    #[test]
    fn a_ramped_heightmap_tilts_the_quad_normal() {
        let mut map = flat_map(2, 1);
        // A pure east-facing ramp: height rises along x only.
        map.heightmap = Some(pandemonium_content::Heightmap {
            rows: vec![vec![0, 100], vec![0, 100]],
        });
        let mesh = terrain_mesh(&map, 0.02);
        let normal = mesh.vertices[0].normal;
        // The ramp climbs along +x, so the surface tilts toward -x while
        // staying unit-length and sky-facing.
        assert!(normal[1] > 0.0, "the normal faces the sky");
        assert!(normal[0] < 0.0, "an east-facing ramp tilts toward -x");
        let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        assert!((length - 1.0).abs() < 1e-4, "the normal is unit length");
    }

    #[test]
    fn tile_variation_is_deterministic_and_bounded() {
        let a = tile_variation(7, 9);
        assert_eq!(a, tile_variation(7, 9), "same tile, same shade");
        for x in 0..16u32 {
            for y in 0..16u32 {
                let value = tile_variation(x, y);
                assert!((0.0..=1.0).contains(&value), "variation stays in range");
            }
        }
    }

    #[test]
    fn the_ground_stays_calm_and_rock_reads_as_an_obstacle() {
        // PLAN-M10.2 §2.5: the ground is calmer (lower saturation) than the
        // entities, and rock is plainly darker than any ground shade — swept
        // across the whole patch drift, not just its midpoint.
        for patch in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
            for variation in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
                let ground = class_color(TerrainMaterial::Grass, variation, patch);
                let dirt = class_color(TerrainMaterial::Dirt, variation, patch);
                let rock = class_color(TerrainMaterial::Stone, variation, patch);
                let lum = |c: [f32; 3]| (c[0] + c[1] + c[2]) / 3.0;
                assert!(
                    lum(rock) < lum(ground) - 0.05,
                    "rock reads darker than ground at patch {patch}, variation {variation}"
                );
                assert!(dirt[0] > dirt[1] && dirt[1] > dirt[2], "dirt reads warm");
                // Grass stays green-leaning while retaining a natural range.
                let spread = ground[0].max(ground[1]).max(ground[2])
                    - ground[0].min(ground[1]).min(ground[2]);
                assert!(
                    spread < 0.20,
                    "the ground stays calm at patch {patch}, variation {variation}"
                );
                assert!(ground[1] > ground[2], "still green-dominant");
            }
        }
    }

    #[test]
    fn patch_noise_is_deterministic_smooth_and_bounded() {
        assert_eq!(
            patch_noise(7, 9),
            patch_noise(7, 9),
            "same tile, same patch"
        );
        for x in 0..24u32 {
            for y in 0..24u32 {
                let value = patch_noise(x, y);
                assert!((0.0..=1.0).contains(&value), "patch stays in range");
                // Smoothness: adjacent tiles drift at most a fraction of a
                // lattice cell — the broad patches never flicker.
                let neighbor = patch_noise(x + 1, y);
                assert!((value - neighbor).abs() < 0.5, "patch drifts smoothly");
            }
        }
    }

    #[test]
    fn hilltops_brighten_and_lowlands_sink_for_both_classes() {
        // Soil tops lift gently; rock ridges lift more (ridgelines read);
        // the valley floor sits slightly under 1.0 either way.
        assert!(corner_shade(true, 3.0) > 1.0);
        assert!(corner_shade(false, 3.0) > corner_shade(true, 3.0));
        assert!(corner_shade(true, 0.0) < 1.0);
        assert!(corner_shade(false, 0.0) < 1.0);
        // The factor never inverts a class's darkness order: rock stays
        // darker than ground even with its stronger height lift, because
        // both live on the same map heights.
        for height in [0.0_f32, 0.5, 1.0, 2.0, 3.5] {
            let ground = class_color(TerrainMaterial::Grass, 0.5, 0.5);
            let rock = class_color(TerrainMaterial::Stone, 0.5, 0.5);
            let lift = |passable: bool| corner_shade(passable, height);
            let lum = |c: [f32; 3]| (c[0] + c[1] + c[2]) / 3.0;
            assert!(
                lum(rock) * lift(false) < lum(ground) * lift(true),
                "rock stays darker than ground at height {height}"
            );
        }
    }
}
