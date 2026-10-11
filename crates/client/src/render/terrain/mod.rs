// `terrain/` (ADR-0100 part c, the ARCHITECTURE.md refactor map): the terrain
// mesh's GPU representation and the fog decal — the two terrain-shaped
// shaders, the GPU vertex copy, the fog uniform, and the fog alpha consts.
use bytemuck::{Pod, Zeroable};
use pandemonium_engine::mesh::TerrainVertex;

pub(super) const TERRAIN_SHADER: &str = r#"
struct CameraUniform { view_projection: mat4x4<f32> };
@group(0) @binding(0) var<uniform> camera: CameraUniform;

const SUN: vec3<f32> = vec3<f32>(0.45, 0.85, 0.30);

struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) color: vec3<f32> };

@vertex fn vs_main(@location(0) position: vec3<f32>, @location(1) color: vec3<f32>, @location(2) normal: vec3<f32>) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_projection * vec4<f32>(position, 1.0);
    let light = 0.52 + 0.48 * max(dot(normalize(normal), normalize(SUN)), 0.0);
    out.color = color * light;
    return out;
}

@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
"#;

/// The fog-of-war pass (plan §11.2's fog visualization): the terrain geometry
/// drawn a second time, UV-mapped in world space (x/z over the map extent) and
/// sampling a per-tile fog-alpha texture the client refreshes from its
/// `PlayerView`. Hidden tiles read nearly opaque, explored tiles half-lit,
/// visible tiles clear. Same geometry as the terrain means the same depth
/// values — the pass depth-tests `LessEqual` with writes off, so the fog hugs
/// the hills without z-fighting.
pub(super) const FOG_SHADER_SRC: &str = r#"
struct FogUniform { view_projection: mat4x4<f32>, map_size: vec2<f32> };
@group(0) @binding(0) var<uniform> fog_camera: FogUniform;
@group(0) @binding(1) var fog_sampler: sampler;
@group(0) @binding(2) var fog_tex: texture_2d<f32>;

struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex fn vs_main(@location(0) position: vec3<f32>, @location(1) _color: vec3<f32>) -> VsOut {
    var out: VsOut;
    out.clip = fog_camera.view_projection * vec4<f32>(position, 1.0);
    out.uv = position.xz / fog_camera.map_size;
    return out;
}

@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let coverage = textureSample(fog_tex, fog_sampler, in.uv).r;
    return vec4<f32>(0.012, 0.02, 0.04, coverage);
}
"#;

/// The GPU-side copy of a terrain vertex (engine vertices are plain data;
/// the Pod representation is the renderer's concern, and bytemuck is a
/// client-only dependency — plan §3.2).
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct GpuTerrainVertex {
    position: [f32; 3],
    color: [f32; 3],
    normal: [f32; 3],
}

impl From<TerrainVertex> for GpuTerrainVertex {
    fn from(vertex: TerrainVertex) -> Self {
        Self {
            position: vertex.position,
            color: vertex.color,
            normal: vertex.normal,
        }
    }
}

/// The fog pass's uniform: the camera's view-projection plus the map extent
/// the world-space UV mapping divides by.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct FogUniform {
    pub(super) view_projection: [[f32; 4]; 4],
    pub(super) map_size: [f32; 2],
    pub(super) _pad: [f32; 2],
}

/// Fog alpha per tile, encoded into the R8 texture (0.0 = clear over
/// visible ground, 1.0 = fully dark over never-seen ground).
pub const FOG_ALPHA_VISIBLE: u8 = 0;
pub const FOG_ALPHA_EXPLORED: u8 = 130;
pub const FOG_ALPHA_HIDDEN: u8 = 235;
