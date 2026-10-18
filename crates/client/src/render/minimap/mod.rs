// `minimap/` (ADR-0100 part c, the ARCHITECTURE.md refactor map): the
// minimap pass's geometry and the marker painter — the composite texture
// itself is baked by `update_minimap` in `super` from these pieces.
use bytemuck::{Pod, Zeroable};

use super::entities::fallback_shape;
use super::KindShape;

/// The minimap pass: a screen-space quad sampling a small map-sized RGBA
/// texture the client composites (terrain base + fog + entity dots) every
/// frame. Drawn through the same pixel-space convention as the UI pass.
pub(super) const MINIMAP_SHADER_SRC: &str = r#"
struct ScreenUniform { size: vec2<f32> };
@group(0) @binding(0) var<uniform> screen: ScreenUniform;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var tex: texture_2d<f32>;

struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex fn vs_main(@location(0) pos_uv: vec4<f32>) -> VsOut {
    var out: VsOut;
    let ndc = vec2<f32>(
        pos_uv.x / screen.size.x * 2.0 - 1.0,
        1.0 - pos_uv.y / screen.size.y * 2.0,
    );
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = pos_uv.zw;
    return out;
}

@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv);
}
"#;

/// One minimap quad vertex: pixel position + atlas-free uv (16 bytes).
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct MinimapVertex {
    pub(super) pos_uv: [f32; 4],
}

/// One cube vertex: corner position and its soft edge normal (24 bytes).
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct CubeVertex {
    pub(super) corner: [f32; 3],
    pub(super) normal: [f32; 3],
}

/// The unit cube's 36 vertices (12 triangles), centered at the origin, half-
/// extent 0.5 on each axis. Corner normals ease the hard lighting breaks on
/// the box-built silhouettes without changing their footprint or shape.
pub(super) fn cube_vertices() -> Vec<CubeVertex> {
    const FACES: [[usize; 4]; 6] = [
        [0, 1, 2, 3], // -Z
        [5, 4, 7, 6], // +Z
        [4, 0, 3, 7], // -X
        [1, 5, 6, 2], // +X
        [4, 5, 1, 0], // -Y
        [3, 2, 6, 7], // +Y
    ];
    let corners = [
        [-0.5, -0.5, -0.5],
        [0.5, -0.5, -0.5],
        [0.5, 0.5, -0.5],
        [-0.5, 0.5, -0.5],
        [-0.5, -0.5, 0.5],
        [0.5, -0.5, 0.5],
        [0.5, 0.5, 0.5],
        [-0.5, 0.5, 0.5],
    ];
    let mut vertices = Vec::with_capacity(36);
    for quad in &FACES {
        let corners = [
            corners[quad[0]],
            corners[quad[1]],
            corners[quad[2]],
            corners[quad[3]],
        ];
        for corner in [
            corners[0], corners[1], corners[2], corners[0], corners[2], corners[3],
        ] {
            vertices.push(CubeVertex {
                corner,
                normal: glam::Vec3::from_array(corner).normalize().to_array(),
            });
        }
    }
    vertices
}

/// The minimap marker palette, mirroring [`TEAM_P0`]/[`TEAM_P1`] (A-102):
/// own blue, enemy orange, ore amber — the colorblind-safe triple.
pub(super) const MINIMAP_OWN: [f32; 3] = [70.0, 130.0, 255.0];
pub(super) const MINIMAP_ENEMY: [f32; 3] = [255.0, 150.0, 40.0];
pub(super) const MINIMAP_ORE: [f32; 3] = [250.0, 190.0, 60.0];

/// Paints the minimap's entity markers into the map-sized RGBA scratch
/// buffer (one byte quad per tile, row-major). Pure geometry + color: units
/// one tile, structures a solid 2×2 block (visibly larger — PLAN-M10.2
/// §2.4), neutral resource nodes a 3×3 amber diamond so the map carries the
/// world's shapes. Off-map entities paint nothing.
pub(super) fn paint_minimap_markers(
    scratch: &mut [u8],
    entities: &[pandemonium_engine::RenderEntity],
    kind_specs: &[KindShape],
    map_size: (u32, u32),
) {
    let (w, h) = map_size;
    for entity in entities {
        let tx = entity.pos.x.floor();
        let ty = entity.pos.z.floor();
        if tx < 0.0 || ty < 0.0 || tx >= w as f32 || ty >= h as f32 {
            continue;
        }
        let shape = kind_specs
            .get(entity.kind.0 as usize)
            .cloned()
            .unwrap_or_else(fallback_shape);
        let (color, extent, diamond) = if entity.owner == pandemonium_sim_api::PlayerId::NEUTRAL {
            (MINIMAP_ORE, 3i32, true)
        } else if shape.unit {
            (
                match entity.owner {
                    pandemonium_sim_api::PlayerId(0) => MINIMAP_OWN,
                    _ => MINIMAP_ENEMY,
                },
                1i32,
                false,
            )
        } else {
            (
                match entity.owner {
                    pandemonium_sim_api::PlayerId(0) => MINIMAP_OWN,
                    _ => MINIMAP_ENEMY,
                },
                2i32,
                false,
            )
        };
        let anchor = extent / 2;
        for dy in -anchor..extent - anchor {
            for dx in -anchor..extent - anchor {
                if diamond && dx.abs() + dy.abs() > 1 {
                    continue; // the diamond trims its own corners
                }
                let px = tx as i32 + dx;
                let py = ty as i32 + dy;
                if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                    continue;
                }
                let offset = ((py as u32 * w + px as u32) as usize) * 4;
                scratch[offset] = color[0] as u8;
                scratch[offset + 1] = color[1] as u8;
                scratch[offset + 2] = color[2] as u8;
                scratch[offset + 3] = 255;
            }
        }
    }
}
