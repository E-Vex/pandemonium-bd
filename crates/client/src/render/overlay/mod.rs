// `overlay/` (ADR-0100 part c, the ARCHITECTURE.md refactor map): the
// ground-decal pass's shared geometry and the world-space cue streams —
// team rings, selection rings — that feed the shared decal buffer born in
// `pipeline/` (the AI-Handoff §9 ordering gotcha: one shared,
// order-dependent stream, shadows+ghost first, rings after).
use bytemuck::{Pod, Zeroable};

use super::entities::{fallback_shape, team_color};
use super::{KindShape, DECAL_LIFT};

/// The ground decal pass: instanced, alpha-blended quads laid flat on the
/// ground plane — blob shadows under everything, selection rings under the
/// selected. Depth-tested with writes off and a hair of lift so the decals
/// ride over the terrain and fog without z-fighting.
///
/// Each instance carries a `mode`: flat decals (rings, the ghost) draw
/// their color as-is, while the soft-radial mode fades alpha from the quad
/// center to its rim — blob shadows with soft edges instead of hard
/// stencil-circle stamps.
pub(super) const DECAL_SHADER_SRC: &str = r#"
struct CameraUniform { view_projection: mat4x4<f32> };
@group(0) @binding(0) var<uniform> camera: CameraUniform;

struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) color: vec4<f32>, @location(1) uv: vec2<f32>, @location(2) mode: f32 };

@vertex fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) instance_position: vec3<f32>,
    @location(2) instance_scale: vec2<f32>,
    @location(3) instance_color: vec4<f32>,
    @location(4) instance_mode: f32,
) -> VsOut {
    var out: VsOut;
    let world = vec3<f32>(corner.x * instance_scale.x, 0.0, corner.y * instance_scale.y) + instance_position;
    out.clip = camera.view_projection * vec4<f32>(world, 1.0);
    out.color = instance_color;
    out.uv = corner;
    out.mode = instance_mode;
    return out;
}

@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Soft-radial mode: alpha fades from full at the center to nothing at
    // the quad rim. Flat mode (rings, ghost) passes straight through.
    let falloff = mix(1.0, 1.0 - smoothstep(0.10, 0.5, length(in.uv)), in.mode);
    return vec4<f32>(in.color.rgb, in.color.a * falloff);
}
"#;

/// One ground decal instance: center (y = the lift above the terrain),
/// extent on the two ground axes, premultiplied-alpha-friendly RGBA, and a
/// decal mode (flat vs soft-radial — see the shader).
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct DecalInstance {
    pub(super) position: [f32; 3],
    pub(super) scale: [f32; 2],
    pub(super) color: [f32; 4],
    pub(super) mode: f32,
}

/// Decal mode: the color draws as-is (rings, the placement ghost).
pub(super) const DECAL_FLAT: f32 = 0.0;
/// Decal mode: alpha fades to nothing at the quad rim (blob shadows).
pub(super) const DECAL_SOFT_RADIAL: f32 = 1.0;

/// The unit quad's corners in the XZ plane (two triangles), extent ±0.5.
pub(super) fn quad_vertices() -> Vec<[f32; 2]> {
    let corners = [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]];
    let (a, b, c, d) = (corners[0], corners[1], corners[2], corners[3]);
    vec![a, b, c, a, c, d]
}

/// The unit ring (annulus) in the XZ plane: 24 segments between the inner
/// and outer radius. Instanced with the same decal pipeline, so the
/// selection ring is a ground-projected ellipse like a shadow, not a
/// screen-space bracket.
pub(super) fn ring_vertices(segments: usize, inner: f32, outer: f32) -> Vec<[f32; 2]> {
    let mut vertices = Vec::with_capacity(segments * 6);
    let point = |angle: f32, radius: f32| [radius * angle.cos(), radius * angle.sin()];
    for segment in 0..segments {
        let a0 = (segment as f32) / segments as f32 * std::f32::consts::TAU;
        let a1 = ((segment + 1) as f32) / segments as f32 * std::f32::consts::TAU;
        let i0 = point(a0, inner);
        let i1 = point(a1, inner);
        let o0 = point(a0, outer);
        let o1 = point(a1, outer);
        vertices.extend_from_slice(&[i0, o0, o1, i0, o1, i1]);
    }
    vertices
}

/// The selection ring color (own-team green — Generals convention).
pub(super) const RING_COLOR: [f32; 4] = [0.35, 1.0, 0.45, 0.85];

/// The team ring's alpha — strong enough to read the owner at a glance,
/// quieter than the bright green selection ring that draws over it.
pub(super) const TEAM_RING_ALPHA: f32 = 0.5;

/// Builds the team-color ground ring stream (PLAN-M10.2 §2.2): one ring
/// under every *unit* silhouette, in the owner's team color — the
/// at-a-glance owner read that survives zoom-out. Structures identify by
/// their band parts instead; ore nodes are neutral and get none.
pub(super) fn build_team_rings(
    snapshot: &pandemonium_engine::RenderSnapshot,
    kind_specs: &[KindShape],
    capacity: usize,
) -> Vec<DecalInstance> {
    let mut decals = Vec::with_capacity(snapshot.entities.len());
    for entity in &snapshot.entities {
        if decals.len() >= capacity {
            break;
        }
        let shape = kind_specs
            .get(entity.kind.0 as usize)
            .cloned()
            .unwrap_or_else(fallback_shape);
        if !shape.unit {
            continue;
        }
        let team = team_color(entity.owner, false, false);
        let radius = shape.radius + 0.04;
        decals.push(DecalInstance {
            position: [entity.pos.x, DECAL_LIFT * 1.5, entity.pos.z],
            scale: [radius * 2.0, radius * 2.0],
            color: [team[0], team[1], team[2], TEAM_RING_ALPHA],
            mode: DECAL_FLAT,
        });
    }
    decals
}

/// Builds the selection ring stream: one ground ellipse around every
/// selected entity, sized to its silhouette.
pub(super) fn build_rings(
    snapshot: &pandemonium_engine::RenderSnapshot,
    selection: &[pandemonium_sim_api::EntityId],
    kind_specs: &[KindShape],
    capacity: usize,
) -> Vec<DecalInstance> {
    let mut decals = Vec::with_capacity(selection.len());
    if decals.len() >= capacity {
        return decals;
    }
    for entity in &snapshot.entities {
        if !selection.contains(&entity.id) {
            continue;
        }
        if decals.len() >= capacity {
            break;
        }
        let shape = kind_specs
            .get(entity.kind.0 as usize)
            .cloned()
            .unwrap_or_else(fallback_shape);
        let radius = shape.radius + 0.12;
        decals.push(DecalInstance {
            position: [entity.pos.x, DECAL_LIFT * 2.0, entity.pos.z],
            scale: [radius * 2.0, radius * 2.0],
            color: RING_COLOR,
            mode: DECAL_FLAT,
        });
    }
    decals
}
