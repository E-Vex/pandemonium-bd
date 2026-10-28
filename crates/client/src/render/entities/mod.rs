// `entities/` (ADR-0100 part c, the ARCHITECTURE.md refactor map): the
// instanced boxes — the silhouette pass, the team palette, the shadow blob
// stream, and the death fades — and, since M12 phase 3, the model pass's
// per-kind instance stream with its orientation seam (ADR-0101 §6).
// `EntityInstance` and the entity shader feed the entity pipeline;
// `ModelInstance` and the model shader feed the model pipeline;
// `PlacementGhost` and `DyingCue` are the client's per-frame cues into it.
use bytemuck::{Pod, Zeroable};

use pandemonium_sim_api::{MoveState, PlayerId};

use super::overlay::DECAL_SOFT_RADIAL;
use super::{DecalInstance, KindShape, DECAL_LIFT};

/// One world tile, in meters (art bible §6, ADR-0101 §6, A-145): models
/// author at 1 glTF unit = 1 m (the glTF spec's own unit) and this is the
/// one named constant that converts between their meters and the renderer's
/// tile-based world (terrain vertices at integer tile corners, silhouette
/// parts in tile units — no camera or terrain change rides along). The
/// model pass divides model-space meters by this at upload; nothing else
/// ever multiplies a world unit by it.
pub const METERS_PER_TILE: f32 = 3.0;

/// The fixed glTF-to-entity basis bake (ADR-0101 §6, the "−Z → forward"
/// mapping): glTF authors models right-handed (+Y up, −Z forward, +X right),
/// while the renderer's world is x-east / y-up / z-north with an entity's
/// facing at `yaw = atan2(facing.z, facing.x)` (so yaw 0 faces east, yaw 90°
/// faces north). Baking `(x, y, z) → (−z, y, x)` is the rotation that puts a
/// model's forward (−Z) on the yaw-0 facing (+X, east) and its right (+X) on
/// +Z — exactly where a box silhouette carries its right-hand parts (right =
/// forward × up, the convention `build_entity_instances` maps offsets with).
/// It is a proper rotation (determinant +1), never a mirror: winding and
/// normals survive intact. Applied once per model at GPU upload, so the
/// per-instance shader math stays a plain yaw about Y.
pub(super) fn bake_gltf_to_entity(vertex: [f32; 3]) -> [f32; 3] {
    [-vertex[2], vertex[1], vertex[0]]
}

/// Rotates a baked model-space vector by an entity's yaw — the *transposed*
/// (correct-handed) convention, the same math `build_entity_instances` uses
/// to map a part's local +x onto the facing direction: at yaw 0 the vector
/// is unchanged (facing east), at yaw 90° local +x lands on +z (facing
/// north). The box shader rotates its unit cubes with the mirrored form
/// (`c·x + s·z, −s·x + c·z`); models are authored geometry, not generated
/// boxes, so the model shader uses this form — the direction-pinned tests
/// below pin both ends of that difference so a copy-paste between the two
/// conventions cannot pass silently.
#[allow(dead_code)] // the pure twin of the model shader's inline yaw math — the
                    // shader carries the live copy; the direction-pinned tests
                    // pin this one (M9.1: presentation direction needs named
                    // tests, and WGSL cannot be the test's subject)
pub(super) fn entity_yaw_rotate(yaw: f32, vertex: [f32; 3]) -> [f32; 3] {
    let (sin, cos) = yaw.sin_cos();
    [
        cos * vertex[0] - sin * vertex[2],
        vertex[1],
        sin * vertex[0] + cos * vertex[2],
    ]
}

/// One entity instance: a rotated, non-uniformly-scaled colored box (48
/// bytes). One *entity* renders as one or more instances — its silhouette
/// (per [`KindShape`]) is composed from boxes on the CPU side.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct EntityInstance {
    /// Instance center (the box's bottom sits at this height).
    pub(super) position: [f32; 3],
    /// Yaw rotation around Y (units face their movement direction).
    pub(super) yaw: f32,
    /// Full extent on each axis (not half — scales read in tile units).
    pub(super) scale: [f32; 3],
    /// Placeholder color (team color, brighter when selected).
    pub(super) color: [f32; 3],
    /// The entity skin slot this instance draws with (0 = the built-in
    /// white placeholder skin; a custom pack's PNG occupies 1..). CPU-side
    /// only — the shader reads it to pick the placeholder vs. textured
    /// fragment path, but the slot never selects GPU resources by itself:
    /// the frame pass partitions the stream by slot so one draw binds one
    /// skin. Sits where the old `_pad` lived, keeping the 48-byte layout.
    pub(super) skin: u32,
    /// 1.0 selects the procedural stone surface; 0.0 keeps painted metal.
    pub(super) material: f32,
}

pub(super) const ENTITY_SHADER_SRC: &str = r#"
struct CameraUniform { view_projection: mat4x4<f32>, eye: vec3<f32>, time: f32, map_size: vec2<f32> };
@group(0) @binding(0) var<uniform> camera: CameraUniform;

// The entity skin: one shared sampler + one texture per draw, bound at
// group(1) by the frame pass. Placeholder draws bind the built-in white
// 1x1 skin — sampling it multiplies by exactly 1.0, so the placeholder
// path is byte-identical to the pre-texture renderer. A custom pack's
// kinds bind their uploaded PNG instead and the texel modulates the lit
// team color (fully transparent texels are cut away).
@group(1) @binding(0) var skin_sampler: sampler;
@group(1) @binding(1) var skin_tex: texture_2d<f32>;

const SUN: vec3<f32> = vec3<f32>(0.45, 0.85, 0.30);
// Distance haze, mirroring the terrain shader's constants: entities far
// from the eye dissolve into the same void color the ground does, so the
// whole scene fades as one at the overview zoom instead of the map alone.
const HAZE_COLOR: vec3<f32> = vec3<f32>(0.04, 0.06, 0.09);
const HAZE_START: f32 = 50.0;
const HAZE_END: f32 = 130.0;

fn haze(world: vec3<f32>) -> f32 {
    return smoothstep(HAZE_START, HAZE_END, distance(world, camera.eye)) * 0.85;
}

struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) color: vec3<f32>, @location(1) world: vec3<f32>, @location(2) normal: vec3<f32>, @location(3) material: f32, @location(4) uv: vec2<f32>, @location(5) skin: u32 };

@vertex fn vs_main(
    @location(0) corner: vec3<f32>,
    @location(1) instance_position: vec3<f32>,
    @location(2) instance_yaw: f32,
    @location(3) instance_scale: vec3<f32>,
    @location(4) instance_color: vec3<f32>,
    @location(5) normal: vec3<f32>,
    @location(6) instance_material: f32,
    @location(7) corner_uv: vec2<f32>,
    @location(8) instance_skin: u32,
) -> VsOut {
    var out: VsOut;
    let c = cos(instance_yaw);
    let s = sin(instance_yaw);
    let rot = mat3x3<f32>(vec3<f32>(c, 0.0, -s), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(s, 0.0, c));
    let world = rot * (corner * instance_scale) + instance_position;
    // Normals are authored on the unscaled cube, so inverse-scale before
    // rotating. This keeps the soft corner shading correct on long, thin
    // silhouette details as well as on the main body parts.
    let n = rot * normalize(normal / instance_scale);
    // Hemisphere lighting, matched to the terrain pass: cool sky above,
    // warm bounce below, warm sun on top. Tops stay near-full brightness so
    // team colors read at a glance, while sun-turned sides drop toward the
    // ambient floor — box silhouettes gain the volume the flat one-lamp
    // light flattened away.
    let up = n.y * 0.5 + 0.5;
    let sky = vec3<f32>(0.46, 0.50, 0.58);
    let bounce = vec3<f32>(0.30, 0.27, 0.22);
    let ambient = mix(bounce, sky, up);
    let sun = vec3<f32>(1.00, 0.94, 0.80) * max(dot(n, normalize(SUN)), 0.0) * 0.62;
    out.clip = camera.view_projection * vec4<f32>(world, 1.0);
    out.color = instance_color * (ambient + sun);
    out.world = world;
    out.normal = n;
    out.material = instance_material;
    out.uv = corner_uv;
    out.skin = instance_skin;
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
    var color = in.color;
    // The skin sample happens unconditionally (WGSL requires textureSample
    // in uniform control flow); the per-instance slot then picks which
    // fragment path consumes it.
    let texel = textureSample(skin_tex, skin_sampler, in.uv);
    if (in.skin == 0u) {
        // The placeholder path: exactly the pre-texture math. The white
        // texel is ignored, so a pack-less client renders identically to
        // the pure-silhouette renderer.
        if in.material > 0.5 {
            // Triplanar stone grain keeps mortar lines aligned on roofs and walls.
            let weights = pow(abs(normalize(in.normal)), vec3<f32>(4.0));
            let total = max(weights.x + weights.y + weights.z, 0.001);
            let uv = (in.world.zy * weights.x + in.world.xz * weights.y + in.world.xy * weights.z) / total;
            let stone = value_noise(uv * 5.0);
            let tile = uv * vec2<f32>(2.0, 3.0);
            let row = floor(tile.y);
            let staggered = tile + vec2<f32>(fract(row * 0.5) * 0.5, 0.0);
            let edge = min(min(fract(staggered.x), 1.0 - fract(staggered.x)),
                           min(fract(staggered.y), 1.0 - fract(staggered.y)));
            let mortar = 1.0 - smoothstep(0.025, 0.075, edge);
            color *= 0.86 + stone * 0.20;
            color = mix(color, color * vec3<f32>(0.48, 0.50, 0.53), mortar * 0.75);
        }
    } else {
        // A custom skin: fully transparent texels are cut away (alpha
        // cutout — the entity pipeline writes opaque pixels), and the
        // texel modulates the lit team color so ownership still reads.
        if (texel.a < 0.5) {
            discard;
        }
        color = color * texel.rgb;
    }
    // View-dependent shading, from the camera's own eye position:
    //
    // A Blinn-Phong sun glint separates materials the diffuse light cannot —
    // painted hulls and tools (material 0) carry a tight, bright highlight,
    // while masonry (material 1) only sheens faintly and broadly — and a
    // cool rim light lifts the silhouettes' grazing edges off the ground so
    // dark-colored parts stop melting into shadowed terrain.
    let view = normalize(camera.eye - in.world);
    let metal = 1.0 - step(0.5, in.material);
    let half_vector = normalize(normalize(SUN) + view);
    let spec = pow(max(dot(in.normal, half_vector), 0.0), mix(10.0, 42.0, metal))
        * mix(0.08, 0.38, metal);
    color += vec3<f32>(1.0, 0.96, 0.88) * spec;
    let fresnel = pow(1.0 - clamp(dot(in.normal, view), 0.0, 1.0), 3.0);
    color += vec3<f32>(0.30, 0.38, 0.52) * fresnel * 0.16;
    return vec4<f32>(mix(color, HAZE_COLOR, haze(in.world)), 1.0);
}
"#;

pub(super) const MODEL_SHADER_SRC: &str = r#"
// The model pass (M12 phase 3, ADR-0101): instanced per-primitive draws of
// loaded glTF static meshes. Everything per-model lives in the vertex data
// (positions already baked into tiles — METERS_PER_TILE divided, the
// glTF-to-entity basis applied, the manifest's scale/rotation_deg/
// ground_offset baked once at upload); everything per-entity is the instance
// stream (position, yaw, team color); everything per-material rides the
// per-vertex base color and the group(1) texture — the skin path's
// discipline, generalized: factor materials bind the built-in white 1x1
// skin, textured materials bind their own uploaded PNG.
struct CameraUniform { view_projection: mat4x4<f32>, eye: vec3<f32>, time: f32, map_size: vec2<f32> };
@group(0) @binding(0) var<uniform> camera: CameraUniform;

@group(1) @binding(0) var skin_sampler: sampler;
@group(1) @binding(1) var skin_tex: texture_2d<f32>;

const SUN: vec3<f32> = vec3<f32>(0.45, 0.85, 0.30);
const HAZE_COLOR: vec3<f32> = vec3<f32>(0.04, 0.06, 0.09);
const HAZE_START: f32 = 50.0;
const HAZE_END: f32 = 130.0;

fn haze(world: vec3<f32>) -> f32 {
    return smoothstep(HAZE_START, HAZE_END, distance(world, camera.eye)) * 0.85;
}

struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) color: vec3<f32>, @location(1) world: vec3<f32>, @location(2) normal: vec3<f32>, @location(3) team_tint: f32, @location(4) uv: vec2<f32> };

@vertex fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) base_color: vec4<f32>,
    @location(4) instance_position: vec3<f32>,
    @location(5) instance_yaw: f32,
    @location(6) instance_color: vec3<f32>,
) -> VsOut {
    var out: VsOut;
    // The entity yaw rotation in its correct-handed form — the same math as
    // entities::entity_yaw_rotate (the pure Rust twin the direction-pinned
    // tests pin), NOT the box shader's mirrored form: authored geometry must
    // turn toward its facing, not away from it.
    let c = cos(instance_yaw);
    let s = sin(instance_yaw);
    let world = vec3<f32>(c * position.x - s * position.z, position.y, s * position.x + c * position.z) + instance_position;
    let n = vec3<f32>(c * normal.x - s * normal.z, normal.y, s * normal.x + c * normal.z);
    // The entity pass's hemisphere + sun lighting, verbatim, so a model and
    // the boxes beside it shade as one scene.
    let up = n.y * 0.5 + 0.5;
    let sky = vec3<f32>(0.46, 0.50, 0.58);
    let bounce = vec3<f32>(0.30, 0.27, 0.22);
    let ambient = mix(bounce, sky, up);
    let sun = vec3<f32>(1.00, 0.94, 0.80) * max(dot(n, normalize(SUN)), 0.0) * 0.62;
    // The team-color multiply (ADR-0101 §3): the `team_color` material's
    // base color (authored white) times the instance's player color; every
    // other material keeps its own albedo.
    let tint = mix(vec3<f32>(1.0, 1.0, 1.0), instance_color, base_color.a);
    out.color = base_color.rgb * tint * (ambient + sun);
    out.world = world;
    out.normal = n;
    out.team_tint = base_color.a;
    out.uv = uv;
    // The one write A-018 phase 3 forgot — and with it the whole pass: a
    // `var out: VsOut` zero-fills every member, so an unwritten
    // `@builtin(position)` ships clip (0,0,0,0) for every vertex, every
    // triangle degenerates onto the clip-space origin, and the model pass
    // rasterizes nothing at all — silently, for every kind, on every
    // backend (the owner's A-020 report: the ore node shows only its
    // shadow blob, the rifleman only its team ring). The regression test
    // (`every_shader_output_member_is_written`) now refuses any embedded
    // shader whose vertex entry point leaves a declared output unwritten.
    out.clip = camera.view_projection * vec4<f32>(world, 1.0);
    return out;
}

@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var color = in.color;
    // The skin path's unconditional sample + alpha cutout: factor materials
    // bind the white 1x1 placeholder (the texel is 1.0 everywhere, nothing
    // is ever cut), a textured material's fully transparent texels are cut
    // away exactly like a skinned box's.
    let texel = textureSample(skin_tex, skin_sampler, in.uv);
    if (texel.a < 0.5) {
        discard;
    }
    color = color * texel.rgb;
    // The entity pass's view-dependent shading, matched (a model is a
    // mixed-material object, so a single mid specular replaces the box
    // pass's metal/mason split).
    let view = normalize(camera.eye - in.world);
    let half_vector = normalize(normalize(SUN) + view);
    let spec = pow(max(dot(in.normal, half_vector), 0.0), 24.0) * 0.20;
    color += vec3<f32>(1.0, 0.96, 0.88) * spec;
    let fresnel = pow(1.0 - clamp(dot(in.normal, view), 0.0, 1.0), 3.0);
    color += vec3<f32>(0.30, 0.38, 0.52) * fresnel * 0.16;
    return vec4<f32>(mix(color, HAZE_COLOR, haze(in.world)), 1.0);
}
"#;

/// The team palette (PLAN-M10.2 §2.2, A-102): P0 blue, P1 orange — the
/// colorblind-safe pair the owner picked — and gold for the neutral slot.
/// The mapping is presentation-only: content files and the content hash
/// never move. The minimap dot table mirrors these (render.rs keeps both
/// in one place).
pub const TEAM_P0: [f32; 3] = [0.22, 0.45, 0.92];
pub const TEAM_P1: [f32; 3] = [0.95, 0.55, 0.15];
pub const TEAM_NEUTRAL: [f32; 3] = [0.78, 0.66, 0.28];

/// Team colors for the placeholder boxes (player slots 0 and 1, neutral).
/// M9 (plan §11.5, visual feedback): a flashing entity — one hit in the
/// last few presented frames — is pushed toward a hot white so a landed
/// hit reads on the frame it happens. Selection brightening still wins
/// when both apply (the player's own click feedback takes precedence).
pub(super) fn team_color(owner: PlayerId, selected: bool, flashing: bool) -> [f32; 3] {
    let base = match owner {
        PlayerId(0) => TEAM_P0,
        PlayerId(1) => TEAM_P1,
        _ => TEAM_NEUTRAL, // Neutral (ore nodes).
    };
    if selected {
        [base[0] + 0.35, base[1] + 0.35, base[2] + 0.35]
    } else if flashing {
        [base[0] + 0.55, base[1] + 0.45, base[2] + 0.40]
    } else {
        base
    }
}

/// Builds one entity's instances — the silhouette pass (pure geometry, so it
/// is unit-testable without a GPU). Each [`Silhouette`] part becomes one
/// instance: the part's local offset rotates with the entity's facing, the
/// part's tone resolves against the team palette, `moving` squats whole
/// unit silhouettes, and selection/hit feedback brightens everything.
pub(super) fn build_entity_instances(
    entity: &pandemonium_engine::RenderEntity,
    shape: &KindShape,
    selected: bool,
    flashing: bool,
    skin: u32,
    out: &mut Vec<EntityInstance>,
) {
    let base = team_color(entity.owner, selected, flashing);
    let x = entity.pos.x;
    let z = entity.pos.z;
    let yaw = if entity.facing.length_squared() > 0.25 {
        entity.facing.z.atan2(entity.facing.x)
    } else {
        0.0
    };
    // A squatting unit is a moving unit (the M3..M9 half-extent cue, kept
    // and carried into the multi-part world: the whole silhouette squats,
    // offsets included, so the parts stay glued together).
    let squat = if shape.unit && entity.move_state == MoveState::Moving {
        0.85
    } else {
        1.0
    };
    let (cos, sin) = (yaw.cos(), yaw.sin());
    for part in &shape.parts {
        // The offset rotates with the facing so "forward" parts (barrels,
        // tools, noses) point where the entity faces. The instance shader
        // rotates a box by yaw as (c*x + s*z, y, -s*x + c*z) — the mirrored
        // convention — so the offset uses the transposed form to map local
        // +x onto the facing direction, while the box itself takes the
        // plain (entity yaw + the part's own tilt) the shader expects.
        let local = part.offset;
        let world_x = cos * local[0] - sin * local[2];
        let world_z = sin * local[0] + cos * local[2];
        out.push(EntityInstance {
            position: [x + world_x, local[1] * squat, z + world_z],
            yaw: yaw + part.yaw,
            scale: [part.scale[0], part.scale[1] * squat, part.scale[2]],
            color: crate::silhouette::part_color(part.tone, base),
            skin,
            material: if part.tone == crate::silhouette::Tone::Stone {
                1.0
            } else {
                0.0
            },
        });
    }
}

/// The defensive fallback for a kind id outside the specs (never used with
/// real content — the plain unit box).
pub(super) fn fallback_shape() -> KindShape {
    crate::silhouette::capability_fallback(None, false, false, false)
}

/// Builds the whole frame's instance stream (ascending entity order — the
/// snapshot's contract) under the instance buffer's capacity. `skin_slots`
/// is the per-kind skin lookup (aligned with `kind_specs` by index); kinds
/// without one render with the placeholder skin, slot 0.
/// `kind_renders_model` (same alignment) names the kinds the model pass
/// carries — their boxes are skipped here so one entity never draws twice
/// (M12 phase 3: the model replaces the silhouette, the way a skin replaced
/// the painted box for its kind). Dying entities are the deliberate
/// exception: their fade is always a placeholder mass, whatever the kind
/// rendered in life.
pub(super) fn build_instances(
    snapshot: &pandemonium_engine::RenderSnapshot,
    selection: &[pandemonium_sim_api::EntityId],
    flashes: &[pandemonium_sim_api::EntityId],
    kind_specs: &[KindShape],
    skin_slots: &[u32],
    kind_renders_model: &[bool],
    capacity: usize,
) -> Vec<EntityInstance> {
    let mut instances = Vec::with_capacity(snapshot.entities.len() * 2);
    for entity in &snapshot.entities {
        if instances.len() >= capacity {
            break; // an over-full frame clips its tail, never panics
        }
        if kind_renders_model
            .get(entity.kind.0 as usize)
            .copied()
            .unwrap_or(false)
        {
            continue; // the model pass draws this kind (see build_model_instances)
        }
        let shape = kind_specs
            .get(entity.kind.0 as usize)
            .cloned()
            .unwrap_or_else(fallback_shape);
        let skin = skin_slots.get(entity.kind.0 as usize).copied().unwrap_or(0);
        let selected = selection.contains(&entity.id);
        let flashing = flashes.contains(&entity.id);
        let before = instances.len();
        build_entity_instances(entity, &shape, selected, flashing, skin, &mut instances);
        // A single entity's silhouette always fits whole: trim back to the
        // capacity boundary if its last block overflowed.
        if instances.len() > capacity {
            instances.truncate(before);
        }
    }
    instances
}

/// Groups one frame's instance stream by skin slot: each slot's instances
/// land in one contiguous run of the returned stream, with the returned
/// `(slot, start, count)` draw list covering them. Stable within a slot —
/// snapshot order is preserved — because the buckets scatter in arrival
/// order; cross-slot order carries no visual meaning (opaque,
/// depth-tested geometry). Slots beyond `skin_count` clamp into the last
/// slot so a corrupt stream can never index past the bind groups. Pure
/// CPU-side geometry, so the grouping law is testable without a GPU.
pub(super) fn group_by_skin(
    instances: Vec<EntityInstance>,
    skin_count: usize,
) -> (Vec<EntityInstance>, Vec<(usize, u64, u64)>) {
    let skin_count = skin_count.max(1); // always at least the placeholder skin
    let mut buckets: Vec<Vec<EntityInstance>> = (0..skin_count).map(|_| Vec::new()).collect();
    for instance in instances {
        let slot = (instance.skin as usize).min(skin_count - 1);
        buckets[slot].push(instance);
    }
    let mut skin_draws: Vec<(usize, u64, u64)> = Vec::new(); // (slot, start, count)
    let mut ordered: Vec<EntityInstance> = Vec::with_capacity(buckets.iter().map(Vec::len).sum());
    for (slot, bucket) in buckets.iter().enumerate() {
        if !bucket.is_empty() {
            skin_draws.push((slot, ordered.len() as u64, bucket.len() as u64));
        }
        ordered.extend_from_slice(bucket);
    }
    (ordered, skin_draws)
}

/// One model instance (M12 phase 3, ADR-0101): everything the per-primitive
/// model draws need per entity — 28 bytes, all `f32`, no padding. The
/// per-primitive data (vertices, material albedo, team-tint flag) lives in
/// the model's own buffers; this is the per-entity half of the instancing:
/// where the model stands (tiles), which way it faces (the entity yaw), and
/// the lit team color the `team_color` material multiplies by.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct ModelInstance {
    /// The entity's ground position (tiles; the frame pass lifts it onto
    /// the display terrain like every box instance).
    pub(super) position: [f32; 3],
    /// The entity's yaw (`atan2(facing.z, facing.x)` — see
    /// [`entity_yaw_rotate`]).
    pub(super) yaw: f32,
    /// The team color (brightened when selected or flashing — the same
    /// [`team_color`] the boxes use) for the `team_color` material's
    /// multiply; non-team materials ignore it.
    pub(super) color: [f32; 3],
}

/// Builds the model pass's instance stream, grouped by kind: every entity
/// whose kind renders a model contributes exactly one instance (a model is
/// one mesh, not a composition — the silhouette pass's per-part boxes have
/// no model-pass equivalent), and each kind's instances land in one
/// contiguous run so the frame pass can draw each of the kind's model
/// primitives over exactly that range. Ascending snapshot order within a
/// kind (the snapshot's contract, same source as the silhouette pass —
/// ADR-0200 §4's invariant); the whole stream respects `capacity` and an
/// over-full frame clips its tail entity by entity, never half-drawing a
/// kind's run. `kind_renders_model` is the per-kind model lookup (aligned
/// with `kind_specs` by index); kinds without one never reach this stream —
/// their boxes carry them. Models do not squat when moving (the box pass's
/// half-extent cue would distort authored geometry; motion is the animation
/// ADR's business, ADR-0101 §2).
pub(super) fn build_model_instances(
    snapshot: &pandemonium_engine::RenderSnapshot,
    selection: &[pandemonium_sim_api::EntityId],
    flashes: &[pandemonium_sim_api::EntityId],
    kind_renders_model: &[bool],
    capacity: usize,
) -> (Vec<ModelInstance>, Vec<(usize, u64, u64)>) {
    let mut buckets: Vec<Vec<ModelInstance>> =
        kind_renders_model.iter().map(|_| Vec::new()).collect();
    let mut written = 0usize;
    for entity in &snapshot.entities {
        if written >= capacity {
            break; // an over-full frame clips its tail, never panics
        }
        if !kind_renders_model
            .get(entity.kind.0 as usize)
            .copied()
            .unwrap_or(false)
        {
            continue; // a box kind (or an unknown kind) — not this pass's business
        }
        let yaw = if entity.facing.length_squared() > 0.25 {
            entity.facing.z.atan2(entity.facing.x)
        } else {
            0.0
        };
        buckets[entity.kind.0 as usize].push(ModelInstance {
            position: [entity.pos.x, entity.pos.y, entity.pos.z],
            yaw,
            color: team_color(
                entity.owner,
                selection.contains(&entity.id),
                flashes.contains(&entity.id),
            ),
        });
        written += 1;
    }
    // One contiguous run per kind that drew anything: (kind, start, count).
    let mut ordered: Vec<ModelInstance> = Vec::with_capacity(written);
    let mut draws: Vec<(usize, u64, u64)> = Vec::new();
    for (kind, bucket) in buckets.iter().enumerate() {
        if !bucket.is_empty() {
            draws.push((kind, ordered.len() as u64, bucket.len() as u64));
            ordered.extend_from_slice(bucket);
        }
    }
    (ordered, draws)
}

/// Builds the shadow blob stream: one dark ellipse under every entity,
/// sized by the silhouette's derived ground radius, soft-edged (the
/// soft-radial decal mode) so the blobs read as cast shade, not stamped
/// circles.
pub(super) fn build_shadows(
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
        let radius = shape.radius;
        decals.push(DecalInstance {
            position: [entity.pos.x, DECAL_LIFT, entity.pos.z],
            scale: [radius * 2.0, radius * 2.0],
            color: [0.0, 0.0, 0.0, 0.34],
            mode: DECAL_SOFT_RADIAL,
        });
    }
    decals
}

/// The active structure-placement ghost (plan §11.3's build placement mode
/// with a legality preview): a footprint-sized quad under the cursor, green
/// when the client preview likes the spot, red when it does not.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PlacementGhost {
    /// The footprint's center on the ground plane.
    pub center: [f32; 3],
    /// Full extent on the two ground axes (tiles).
    pub extent: [f32; 2],
    /// Whether the client-side preview considers the spot legal.
    pub legal: bool,
}

/// One dying entity, as the renderer needs it: last ground position, its
/// kind (the silhouette to shrink), its owner (the team color to char), and
/// the fade progress in 0.0 (just died) .. 1.0 (gone).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DyingCue {
    pub pos: [f32; 3],
    pub kind: pandemonium_sim_api::KindId,
    pub owner: pandemonium_sim_api::PlayerId,
    pub progress: f32,
}

/// Builds the dying entity's single instance: its silhouette's main mass
/// (the largest-volume part), shrinking to a quarter and charring toward
/// soot as `progress` advances.
pub(super) fn death_instance(cue: &DyingCue, shape: &KindShape) -> EntityInstance {
    let shrink = 1.0 - 0.75 * cue.progress.clamp(0.0, 1.0);
    let base = team_color(cue.owner, false, false);
    let charred = [
        base[0] * (1.0 - 0.85 * cue.progress) + 0.10 * cue.progress,
        base[1] * (1.0 - 0.85 * cue.progress) + 0.08 * cue.progress,
        base[2] * (1.0 - 0.85 * cue.progress) + 0.08 * cue.progress,
    ];
    let main = shape.main_part();
    let scale = [
        main.scale[0] * shrink,
        main.scale[1] * shrink,
        main.scale[2] * shrink,
    ];
    EntityInstance {
        position: [cue.pos[0], main.offset[1] * shrink, cue.pos[2]],
        yaw: 0.0,
        scale,
        color: charred,
        skin: 0, // death fades always render as the placeholder mass
        material: 0.0,
    }
}
