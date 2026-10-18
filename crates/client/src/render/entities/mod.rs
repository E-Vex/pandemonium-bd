// `entities/` (ADR-0100 part c, the ARCHITECTURE.md refactor map): the
// instanced boxes — the silhouette pass, the team palette, the shadow blob
// stream, and the death fades. `EntityInstance` and the entity shader feed
// the entity pipeline; `PlacementGhost` and `DyingCue` are the client's
// per-frame cues into it.
use bytemuck::{Pod, Zeroable};

use pandemonium_sim_api::{MoveState, PlayerId};

use super::overlay::DECAL_SOFT_RADIAL;
use super::{DecalInstance, KindShape, DECAL_LIFT};

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
    /// Padding keeps the instance fields 4-byte aligned.
    pub(super) _pad: f32,
    /// 1.0 selects the procedural stone surface; 0.0 keeps painted metal.
    pub(super) material: f32,
}

pub(super) const ENTITY_SHADER_SRC: &str = r#"
struct CameraUniform { view_projection: mat4x4<f32> };
@group(0) @binding(0) var<uniform> camera: CameraUniform;

const SUN: vec3<f32> = vec3<f32>(0.45, 0.85, 0.30);

struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) color: vec3<f32>, @location(1) world: vec3<f32>, @location(2) normal: vec3<f32>, @location(3) material: f32 };

@vertex fn vs_main(
    @location(0) corner: vec3<f32>,
    @location(1) instance_position: vec3<f32>,
    @location(2) instance_yaw: f32,
    @location(3) instance_scale: vec3<f32>,
    @location(4) instance_color: vec3<f32>,
    @location(5) normal: vec3<f32>,
    @location(6) instance_material: f32,
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
    return vec4<f32>(color, 1.0);
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
            _pad: 0.0,
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
/// snapshot's contract) under the instance buffer's capacity.
pub(super) fn build_instances(
    snapshot: &pandemonium_engine::RenderSnapshot,
    selection: &[pandemonium_sim_api::EntityId],
    flashes: &[pandemonium_sim_api::EntityId],
    kind_specs: &[KindShape],
    capacity: usize,
) -> Vec<EntityInstance> {
    let mut instances = Vec::with_capacity(snapshot.entities.len() * 2);
    for entity in &snapshot.entities {
        if instances.len() >= capacity {
            break; // an over-full frame clips its tail, never panics
        }
        let shape = kind_specs
            .get(entity.kind.0 as usize)
            .cloned()
            .unwrap_or_else(fallback_shape);
        let selected = selection.contains(&entity.id);
        let flashing = flashes.contains(&entity.id);
        let before = instances.len();
        build_entity_instances(entity, &shape, selected, flashing, &mut instances);
        // A single entity's silhouette always fits whole: trim back to the
        // capacity boundary if its last block overflowed.
        if instances.len() > capacity {
            instances.truncate(before);
        }
    }
    instances
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
        _pad: 0.0,
        material: 0.0,
    }
}
