//! Terrain mesh generation (plan §11.2 as amended by ADR-0001): the map grid +
//! the display-only heightmap become the vertex/index data the wgpu renderer
//! draws — one quad per tile, vertically displaced by the heightmap, colored by
//! terrain class.
//!
//! Pure data in, pure data out: this module knows nothing about wgpu, so the
//! mesh shape is unit-testable on headless CI (plan §11.2's Renderer-trait
//! spirit — the GPU layer stays in `client`). The heightmap is display-only by
//! construction: it displaces vertices here, in the presentation layer, and is
//! never part of the world definition the simulation receives.

use pandemonium_content::MapDef;

/// One terrain vertex: position (world units, Y up) and placeholder color.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TerrainVertex {
    /// Position in ground-plane world space (tile corners at integer x/z,
    /// height from the heightmap).
    pub position: [f32; 3],
    /// Placeholder color (RGB), chosen by terrain class.
    pub color: [f32; 3],
}

/// The generated terrain: a triangle list with per-tile quad vertices.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct TerrainMesh {
    /// Four vertices per tile (flat shading — no corner sharing, so per-tile
    /// colors stay trivial).
    pub vertices: Vec<TerrainVertex>,
    /// Six indices per tile (two triangles).
    pub indices: Vec<u32>,
}

/// Placeholder palette by terrain class: passable ground reads as grass,
/// blocked terrain as rock. Placeholder art only (plan §0).
fn class_color(passable: bool) -> [f32; 3] {
    if passable {
        [0.34, 0.49, 0.29]
    } else {
        [0.46, 0.43, 0.40]
    }
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
            let color = class_color(map.passable(x as i32, y as i32));
            let base = mesh.vertices.len() as u32;
            // Corners: a=(x,y) b=(x+1,y) c=(x,y+1) d=(x+1,y+1), Y up.
            for (cx, cy) in [(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)] {
                mesh.vertices.push(TerrainVertex {
                    position: [cx as f32, height(cx, cy), cy as f32],
                    color,
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
        // Tile (0,0) is rock: its four vertices carry the rock color.
        let rock = class_color(false);
        assert!(
            mesh.vertices[0..4].iter().all(|v| v.color == rock),
            "the (0,0) quad is colored by its impassable terrain class"
        );
        // The next tile is ground-colored.
        let ground = class_color(true);
        assert!(mesh.vertices[4..8].iter().all(|v| v.color == ground));
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
}
