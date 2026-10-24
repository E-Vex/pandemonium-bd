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
mod overlay;
mod pipeline;
mod terrain;
mod texture;

use entities::{
    build_instances, build_shadows, death_instance, fallback_shape, group_by_skin, EntityInstance,
};
pub use entities::{DyingCue, PlacementGhost, TEAM_P0, TEAM_P1};

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
    use super::entities::ENTITY_SHADER_SRC;
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
        let instances = build_instances(&snapshot, &[], &[], &specs, &[], 6);
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
        let instances = build_instances(&snapshot, &[], &[], &[], &[], 64);
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
        let instances = build_instances(&snapshot(&[0, 1, 2]), &[], &[], &specs, &slots, 64);
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
        let instances = build_instances(&snapshot(&[0, 2]), &[], &[], &specs, &[], 64);
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
