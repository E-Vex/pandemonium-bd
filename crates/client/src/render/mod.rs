//! The wgpu renderer (plan §11.2 as amended by ADR-0001): a depth-tested 3D
//! pass drawing the terrain mesh (from the map grid + display-only heightmap)
//! and instanced multi-part entity silhouettes, through the engine's
//! [`Renderer`] trait so the sim-facing code stays GPU-independent.
//!
//! All-procedural presentation (plan §0's no-asset law): box silhouettes over
//! a noise-detailed terrain, lit by a shared hemisphere+sun model with
//! view-dependent specular and rim shading, dissolving into the void color
//! at overview distance. Fog of war drifts gently; selection rings and the
//! placement ghost breathe. The renderer receives interpolated
//! [`RenderSnapshot`]s and camera matrices — never the simulation itself
//! (FD-6, FD-9).

use anyhow::Context;
use bytemuck::{Pod, Zeroable};
use pandemonium_engine::mesh::{TerrainMesh, TerrainVertex};
use pandemonium_engine::renderer::Frame;
use pandemonium_engine::Renderer;
use wgpu::util::DeviceExt;

use crate::pack::{AssetEntry, AssetPack};
use crate::text::{TextAtlas, UiQuad};

mod entities;
mod minimap;
// The M12 GLB pipeline (ADR-0101): `model` owns the pure-data loader
// (phase 1) and the GPU-ready conversion (phase 3) — consumed by the model
// pass below, which loads and uploads each kind's model exactly once at
// construction and only binds and draws per frame.
mod model;
mod overlay;
mod pipeline;
mod terrain;
mod texture;

use entities::{
    bake_gltf_to_entity, build_instances, build_model_instances, build_shadows, death_instance,
    fallback_shape, group_by_skin, EntityInstance, ModelInstance, METERS_PER_TILE,
};
pub use entities::{DyingCue, PlacementGhost, TEAM_P0, TEAM_P1};
use model::{ModelPlacement, ModelVertex};

use minimap::{paint_minimap_markers, MinimapVertex};
use overlay::{build_rings, build_team_rings, ring_pulse, DecalInstance, DECAL_FLAT};
use terrain::{FogUniform, GpuTerrainVertex, FOG_SHADER_SRC, TERRAIN_SHADER};
pub use terrain::{FOG_ALPHA_EXPLORED, FOG_ALPHA_HIDDEN, FOG_ALPHA_VISIBLE};

/// The camera uniform: the combined view-projection matrix, the camera's
/// world position (the view-dependent shading the entity and terrain
/// fragments need — specular glints and distance haze), and the seconds
/// since the renderer was created (the animation phase the fog's drift and
/// the decal pulses read). The two shaders that read beyond the matrix
/// mirror this struct exactly (80 bytes; WGSL's `vec3` alignment lands
/// `eye` at the same offset 64 this layout gives).
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct CameraUniform {
    view_projection: [[f32; 4]; 4],
    eye: [f32; 3],
    time: f32,
    /// Map extent in tiles; the terrain fragment divides world position by
    /// this to get world-space UVs into the pack texture. Mirrors the WGSL
    /// `CameraUniform` in both the terrain and entity shaders.
    map_size: [f32; 2],
    /// Trailing pad: WGSL std140 rounds the uniform struct up to its base
    /// alignment (16), so this keeps the Rust layout (88 bytes) matching the
    /// shader's expected 96 bytes.
    _pad: [f32; 2],
}

/// What each kind renders as: the multi-part silhouette from the pure
/// [`silhouette`] module (PLAN-M10.2 §2.1 — distinct box compositions per
/// kind, keyed by kind name, with the capability-shape fallback living in
/// `silhouette::capability_fallback`). The type alias keeps the renderer's
/// historical `KindShape` name for the per-kind spec.
pub type KindShape = crate::silhouette::Silhouette;

/// The UI overlay pass: screen-space pixels (y down) → NDC, glyph coverage
/// → alpha. Draws after the world, on top of it, with alpha blending.
const UI_SHADER_SRC: &str = r#"
struct ScreenUniform { size: vec2<f32> };
@group(0) @binding(0) var<uniform> screen: ScreenUniform;
@group(1) @binding(0) var atlas_sampler: sampler;
@group(1) @binding(1) var atlas: texture_2d<f32>;

struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) color: vec4<f32> };

@vertex fn vs_main(
    @location(0) pixels: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
) -> VsOut {
    var out: VsOut;
    let ndc = vec2<f32>(
        pixels.x / screen.size.x * 2.0 - 1.0,
        1.0 - pixels.y / screen.size.y * 2.0,
    );
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    out.color = color;
    return out;
}

@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let coverage = textureSample(atlas, atlas_sampler, in.uv).r;
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}
"#;

/// One UI vertex: screen-space pixels, atlas UV, color (32 bytes).
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct UiVertex {
    pixels: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

impl UiVertex {
    /// Six vertices per quad (two triangles, no index buffer — HUD scale).
    fn from_quad(quad: &UiQuad) -> [Self; 6] {
        let x0 = quad.x;
        let y0 = quad.y;
        let x1 = quad.x + quad.w;
        let y1 = quad.y + quad.h;
        let color = quad.color;
        let tl = Self {
            pixels: [x0, y0],
            uv: [quad.u0, quad.v0],
            color,
        };
        let tr = Self {
            pixels: [x1, y0],
            uv: [quad.u1, quad.v0],
            color,
        };
        let bl = Self {
            pixels: [x0, y1],
            uv: [quad.u0, quad.v1],
            color,
        };
        let br = Self {
            pixels: [x1, y1],
            uv: [quad.u1, quad.v1],
            color,
        };
        [tl, tr, bl, tr, br, bl]
    }
}

/// The wgpu-backed renderer: owns the GPU device, the terrain pipeline and
/// buffers, and the entity instance stream. Implements the engine's
/// [`Renderer`] trait. The UI overlay pass (HUD text + debug overlays) draws
/// the [`TextAtlas`] through [`WgpuRenderer::queue_ui`] before every frame.
pub struct WgpuRenderer {
    _window: std::sync::Arc<winit::window::Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth_view: wgpu::TextureView,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    terrain_pipeline: wgpu::RenderPipeline,
    terrain_vertex_buf: wgpu::Buffer,
    terrain_index_buf: wgpu::Buffer,
    terrain_index_count: u32,
    /// CPU copy of the display-only terrain corner heights. Entities and
    /// ground decals use it to sit on raised terrain instead of below it.
    terrain_heights: Vec<f32>,
    terrain_height_size: (u32, u32),
    entity_pipeline: wgpu::RenderPipeline,
    entity_vertex_buf: wgpu::Buffer,
    entity_instance_buf: wgpu::Buffer,
    entity_instance_capacity: u64,
    /// The uploaded entity skins, slot-indexed: slot 0 is the built-in
    /// white 1x1 placeholder; a custom pack's loadable PNGs occupy 1.. in
    /// kind order. All of these are born once in the constructor — the
    /// frame pass only rebinds them, so no PNG is decoded, uploaded, or
    /// given a bind group during rendering.
    entity_skins: Vec<texture::EntitySkin>,
    /// Per-kind skin slot (KindId → slot, aligned with `kind_specs`).
    /// Kinds without a loadable texture stay at slot 0 (the placeholder).
    kind_skin_slots: Vec<u32>,
    /// The model pass (M12 phase 3, ADR-0101): the pipeline over the same
    /// camera + skin bind layouts the entity pass uses, the shared model
    /// instance stream, and the per-kind uploaded models. Everything here
    /// is born once in the constructor — file reads, GLB parses, the
    /// meters-to-tiles conversion, GPU uploads, and bind groups — so the
    /// frame pass only binds and draws (A-146's build-once discipline).
    model_pipeline: wgpu::RenderPipeline,
    model_instance_buf: wgpu::Buffer,
    model_instance_capacity: u64,
    /// Per kind (index = KindId, aligned with `kind_specs`): its uploaded
    /// model, or `None` (the silhouette/skin box path carries it).
    kind_models: Vec<Option<UploadedModel>>,
    /// The flat per-kind model lookup the two instance builders share
    /// (`kind_models[k].is_some()`); kept as its own table so the frame
    /// pass never allocates it.
    kind_renders_model: Vec<bool>,
    atlas: TextAtlas,
    screen_buffer: wgpu::Buffer,
    screen_bind_group: wgpu::BindGroup,
    ui_pipeline: wgpu::RenderPipeline,
    ui_bind_group: wgpu::BindGroup,
    ui_vertex_buf: wgpu::Buffer,
    ui_vertex_capacity: u64,
    pending_ui: Vec<UiQuad>,
    fog_pipeline: wgpu::RenderPipeline,
    fog_uniform_buf: wgpu::Buffer,
    fog_layout: wgpu::BindGroupLayout,
    fog_sampler: wgpu::Sampler,
    fog_bind_group: wgpu::BindGroup,
    fog_texture: wgpu::Texture,
    fog_texture_size: (u32, u32),
    fog_map_size: (f32, f32),
    /// Per-kind silhouette specs (index = KindId), derived from the loaded
    /// bundle by the client (the kind-name table + capability fallback in
    /// [`crate::silhouette`]) and handed to the renderer once at startup.
    kind_specs: Vec<KindShape>,
    /// Loaded PNG texture (terrain/entity) — natural iteration: basic PNG loaded
    /// from pack. Kept alive so the pack-texture bind group's view stays valid;
    /// bound at group 1 of the terrain pipeline by [`WgpuRenderer::render`].
    #[allow(dead_code)] // stored for the renderer follow-up that consumes pack assets
    texture: Option<(wgpu::Texture, wgpu::TextureView, wgpu::Sampler)>,
    /// Pack texture bound at group 1 of the terrain pipeline (the pack texture,
    /// or a 1x1 white fallback when no pack is loaded).
    texture_bind_group: wgpu::BindGroup,
    /// The loaded client-side asset pack, if any. Presentation-only — the
    /// renderer resolves an entity's kind id through this pack for asset
    /// references: loadable `.png` entries become uploaded entity skins at
    /// startup, everything else keeps the silhouette placeholders. Never
    /// enters the simulation.
    #[allow(dead_code)] // consumed by the constructor's skin loader; kept for lookup
    pack: Option<AssetPack>,
    decal_pipeline: wgpu::RenderPipeline,
    quad_vertex_buf: wgpu::Buffer,
    quad_vertex_count: u32,
    ring_vertex_buf: wgpu::Buffer,
    ring_vertex_count: u32,
    decal_instance_buf: wgpu::Buffer,
    decal_instance_capacity: u64,
    /// Death-fade cues set by the client each frame (like the UI queue) and
    /// drained into extra, shrinking entity instances by the next render.
    dying_cues: Vec<DyingCue>,
    /// The active placement ghost (None outside placement mode). Kept until
    /// replaced — the client sets it every frame it draws.
    ghost: Option<PlacementGhost>,
    minimap_tex: wgpu::Texture,
    minimap_layout: wgpu::BindGroupLayout,
    minimap_sampler: wgpu::Sampler,
    minimap_bind_group: wgpu::BindGroup,
    minimap_pipeline: wgpu::RenderPipeline,
    minimap_vertex_buf: wgpu::Buffer,
    minimap_rect: Option<[f32; 4]>,
    /// The baked per-tile terrain base color (RGBA, map-sized row-major).
    minimap_base: Vec<[u8; 4]>,
    minimap_scratch: Vec<u8>,
    minimap_size: (u32, u32),
    /// When this renderer was created — the origin of the presentation
    /// animation clock (`CameraUniform::time`). Presentation-only wall
    /// clock: it never touches the simulation, which runs on its own ticks.
    start: std::time::Instant,
}

/// One kind's presentation as the gallery (A-020) reports it: the tier
/// the fallback chain resolved at startup, the triangles the tier draws,
/// and the drawn size in meters — the tier's own geometry's world-space
/// bounding box (the model's baked mesh bounds for the model tier; the
/// silhouette's box extents for the skin and placeholder tiers — the skin
/// paints the same boxes the placeholder draws).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KindPresentation {
    /// The tier the chain landed on for this kind.
    pub tier: crate::pack::PresentationTier,
    /// The triangles the tier's geometry carries (a box part is 12).
    pub triangles: u32,
    /// The drawn size (x, y, z extent) in meters.
    pub drawn_meters: [f32; 3],
}

impl WgpuRenderer {
    /// The glyph atlas (layout math for the caller; the texture stays here).
    pub fn atlas(&self) -> &TextAtlas {
        &self.atlas
    }

    /// Looks up a loaded asset reference for an entity kind id. Returns
    /// `None` when the pack is absent, the kind is unmapped, or the asset
    /// could not be loaded — the caller keeps using placeholder rendering.
    /// The skin loader resolves through `texture::resolve_kind_textures` at
    /// startup; this lookup stays as the renderer's public seam for
    /// diagnostics and follow-ups (terrain textures next).
    #[allow(dead_code)] // the terrain-texture follow-up consumes it; kept tested
    pub fn asset_for(&self, kind_id: &str) -> Option<&AssetEntry> {
        self.pack.as_ref().and_then(|pack| pack.asset_for(kind_id))
    }

    /// The gallery's per-kind stats (A-020): which tier the chain resolved
    /// this kind to at startup, how many triangles that tier draws, and the
    /// drawn size in meters. `None` for a kind id outside the content's
    /// table. Pure lookup over the constructor's own state — no GPU work,
    /// safe to call every frame.
    pub fn presentation_of(&self, kind: pandemonium_sim_api::KindId) -> Option<KindPresentation> {
        let index = usize::try_from(kind.0).ok()?;
        let model = self.kind_models.get(index)?;
        if let Some(model) = model {
            return Some(KindPresentation {
                tier: crate::pack::PresentationTier::Model,
                triangles: model.triangles,
                drawn_meters: model.drawn_meters,
            });
        }
        // The box tiers: the silhouette's parts are the drawn geometry —
        // 12 triangles per box part, and the drawn size is the composed
        // extents (the skin paints the placeholder's own boxes).
        let shape = self.kind_specs.get(index)?;
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for part in &shape.parts {
            for axis in 0..3 {
                // A box part's bottom sits at its offset's y (the instance
                // shader scales ±0.5 corners); x/z center on the offset.
                let (a, b) = if axis == 1 {
                    (part.offset[1], part.offset[1] + part.scale[1])
                } else {
                    (
                        part.offset[axis] - part.scale[axis] / 2.0,
                        part.offset[axis] + part.scale[axis] / 2.0,
                    )
                };
                lo[axis] = lo[axis].min(a);
                hi[axis] = hi[axis].max(b);
            }
        }
        let triangles = (shape.parts.len() as u32).saturating_mul(12);
        let drawn_meters = [
            (hi[0] - lo[0]) * METERS_PER_TILE,
            (hi[1] - lo[1]) * METERS_PER_TILE,
            (hi[2] - lo[2]) * METERS_PER_TILE,
        ];
        let tier = if self
            .kind_skin_slots
            .get(index)
            .is_some_and(|slot| *slot > 0)
        {
            crate::pack::PresentationTier::Skin
        } else {
            crate::pack::PresentationTier::Placeholder
        };
        Some(KindPresentation {
            tier,
            triangles,
            drawn_meters,
        })
    }

    /// Replaces the map-shaped GPU data when a match starts on a different
    /// map. Terrain, fog, and minimap remain presentation-only and use the
    /// selected map's dimensions and tile colors.
    pub fn set_terrain(&mut self, terrain: &TerrainMesh) {
        let vertices: Vec<GpuTerrainVertex> = terrain
            .vertices
            .iter()
            .copied()
            .map(GpuTerrainVertex::from)
            .collect();
        let terrain_vertex_buf =
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("terrain vertices"),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
        let terrain_index_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("terrain indices"),
                contents: bytemuck::cast_slice(&terrain.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
        let map_size = terrain.vertices.iter().fold([0.0f32; 2], |size, vertex| {
            [
                size[0].max(vertex.position[0]),
                size[1].max(vertex.position[2]),
            ]
        });
        let texture_size = (map_size[0] as u32, map_size[1] as u32);
        let terrain_heights = terrain_height_grid(terrain, texture_size.0, texture_size.1);
        let extent = wgpu::Extent3d {
            width: texture_size.0.max(1),
            height: texture_size.1.max(1),
            depth_or_array_layers: 1,
        };

        let fog_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fog texture"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let fog_view = fog_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let fog_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fog bind group"),
            layout: &self.fog_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.fog_uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.fog_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&fog_view),
                },
            ],
        });

        let minimap_size = (texture_size.0.max(1), texture_size.1.max(1));
        let minimap_base: Vec<[u8; 4]> = terrain
            .vertices
            .as_chunks::<4>()
            .0
            .iter()
            .map(|tile| {
                let c = tile[0].color;
                [
                    (c[0].clamp(0.0, 1.0) * 255.0) as u8,
                    (c[1].clamp(0.0, 1.0) * 255.0) as u8,
                    (c[2].clamp(0.0, 1.0) * 255.0) as u8,
                    255,
                ]
            })
            .collect();
        let minimap_scratch = vec![0; (minimap_size.0 * minimap_size.1 * 4) as usize];
        let minimap_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("minimap texture"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let minimap_view = minimap_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let minimap_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("minimap bind group"),
            layout: &self.minimap_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.screen_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.minimap_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&minimap_view),
                },
            ],
        });

        self.terrain_vertex_buf = terrain_vertex_buf;
        self.terrain_index_buf = terrain_index_buf;
        self.terrain_index_count = terrain.indices.len() as u32;
        self.terrain_heights = terrain_heights;
        self.terrain_height_size = texture_size;
        self.fog_texture = fog_texture;
        self.fog_texture_size = texture_size;
        self.fog_map_size = (map_size[0], map_size[1]);
        self.fog_bind_group = fog_bind_group;
        self.minimap_tex = minimap_tex;
        self.minimap_bind_group = minimap_bind_group;
        self.minimap_base = minimap_base;
        self.minimap_scratch = minimap_scratch;
        self.minimap_size = minimap_size;
    }

    /// Samples the same piecewise-linear surface as the terrain triangles.
    fn ground_height_at(&self, x: f32, z: f32) -> f32 {
        let (width, height) = self.terrain_height_size;
        if width == 0 || height == 0 || self.terrain_heights.is_empty() {
            return 0.0;
        }
        let x = x.clamp(0.0, width as f32 - 0.001);
        let z = z.clamp(0.0, height as f32 - 0.001);
        let tile_x = x.floor() as u32;
        let tile_z = z.floor() as u32;
        let tx = x - tile_x as f32;
        let tz = z - tile_z as f32;
        let stride = width as usize + 1;
        let corner = |dx: usize, dz: usize| {
            self.terrain_heights[(tile_z as usize + dz) * stride + tile_x as usize + dx]
        };
        let (a, b, c, d) = (corner(0, 0), corner(1, 0), corner(0, 1), corner(1, 1));
        if tx + tz <= 1.0 {
            a * (1.0 - tx - tz) + b * tx + c * tz
        } else {
            b * (1.0 - tz) + d * (tx + tz - 1.0) + c * (1.0 - tx)
        }
    }

    /// Refreshes the fog texture from one alpha byte per tile (row-major in
    /// the map's own tile order — the exact encoding of `PlayerView.fog`).
    /// The client calls this when the sim tick advanced, not every frame.
    pub fn update_fog(&mut self, fog_alpha: &[u8]) {
        let (width, height) = self.fog_texture_size;
        let expected = (width.max(1) as usize) * (height.max(1) as usize);
        if fog_alpha.len() != expected {
            return; // mismatched map — keep the old fog rather than panic
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.fog_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            fog_alpha,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width.max(1)),
                rows_per_image: Some(height.max(1)),
            },
            wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
        );
    }

    /// Queues the death-fade cues for the next [`Renderer::render`] — the
    /// same set-then-drain contract as [`WgpuRenderer::queue_ui`].
    pub fn set_death_cues(&mut self, cues: Vec<DyingCue>) {
        self.dying_cues = cues;
    }

    /// Sets (or clears) the structure-placement ghost. The client calls
    /// this every frame while placing; the ghost persists until replaced.
    pub fn set_ghost(&mut self, ghost: Option<PlacementGhost>) {
        self.ghost = ghost;
    }

    /// Rebuilds the minimap quad for a panel rectangle (window pixels).
    /// The screen-size uniform is shared with the UI pass and refreshed in
    /// `render`, so only the rect matters here.
    /// Clears the minimap panel (PLAN-M11 §2 phase 5: the hidden command
    /// bar's input fall-through — `minimap_hit` answers None until the
    /// bar returns).
    pub fn clear_minimap_rect(&mut self) {
        self.minimap_rect = None;
    }

    pub fn set_minimap_rect(&mut self, rect: [f32; 4]) {
        let (x, y, w, h) = (rect[0], rect[1], rect[2], rect[3]);
        let v = |px: f32, py: f32, u: f32, vv: f32| MinimapVertex {
            pos_uv: [px, py, u, vv],
        };
        let corners = [
            v(x, y, 0.0, 0.0),
            v(x + w, y, 1.0, 0.0),
            v(x + w, y + h, 1.0, 1.0),
            v(x, y + h, 0.0, 1.0),
        ];
        let vertices = [
            corners[0], corners[1], corners[2], corners[0], corners[2], corners[3],
        ];
        self.queue
            .write_buffer(&self.minimap_vertex_buf, 0, bytemuck::cast_slice(&vertices));
        self.minimap_rect = Some(rect);
    }

    /// Composites the minimap texture for this frame: terrain base, fog
    /// darkening (one alpha byte per tile, the PlayerView encoding), then
    /// entity dots (one per entity, team-tinted) on top.
    pub fn update_minimap(&mut self, entities: &[pandemonium_engine::RenderEntity], fog: &[u8]) {
        let (w, h) = self.minimap_size;
        let tile_count = (w * h) as usize;
        if self.minimap_base.len() != tile_count {
            return; // the terrain mesh and the map disagree — skip this frame
        }
        let scratch = &mut self.minimap_scratch;
        for tile in 0..tile_count {
            let base = self.minimap_base[tile];
            // Fog dims the terrain: never-seen tiles go near-black, explored
            // tiles half-dark, visible tiles keep their color.
            let shade = match fog.get(tile) {
                Some(&FOG_ALPHA_VISIBLE) => 1.0,
                Some(&FOG_ALPHA_EXPLORED) => 0.55,
                Some(&FOG_ALPHA_HIDDEN) => 0.22,
                _ => 1.0, // no fog row yet — show the terrain
            };
            let offset = tile * 4;
            scratch[offset] = (base[0] as f32 * shade) as u8;
            scratch[offset + 1] = (base[1] as f32 * shade) as u8;
            scratch[offset + 2] = (base[2] as f32 * shade) as u8;
            scratch[offset + 3] = 255;
        }
        // Entity markers (PLAN-M10.2 §2.4): distinct per role — units one
        // tile, buildings a solid 2×2 block, ore nodes a 3×3 amber diamond —
        // painted over the fog-shaded terrain (the entity list is already
        // the fog-filtered view).
        let specs = &self.kind_specs;
        paint_minimap_markers(
            &mut self.minimap_scratch,
            entities,
            specs,
            self.minimap_size,
        );
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.minimap_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.minimap_scratch,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Whether a window-pixel point lands in the minimap panel (the
    /// client's click routing) — plus the world point it selects.
    pub fn minimap_hit(&self, px: f32, py: f32) -> Option<(f32, f32)> {
        let rect = self.minimap_rect?;
        if px < rect[0] || py < rect[1] || px > rect[0] + rect[2] || py > rect[1] + rect[3] {
            return None;
        }
        let (w, h) = self.minimap_size;
        let world_x = (px - rect[0]) / rect[2] * w as f32;
        let world_z = (py - rect[1]) / rect[3] * h as f32;
        Some((world_x, world_z))
    }

    /// Queues UI quads for the *next* [`Renderer::render`] — they are drawn
    /// after the world, on top of it, and the queue drains with the frame.
    /// Layout coordinates are pixels of the current surface size (`atlas()`
    /// provides the measuring).
    pub fn queue_ui(&mut self, quads: &[UiQuad]) {
        self.pending_ui.extend_from_slice(quads);
    }

    /// Builds the UI vertex stream for the pending quads, capped at the
    /// buffer's capacity, and drains the queue.
    fn drain_ui_vertices(&mut self) -> (u32, Vec<UiVertex>) {
        let capacity = self.ui_vertex_capacity as usize;
        let mut vertices: Vec<UiVertex> = Vec::with_capacity(self.pending_ui.len() * 6);
        for quad in self.pending_ui.drain(..) {
            if vertices.len() + 6 > capacity {
                break; // an over-full HUD frame clips its tail, never panics
            }
            vertices.extend_from_slice(&UiVertex::from_quad(&quad));
        }
        (vertices.len() as u32, vertices)
    }
}

/// The ground lift shared by every decal — enough to clear terrain/fog
/// rasterization noise, small enough to look glued to the ground.
const DECAL_LIFT: f32 = 0.02;

/// One uploaded model primitive (M12 phase 3): the vertex/index buffers and
/// the group(1) binding its draws sample — the material's own uploaded PNG
/// (kept alive in `skin`) when the albedo is a texture, or a clone of the
/// built-in white placeholder skin's bind group when it is a factor
/// (sampling the 1×1 white multiplies by exactly 1.0, so factor materials
/// never see a texture).
struct UploadedModelPrimitive {
    vertex_buf: wgpu::Buffer,
    /// The file's index buffer and its index count, when it carried one.
    index_buf: Option<(wgpu::Buffer, u32)>,
    /// The vertex count (non-indexed draws).
    vertex_count: u32,
    /// The material's uploaded texture, kept alive for the bind group.
    #[allow(dead_code)] // held to keep the texture alive through the bind group
    skin: Option<texture::EntitySkin>,
    bind_group: wgpu::BindGroup,
}

/// One kind's uploaded model: every primitive converted, uploaded, and
/// bound once at construction — the frame pass only rebinds them. The
/// gallery's per-kind stats (A-020) ride along: the triangle count and the
/// drawn size in meters, both computed once here from the same converted
/// vertices the GPU received.
struct UploadedModel {
    #[allow(dead_code)] // GPU debugging labels; not read on the CPU
    label: String,
    primitives: Vec<UploadedModelPrimitive>,
    /// The model's triangles — what the loaded evidence line prints and
    /// the gallery shows.
    triangles: u32,
    /// The drawn size (x, y, z extent) in meters: the baked tile-space
    /// vertex bounds times [`METERS_PER_TILE`].
    drawn_meters: [f32; 3],
}

/// What [`WgpuRenderer::build_model_pass`] hands the constructor: the whole
/// model pass plus the skin tier's slot table in one value, so the
/// constructor's footprint on this state is one call and a few field moves.
struct ModelPassGpu {
    pipeline: wgpu::RenderPipeline,
    instance_buf: wgpu::Buffer,
    capacity: u64,
    models: Vec<Option<UploadedModel>>,
    kind_renders_model: Vec<bool>,
    /// Per-kind skin slot (the skin tier's outcome; 0 = the placeholder
    /// skin, the placeholder tier) — the field the pre-model constructor's
    /// skin loop used to fill.
    kind_skin_slots: Vec<u32>,
}

/// Decodes a GLB-embedded PNG (a model material's albedo bytes, straight
/// from the BIN chunk) under the same guards the skin path's file decoder
/// applies: the header's dimensions are read before a single pixel decodes
/// (the decode bomb never fires before the limit check), the per-side cap
/// holds, and the output is RGBA8. A model whose material cannot decode is
/// not presentation-ready — the caller fails the whole model and the kind
/// walks the fallback chain (one line, A-146).
fn decode_embedded_png(bytes: &[u8]) -> Result<texture::DecodedTexture, String> {
    use std::io::Cursor;
    let dimensions = image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png)
        .into_dimensions()
        .map_err(|error| format!("not a decodable PNG: {error}"))?;
    if dimensions.0 == 0
        || dimensions.1 == 0
        || dimensions.0 > texture::MAX_TEXTURE_DIMENSION
        || dimensions.1 > texture::MAX_TEXTURE_DIMENSION
    {
        return Err(format!(
            "image is {}x{} but the per-side limit is {}x{}",
            dimensions.0,
            dimensions.1,
            texture::MAX_TEXTURE_DIMENSION,
            texture::MAX_TEXTURE_DIMENSION
        ));
    }
    let decoded = image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png)
        .decode()
        .map_err(|error| format!("PNG decode failed: {error}"))?;
    let rgba = decoded.into_rgba8();
    Ok(texture::DecodedTexture {
        width: dimensions.0,
        height: dimensions.1,
        rgba: rgba.into_raw(),
    })
}

/// What [`WgpuRenderer::build_model_pass`] needs (grouped so the builder
/// stays one argument): the device/queue pair, the entity pass's own
/// `[camera, skin]` pipeline layout and skin bind layout + sampler (the
/// model pass deliberately shares them), the built-in white placeholder
/// skin (factor materials bind its group), the pack, the content kind
/// names, and the surface format.
struct ModelPassInputs<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    pipeline_layout: &'a wgpu::PipelineLayout,
    skin_layout: &'a wgpu::BindGroupLayout,
    skin_sampler: &'a wgpu::Sampler,
    placeholder_skin: &'a texture::EntitySkin,
    pack: Option<&'a AssetPack>,
    kind_names: &'a [String],
    format: wgpu::TextureFormat,
}

/// One kind's model probe outcome: the loaded data, the converted
/// primitives, and every material's decoded texture (aligned by primitive,
/// `None` for factor materials) when the model won. The probe is total —
/// by the time it returns `Ok`, nothing between here and the GPU upload can
/// fail, so a tier that wins always uploads whole (A-146: the tier decision
/// and the upload happen in one walk, exactly once per run).
type ModelProbe = Option<
    Result<
        (
            model::ModelData,
            Vec<model::GpuPrimitive>,
            Vec<Option<texture::DecodedTexture>>,
        ),
        String,
    >,
>;

impl WgpuRenderer {
    /// Builds the model pass and resolves every kind's presentation tier
    /// (M12 phases 3-4, ADR-0101 §§3-5): the pipeline over the entity pass's
    /// own `[camera, skin]` layout, the shared instance buffer, and — per
    /// kind, in one walk — the fallback chain model → skin → placeholder.
    /// Runs exactly once per renderer: every file read, GLB parse,
    /// meters-to-tiles conversion, PNG decode, GPU upload, and bind group
    /// is born here and never again, and the chain is lazy — a kind that
    /// renders its model never decodes its skin (A-146: the frame path
    /// resolves and logs nothing; the tier decision never re-runs).
    ///
    /// Diagnostics (the A-146 pin): a kind whose walk passed any failure —
    /// a pack-recorded missing file, a model that would not load, a skin
    /// that would not decode — prints exactly ONE line naming the tier it
    /// landed on; a clean load prints its positive evidence line; a kind
    /// with nothing declared prints nothing. The chain rule itself is
    /// [`AssetPack::resolve_presentation`] in the pack module, pure and
    /// unit-pinned; this function owns the probes and the uploads.
    fn build_model_pass(inputs: ModelPassInputs<'_>) -> (ModelPassGpu, Vec<texture::EntitySkin>) {
        let ModelPassInputs {
            device,
            queue,
            pipeline_layout,
            skin_layout,
            skin_sampler,
            placeholder_skin,
            pack,
            kind_names,
            format,
        } = inputs;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("model shader"),
            source: wgpu::ShaderSource::Wgsl(entities::MODEL_SHADER_SRC.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("model pipeline"),
            layout: Some(pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<ModelVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 0,
                                shader_location: 0,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 12,
                                shader_location: 1,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 24,
                                shader_location: 2,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 32,
                                shader_location: 3,
                            },
                        ],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<ModelInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 0,
                                shader_location: 4,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32,
                                offset: 12,
                                shader_location: 5,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 16,
                                shader_location: 6,
                            },
                        ],
                    },
                ],
            },
            primitive: wgpu::PrimitiveState {
                // Cull stays off, matching the entity pass: exporter winding
                // conventions vary, and a wrongly-culled model is worse
                // than a double-shaded backface the depth buffer eats.
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        let capacity = 1024u64;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("model instances"),
            size: capacity * std::mem::size_of::<ModelInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut models: Vec<Option<UploadedModel>> = kind_names.iter().map(|_| None).collect();
        let mut kind_skin_slots: Vec<u32> = kind_names.iter().map(|_| 0).collect();
        // The skins this walk uploads (the caller extends its library with
        // them after the borrow on the placeholder ends; slot 0 is always
        // the placeholder, so the first new skin is slot 1).
        let mut new_skins: Vec<texture::EntitySkin> = Vec::new();
        for (index, kind) in kind_names.iter().enumerate() {
            let Some(pack) = pack else {
                break; // no pack at all — every kind keeps the placeholder
            };
            let Some(entry) = pack.asset_for(kind) else {
                continue; // an unmapped kind — the box path (v1 skips print at load)
            };
            // Tier 1 — the model probe: read, parse, convert. The outcome
            // (and the converted data, when it won) is kept for the upload
            // below; the placement comes from the manifest's typed fields
            // (A-155 — never the v1 `asset` primary).
            let model_probe: ModelProbe = entry.model.as_ref().map(|model_ref| {
                std::fs::read(&model_ref.resolved)
                    .map_err(|error| {
                        format!("model '{}' could not be read ({error})", model_ref.path)
                    })
                    .and_then(|bytes| {
                        model::load(&bytes).map_err(|error| {
                            format!("model '{}' could not be loaded ({error})", model_ref.path)
                        })
                    })
                    .and_then(|data| {
                        let placement = ModelPlacement {
                            scale: entry.scale,
                            rotation_deg: entry.rotation_deg,
                            ground_offset: entry.ground_offset,
                        };
                        let primitives = model::gpu_primitives(
                            &data,
                            &placement,
                            METERS_PER_TILE,
                            bake_gltf_to_entity,
                        );
                        // A material whose embedded PNG cannot decode fails
                        // the whole model (a half-textured model is worse
                        // than the clean fallback tier) — decoded HERE, in
                        // the probe, so the chain routes to a lower tier
                        // before anything uploads.
                        let mut textures = Vec::with_capacity(primitives.len());
                        for (primitive_index, gpu) in primitives.iter().enumerate() {
                            let decoded = match &gpu.texture {
                                Some(png_bytes) => {
                                    Some(decode_embedded_png(png_bytes).map_err(|error| {
                                        format!(
                                            "model '{}' could not be loaded (material \
                                             {primitive_index}: {error})",
                                            model_ref.path
                                        )
                                    })?)
                                }
                                None => None,
                            };
                            textures.push(decoded);
                        }
                        Ok((data, primitives, textures))
                    })
            });
            // Tier 2 — the skin probe, lazy: only when the model tier did
            // not win (a kind that renders its model never decodes its
            // skin). The skin reference is the typed field (A-155).
            let model_won = matches!(model_probe, Some(Ok(_)));
            let skin_probe = if model_won {
                None
            } else {
                entry.skin.as_ref().map(|skin_ref| {
                    texture::decode_png(&skin_ref.resolved).map_err(|error| {
                        format!("skin '{}' could not be loaded ({error})", skin_ref.path)
                    })
                })
            };
            // The chain rule, pure and unit-pinned in the pack module: tier
            // + every failure the walk passed (pack notes first, then the
            // model probe, then the skin probe).
            let resolution = pack.resolve_presentation(
                kind,
                model_probe
                    .as_ref()
                    .map(|result| result.as_ref().map(|_| ()).map_err(Clone::clone)),
                skin_probe
                    .as_ref()
                    .map(|result| result.as_ref().map(|_| ()).map_err(Clone::clone)),
            );
            // The upload follows the decision, still inside this one walk —
            // and cannot fail: the probe decoded every texture already.
            match resolution.tier {
                crate::pack::PresentationTier::Model => {
                    let Some(Ok((data, primitives, textures))) = model_probe else {
                        // Unreachable by the chain rule (Model only when the
                        // probe won) — guard anyway: never index past.
                        continue;
                    };
                    // One uploaded primitive per mesh primitive.
                    let mut uploaded = Vec::with_capacity(primitives.len());
                    for (primitive_index, gpu) in primitives.iter().enumerate() {
                        let label = format!("{kind}#{primitive_index}");
                        let (skin, bind_group) = match &textures[primitive_index] {
                            Some(decoded) => {
                                let skin = texture::upload_skin(
                                    device,
                                    queue,
                                    skin_layout,
                                    skin_sampler,
                                    decoded,
                                    &label,
                                );
                                let bind_group = skin.bind_group.clone();
                                (Some(skin), bind_group)
                            }
                            None => (None, placeholder_skin.bind_group.clone()),
                        };
                        let vertex_buf =
                            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                label: Some(&label),
                                contents: bytemuck::cast_slice(&gpu.vertices),
                                usage: wgpu::BufferUsages::VERTEX,
                            });
                        let index_buf = gpu.indices.as_ref().map(|indices| {
                            let buffer =
                                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                    label: Some(&label),
                                    contents: bytemuck::cast_slice(indices),
                                    usage: wgpu::BufferUsages::INDEX,
                                });
                            (buffer, indices.len() as u32)
                        });
                        uploaded.push(UploadedModelPrimitive {
                            vertex_buf,
                            index_buf,
                            vertex_count: gpu.vertices.len() as u32,
                            skin,
                            bind_group,
                        });
                    }
                    // The positive evidence line the Xvfb machine pass greps
                    // — the loader's per-file metadata (ignored animation
                    // blocks, duplicated team_color materials) rides the
                    // same line so a load says exactly one thing (A-146).
                    let triangles: usize = primitives
                        .iter()
                        .map(|gpu| {
                            gpu.indices
                                .as_ref()
                                .map_or(gpu.vertices.len() / 3, |i| i.len() / 3)
                        })
                        .sum();
                    // The gallery's drawn size (A-020): the extent of the
                    // baked vertices — after scale, rotation, ground offset,
                    // the meters-to-tiles division, and the basis bake — so
                    // it is exactly the size the frame pass draws, in
                    // meters. Computed once per kind here, never per frame.
                    let mut bounds = ([f32::MAX; 3], [f32::MIN; 3]);
                    for gpu in &primitives {
                        for vertex in &gpu.vertices {
                            for axis in 0..3 {
                                bounds.0[axis] = bounds.0[axis].min(vertex.position[axis]);
                                bounds.1[axis] = bounds.1[axis].max(vertex.position[axis]);
                            }
                        }
                    }
                    let drawn_meters = [
                        (bounds.1[0] - bounds.0[0]) * METERS_PER_TILE,
                        (bounds.1[1] - bounds.0[1]) * METERS_PER_TILE,
                        (bounds.1[2] - bounds.0[2]) * METERS_PER_TILE,
                    ];
                    let mut extras = String::new();
                    if data.ignored_animation_data > 0 {
                        extras.push_str(&format!(
                            ", {} ignored animation blocks",
                            data.ignored_animation_data
                        ));
                    }
                    if data.team_color_materials > 1 {
                        extras.push_str(&format!(
                            ", {} team_color materials (first wins)",
                            data.team_color_materials
                        ));
                    }
                    println!(
                        "pandemonium client — entity model '{kind}' loaded ({} primitives, \
                         {triangles} triangles{extras}, from {})",
                        uploaded.len(),
                        entry
                            .model
                            .as_ref()
                            .expect("the model tier implies a ref")
                            .path
                    );
                    models[index] = Some(UploadedModel {
                        label: kind.clone(),
                        primitives: uploaded,
                        triangles: triangles as u32,
                        drawn_meters,
                    });
                }
                crate::pack::PresentationTier::Skin => {
                    let Some(Ok(decoded)) = skin_probe else {
                        continue; // the chain rule guarantees this arm's probe won
                    };
                    println!(
                        "pandemonium client — entity skin '{kind}' loaded ({}x{} PNG from {})",
                        decoded.width,
                        decoded.height,
                        entry
                            .skin
                            .as_ref()
                            .expect("the skin tier implies a ref")
                            .path
                    );
                    kind_skin_slots[index] = (1 + new_skins.len()) as u32;
                    new_skins.push(texture::upload_skin(
                        device,
                        queue,
                        skin_layout,
                        skin_sampler,
                        &decoded,
                        kind,
                    ));
                }
                crate::pack::PresentationTier::Placeholder => {}
            }
            // The A-146 pin, one line per failing kind naming the tier it
            // landed on (pure formatter, unit-pinned in the pack module).
            if let Some(line) = crate::pack::presentation_diagnostic(kind, &resolution) {
                eprintln!("{line}");
            }
        }
        let kind_renders_model: Vec<bool> = models.iter().map(Option::is_some).collect();
        (
            ModelPassGpu {
                pipeline,
                instance_buf,
                capacity,
                models,
                kind_renders_model,
                kind_skin_slots,
            },
            new_skins,
        )
    }
}

impl Renderer for WgpuRenderer {
    fn render(&mut self, frame: Frame<'_>) {
        // Camera uniform: the matrix plus the eye the view-dependent
        // fragment shading reads, and the animation phase (seconds since
        // the renderer was born) the fog drift and decal pulses run on.
        let time = self.start.elapsed().as_secs_f32();
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::bytes_of(&CameraUniform {
                view_projection: frame.view_projection.to_cols_array_2d(),
                eye: frame.eye.to_array(),
                time,
                map_size: [self.fog_map_size.0, self.fog_map_size.1],
                _pad: [0.0; 2],
            }),
        );
        // The fog pass shares the camera; it also carries the map size for
        // the world-space UV mapping and the same animation clock.
        self.queue.write_buffer(
            &self.fog_uniform_buf,
            0,
            bytemuck::bytes_of(&FogUniform {
                view_projection: frame.view_projection.to_cols_array_2d(),
                map_size: [self.fog_map_size.0, self.fog_map_size.1],
                time,
                _pad: 0.0,
            }),
        );

        // Entity instances from the interpolated snapshot (ascending id),
        // composed into per-kind silhouettes by the pure instance builder.
        // Each instance carries its kind's skin slot so the pass below can
        // group the stream by texture without touching the builder.
        let mut instances = build_instances(
            frame.snapshot,
            frame.selection,
            frame.flashes,
            &self.kind_specs,
            &self.kind_skin_slots,
            &self.kind_renders_model,
            self.entity_instance_capacity as usize,
        );
        // Death fades append their shrinking masses (the entity is gone
        // from the snapshot; only the cue remains, and it drains here).
        for cue in self.dying_cues.drain(..) {
            if instances.len() >= self.entity_instance_capacity as usize {
                break;
            }
            let shape = self
                .kind_specs
                .get(cue.kind.0 as usize)
                .cloned()
                .unwrap_or_else(fallback_shape);
            instances.push(death_instance(&cue, &shape));
        }
        // The heightmap only moves rendered terrain; the simulation keeps
        // positions on the flat logical grid. Lift every rendered part onto
        // the matching display surface so low units do not disappear inside
        // hills.
        for instance in &mut instances {
            instance.position[1] +=
                self.ground_height_at(instance.position[0], instance.position[2]);
        }
        // Group the stream by skin slot (the pure helper owns the law):
        // one contiguous run per slot means one draw per slot binds one
        // texture bind group — and no GPU resource is created here.
        let (ordered, skin_draws) = group_by_skin(instances, self.entity_skins.len());
        let needed = ordered.len() as u64;
        if needed > 0 {
            let bytes = bytemuck::cast_slice(&ordered);
            let capacity_bytes =
                self.entity_instance_capacity as usize * std::mem::size_of::<EntityInstance>();
            let capped = &bytes[..bytes.len().min(capacity_bytes)];
            self.queue
                .write_buffer(&self.entity_instance_buf, 0, capped);
        }

        let Ok(frame_texture) = self.surface.get_current_texture() else {
            return; // Window occluded/minimized — nothing to present.
        };
        let view = frame_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("main encoder"),
            });
        // The overlay's screen-size uniform (kept in sync with the surface).
        self.queue.write_buffer(
            &self.screen_buffer,
            0,
            bytemuck::bytes_of(&[self.config.width as f32, self.config.height as f32]),
        );
        let (ui_vertex_count, ui_vertices) = self.drain_ui_vertices();
        if ui_vertex_count > 0 {
            self.queue
                .write_buffer(&self.ui_vertex_buf, 0, bytemuck::cast_slice(&ui_vertices));
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.04,
                            g: 0.06,
                            b: 0.09,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_bind_group(1, &self.texture_bind_group, &[]);
            pass.set_pipeline(&self.terrain_pipeline);
            pass.set_vertex_buffer(0, self.terrain_vertex_buf.slice(..));
            pass.set_index_buffer(self.terrain_index_buf.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.terrain_index_count, 0, 0..1);
            // The fog of war over the terrain (same geometry, depth-tested):
            // hidden tiles go dark, explored tiles half-lit, visible clear.
            pass.set_pipeline(&self.fog_pipeline);
            pass.set_bind_group(0, &self.fog_bind_group, &[]);
            pass.set_vertex_buffer(0, self.terrain_vertex_buf.slice(..));
            pass.set_index_buffer(self.terrain_index_buf.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.terrain_index_count, 0, 0..1);
            // Ground decals: blob shadows under everything, then the
            // placement ghost, then selection rings (all flat on the plane).
            let decal_capacity = self.decal_instance_capacity as usize;
            // The decal pass's one animation: the interactive decals breathe
            // (a slow, gentle alpha pulse) so "this is live UI, pointing at
            // your selection / your build spot" reads even on a still frame.
            // Shadows and team rings stay static — owner identity should not
            // shimmer.
            let pulse = ring_pulse(time);
            let shadows = build_shadows(frame.snapshot, &self.kind_specs, decal_capacity);
            let shadow_count = shadows.len() as u32;
            let mut decals = shadows;
            // The ghost rides above shadows: one quad at the footprint.
            let ghost_count = if let Some(ghost) = self.ghost {
                if decals.len() < decal_capacity {
                    let mut color = if ghost.legal {
                        [0.30, 1.0, 0.40, 0.30]
                    } else {
                        [1.0, 0.30, 0.25, 0.38]
                    };
                    color[3] *= pulse;
                    decals.push(DecalInstance {
                        position: [ghost.center[0], DECAL_LIFT * 1.5, ghost.center[2]],
                        scale: ghost.extent,
                        color,
                        mode: DECAL_FLAT,
                    });
                    1u32
                } else {
                    0
                }
            } else {
                0
            };
            // Team rings land first, selection rings after them, so a
            // selected unit reads both: owner color beneath, green on top.
            let team_rings = build_team_rings(
                frame.snapshot,
                &self.kind_specs,
                decal_capacity - decals.len(),
            );
            let team_ring_count = team_rings.len() as u32;
            decals.extend(team_rings);
            let rings = build_rings(
                frame.snapshot,
                frame.selection,
                &self.kind_specs,
                decal_capacity - decals.len(),
            );
            let ring_count = rings.len() as u32;
            decals.extend(rings.into_iter().map(|mut ring| {
                ring.color[3] *= pulse;
                ring
            }));
            for decal in &mut decals {
                decal.position[1] += self.ground_height_at(decal.position[0], decal.position[2]);
            }
            if !decals.is_empty() {
                let bytes = bytemuck::cast_slice(&decals);
                let capacity_bytes =
                    self.decal_instance_capacity as usize * std::mem::size_of::<DecalInstance>();
                self.queue.write_buffer(
                    &self.decal_instance_buf,
                    0,
                    &bytes[..bytes.len().min(capacity_bytes)],
                );
                pass.set_pipeline(&self.decal_pipeline);
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                // Slot 1 carries the per-instance data for both decal draws
                // (quads and rings share one instance buffer): without this
                // binding the pass draws with the pipeline's instance slot
                // unset and wgpu's validation rejects the very first draw.
                pass.set_vertex_buffer(1, self.decal_instance_buf.slice(..));
                if shadow_count + ghost_count > 0 {
                    pass.set_vertex_buffer(0, self.quad_vertex_buf.slice(..));
                    pass.draw(0..self.quad_vertex_count, 0..(shadow_count + ghost_count));
                }
                if team_ring_count + ring_count > 0 {
                    pass.set_vertex_buffer(0, self.ring_vertex_buf.slice(..));
                    pass.draw(
                        0..self.ring_vertex_count,
                        (shadow_count + ghost_count)
                            ..(shadow_count + ghost_count + team_ring_count + ring_count),
                    );
                }
            }
            pass.set_pipeline(&self.entity_pipeline);
            // Re-bind the camera group for the entity draw: the fog pass
            // leaves the *fog* group in slot 0, and the decal pass between
            // them (which restores the camera group) is skipped when there
            // is nothing to draw — a menu frame with zero entities hit
            // exactly that (PLAN-M10.2 §3.2's menu-first world). wgpu
            // validates the bind group even for a zero-instance draw.
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_vertex_buffer(0, self.entity_vertex_buf.slice(..));
            pass.set_vertex_buffer(1, self.entity_instance_buf.slice(..));
            // One draw per skin slot: group(1) carries that slot's bind
            // group over the slot's contiguous instance range. An empty
            // frame still issues one zero-instance draw against the
            // placeholder skin — wgpu validates the bound groups even when
            // nothing rasterizes.
            if skin_draws.is_empty() {
                pass.set_bind_group(1, &self.entity_skins[0].bind_group, &[]);
                pass.draw(0..36, 0..0);
            } else {
                for (slot, start, count) in skin_draws {
                    pass.set_bind_group(1, &self.entity_skins[slot].bind_group, &[]);
                    pass.draw(0..36, start as u32..(start + count) as u32);
                }
            }
            // The model pass (M12 phase 3, ADR-0101): the kinds the boxes
            // skipped draw here instead — the same snapshot (ADR-0200 §4's
            // invariant), the same camera group, the same depth buffer —
            // one instance per entity, grouped per kind, and one draw per
            // (kind primitive, kind instance run). All GPU state was born
            // in the constructor; this only binds and draws.
            let (mut model_instances, model_draws) = build_model_instances(
                frame.snapshot,
                frame.selection,
                frame.flashes,
                &self.kind_renders_model,
                self.model_instance_capacity as usize,
            );
            if !model_instances.is_empty() {
                for instance in &mut model_instances {
                    instance.position[1] +=
                        self.ground_height_at(instance.position[0], instance.position[2]);
                }
                let bytes = bytemuck::cast_slice(&model_instances);
                let capacity_bytes =
                    self.model_instance_capacity as usize * std::mem::size_of::<ModelInstance>();
                let capped = &bytes[..bytes.len().min(capacity_bytes)];
                self.queue.write_buffer(&self.model_instance_buf, 0, capped);
            }
            if !model_draws.is_empty() {
                pass.set_pipeline(&self.model_pipeline);
                // The camera group is still bound from the entity pass, but
                // rebind it anyway — the pass's own law (and the source
                // scan test) keeps each pipeline's contract self-contained.
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                pass.set_vertex_buffer(1, self.model_instance_buf.slice(..));
                for (kind, start, count) in model_draws {
                    let Some(Some(model)) = self.kind_models.get(kind) else {
                        continue; // an unknown kind index — never index past
                    };
                    for primitive in &model.primitives {
                        pass.set_bind_group(1, &primitive.bind_group, &[]);
                        pass.set_vertex_buffer(0, primitive.vertex_buf.slice(..));
                        let instances = start as u32..(start + count) as u32;
                        match &primitive.index_buf {
                            Some((index_buf, index_count)) => {
                                pass.set_index_buffer(
                                    index_buf.slice(..),
                                    wgpu::IndexFormat::Uint32,
                                );
                                pass.draw_indexed(0..*index_count, 0, instances);
                            }
                            None => {
                                pass.draw(0..primitive.vertex_count, instances);
                            }
                        }
                    }
                }
            }
            // The minimap rides over the world but under the HUD quads (the
            // viewport indicator and panel border draw in the UI pass on
            // top of it): it rebinds group(0) to its own layout, like the
            // UI pass does.
            if self.minimap_rect.is_some() {
                pass.set_pipeline(&self.minimap_pipeline);
                pass.set_bind_group(0, &self.minimap_bind_group, &[]);
                pass.set_vertex_buffer(0, self.minimap_vertex_buf.slice(..));
                pass.draw(0..6, 0..1);
            }
            // The HUD/debug overlay goes on top of everything.
            if ui_vertex_count > 0 {
                pass.set_pipeline(&self.ui_pipeline);
                // group(0) rebinds from the camera uniform to the screen
                // uniform (a different layout, legal within the pass).
                pass.set_bind_group(0, &self.screen_bind_group, &[]);
                pass.set_bind_group(1, &self.ui_bind_group, &[]);
                pass.set_vertex_buffer(0, self.ui_vertex_buf.slice(..));
                pass.draw(0..ui_vertex_count, 0..1);
            }
        }
        self.queue.submit(Some(encoder.finish()));
        frame_texture.present();
    }
}

/// Extracts the shared grid of corner heights from the four row-major
/// vertices generated for each terrain tile.
fn terrain_height_grid(terrain: &TerrainMesh, width: u32, height: u32) -> Vec<f32> {
    let stride = width as usize + 1;
    let mut heights = vec![0.0; stride * (height as usize + 1)];
    for z in 0..height as usize {
        for x in 0..width as usize {
            let base = (z * width as usize + x) * 4;
            let Some(tile) = terrain.vertices.get(base..base + 4) else {
                continue;
            };
            heights[z * stride + x] = tile[0].position[1];
            heights[z * stride + x + 1] = tile[1].position[1];
            heights[(z + 1) * stride + x] = tile[2].position[1];
            heights[(z + 1) * stride + x + 1] = tile[3].position[1];
        }
    }
    heights
}

#[cfg(test)]
mod tests {
    use super::entities::{ENTITY_SHADER_SRC, MODEL_SHADER_SRC};
    use super::overlay::DECAL_SHADER_SRC;
    use super::*;

    /// Every embedded WGSL module parses, type-checks, and validates (and
    /// keeps its expected entry points). The GPU path cannot run on a
    /// headless CI machine, so the shaders are machine-checked to the extent
    /// the environment allows — a parse error would otherwise only surface
    /// as a runtime panic in `WgpuRenderer::new` on a machine with a
    /// display, and a *type* error would surface even later, as a pipeline
    /// creation failure. naga's validator is the same one wgpu runs at
    /// module creation, so a pass here covers everything short of the
    /// bind-group/pipeline wiring the layouts own.
    #[test]
    fn every_embedded_shader_parses_with_expected_entry_points() {
        for (name, source) in [
            ("terrain", TERRAIN_SHADER),
            ("entity", ENTITY_SHADER_SRC),
            ("model", MODEL_SHADER_SRC),
            ("ui", UI_SHADER_SRC),
            ("fog", FOG_SHADER_SRC),
            ("decal", DECAL_SHADER_SRC),
        ] {
            let module = wgpu::naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|error| panic!("{name} shader does not parse: {error}"));
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|error| panic!("{name} shader does not validate: {error}"));
            for entry in ["vs_main", "fs_main"] {
                assert!(
                    module.entry_points.iter().any(|point| point.name == entry),
                    "{name} shader lost its {entry} entry point"
                );
            }
        }
    }

    /// A-020's root cause was a shader that compiled, validated, and then
    /// rasterized nothing: the model pass's vertex entry point declared
    /// `@builtin(position)` in its output struct and never wrote it, so the
    /// zero-initialized `var out` shipped clip (0,0,0,0) for every vertex
    /// and every triangle degenerated onto the clip-space origin — silently,
    /// on every backend, for every model kind (the owner's "ore node
    /// invisible" and "rifleman invisible" reports; the A-018 "visible"
    /// evidence lines only proved the models LOADED). naga accepts an
    /// unwritten builtin, so this scan is the guard: any embedded shader
    /// whose vertex entry point builds its result through `var out: <Type>`
    /// must write every member that type declares before `return out;`.
    #[test]
    fn every_shader_output_member_is_written() {
        for (name, source) in [("entity", ENTITY_SHADER_SRC), ("model", MODEL_SHADER_SRC)] {
            let Some(vertex_body) = source
                .split("@vertex fn")
                .nth(1)
                .and_then(|rest| rest.split("@fragment").next())
            else {
                panic!("{name} shader lost its vertex entry point");
            };
            for var_line in vertex_body.lines().filter(|line| {
                let trimmed = line.trim_start();
                trimmed.starts_with("var out:")
            }) {
                let struct_name = var_line
                    .trim()
                    .trim_start_matches("var out:")
                    .trim_end_matches(';')
                    .trim();
                let Some(struct_decl) = source.split("struct ").find_map(|piece| {
                    piece.starts_with(struct_name).then(|| {
                        &piece[struct_name.len()..piece.find('}').expect("the struct closes")]
                    })
                }) else {
                    panic!("{name} shader: `var out: {struct_name}` names no struct");
                };
                for member in struct_decl
                    .trim_start_matches('{')
                    .split(',')
                    .map(|part| part.trim())
                    .filter(|part| !part.is_empty())
                {
                    let member_name = member
                        .split(':')
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    // Annotations (`@builtin(position) clip: vec4<f32>`) ride
                    // the part; the member is the last identifier before ':'.
                    let member_name = member_name
                        .rsplit(|c: char| c.is_whitespace() || c == ')')
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    assert!(
                        !member_name.is_empty(),
                        "{name} shader: a {struct_name} member could not be named from \
                         `{member}`"
                    );
                    assert!(
                        vertex_body.contains(&format!("out.{member_name}")),
                        "{name} shader: vs_main never writes {struct_name}::{member_name} — an \
                         unwritten `@builtin(position)` is exactly the A-020 bug (a zero clip \
                         for every vertex, and the pass silently draws nothing)"
                    );
                }
            }
        }
    }

    /// A-020 brief cause (b): the render tables (`kind_specs`,
    /// `kind_renders_model`, `kind_skin_slots` — all built from
    /// [`crate::app::kind_names`]) index by the SIMULATION's KindId, while
    /// the pack keys by the content's kind NAME. `kind_names` is the whole
    /// bridge, and nothing else pinned it against the real content's ids —
    /// the exact seam the brief told this task to test with real content.
    /// Pinned here end to end: (1) the map's ore nodes are declared by name
    /// and spawn under KindIds that `kind_names` names back to "ore_node"
    /// (the owner's invisible-ore case, through the real loader); (2) every
    /// spawned KindId names a real entity kind through the bridge; (3) a
    /// schema-2 pack's declared model kinds each resolve to a real slot,
    /// and a pack kind outside the content never does.
    #[test]
    fn every_pack_model_kind_lands_in_its_own_table_slot() {
        let tree = pandemonium_content::ContentTree::load_dir(crate::content_path())
            .expect("the real content tree loads");
        let bundle = tree
            .bundle(
                &tree
                    .maps
                    .first()
                    .expect("the content tree lists maps")
                    .id
                    .clone(),
            )
            .expect("the default map bundles");
        let names = crate::app::kind_names(&bundle);
        let world = bundle.world();

        // (1) The ore-node pin, name -> KindId -> name through the real
        // loader: the map declares its nodes by string, the world carries
        // them as KindIds, and the renderer's own name table must land on
        // the same kind.
        let ore_index = bundle
            .entities
            .iter()
            .position(|def| def.id == "ore_node")
            .expect("the real content defines ore_node");
        let declared: Vec<_> = bundle
            .map
            .ore_nodes
            .iter()
            .filter(|node| node.kind == "ore_node")
            .collect();
        let spawned: Vec<_> = world
            .initial_spawns
            .iter()
            .filter(|spawn| spawn.kind.0 as usize == ore_index)
            .collect();
        assert!(!declared.is_empty(), "the real map declares ore_node nodes");
        assert_eq!(
            declared.len(),
            spawned.len(),
            "the map's ore_node strings and the world's ore-node KindIds disagree — the \
             name/KindId bridge is misaligned"
        );
        assert_eq!(names[ore_index], "ore_node");

        // (2) Every KindId the world actually spawns names a real kind
        // through the bridge (a slot outside the table, or a name that is
        // not the spawned kind, is the same silent-swap failure).
        for spawn in &world.initial_spawns {
            let Some(name) = names.get(spawn.kind.0 as usize) else {
                panic!("a spawned KindId {} indexes past kind_names", spawn.kind.0);
            };
            assert!(
                bundle.entities.iter().any(|def| &def.id == name),
                "the bridge names '{name}' for a spawned KindId that is not that kind"
            );
        }

        // (3) The pack half, against a schema-2 fixture pack: every model
        // kind the pack declares must resolve to a table slot (the same
        // index `build_model_pass` walks and `kind_renders_model` is built
        // from), and a pack kind the content does not define must never
        // find one — it is the box path's "unmapped kind", never a swap.
        let dir =
            std::env::temp_dir().join(format!("pandemonium-a020-alignment-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("models/units"))
            .and_then(|_| std::fs::create_dir_all(dir.join("models/buildings")))
            .and_then(|_| std::fs::create_dir_all(dir.join("models/nodes")))
            .expect("the fixture pack dirs create");
        for file in [
            "models/units/rifleman.glb",
            "models/buildings/barracks.glb",
            "models/nodes/ore_node.glb",
            "models/units/nowhere.glb",
        ] {
            std::fs::write(
                dir.join(file),
                b"glb bytes: the pack load only resolves paths",
            )
            .expect("the fixture model file writes");
        }
        std::fs::write(
            dir.join("manifest.ron"),
            r#"(
    pack_id: "a020_alignment_fixture",
    schema_version: 2,
    entities: {
        "rifleman": ( model: Some("models/units/rifleman.glb") ),
        "barracks": ( model: Some("models/buildings/barracks.glb") ),
        "ore_node": ( model: Some("models/nodes/ore_node.glb") ),
        "not_a_content_kind": ( model: Some("models/units/nowhere.glb") ),
    },
    terrain: {},
)"#,
        )
        .expect("the fixture manifest writes");
        let pack = crate::pack::AssetPack::load(&dir)
            .expect("the fixture pack parses")
            .expect("the fixture pack loads");
        let _ = std::fs::remove_dir_all(&dir);
        for kind in ["rifleman", "barracks", "ore_node"] {
            let index = names
                .iter()
                .position(|name| name == kind)
                .unwrap_or_else(|| panic!("the pack model kind '{kind}' has no kind_names slot"));
            let entry = pack
                .asset_for(kind)
                .unwrap_or_else(|| panic!("the pack kind '{kind}' did not resolve"));
            assert!(
                entry.model.is_some(),
                "the pack model for '{kind}' did not survive the load"
            );
            // The slot the renderer will mark true is the name table's own
            // index — the same index the world-spawn pin above proves is
            // the simulation's KindId for the kind.
            assert_eq!(names[index], kind);
            assert!(
                bundle
                    .entities
                    .iter()
                    .position(|def| def.id == kind)
                    .expect("the pack kind is a real content kind")
                    == index,
                "kind_names' slot for '{kind}' is not its content position — the tables would \
                 index another kind's tier"
            );
        }
        assert!(
            names.iter().all(|name| name != "not_a_content_kind"),
            "a pack kind outside the content must never find a table slot"
        );
    }

    /// A-002 Part 1(a): adapter enumeration is the deepest wgpu call that
    /// does not need a window, and it runs ONLY behind the hardware gate.
    /// Headless CI (the usual case) sees the skip line; under
    /// `PANDEMONIUM_HW_TESTS=1` this is the deliberate canary for the
    /// Windows-crash isolation: if instance/adapter creation can kill the
    /// process on a GPU-less machine, this is the test it dies in.
    #[test]
    fn wgpu_adapter_enumeration_runs_behind_the_hardware_gate() {
        if !crate::hw::enabled(crate::hw::Scope::Gpu) {
            println!(
                "skipped: GPU hardware not positively plausible (set PANDEMONIUM_HW_TESTS=1 to \
                 force enumeration)"
            );
            return;
        }
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        // wgpu 26: `request_adapter` yields an adapter directly (the
        // software fallback counts) or an error — either way, RETURNING is
        // the claim under test.
        let _adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("enumerating an adapter on a positively-plausible GPU must not error");
        println!("probe: an adapter answered (wgpu enumeration returned)");
    }

    /// The instanced pipelines (decal + entity) draw with two vertex buffer
    /// slots: slot 0 geometry, slot 1 per-instance data. wgpu validates slot
    /// bindings at draw time, so a pipeline left with its instance slot unset
    /// is a first-frame GPU validation panic — the render pass only runs on a
    /// machine with a display, so CPU-side tests cannot reach it. As far as
    /// this environment allows (the same bargain as the shader-parse test
    /// above), the draw state is machine-checked: each instanced pipeline
    /// must bind its instance buffer to slot 1 between its `set_pipeline`
    /// and its first `draw`. `queue.write_buffer` alone does not bind.
    #[test]
    fn every_instanced_pipeline_binds_its_instance_slot_before_drawing() {
        let source =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/render/mod.rs"))
                .expect("the render module sits at the crate's src/render/mod.rs");
        // Scan only the rendering code — everything before the first test
        // module, with line comments stripped — so this test's own literals
        // and a commented-out binding cannot satisfy the law.
        let cut = source.find("#[cfg(test)]").expect("a test module exists");
        let code = source[..cut]
            .lines()
            .map(|line| &line[..line.find("//").unwrap_or(line.len())])
            .collect::<Vec<_>>()
            .join("\n");
        for (pipeline, slot1) in [
            (
                "pass.set_pipeline(&self.decal_pipeline);",
                "pass.set_vertex_buffer(1, self.decal_instance_buf.slice(..));",
            ),
            (
                "pass.set_pipeline(&self.entity_pipeline);",
                "pass.set_vertex_buffer(1, self.entity_instance_buf.slice(..));",
            ),
            (
                "pass.set_pipeline(&self.model_pipeline);",
                "pass.set_vertex_buffer(1, self.model_instance_buf.slice(..));",
            ),
        ] {
            let set_pipeline = code
                .find(pipeline)
                .unwrap_or_else(|| panic!("the {pipeline} call disappeared from render_frame"));
            let bind = code[set_pipeline..].find(slot1).unwrap_or_else(|| {
                panic!("{pipeline} draws without its instance buffer bound to slot 1")
            });
            let draw = code[set_pipeline..]
                .find("pass.draw(")
                .unwrap_or_else(|| panic!("{pipeline} never draws"));
            assert!(
                bind < draw,
                "{pipeline} draws before binding its instance buffer to slot 1"
            );
        }
    }
}

#[cfg(test)]
mod silhouette_tests {
    use super::entities::build_entity_instances;
    use super::*;
    use glam::Vec3;
    use pandemonium_engine::RenderEntity;
    use pandemonium_sim_api::{EntityId, KindId, MoveState, PlayerId};

    fn entity(id: u64, kind: u32, owner: PlayerId, moving: MoveState) -> RenderEntity {
        RenderEntity {
            id: EntityId(id),
            owner,
            kind: KindId(kind),
            pos: Vec3::new(5.0, 0.0, 7.0),
            facing: Vec3::ZERO,
            hp_fraction_milli: 1000,
            move_state: moving,
        }
    }

    fn unit_shape() -> KindShape {
        crate::silhouette::Silhouette::from_parts(
            true,
            vec![
                crate::silhouette::Part::at(
                    [0.0, 0.3, 0.0],
                    [0.4, 0.6, 0.4],
                    crate::silhouette::Tone::Body,
                ),
                crate::silhouette::Part::at(
                    [0.0, 0.7, 0.0],
                    [0.2, 0.2, 0.2],
                    crate::silhouette::Tone::Dark,
                ),
            ],
        )
    }

    #[test]
    fn a_structure_is_a_slab_plus_a_roof_sized_to_its_footprint() {
        let shape = crate::silhouette::capability_fallback(Some((4, 3)), false, false, false);
        let mut out = Vec::new();
        build_entity_instances(
            &entity(1, 0, PlayerId(0), MoveState::Idle),
            &shape,
            false,
            false,
            0,
            &mut out,
        );
        assert_eq!(out.len(), 3, "slab + roof + team band");
        let slab = &out[0];
        assert!(
            (slab.scale[0] - 4.0 * 0.94).abs() < 1e-4,
            "slab spans the footprint width"
        );
        assert!(
            (slab.scale[2] - 3.0 * 0.94).abs() < 1e-4,
            "slab spans the footprint depth"
        );
        // The slab sits flush on the ground (center at half its height).
        assert!((slab.position[1] - slab.scale[1] * 0.5).abs() < 1e-4);
        // The roof rides above the slab.
        assert!(out[1].position[1] > slab.position[1] + slab.scale[1] * 0.5);
        // Structures never rotate (footprints are axis-aligned).
        assert_eq!(slab.yaw, 0.0);
        // The band trim hugs the ground in the team color.
        assert_eq!(out[2].scale[1], 0.12);
        assert!(out[2].position[1] < slab.position[1]);
    }

    #[test]
    fn a_unit_is_a_body_plus_a_head_and_faces_its_direction() {
        let shape = crate::silhouette::named("rifleman").expect("authored");
        let mut e = entity(1, 0, PlayerId(0), MoveState::Idle);
        e.facing = Vec3::new(1.0, 0.0, 0.0); // east
        let mut out = Vec::new();
        build_entity_instances(&e, &shape, false, false, 0, &mut out);
        assert_eq!(out.len(), 5, "body + head + backpack + yoke + rifle");
        let yaw = out[0].yaw;
        assert!(
            (yaw - 0.0).abs() < 1e-4,
            "an east-facing unit yaws to 0, got {yaw}"
        );
        // The rifle rides ahead of the body center along the facing.
        assert!(out[4].position[0] > out[0].position[0], "rifle forward");
        // A moving unit squats: its whole silhouette is shorter.
        let mut moving = entity(1, 0, PlayerId(0), MoveState::Moving);
        moving.facing = Vec3::new(1.0, 0.0, 0.0);
        let mut out2 = Vec::new();
        build_entity_instances(&moving, &shape, false, false, 0, &mut out2);
        assert!(out2[0].scale[1] < out[0].scale[1], "moving squats the body");
        assert!(
            out2[4].position[1] < out[4].position[1],
            "offsets squat with the body"
        );
        // The head rides on top of the (squatting) body.
        assert!(out2[1].position[1] > out2[0].position[1]);
    }

    #[test]
    fn a_facing_turn_rotates_forward_parts_with_the_body() {
        // North-facing (+z): the rifle's forward offset maps onto +z and its
        // right-hand carry maps onto -x (right = forward x up). Both prove
        // the whole silhouette turned with the body.
        let shape = crate::silhouette::named("rifleman").expect("authored");
        let mut e = entity(1, 0, PlayerId(0), MoveState::Idle);
        e.facing = Vec3::new(0.0, 0.0, 1.0);
        let mut out = Vec::new();
        build_entity_instances(&e, &shape, false, false, 0, &mut out);
        assert!(
            (out[0].yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
            "a north-facing unit yaws to +90 degrees"
        );
        assert!(
            out[4].position[2] > out[0].position[2],
            "rifle forward (+z)"
        );
        assert!(
            out[4].position[0] < out[0].position[0],
            "rifle carried right"
        );
    }

    #[test]
    fn an_ore_node_is_amber_regardless_of_owner() {
        let shape = crate::silhouette::named("ore_node").expect("authored");
        let mut out = Vec::new();
        build_entity_instances(
            &entity(1, 4, PlayerId(1), MoveState::Idle),
            &shape,
            false,
            false,
            0,
            &mut out,
        );
        assert_eq!(out.len(), 4, "crystal cluster");
        for instance in &out {
            assert!(
                instance.color[0] > instance.color[2],
                "the node reads amber, not team red"
            );
        }
        // The amber is team-independent: the same shards resolve identically
        // for player 0.
        let mut out_p0 = Vec::new();
        build_entity_instances(
            &entity(1, 4, PlayerId(0), MoveState::Idle),
            &shape,
            false,
            false,
            0,
            &mut out_p0,
        );
        for (red, blue) in out.iter().zip(out_p0.iter()) {
            assert_eq!(red.color, blue.color, "ore reads the same for both");
        }
    }

    #[test]
    fn selection_brightens_and_flashing_brightens_more() {
        let mut plain = Vec::new();
        build_entity_instances(
            &entity(1, 0, PlayerId(0), MoveState::Idle),
            &unit_shape(),
            false,
            false,
            0,
            &mut plain,
        );
        let mut selected = Vec::new();
        build_entity_instances(
            &entity(1, 0, PlayerId(0), MoveState::Idle),
            &unit_shape(),
            true,
            false,
            0,
            &mut selected,
        );
        let mut flashing = Vec::new();
        build_entity_instances(
            &entity(1, 0, PlayerId(0), MoveState::Idle),
            &unit_shape(),
            false,
            true,
            0,
            &mut flashing,
        );
        let brightness = |instances: &[EntityInstance]| instances[0].color.iter().sum::<f32>();
        assert!(
            brightness(&selected) > brightness(&plain),
            "selection brightens"
        );
        assert!(brightness(&flashing) > brightness(&plain), "a hit flashes");
    }

    #[test]
    fn the_stream_respects_the_instance_capacity_whole_silhouettes_only() {
        let snapshot_entities: Vec<RenderEntity> = (1..=4)
            .map(|id| entity(id, 0, PlayerId(0), MoveState::Idle))
            .collect();
        let snapshot = pandemonium_engine::RenderSnapshot {
            tick: 0,
            entities: snapshot_entities,
        };
        // Room for exactly three entities' silhouettes (2 instances each) —
        // the fourth is dropped whole, never half-drawn.
        let specs = [unit_shape(), unit_shape(), unit_shape(), unit_shape()];
        let instances = build_instances(&snapshot, &[], &[], &specs, &[], &[], 6);
        assert_eq!(instances.len(), 6);
        assert!(instances
            .iter()
            .all(|i| i.position[0] == 5.0 && i.position[2] == 7.0));
    }

    #[test]
    fn unknown_kinds_fall_back_to_a_plain_unit() {
        let snapshot = pandemonium_engine::RenderSnapshot {
            tick: 0,
            entities: vec![entity(1, 99, PlayerId(0), MoveState::Idle)],
        };
        let instances = build_instances(&snapshot, &[], &[], &[], &[], &[], 64);
        assert_eq!(instances.len(), 1, "the fallback silhouette still renders");
    }
}

#[cfg(test)]
mod decal_tests {
    use super::entities::{team_color, TEAM_NEUTRAL};
    use super::overlay::{RING_COLOR, TEAM_RING_ALPHA};
    use super::*;
    use glam::Vec3;
    use pandemonium_engine::RenderEntity;
    use pandemonium_sim_api::{EntityId, KindId, MoveState, PlayerId};

    fn snapshot_with(kinds: &[(u64, u32)]) -> pandemonium_engine::RenderSnapshot {
        pandemonium_engine::RenderSnapshot {
            tick: 0,
            entities: kinds
                .iter()
                .map(|&(id, kind)| RenderEntity {
                    id: EntityId(id),
                    owner: PlayerId(0),
                    kind: KindId(kind),
                    pos: Vec3::new(id as f32, 0.0, 0.0),
                    facing: Vec3::ZERO,
                    hp_fraction_milli: 1000,
                    move_state: MoveState::Idle,
                })
                .collect(),
        }
    }

    #[test]
    fn every_entity_gets_exactly_one_shadow() {
        let snapshot = snapshot_with(&[(1, 0), (2, 0), (3, 0)]);
        let shadows = build_shadows(&snapshot, &[], 64);
        assert_eq!(shadows.len(), 3);
        for (index, shadow) in shadows.iter().enumerate() {
            assert_eq!(shadow.position[1], DECAL_LIFT, "shadows hug the ground");
            assert!(shadow.scale[0] > 0.0 && shadow.scale[1] > 0.0);
            assert_eq!(shadow.color[3], 0.34, "uniform shadow opacity");
            assert_eq!(
                shadow.mode,
                overlay::DECAL_SOFT_RADIAL,
                "shadows fade at the rim"
            );
            assert_eq!(shadow.position[0], (index + 1) as f32, "under its entity");
        }
    }

    #[test]
    fn rings_land_only_under_selected_entities_and_read_green() {
        let snapshot = snapshot_with(&[(1, 0), (2, 0)]);
        let selection = [EntityId(2)];
        let rings = build_rings(&snapshot, &selection, &[], 64);
        assert_eq!(rings.len(), 1);
        assert_eq!(rings[0].position[0], 2.0, "under the selected entity");
        assert_eq!(rings[0].color, RING_COLOR);
        // The ring rides a hair above the shadow plane.
        assert!(rings[0].position[1] > DECAL_LIFT);
        let empty = build_rings(&snapshot, &[], &[], 64);
        assert!(empty.is_empty());
    }

    #[test]
    fn team_p1_reads_orange_and_p0_blue() {
        // A-102's colorblind-safe pair: P1 shifted off content-red onto
        // orange, P0 keeping the content blue. Presentation-only mapping.
        assert_eq!(team_color(PlayerId(0), false, false), TEAM_P0);
        assert_eq!(team_color(PlayerId(1), false, false), TEAM_P1);
        assert_eq!(team_color(PlayerId::NEUTRAL, false, false), TEAM_NEUTRAL);
        // Blue and orange stay far apart in the red channel (the channel a
        // deuteranope still reads reliably).
        let spread = TEAM_P1[0] - TEAM_P0[0];
        assert!(spread > 0.5, "the two team colors separate on red");
        // Selection brightens but never flips teams.
        assert!(team_color(PlayerId(1), true, false)[0] > TEAM_P1[0]);
    }

    #[test]
    fn team_rings_land_under_units_in_team_colors() {
        // A P0 worker and a P1 worker side by side: every unit rings in its
        // owner's team color.
        let snapshot = pandemonium_engine::RenderSnapshot {
            tick: 0,
            entities: vec![
                RenderEntity {
                    id: EntityId(1),
                    owner: PlayerId(0),
                    kind: KindId(0),
                    pos: Vec3::new(5.0, 0.0, 0.0),
                    facing: Vec3::ZERO,
                    hp_fraction_milli: 1000,
                    move_state: MoveState::Idle,
                },
                RenderEntity {
                    id: EntityId(2),
                    owner: PlayerId(1),
                    kind: KindId(0),
                    pos: Vec3::new(9.0, 0.0, 0.0),
                    facing: Vec3::ZERO,
                    hp_fraction_milli: 1000,
                    move_state: MoveState::Idle,
                },
            ],
        };
        let worker = crate::silhouette::named("worker").expect("authored");
        let rings = build_team_rings(&snapshot, &[worker.clone(), worker], 64);
        assert_eq!(rings.len(), 2);
        assert_eq!(&rings[0].color[..3], &TEAM_P0[..]);
        assert_eq!(&rings[1].color[..3], &TEAM_P1[..]);
        assert_eq!(rings[0].color[3], TEAM_RING_ALPHA);
        // A structure silhouette never rings (it has the band instead) —
        // the spec sits at the entity's kind index.
        let cc = crate::silhouette::named("command_center").expect("authored");
        assert!(
            build_team_rings(&snapshot_with(&[(3, 0)]), &[cc], 64).is_empty(),
            "a command center gets no unit ring"
        );
    }

    #[test]
    fn bigger_kinds_cast_bigger_shadows() {
        let structure = crate::silhouette::named("command_center").expect("authored");
        let unit = crate::silhouette::named("worker").expect("authored");
        assert!(
            structure.radius > unit.radius,
            "a command center shadows more than a worker"
        );
        // And the ring is slightly larger than its shadow.
        assert!(unit.radius + 0.12 > unit.radius);
    }

    #[test]
    fn the_decal_pulse_breathes_gently_and_repeats() {
        // The pulse swings in a calm 0.80..1.0 band — visible motion, never
        // a flicker — and every part of the swing stays a legal alpha
        // multiplier. (The bounds carry an epsilon: the sine's minimum is
        // 0.90 - 0.10, which f32 rounds to 0.79999995.)
        for step in 0..180 {
            let phase = step as f32 * 0.01;
            let pulse = ring_pulse(phase);
            assert!(
                (0.7999..=1.0001).contains(&pulse),
                "pulse at {phase}s is {pulse}, outside the breathing band"
            );
        }
        // It breathes: the extremes and the midpoint all appear.
        let samples: Vec<f32> = (0..180).map(|s| ring_pulse(s as f32 * 0.01)).collect();
        assert!(samples.iter().any(|p| *p > 0.995), "the pulse peaks");
        assert!(samples.iter().any(|p| *p < 0.805), "the pulse dips");
        // And it is periodic on the documented period, so a long session
        // never drifts (1.8 s = the 180 samples above, all cycles equal).
        assert!((ring_pulse(0.0) - ring_pulse(1.8)).abs() < 1e-4);
        assert!((ring_pulse(0.3) - ring_pulse(2.1)).abs() < 1e-4);
    }
}

#[cfg(test)]
mod death_render_tests {
    use super::*;
    use pandemonium_sim_api::{KindId, PlayerId};

    fn cue(progress: f32) -> DyingCue {
        DyingCue {
            pos: [5.0, 0.0, 7.0],
            kind: KindId(0),
            owner: PlayerId(0),
            progress,
        }
    }

    fn unit_shape() -> KindShape {
        crate::silhouette::Silhouette::from_parts(
            true,
            vec![
                crate::silhouette::Part::at(
                    [0.0, 0.3, 0.0],
                    [0.4, 0.6, 0.4],
                    crate::silhouette::Tone::Body,
                ),
                crate::silhouette::Part::at(
                    [0.0, 0.7, 0.0],
                    [0.2, 0.2, 0.2],
                    crate::silhouette::Tone::Dark,
                ),
            ],
        )
    }

    #[test]
    fn a_dying_mass_shrinks_and_chars_with_progress() {
        let fresh = death_instance(&cue(0.0), &unit_shape());
        let late = death_instance(&cue(1.0), &unit_shape());
        assert!(
            late.scale[1] < fresh.scale[1],
            "the mass shrinks as it dies"
        );
        assert!(
            late.position[1] < fresh.position[1],
            "it sinks into the ground"
        );
        let brightness = |i: &EntityInstance| i.color.iter().sum::<f32>();
        assert!(
            brightness(&late) < brightness(&fresh),
            "it chars toward soot"
        );
        // The shrink bottoms out at a quarter (it never inverts).
        let clamped = death_instance(&cue(2.0), &unit_shape());
        assert!(clamped.scale[1] > 0.0);
    }
}

#[cfg(test)]
mod minimap_marker_tests {
    use super::minimap::{MINIMAP_ENEMY, MINIMAP_ORE, MINIMAP_OWN};
    use super::*;
    use glam::Vec3;
    use pandemonium_engine::RenderEntity;
    use pandemonium_sim_api::{EntityId, KindId, MoveState, PlayerId};

    const MAP: (u32, u32) = (8, 8);

    fn entity(id: u64, kind: u32, owner: PlayerId, x: f32, z: f32) -> RenderEntity {
        RenderEntity {
            id: EntityId(id),
            owner,
            kind: KindId(kind),
            pos: Vec3::new(x, 0.0, z),
            facing: Vec3::ZERO,
            hp_fraction_milli: 1000,
            move_state: MoveState::Idle,
        }
    }

    fn tile(scratch: &[u8], x: u32, y: u32) -> [u8; 4] {
        let offset = ((y * MAP.0 + x) as usize) * 4;
        [
            scratch[offset],
            scratch[offset + 1],
            scratch[offset + 2],
            scratch[offset + 3],
        ]
    }

    fn specs() -> Vec<KindShape> {
        vec![
            crate::silhouette::named("worker").expect("authored"),
            crate::silhouette::named("command_center").expect("authored"),
            crate::silhouette::named("ore_node").expect("authored"),
        ]
    }

    #[test]
    fn units_paint_one_tile_in_the_owner_color() {
        let mut scratch = vec![0u8; (MAP.0 * MAP.1 * 4) as usize];
        paint_minimap_markers(
            &mut scratch,
            &[entity(1, 0, PlayerId(0), 2.5, 3.7)],
            &specs(),
            MAP,
        );
        assert_eq!(tile(&scratch, 2, 3)[0], MINIMAP_OWN[0] as u8);
        assert_eq!(tile(&scratch, 2, 3)[1], MINIMAP_OWN[1] as u8);
        // The neighbors stay bare: one tile, no block.
        assert_eq!(tile(&scratch, 3, 3)[3], 0);
        assert_eq!(tile(&scratch, 2, 2)[3], 0);
        // Enemy units read orange.
        let mut scratch2 = vec![0u8; (MAP.0 * MAP.1 * 4) as usize];
        paint_minimap_markers(
            &mut scratch2,
            &[entity(2, 0, PlayerId(1), 2.5, 3.7)],
            &specs(),
            MAP,
        );
        assert_eq!(tile(&scratch2, 2, 3)[0], MINIMAP_ENEMY[0] as u8);
    }

    #[test]
    fn buildings_paint_bigger_blocks_than_units() {
        let mut scratch = vec![0u8; (MAP.0 * MAP.1 * 4) as usize];
        paint_minimap_markers(
            &mut scratch,
            &[entity(1, 1, PlayerId(0), 5.0, 5.0)],
            &specs(),
            MAP,
        );
        // The 2x2 block: four painted tiles around the anchor.
        let painted = [(4, 4), (5, 4), (4, 5), (5, 5)];
        for (x, y) in painted {
            assert_eq!(tile(&scratch, x, y)[3], 255, "painted at {x},{y}");
            assert_eq!(tile(&scratch, x, y)[0], MINIMAP_OWN[0] as u8);
        }
        // The tiles just outside the block stay bare.
        assert_eq!(tile(&scratch, 6, 6)[3], 0);
        assert_eq!(tile(&scratch, 3, 3)[3], 0);
    }

    #[test]
    fn ore_paints_an_amber_diamond() {
        let mut scratch = vec![0u8; (MAP.0 * MAP.1 * 4) as usize];
        paint_minimap_markers(
            &mut scratch,
            &[entity(1, 2, PlayerId::NEUTRAL, 4.0, 4.0)],
            &specs(),
            MAP,
        );
        // The plus-shaped diamond: center + the four edge-adjacent tiles.
        for (x, y) in [(4, 4), (3, 4), (5, 4), (4, 3), (4, 5)] {
            assert_eq!(tile(&scratch, x, y)[0], MINIMAP_ORE[0] as u8, "{x},{y}");
        }
        // The corners the diamond trims stay bare.
        assert_eq!(tile(&scratch, 3, 3)[3], 0);
        assert_eq!(tile(&scratch, 5, 5)[3], 0);
    }

    #[test]
    fn off_map_entities_paint_nothing_and_clamp_does_not_panic() {
        let mut scratch = vec![0u8; (MAP.0 * MAP.1 * 4) as usize];
        paint_minimap_markers(
            &mut scratch,
            &[
                entity(1, 0, PlayerId(0), 20.0, 20.0),
                entity(2, 1, PlayerId(0), 0.0, 0.0),
            ],
            &specs(),
            MAP,
        );
        // The corner building clamps inside the map without panicking.
        assert_eq!(tile(&scratch, 0, 0)[3], 255);
        assert_eq!(tile(&scratch, 7, 7)[3], 0, "the far corner stays terrain");
    }
}

#[cfg(test)]
mod skin_tests {
    use super::entities::group_by_skin;
    use super::minimap::cube_vertices;
    use super::*;
    use glam::Vec3;
    use pandemonium_engine::RenderEntity;
    use pandemonium_sim_api::{EntityId, KindId, MoveState, PlayerId};

    fn entity(id: u64, kind: u32) -> RenderEntity {
        RenderEntity {
            id: EntityId(id),
            owner: PlayerId(0),
            kind: KindId(kind),
            pos: Vec3::new(id as f32, 0.0, 0.0),
            facing: Vec3::ZERO,
            hp_fraction_milli: 1000,
            move_state: MoveState::Idle,
        }
    }

    fn snapshot(kinds: &[u32]) -> pandemonium_engine::RenderSnapshot {
        pandemonium_engine::RenderSnapshot {
            tick: 0,
            entities: kinds
                .iter()
                .enumerate()
                .map(|(index, &kind)| entity(index as u64 + 1, kind))
                .collect(),
        }
    }

    #[test]
    fn every_kind_draws_with_its_own_skin_slot() {
        // Three kinds — kind 0 skinned (slot 2), kind 1 skinned (slot 1),
        // kind 2 unskinned — every instance a kind emits carries that
        // kind's slot, never the placeholder's.
        let specs = [unit_shape(), unit_shape(), unit_shape()];
        let slots = [2u32, 1u32, 0u32];
        let instances = build_instances(&snapshot(&[0, 1, 2]), &[], &[], &specs, &slots, &[], 64);
        assert!(instances
            .iter()
            .filter(|i| i.position[0] == 1.0)
            .all(|i| i.skin == 2));
        assert!(instances
            .iter()
            .filter(|i| i.position[0] == 2.0)
            .all(|i| i.skin == 1));
        assert!(instances
            .iter()
            .filter(|i| i.position[0] == 3.0)
            .all(|i| i.skin == 0));
    }

    #[test]
    fn kinds_without_a_slot_table_entry_draw_placeholder() {
        // A slot table shorter than the kind list (or empty — the no-pack
        // client) must fall back to slot 0, never panic.
        let specs = [unit_shape(), unit_shape(), unit_shape()];
        let instances = build_instances(&snapshot(&[0, 2]), &[], &[], &specs, &[], &[], 64);
        assert!(!instances.is_empty());
        assert!(
            instances.iter().all(|i| i.skin == 0),
            "no slot table => the placeholder skin"
        );
    }

    #[test]
    fn grouping_makes_one_contiguous_run_per_skin() {
        // An interleaved stream (0, 2, 1, 0) groups into contiguous runs:
        // slot 0 first, then 1, then 2, each run preserving arrival order,
        // and the draw list covering every instance exactly once.
        let stream = |skins: &[u32]| {
            skins
                .iter()
                .map(|&skin| EntityInstance {
                    position: [skin as f32, 0.0, 0.0],
                    yaw: 0.0,
                    scale: [1.0, 1.0, 1.0],
                    color: [1.0, 1.0, 1.0],
                    skin,
                    material: 0.0,
                })
                .collect::<Vec<_>>()
        };
        let (ordered, draws) = group_by_skin(stream(&[0, 2, 1, 0]), 3);
        assert_eq!(draws, vec![(0, 0, 2), (1, 2, 1), (2, 3, 1)]);
        // Within slot 0, arrival order survives (both position[0] == 0).
        assert_eq!(ordered[0].position[0], 0.0);
        assert_eq!(ordered[1].position[0], 0.0);
        // The runs are contiguous: start + count chains to the next start.
        let mut covered = 0u64;
        for (_, start, count) in &draws {
            assert_eq!(*start, covered, "runs leave no gaps");
            covered += *count;
        }
        assert_eq!(covered, ordered.len() as u64);
    }

    #[test]
    fn grouping_skips_empty_skins_and_clamps_bogus_slots() {
        // Slot 1 has no instances: it gets no draw. A corrupt slot beyond
        // the table clamps into the last slot instead of indexing past the
        // bind groups.
        let stream = |skin: u32| {
            vec![EntityInstance {
                position: [0.0; 3],
                yaw: 0.0,
                scale: [1.0, 1.0, 1.0],
                color: [1.0, 1.0, 1.0],
                skin,
                material: 0.0,
            }]
        };
        let (ordered, draws) = group_by_skin(stream(0), 3);
        assert_eq!(draws, vec![(0, 0, 1)], "only the used slot draws");
        assert_eq!(ordered.len(), 1);
        let (ordered, draws) = group_by_skin(stream(9), 3);
        assert_eq!(draws, vec![(2, 0, 1)], "a bogus slot clamps into the last");
        assert_eq!(ordered[0].skin, 9, "clamping never rewrites the stream");
        // An empty stream answers with an empty draw list.
        let (_, draws) = group_by_skin(Vec::new(), 3);
        assert!(draws.is_empty());
    }

    #[test]
    fn every_cube_face_unwraps_the_whole_texture_space() {
        // Box mapping: all six faces must sample the full 0..1 square —
        // each face's four corners are the four UV corners, so a textured
        // box shows the whole image on every side (and a distinctive test
        // texture is unmistakably *textured*, not tinted).
        let vertices = cube_vertices();
        assert_eq!(vertices.len(), 36);
        for (face, quad) in vertices.chunks(6).enumerate() {
            let mut corners = std::collections::BTreeSet::new();
            for vertex in quad {
                assert!(
                    (0.0..=1.0).contains(&vertex.uv[0]) && (0.0..=1.0).contains(&vertex.uv[1]),
                    "face {face} uv {} {} outside the unit square",
                    vertex.uv[0],
                    vertex.uv[1]
                );
                let (u, v) = (vertex.uv[0] * 2.0, vertex.uv[1] * 2.0);
                corners.insert((u.round() as i32, v.round() as i32));
            }
            assert_eq!(corners.len(), 4, "face {face} must use all four UV corners");
        }
        // The four side faces (faces 0..4 in emission order) stand the
        // image upright: their top edge (y = +0.5) maps to v = 0, the
        // image's first row. The top/bottom faces map the ground plane
        // instead, so only the side quads carry the upright law.
        for (face, quad) in vertices.chunks(6).enumerate() {
            if face >= 4 {
                continue;
            }
            for vertex in quad {
                if vertex.corner[1] > 0.4 {
                    assert!(
                        vertex.uv[1].abs() < 1e-4,
                        "side face {face}'s top edge must be the image's top"
                    );
                }
            }
        }
    }

    fn unit_shape() -> KindShape {
        crate::silhouette::Silhouette::from_parts(
            true,
            vec![crate::silhouette::Part::at(
                [0.0, 0.3, 0.0],
                [0.4, 0.6, 0.4],
                crate::silhouette::Tone::Body,
            )],
        )
    }
}

#[cfg(test)]
mod model_pass_tests {
    //! The M12 phase 3 acceptance set (A-016 §12): the direction-pinned
    //! orientation math (the M9.1 lesson — presentation direction is
    //! invisible to hashes, so it is pinned by name at yaw 0 and yaw 90),
    //! the headless model-pipeline conversion (fixture GLB → GPU-ready
    //! buffers), and the instance-stream laws.

    use super::entities::{
        bake_gltf_to_entity, build_model_instances, entity_yaw_rotate, ModelInstance,
    };
    use super::*;
    use glam::Vec3;
    use pandemonium_engine::RenderEntity;
    use pandemonium_sim_api::{EntityId, KindId, MoveState, PlayerId};

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    fn assert_vec3(actual: [f32; 3], expected: [f32; 3], what: &str) {
        assert!(
            approx(actual[0], expected[0])
                && approx(actual[1], expected[1])
                && approx(actual[2], expected[2]),
            "{what}: got ({}, {}, {}), want ({}, {}, {})",
            actual[0],
            actual[1],
            actual[2],
            expected[0],
            expected[1],
            expected[2]
        );
    }

    // -- The direction-pinned orientation math (yaw 0 and yaw 90) ----------

    /// Yaw 0: an entity facing east (facing (1, 0, 0), yaw = atan2(0, 1) =
    /// 0). The model's glTF forward (−Z) must land on east — the same
    /// direction the box silhouettes put "forward" parts at yaw 0.
    #[test]
    fn at_yaw_0_the_model_forward_points_east() {
        let forward = bake_gltf_to_entity([0.0, 0.0, -1.0]);
        assert_vec3(forward, [1.0, 0.0, 0.0], "the baked forward");
        let world = entity_yaw_rotate(0.0, forward);
        assert_vec3(world, [1.0, 0.0, 0.0], "the drawn forward at yaw 0");
        // And the entity convention agrees: facing east *is* yaw 0.
        let east = Vec3::new(1.0, 0.0, 0.0);
        assert!((east.z.atan2(east.x)).abs() < 1e-5);
    }

    /// Yaw 90: an entity facing north (facing (0, 0, 1), yaw = atan2(1, 0) =
    /// π/2). The same baked forward must now land on north.
    #[test]
    fn at_yaw_90_the_model_forward_points_north() {
        let north = Vec3::new(0.0, 0.0, 1.0);
        let yaw = north.z.atan2(north.x);
        assert!((yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        let forward = bake_gltf_to_entity([0.0, 0.0, -1.0]);
        let world = entity_yaw_rotate(yaw, forward);
        assert_vec3(world, [0.0, 0.0, 1.0], "the drawn forward at yaw 90");
    }

    /// The model's right side and up land where the box silhouettes carry
    /// theirs: at yaw 0 (facing east) the model's right (+X) points to +Z
    /// (the a_facing_turn test's "right = forward × up" law), and at yaw 90
    /// (facing north) it points west (−X) — exactly where that test watches
    /// a north-facing rifleman's right-hand carry land.
    #[test]
    fn the_model_right_and_up_match_the_box_convention() {
        let right = bake_gltf_to_entity([1.0, 0.0, 0.0]);
        assert_vec3(right, [0.0, 0.0, 1.0], "the baked right");
        assert_vec3(
            entity_yaw_rotate(0.0, right),
            [0.0, 0.0, 1.0],
            "right at yaw 0 (east-facing): +Z",
        );
        assert_vec3(
            entity_yaw_rotate(std::f32::consts::FRAC_PI_2, right),
            [-1.0, 0.0, 0.0],
            "right at yaw 90 (north-facing): -X, like the rifleman's carry",
        );
        let up = bake_gltf_to_entity([0.0, 1.0, 0.0]);
        assert_vec3(up, [0.0, 1.0, 0.0], "up stays up through the bake");
        assert_vec3(
            entity_yaw_rotate(2.1, up),
            [0.0, 1.0, 0.0],
            "and through any yaw",
        );
    }

    /// The bake is a proper rotation (determinant +1), never a mirror: a
    /// mirrored model would render inside-out and light backwards, and no
    /// direction test would catch it directly — the handedness does.
    #[test]
    fn the_bake_preserves_handedness() {
        let x = bake_gltf_to_entity([1.0, 0.0, 0.0]);
        let y = bake_gltf_to_entity([0.0, 1.0, 0.0]);
        let z = bake_gltf_to_entity([0.0, 0.0, 1.0]);
        let cross = [
            y[1] * z[2] - y[2] * z[1],
            y[2] * z[0] - y[0] * z[2],
            y[0] * z[1] - y[1] * z[0],
        ];
        assert_vec3(cross, x, "cross(baked Y, baked Z) == baked X");
    }

    // -- The headless pipeline conversion (fixture GLB → GPU-ready) --------

    /// Assembles a GLB container around one JSON chunk and one BIN chunk
    /// (the loader suite's container discipline, kept local so this module
    /// stays self-contained).
    fn assemble_glb(json: &str, bin: &[u8]) -> Vec<u8> {
        let mut json_chunk = json.as_bytes().to_vec();
        while !json_chunk.len().is_multiple_of(4) {
            json_chunk.push(b' ');
        }
        let bin_padded = bin.len().div_ceil(4) * 4;
        let total = 12 + 8 + json_chunk.len() + 8 + bin_padded;
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json_chunk);
        out.extend_from_slice(&(bin_padded as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(bin);
        out.extend(std::iter::repeat_n(0u8, bin_padded - bin.len()));
        out
    }

    fn push_f32s(out: &mut Vec<u8>, values: &[f32]) {
        for value in values {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }

    fn push_u16s(out: &mut Vec<u8>, values: &[u16]) {
        for value in values {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }

    /// One team-colored quad in meters: 4 positions (a recognizable
    /// corner first), 4 up-normals, 4 uvs, 6 u16 indices.
    fn quad_glb() -> Vec<u8> {
        let mut bin = Vec::new();
        push_f32s(
            &mut bin,
            &[
                -0.5, -0.5, -0.5, 0.5, -0.5, -0.5, -0.5, 0.5, -0.5, 0.5, 0.5, -0.5,
            ],
        );
        for _ in 0..4 {
            push_f32s(&mut bin, &[0.0, 1.0, 0.0]);
        }
        for uv in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]] {
            push_f32s(&mut bin, &uv);
        }
        push_u16s(&mut bin, &[0, 1, 2, 1, 3, 2]);
        let json = r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],"meshes":[{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2},"indices":3,"material":0}]}],"materials":[{"name":"team_color","pbrMetallicRoughness":{"baseColorFactor":[0.25,0.5,0.75,1.0]}}],"accessors":[{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3","min":[-0.5,-0.5,-0.5],"max":[0.5,0.5,0.5]},{"bufferView":1,"componentType":5126,"count":4,"type":"VEC3"},{"bufferView":2,"componentType":5126,"count":4,"type":"VEC2"},{"bufferView":3,"componentType":5123,"count":6,"type":"SCALAR"}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":48},{"buffer":0,"byteOffset":48,"byteLength":48},{"buffer":0,"byteOffset":96,"byteLength":32},{"buffer":0,"byteOffset":128,"byteLength":12}],"buffers":[{"byteLength":140}]}"#;
        assemble_glb(json, &bin)
    }

    /// A fixture GLB is GPU-ready headlessly: load → convert → assert every
    /// baked quantity — the meters-to-tiles division, the glTF-to-entity
    /// basis, the passthrough of indices/uvs, the team flag, and the
    /// material factor the vertices carry (the plan's "fixture .glb →
    /// GPU-ready buffers", all pure functions, no GPU).
    #[test]
    fn a_fixture_glb_becomes_gpu_ready_buffers_headless() {
        let data = model::load(&quad_glb()).expect("the fixture quad loads");
        let primitives = model::gpu_primitives(
            &data,
            &ModelPlacement::default(),
            METERS_PER_TILE,
            bake_gltf_to_entity,
        );
        assert_eq!(primitives.len(), 1);
        let quad = &primitives[0];
        assert_eq!(
            quad.vertices.len(),
            4,
            "no duplication with normals present"
        );
        assert_eq!(quad.indices.as_deref(), Some(&[0u32, 1, 2, 1, 3, 2][..]));
        // Vertex 0 sits at (-0.5, -0.5, -0.5) m: ÷3 → (-1/6, -1/6, -1/6)
        // tiles, then the basis bake (x,y,z) → (-z,y,x) → (1/6, -1/6, -1/6).
        assert_vec3(
            quad.vertices[0].position,
            [1.0 / 6.0, -1.0 / 6.0, -1.0 / 6.0],
            "the first corner, in tiles on the entity basis",
        );
        // The up-normal survives both the bake and the placement untouched.
        assert_vec3(quad.vertices[0].normal, [0.0, 1.0, 0.0], "the up normal");
        // The factor rides every vertex with the team flag set.
        for vertex in &quad.vertices {
            assert_eq!(vertex.base_color, [0.25, 0.5, 0.75, 1.0]);
        }
        assert!(quad.team_tint);
        assert_eq!(quad.factor, [0.25, 0.5, 0.75]);
        assert!(quad.texture.is_none());
        assert_eq!(quad.vertices[2].uv, [0.0, 1.0], "uvs pass through");
    }

    /// The manifest placement (scale, rotation_deg, ground_offset — A-144)
    /// bakes into the vertices exactly once, in the documented order:
    /// rotate about +Y, scale in meters, lift in meters, divide by
    /// METERS_PER_TILE, then the basis bake.
    #[test]
    fn the_manifest_placement_bakes_in_the_documented_order() {
        let data = model::load(&quad_glb()).expect("the fixture quad loads");
        let placement = ModelPlacement {
            scale: 2.0,
            rotation_deg: 90.0,
            ground_offset: 3.0,
        };
        let primitives =
            model::gpu_primitives(&data, &placement, METERS_PER_TILE, bake_gltf_to_entity);
        // Vertex 1 at (0.5, -0.5, -0.5) m: Ry(90°) → (cos90·0.5 + sin90·(-0.5),
        // -0.5, -sin90·0.5 + cos90·(-0.5)) = (-0.5, -0.5, -0.5); ×2 →
        // (-1, -1, -1); +offset → (-1, 2, -1); ÷3 → (-1/3, 2/3, -1/3);
        // bake → (1/3, 2/3, -1/3).
        assert_vec3(
            primitives[0].vertices[1].position,
            [1.0 / 3.0, 2.0 / 3.0, -1.0 / 3.0],
            "vertex 1 under the full placement",
        );
    }

    /// A normal-less primitive gets true flat normals — one face normal per
    /// triangle on its own three vertices, the index stream dropped (the
    /// vertices were duplicated per triangle).
    #[test]
    fn a_normalless_primitive_gets_flat_normals() {
        let mut bin = Vec::new();
        // One ground triangle in meters.
        push_f32s(&mut bin, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
        let json = r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],"meshes":[{"primitives":[{"attributes":{"POSITION":0},"material":0}]}],"materials":[{"name":"body","pbrMetallicRoughness":{"baseColorFactor":[1.0,1.0,1.0,1.0]}}],"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,0.0,1.0]}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36}],"buffers":[{"byteLength":36}]}"#;
        let data = model::load(&assemble_glb(json, &bin)).expect("the triangle loads");
        let primitives = model::gpu_primitives(
            &data,
            &ModelPlacement::default(),
            METERS_PER_TILE,
            bake_gltf_to_entity,
        );
        let triangle = &primitives[0];
        assert_eq!(triangle.vertices.len(), 3, "one vertex per corner");
        assert!(
            triangle.indices.is_none(),
            "the duplicated stream drops indices"
        );
        assert!(!triangle.team_tint, "the body material is not team_color");
        // The face normal of (0,0,0)-(1,0,0)-(0,0,1) is -Y in glTF; the
        // bake keeps it -Y.
        for vertex in &triangle.vertices {
            assert_vec3(vertex.normal, [0.0, -1.0, 0.0], "the flat face normal");
        }
    }

    /// A textured material's PNG bytes ride the primitive for the renderer
    /// to decode and upload, with a white factor on the vertices (the texel
    /// modulates instead) — the skin path's discipline, generalized.
    #[test]
    fn a_textured_material_carries_its_png_and_a_white_factor() {
        let mut bin = Vec::new();
        push_f32s(
            &mut bin,
            &[
                -0.5, -0.5, -0.5, 0.5, -0.5, -0.5, -0.5, 0.5, -0.5, 0.5, 0.5, -0.5,
            ],
        );
        for _ in 0..4 {
            push_f32s(&mut bin, &[0.0, 1.0, 0.0]);
        }
        for uv in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]] {
            push_f32s(&mut bin, &uv);
        }
        push_u16s(&mut bin, &[0, 1, 2, 1, 3, 2]);
        bin.extend_from_slice(&[1u8, 2, 3, 4]); // the "PNG" payload (decoding is the renderer's step)
        let json = r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],"meshes":[{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2},"indices":3,"material":0}]}],"materials":[{"name":"camo","pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}],"textures":[{"source":0}],"images":[{"bufferView":4,"mimeType":"image/png"}],"accessors":[{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3","min":[-0.5,-0.5,-0.5],"max":[0.5,0.5,0.5]},{"bufferView":1,"componentType":5126,"count":4,"type":"VEC3"},{"bufferView":2,"componentType":5126,"count":4,"type":"VEC2"},{"bufferView":3,"componentType":5123,"count":6,"type":"SCALAR"}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":48},{"buffer":0,"byteOffset":48,"byteLength":48},{"buffer":0,"byteOffset":96,"byteLength":32},{"buffer":0,"byteOffset":128,"byteLength":12},{"buffer":0,"byteOffset":140,"byteLength":4}],"buffers":[{"byteLength":144}]}"#;
        let data = model::load(&assemble_glb(json, &bin)).expect("the textured quad loads");
        let primitives = model::gpu_primitives(
            &data,
            &ModelPlacement::default(),
            METERS_PER_TILE,
            bake_gltf_to_entity,
        );
        let quad = &primitives[0];
        assert_eq!(quad.texture.as_deref(), Some(&[1u8, 2, 3, 4][..]));
        assert_eq!(
            quad.factor,
            [1.0, 1.0, 1.0],
            "the factor is white — the texel modulates"
        );
        assert!(!quad.team_tint);
        for vertex in &quad.vertices {
            assert_eq!(vertex.base_color, [1.0, 1.0, 1.0, 0.0]);
        }
    }

    /// The renderer's embedded-PNG decoder (the material-albedo half of the
    /// upload path) round-trips a real encoded PNG under the skin path's
    /// guards — pure `image`-crate work, so it is testable headless.
    #[test]
    fn embedded_pngs_decode_under_the_skin_guards() {
        let mut png = Vec::new();
        let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 100, 50, 255]));
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encoding a 2x2 PNG");
        let decoded = decode_embedded_png(&png).expect("a real PNG decodes");
        assert_eq!((decoded.width, decoded.height), (2, 2));
        assert_eq!(decoded.rgba.len(), 16);
        // The guards: garbage bytes refuse with a reason, never a panic.
        assert!(decode_embedded_png(&[9, 9, 9, 9]).is_err());
        assert!(decode_embedded_png(b"").is_err());
    }

    // -- The instance stream (the per-entity half of the model pass) -------

    fn entity(id: u64, kind: u32, owner: PlayerId, facing: Vec3) -> RenderEntity {
        RenderEntity {
            id: EntityId(id),
            owner,
            kind: KindId(kind),
            pos: Vec3::new(5.0, 0.0, 7.0),
            facing,
            hp_fraction_milli: 1000,
            move_state: MoveState::Idle,
        }
    }

    fn snapshot(entities: Vec<RenderEntity>) -> pandemonium_engine::RenderSnapshot {
        pandemonium_engine::RenderSnapshot { tick: 0, entities }
    }

    /// Model kinds contribute one instance per entity, grouped per kind
    /// into contiguous runs, yawed by their facing, and team-colored —
    /// while their boxes vanish from the silhouette stream (one entity,
    /// one draw path).
    #[test]
    fn model_kinds_draw_one_instance_per_entity_and_skip_their_boxes() {
        let entities = vec![
            entity(1, 0, PlayerId(0), Vec3::new(1.0, 0.0, 0.0)),
            entity(2, 1, PlayerId(0), Vec3::ZERO), // a box kind
            entity(3, 0, PlayerId(1), Vec3::new(0.0, 0.0, 1.0)),
        ];
        let snap = snapshot(entities);
        let renders_model = [true, false];
        // The box stream: kind 1's silhouette only.
        let specs = [
            crate::silhouette::Silhouette::from_parts(
                true,
                vec![crate::silhouette::Part::at(
                    [0.0, 0.3, 0.0],
                    [0.4, 0.6, 0.4],
                    crate::silhouette::Tone::Body,
                )],
            ),
            crate::silhouette::Silhouette::from_parts(
                true,
                vec![crate::silhouette::Part::at(
                    [0.0, 0.3, 0.0],
                    [0.4, 0.6, 0.4],
                    crate::silhouette::Tone::Body,
                )],
            ),
        ];
        let boxes = build_instances(&snap, &[], &[], &specs, &[0, 0], &renders_model, 64);
        assert!(!boxes.is_empty(), "the box kind still draws its box");
        assert!(
            boxes.iter().all(|b| b.position[0] == 5.0),
            "only kind 1's box remains"
        );
        // The model stream: kind 0's two entities, one instance each.
        let (instances, draws) = build_model_instances(&snap, &[], &[], &renders_model, 64);
        assert_eq!(instances.len(), 2);
        assert_eq!(draws, vec![(0, 0, 2)], "one contiguous run for kind 0");
        assert!(
            (instances[0].yaw - 0.0).abs() < 1e-5,
            "east-facing yaws to 0"
        );
        assert!(
            (instances[1].yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-5,
            "north-facing yaws to 90"
        );
        assert_eq!(instances[0].color, TEAM_P0);
        assert_eq!(instances[1].color, TEAM_P1);
        assert_eq!(instances[0].position, [5.0, 0.0, 7.0]);
    }

    /// Selection and hit flashes brighten a model's team color exactly as
    /// they brighten the boxes' (the same `team_color` resolve).
    #[test]
    fn selection_brightens_a_models_team_color() {
        let snap = snapshot(vec![entity(1, 0, PlayerId(0), Vec3::new(1.0, 0.0, 0.0))]);
        let renders_model = [true];
        let (plain, _) = build_model_instances(&snap, &[], &[], &renders_model, 64);
        let (instances, _) = build_model_instances(&snap, &[EntityId(1)], &[], &renders_model, 64);
        let brightness = |instance: &ModelInstance| instance.color.iter().sum::<f32>();
        assert!(brightness(&instances[0]) > brightness(&plain[0]));
    }

    /// The stream respects the capacity whole entities at a time: a
    /// too-small budget clips the tail, never half-drawing a kind's run.
    #[test]
    fn the_model_stream_respects_the_capacity() {
        let entities: Vec<RenderEntity> = (1..=4)
            .map(|id| entity(id, 0, PlayerId(0), Vec3::new(1.0, 0.0, 0.0)))
            .collect();
        let snap = snapshot(entities);
        let renders_model = [true];
        let (instances, draws) = build_model_instances(&snap, &[], &[], &renders_model, 3);
        assert_eq!(instances.len(), 3, "the fourth is clipped");
        assert_eq!(draws, vec![(0, 0, 3)], "the run still covers exactly it");
    }

    /// No models anywhere (the no-pack client, or a skins-only pack): both
    /// paths behave exactly as before — every kind keeps its boxes, the
    /// model stream is empty.
    #[test]
    fn without_models_both_paths_stay_untouched() {
        let snap = snapshot(vec![entity(1, 0, PlayerId(0), Vec3::ZERO)]);
        let specs = [crate::silhouette::Silhouette::from_parts(
            true,
            vec![crate::silhouette::Part::at(
                [0.0, 0.3, 0.0],
                [0.4, 0.6, 0.4],
                crate::silhouette::Tone::Body,
            )],
        )];
        let boxes = build_instances(&snap, &[], &[], &specs, &[0], &[], 64);
        assert_eq!(boxes.len(), 1, "the box still draws");
        let (instances, draws) = build_model_instances(&snap, &[], &[], &[], 64);
        assert!(instances.is_empty());
        assert!(draws.is_empty());
    }
}
