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

/// Terrain albedo by class with per-tile variation: passable ground reads
/// as calm, low-saturation dry steppe — muted enough that the team-colored
/// entities pop against it (PLAN-M10.2 §2.5); blocked terrain as dark
/// gray-brown rock, clearly darker than the ground so walls and ridges read
/// as obstacles. Placeholder-plus (plan §0): flat colors, but shaded ones.
fn class_color(passable: bool, variation: f32, height: f32) -> [f32; 3] {
    // ±0.045 of jitter around the class base, plus a subtle two-tile checker
    // so flat areas keep a readable grain.
    let jitter = (variation - 0.5) * 0.09;
    let checker = if ((variation * 255.0) as u32).is_multiple_of(2) {
        0.015
    } else {
        -0.015
    };
    let mut color = if passable {
        // Dry steppe: the old grass carried a strong green dominance that
        // fought the team colors; this stays green-leaning but muted, with
        // a gentle dryness drift toward tan.
        let dryness = variation;
        [
            0.36 + 0.06 * dryness + jitter,
            0.41 + 0.02 * dryness + jitter,
            0.33 - 0.02 * dryness + jitter,
        ]
    } else {
        // Rock: distinctly darker than any passable tile — the obstacle
        // read is the point (the walls of the Crossroads crossing).
        [0.30 + jitter, 0.27 + jitter, 0.25 + jitter]
    };
    // Hills pick up a rocky tint with height (the heightmap is display-only,
    // so this is pure presentation math).
    let rocky = (height / 3.0).clamp(0.0, 0.4);
    for channel in &mut color {
        *channel = *channel * (1.0 - rocky) + 0.38 * rocky;
        *channel = (*channel + checker).clamp(0.0, 1.0);
    }
    color
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
            let tile_height =
                (height(x, y) + height(x + 1, y) + height(x, y + 1) + height(x + 1, y + 1)) * 0.25;
            let color = class_color(map.passable(x as i32, y as i32), variation, tile_height);
            // Corners: a=(x,y) b=(x+1,y) c=(x,y+1) d=(x+1,y+1), Y up.
            let corner = |cx: u32, cy: u32| [cx as f32, height(cx, cy), cy as f32];
            let a = corner(x, y);
            let b = corner(x + 1, y);
            let c = corner(x, y + 1);
            let d = corner(x + 1, y + 1);
            // The quad's face normal from its first triangle's edges, flipped
            // to point up (the winding below may face either way in cross
            // order; terrain shading wants the sky-facing side).
            let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let mut normal = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            if normal[1] < 0.0 {
                normal = [-normal[0], -normal[1], -normal[2]];
            }
            let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2])
                .sqrt()
                .max(1e-6);
            let normal = [normal[0] / length, normal[1] / length, normal[2] / length];
            let base = mesh.vertices.len() as u32;
            for position in [a, b, c, d] {
                mesh.vertices.push(TerrainVertex {
                    position,
                    color,
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
        // entities, and rock is plainly darker than any ground shade.
        for variation in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
            let ground = class_color(true, variation, 0.0);
            let rock = class_color(false, variation, 0.0);
            let lum = |c: [f32; 3]| (c[0] + c[1] + c[2]) / 3.0;
            assert!(
                lum(rock) < lum(ground) - 0.05,
                "rock reads darker than ground at variation {variation}"
            );
            // Green-leaning but muted: the channel spread stays small.
            let spread =
                ground[0].max(ground[1]).max(ground[2]) - ground[0].min(ground[1]).min(ground[2]);
            assert!(spread < 0.13, "the ground stays calm at {variation}");
            assert!(ground[1] > ground[2], "still green-dominant");
        }
    }
}
