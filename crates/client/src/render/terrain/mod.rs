// `terrain/` (ADR-0100 part c, the ARCHITECTURE.md refactor map): the terrain
// mesh's GPU representation and the fog decal — the two terrain-shaped
// shaders, the GPU vertex copy, the fog uniform, and the fog alpha consts.
use bytemuck::{Pod, Zeroable};
use pandemonium_engine::mesh::TerrainVertex;

pub(super) const TERRAIN_SHADER: &str = r#"
struct CameraUniform { view_projection: mat4x4<f32> };
@group(0) @binding(0) var<uniform> camera: CameraUniform;

const SUN: vec3<f32> = vec3<f32>(0.45, 0.85, 0.30);

struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) color: vec3<f32>, @location(1) world: vec2<f32>, @location(2) material: f32 };

@vertex fn vs_main(@location(0) position: vec3<f32>, @location(1) color: vec3<f32>, @location(2) normal: vec3<f32>, @location(3) material: f32) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_projection * vec4<f32>(position, 1.0);
    // Hemisphere lighting: a cool sky term from above, a warm soil-bounce
    // term from below, plus the warm sun. Slopes that turn away from the
    // sun drop to ambient only, so hillsides shade with real depth instead
    // of the flat one-lamp look.
    let n = normalize(normal);
    let up = n.y * 0.5 + 0.5;
    let sky = vec3<f32>(0.40, 0.44, 0.50);
    let bounce = vec3<f32>(0.30, 0.28, 0.24);
    let ambient = mix(bounce, sky, up);
    let sun = vec3<f32>(1.00, 0.94, 0.82) * max(dot(n, normalize(SUN)), 0.0) * 0.57;
    out.color = color * (ambient + sun);
    out.world = position.xz;
    out.material = material;
    return out;
}

fn hash2(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn value_noise(p: vec2<f32>) -> f32 {
    let cell = floor(p);
    let local = fract(p);
    let blend = local * local * (3.0 - 2.0 * local);
    let a = hash2(cell);
    let b = hash2(cell + vec2<f32>(1.0, 0.0));
    let c = hash2(cell + vec2<f32>(0.0, 1.0));
    let d = hash2(cell + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, blend.x), mix(c, d, blend.x), blend.y);
}

@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let broad = value_noise(in.world * 2.4);
    let fine = value_noise(in.world * 8.0);
    var color = in.color * (0.87 + broad * 0.16 + fine * 0.08);

    if in.material < 0.5 {
        // Tiny tapered grass strokes, anchored in world space so they do not
        // swim across the surface as the camera moves.
        let cell = floor(in.world * 2.5);
        let local = fract(in.world * 2.5) - vec2<f32>(0.5, 0.5);
        let angle = hash2(cell) * 6.2831853;
        let cs = cos(angle);
        let sn = sin(angle);
        let blade_xy = vec2<f32>(local.x * cs - local.y * sn, local.x * sn + local.y * cs);
        let blade_d = length(blade_xy / vec2<f32>(0.11, 0.27));
        let blade = (1.0 - smoothstep(0.45, 1.05, blade_d)) * step(0.34, hash2(cell + vec2<f32>(13.0, 7.0)));
        color = mix(color, color * vec3<f32>(0.68, 1.10, 0.62), blade * 0.72);
        // Scattered pale tips break up large fields without a repeated grid.
        let speckle = step(0.90, hash2(floor(in.world * 7.0)));
        color = mix(color, color * vec3<f32>(1.12, 1.08, 0.78), speckle * 0.18);
    } else if in.material < 1.5 {
        // Warm soil has visible grain and occasional small pebbles.
        let pebble_cell = floor(in.world * 3.2);
        let pebble_local = fract(in.world * 3.2);
        let pebble_center = vec2<f32>(
            hash2(pebble_cell + vec2<f32>(19.0, 3.0)),
            hash2(pebble_cell + vec2<f32>(5.0, 29.0))
        );
        let pebble = (1.0 - smoothstep(0.07, 0.18, length(pebble_local - pebble_center)))
            * step(0.56, hash2(pebble_cell + vec2<f32>(31.0, 11.0)));
        color = mix(color, color * vec3<f32>(0.54, 0.48, 0.40), pebble * 0.65);
        color *= 0.92 + fine * 0.13;
    } else {
        // Rocky ground reads as fractured slabs with cool shadowed joints.
        let stone_uv = in.world * vec2<f32>(1.15, 1.65);
        let staggered = stone_uv + vec2<f32>(fract(floor(stone_uv.y) * 0.5) * 0.5, 0.0);
        let joint = min(min(fract(staggered.x), 1.0 - fract(staggered.x)),
                        min(fract(staggered.y), 1.0 - fract(staggered.y)));
        let seam = 1.0 - smoothstep(0.025, 0.07, joint);
        color = mix(color, color * vec3<f32>(0.46, 0.48, 0.50), seam * 0.62);
        color *= 0.84 + fine * 0.25;
    }
    return vec4<f32>(color, 1.0);
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
    material: f32,
}

impl From<TerrainVertex> for GpuTerrainVertex {
    fn from(vertex: TerrainVertex) -> Self {
        Self {
            position: vertex.position,
            color: vertex.color,
            normal: vertex.normal,
            material: vertex.material,
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
