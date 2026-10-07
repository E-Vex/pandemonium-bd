//! The wgpu renderer (plan §11.2 as amended by ADR-0001): a depth-tested 3D
//! pass drawing the terrain mesh (from the map grid + display-only heightmap)
//! and instanced placeholder boxes for entities, through the engine's
//! [`Renderer`] trait so the sim-facing code stays GPU-independent.
//!
//! Placeholder art only (plan §0): flat per-tile colors, unit boxes, team
//! colors. The renderer receives interpolated [`RenderSnapshot`]s and camera
//! matrices — never the simulation itself (FD-6, FD-9).

use anyhow::Context;
use bytemuck::{Pod, Zeroable};
use pandemonium_engine::mesh::{TerrainMesh, TerrainVertex};
use pandemonium_engine::renderer::Frame;
use pandemonium_engine::Renderer;
use pandemonium_sim_api::{MoveState, PlayerId};
use wgpu::util::DeviceExt;

use crate::text::{TextAtlas, UiQuad};

/// The camera uniform: the combined view-projection matrix.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct CameraUniform {
    view_projection: [[f32; 4]; 4],
}

/// One entity instance: a rotated, non-uniformly-scaled colored box (40
/// bytes). One *entity* renders as one or more instances — its silhouette
/// (per [`KindShape`]) is composed from boxes on the CPU side.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct EntityInstance {
    /// Instance center (the box's bottom sits at this height).
    position: [f32; 3],
    /// Yaw rotation around Y (units face their movement direction).
    yaw: f32,
    /// Full extent on each axis (not half — scales read in tile units).
    scale: [f32; 3],
    /// Placeholder color (team color, brighter when selected).
    color: [f32; 3],
    /// Padding to a 40-byte stride (keeps vec3 attributes 4-byte aligned).
    _pad: f32,
}

/// What each kind renders as: the multi-part silhouette from the pure
/// [`silhouette`] module (PLAN-M10.2 §2.1 — distinct box compositions per
/// kind, keyed by kind name, with the capability-shape fallback living in
/// `silhouette::capability_fallback`). The type alias keeps the renderer's
/// historical `KindShape` name for the per-kind spec.
pub type KindShape = crate::silhouette::Silhouette;

const TERRAIN_SHADER: &str = r#"
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

const ENTITY_SHADER_SRC: &str = r#"
struct CameraUniform { view_projection: mat4x4<f32> };
@group(0) @binding(0) var<uniform> camera: CameraUniform;

const SUN: vec3<f32> = vec3<f32>(0.45, 0.85, 0.30);

struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) color: vec3<f32> };

@vertex fn vs_main(
    @location(0) corner: vec3<f32>,
    @location(1) instance_position: vec3<f32>,
    @location(2) instance_yaw: f32,
    @location(3) instance_scale: vec3<f32>,
    @location(4) instance_color: vec3<f32>,
    @location(5) normal: vec3<f32>,
) -> VsOut {
    var out: VsOut;
    let c = cos(instance_yaw);
    let s = sin(instance_yaw);
    let rot = mat3x3<f32>(vec3<f32>(c, 0.0, -s), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(s, 0.0, c));
    let world = rot * (corner * instance_scale) + instance_position;
    let n = rot * normalize(normal);
    let light = 0.42 + 0.58 * max(dot(n, normalize(SUN)), 0.0);
    out.clip = camera.view_projection * vec4<f32>(world, 1.0);
    out.color = instance_color * light;
    return out;
}

@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
"#;

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

/// The fog-of-war pass (plan §11.2's fog visualization): the terrain geometry
/// drawn a second time, UV-mapped in world space (x/z over the map extent) and
/// sampling a per-tile fog-alpha texture the client refreshes from its
/// `PlayerView`. Hidden tiles read nearly opaque, explored tiles half-lit,
/// visible tiles clear. Same geometry as the terrain means the same depth
/// values — the pass depth-tests `LessEqual` with writes off, so the fog hugs
/// the hills without z-fighting.
const FOG_SHADER_SRC: &str = r#"
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
struct GpuTerrainVertex {
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
struct FogUniform {
    view_projection: [[f32; 4]; 4],
    map_size: [f32; 2],
    _pad: [f32; 2],
}

/// Fog alpha per tile, encoded into the R8 texture (0.0 = clear over
/// visible ground, 1.0 = fully dark over never-seen ground).
pub const FOG_ALPHA_VISIBLE: u8 = 0;
pub const FOG_ALPHA_EXPLORED: u8 = 130;
pub const FOG_ALPHA_HIDDEN: u8 = 235;

/// The ground decal pass: instanced, alpha-blended quads laid flat on the
/// ground plane — blob shadows under everything, selection rings under the
/// selected. Depth-tested with writes off and a hair of lift so the decals
/// ride over the terrain and fog without z-fighting.
const DECAL_SHADER_SRC: &str = r#"
struct CameraUniform { view_projection: mat4x4<f32> };
@group(0) @binding(0) var<uniform> camera: CameraUniform;

struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) color: vec4<f32> };

@vertex fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) instance_position: vec3<f32>,
    @location(2) instance_scale: vec2<f32>,
    @location(3) instance_color: vec4<f32>,
) -> VsOut {
    var out: VsOut;
    let world = vec3<f32>(corner.x * instance_scale.x, 0.0, corner.y * instance_scale.y) + instance_position;
    out.clip = camera.view_projection * vec4<f32>(world, 1.0);
    out.color = instance_color;
    return out;
}

@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

/// One ground decal instance: center (y = the lift above the terrain),
/// extent on the two ground axes, and a premultiplied-alpha-friendly RGBA.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct DecalInstance {
    position: [f32; 3],
    scale: [f32; 2],
    color: [f32; 4],
}

/// The unit quad's corners in the XZ plane (two triangles), extent ±0.5.
fn quad_vertices() -> Vec<[f32; 2]> {
    let corners = [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]];
    let (a, b, c, d) = (corners[0], corners[1], corners[2], corners[3]);
    vec![a, b, c, a, c, d]
}

/// The unit ring (annulus) in the XZ plane: 24 segments between the inner
/// and outer radius. Instanced with the same decal pipeline, so the
/// selection ring is a ground-projected ellipse like a shadow, not a
/// screen-space bracket.
fn ring_vertices(segments: usize, inner: f32, outer: f32) -> Vec<[f32; 2]> {
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

/// The minimap pass: a screen-space quad sampling a small map-sized RGBA
/// texture the client composites (terrain base + fog + entity dots) every
/// frame. Drawn through the same pixel-space convention as the UI pass.
const MINIMAP_SHADER_SRC: &str = r#"
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
struct MinimapVertex {
    pos_uv: [f32; 4],
}

/// One cube vertex: corner position and its face normal (24 bytes).
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct CubeVertex {
    corner: [f32; 3],
    normal: [f32; 3],
}

/// The unit cube's 36 vertices (12 triangles, per-face normals), centered at
/// the origin, half-extent 0.5 on each axis.
fn cube_vertices() -> Vec<CubeVertex> {
    const FACES: [[usize; 4]; 6] = [
        [0, 1, 2, 3], // -Z
        [5, 4, 7, 6], // +Z
        [4, 0, 3, 7], // -X
        [1, 5, 6, 2], // +X
        [4, 5, 1, 0], // -Y
        [3, 2, 6, 7], // +Y
    ];
    const NORMALS: [[f32; 3]; 6] = [
        [0.0, 0.0, -1.0],
        [0.0, 0.0, 1.0],
        [-1.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 1.0, 0.0],
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
    for (face, quad) in FACES.iter().enumerate() {
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
                normal: NORMALS[face],
            });
        }
    }
    vertices
}

/// Team colors for the placeholder boxes (player slots 0 and 1, neutral).
/// M9 (plan §11.5, visual feedback): a flashing entity — one hit in the
/// last few presented frames — is pushed toward a hot white so a landed
/// hit reads on the frame it happens. Selection brightening still wins
/// when both apply (the player's own click feedback takes precedence).
fn team_color(owner: PlayerId, selected: bool, flashing: bool) -> [f32; 3] {
    let base = match owner {
        PlayerId(0) => [0.22, 0.45, 0.92],
        PlayerId(1) => [0.90, 0.30, 0.24],
        _ => [0.78, 0.66, 0.28], // Neutral (ore nodes).
    };
    if selected {
        [base[0] + 0.35, base[1] + 0.35, base[2] + 0.35]
    } else if flashing {
        [base[0] + 0.55, base[1] + 0.45, base[2] + 0.40]
    } else {
        base
    }
}

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
    entity_pipeline: wgpu::RenderPipeline,
    entity_vertex_buf: wgpu::Buffer,
    entity_instance_buf: wgpu::Buffer,
    entity_instance_capacity: u64,
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
    fog_bind_group: wgpu::BindGroup,
    fog_texture: wgpu::Texture,
    fog_texture_size: (u32, u32),
    fog_map_size: (f32, f32),
    /// Per-kind silhouette specs (index = KindId), derived from the loaded
    /// bundle by the client (the kind-name table + capability fallback in
    /// [`crate::silhouette`]) and handed to the renderer once at startup.
    kind_specs: Vec<KindShape>,
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
    minimap_bind_group: wgpu::BindGroup,
    minimap_pipeline: wgpu::RenderPipeline,
    minimap_vertex_buf: wgpu::Buffer,
    minimap_rect: Option<[f32; 4]>,
    /// The baked per-tile terrain base color (RGBA, map-sized row-major).
    minimap_base: Vec<[u8; 4]>,
    minimap_scratch: Vec<u8>,
    minimap_size: (u32, u32),
}

impl WgpuRenderer {
    /// Creates the renderer for a window: device, queues, pipelines, and the
    /// static terrain buffers. `kind_specs` maps each content kind id to its
    /// multi-part silhouette (the client derives them from the loaded bundle).
    pub fn new(
        window: std::sync::Arc<winit::window::Window>,
        terrain: &TerrainMesh,
        kind_specs: Vec<KindShape>,
    ) -> anyhow::Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = instance
            .create_surface(window.clone())
            .context("creating the window surface")?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .context("requesting a GPU adapter (software rasterizers count)")?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .context("requesting the GPU device")?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .find(|format| format.is_srgb())
            .copied()
            .unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: Vec::new(),
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let depth_view = Self::make_depth_view(&device, config.width, config.height);

        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera"),
            contents: bytemuck::bytes_of(&CameraUniform {
                view_projection: glam::Mat4::IDENTITY.to_cols_array_2d(),
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera bind group"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("main layout"),
            bind_group_layouts: &[&camera_layout],
            push_constant_ranges: &[],
        });

        let terrain_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain shader"),
            source: wgpu::ShaderSource::Wgsl(TERRAIN_SHADER.into()),
        });
        let terrain_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &terrain_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuTerrainVertex>() as u64,
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
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 24,
                            shader_location: 2,
                        },
                    ],
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None, // the placeholder terrain's winding is not load-bearing
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
                module: &terrain_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        let terrain_vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain vertices"),
            contents: bytemuck::cast_slice(
                &terrain
                    .vertices
                    .iter()
                    .copied()
                    .map(GpuTerrainVertex::from)
                    .collect::<Vec<_>>(),
            ),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let terrain_index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain indices"),
            contents: bytemuck::cast_slice(&terrain.indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let entity_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("entity shader"),
            source: wgpu::ShaderSource::Wgsl(ENTITY_SHADER_SRC.into()),
        });
        let entity_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("entity pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &entity_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<CubeVertex>() as u64,
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
                                shader_location: 5,
                            },
                        ],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<EntityInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 0,
                                shader_location: 1,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32,
                                offset: 12,
                                shader_location: 2,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 16,
                                shader_location: 3,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 28,
                                shader_location: 4,
                            },
                        ],
                    },
                ],
            },
            primitive: wgpu::PrimitiveState {
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
                module: &entity_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        let cube = cube_vertices();
        let entity_vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("entity cube"),
            contents: bytemuck::cast_slice(&cube),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let entity_instance_capacity = 1024u64;
        let entity_instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("entity instances"),
            size: entity_instance_capacity * std::mem::size_of::<EntityInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // The UI overlay pass (plan §11.4 HUD + §11.6 debug overlays): the
        // glyph atlas as an R8Unorm texture, a screen-size uniform, an
        // alpha-blended pipeline, and a dynamic vertex stream.
        let atlas = TextAtlas::new();
        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: atlas.width,
                height: atlas.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &atlas.coverage,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(atlas.width),
                rows_per_image: Some(atlas.height),
            },
            wgpu::Extent3d {
                width: atlas.width,
                height: atlas.height,
                depth_or_array_layers: 1,
            },
        );
        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let screen_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("screen size"),
            contents: bytemuck::bytes_of(&[config.width as f32, config.height as f32]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let screen_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("screen layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let atlas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("atlas layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let ui_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atlas bind group"),
            layout: &atlas_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(&atlas_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
            ],
        });
        let screen_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("screen bind group"),
            layout: &screen_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: screen_buffer.as_entire_binding(),
            }],
        });
        let ui_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui layout"),
            bind_group_layouts: &[&screen_layout, &atlas_layout],
            push_constant_ranges: &[],
        });
        let ui_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ui shader"),
            source: wgpu::ShaderSource::Wgsl(UI_SHADER_SRC.into()),
        });
        let ui_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui pipeline"),
            layout: Some(&ui_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &ui_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<UiVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 8,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 16,
                            shader_location: 2,
                        },
                    ],
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            // The overlay always passes depth and never writes it: it is on top.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &ui_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        let ui_vertex_capacity = 8192u64;
        let ui_vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ui vertices"),
            size: ui_vertex_capacity * std::mem::size_of::<UiVertex>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // The fog-of-war pass: one texel per tile of the terrain mesh's
        // extent, linear sampling for soft fog edges, and its own uniform
        // carrying the camera plus the map size the world-space UVs divide by.
        let fog_map_size = terrain
            .vertices
            .iter()
            .fold([0.0f32, 0.0f32], |size, vertex| {
                [
                    size[0].max(vertex.position[0]),
                    size[1].max(vertex.position[2]),
                ]
            });
        let fog_texture_size = (fog_map_size[0] as u32, fog_map_size[1] as u32);
        let fog_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fog texture"),
            size: wgpu::Extent3d {
                width: fog_texture_size.0.max(1),
                height: fog_texture_size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let fog_texture_view = fog_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let fog_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("fog sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let fog_uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fog uniform"),
            contents: bytemuck::bytes_of(&FogUniform {
                view_projection: glam::Mat4::IDENTITY.to_cols_array_2d(),
                map_size: fog_map_size,
                _pad: [0.0; 2],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let fog_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fog layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let fog_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fog bind group"),
            layout: &fog_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: fog_uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&fog_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&fog_texture_view),
                },
            ],
        });
        let fog_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fog layout"),
            bind_group_layouts: &[&fog_layout],
            push_constant_ranges: &[],
        });
        let fog_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fog shader"),
            source: wgpu::ShaderSource::Wgsl(FOG_SHADER_SRC.into()),
        });
        let fog_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fog pipeline"),
            layout: Some(&fog_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &fog_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                // The fog pass consumes the terrain mesh's own vertex layout
                // (position + placeholder color) — same geometry, same depth.
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<TerrainVertex>() as u64,
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
                    ],
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &fog_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        // The ground decal pass: one pipeline, two geometry streams (flat
        // quad for shadows, annulus for selection rings), one shared
        // instance buffer drawn in two instance ranges.
        let decal_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("decal shader"),
            source: wgpu::ShaderSource::Wgsl(DECAL_SHADER_SRC.into()),
        });
        let decal_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("decal pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &decal_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        }],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<DecalInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 0,
                                shader_location: 1,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 12,
                                shader_location: 2,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 20,
                                shader_location: 3,
                            },
                        ],
                    },
                ],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &decal_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        let quad = quad_vertices();
        let quad_vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("decal quad"),
            contents: bytemuck::cast_slice(&quad),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ring = ring_vertices(24, 0.80, 0.95);
        let ring_vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("decal ring"),
            contents: bytemuck::cast_slice(&ring),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let decal_instance_capacity = 1024u64;
        let decal_instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("decal instances"),
            size: decal_instance_capacity * std::mem::size_of::<DecalInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // The minimap: one map-sized RGBA texture composited on the CPU
        // (terrain base baked once; fog and entity dots layered per frame),
        // one screen-space quad, one tiny pipeline.
        let minimap_size = (fog_texture_size.0.max(1), fog_texture_size.1.max(1));
        let minimap_base: Vec<[u8; 4]> = terrain
            .vertices
            .as_chunks::<4>()
            .0
            .iter()
            .map(|tile| {
                let c = tile[0].color;
                [
                    (c[0] * 255.0) as u8,
                    (c[1] * 255.0) as u8,
                    (c[2] * 255.0) as u8,
                    255,
                ]
            })
            .collect();
        let minimap_scratch: Vec<u8> = vec![0; (minimap_size.0 * minimap_size.1 * 4) as usize];
        let minimap_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("minimap texture"),
            size: wgpu::Extent3d {
                width: minimap_size.0,
                height: minimap_size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let minimap_view = minimap_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let minimap_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("minimap sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let minimap_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("minimap layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let minimap_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("minimap bind group"),
            layout: &minimap_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: screen_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&minimap_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&minimap_view),
                },
            ],
        });
        let minimap_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("minimap layout"),
                bind_group_layouts: &[&minimap_layout],
                push_constant_ranges: &[],
            });
        let minimap_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("minimap shader"),
            source: wgpu::ShaderSource::Wgsl(MINIMAP_SHADER_SRC.into()),
        });
        let minimap_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("minimap pipeline"),
            layout: Some(&minimap_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &minimap_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<MinimapVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 0,
                        shader_location: 0,
                    }],
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &minimap_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        let minimap_vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("minimap quad"),
            size: 6 * std::mem::size_of::<MinimapVertex>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            _window: window,
            surface,
            device,
            queue,
            config,
            depth_view,
            camera_buffer,
            camera_bind_group,
            terrain_pipeline,
            terrain_vertex_buf,
            terrain_index_buf,
            terrain_index_count: terrain.indices.len() as u32,
            entity_pipeline,
            entity_vertex_buf,
            entity_instance_buf,
            entity_instance_capacity,
            atlas,
            screen_buffer,
            screen_bind_group,
            ui_pipeline,
            ui_bind_group,
            ui_vertex_buf,
            ui_vertex_capacity,
            pending_ui: Vec::new(),
            fog_pipeline,
            fog_uniform_buf,
            fog_bind_group,
            fog_texture,
            fog_texture_size,
            fog_map_size: (fog_map_size[0], fog_map_size[1]),
            kind_specs,
            decal_pipeline,
            quad_vertex_buf,
            quad_vertex_count: quad.len() as u32,
            ring_vertex_buf,
            ring_vertex_count: ring.len() as u32,
            decal_instance_buf,
            decal_instance_capacity,
            dying_cues: Vec::new(),
            ghost: None,
            minimap_tex,
            minimap_bind_group,
            minimap_pipeline,
            minimap_vertex_buf,
            minimap_rect: None,
            minimap_base,
            minimap_scratch,
            minimap_size,
        })
    }

    fn make_depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d {
                    width: width.max(1),
                    height: height.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default())
    }

    /// Reconfigures the surface and depth target on window resize.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = Self::make_depth_view(&self.device, width, height);
    }

    /// The glyph atlas (layout math for the caller; the texture stays here).
    pub fn atlas(&self) -> &TextAtlas {
        &self.atlas
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
        // Entity dots: one tile dot per entity, brighter than any terrain.
        for entity in entities {
            let tx = entity.pos.x.floor();
            let ty = entity.pos.z.floor();
            if tx < 0.0 || ty < 0.0 || tx >= w as f32 || ty >= h as f32 {
                continue;
            }
            let dot = match entity.owner {
                pandemonium_sim_api::PlayerId(0) => [90.0, 150.0, 255.0],
                pandemonium_sim_api::PlayerId(1) => [255.0, 95.0, 80.0],
                _ => [235.0, 190.0, 70.0],
            };
            let offset = ((ty as u32 * w + tx as u32) as usize) * 4;
            scratch[offset] = dot[0] as u8;
            scratch[offset + 1] = dot[1] as u8;
            scratch[offset + 2] = dot[2] as u8;
            scratch[offset + 3] = 255;
        }
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

/// Builds one entity's instances — the silhouette pass (pure geometry, so it
/// is unit-testable without a GPU). Each [`Silhouette`] part becomes one
/// instance: the part's local offset rotates with the entity's facing, the
/// part's tone resolves against the team palette, `moving` squats whole
/// unit silhouettes, and selection/hit feedback brightens everything.
fn build_entity_instances(
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
        });
    }
}

/// The defensive fallback for a kind id outside the specs (never used with
/// real content — the plain unit box).
fn fallback_shape() -> KindShape {
    crate::silhouette::capability_fallback(None, false, false, false)
}

/// Builds the whole frame's instance stream (ascending entity order — the
/// snapshot's contract) under the instance buffer's capacity.
fn build_instances(
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

/// The ground lift shared by every decal — enough to clear terrain/fog
/// rasterization noise, small enough to look glued to the ground.
const DECAL_LIFT: f32 = 0.02;

/// Builds the shadow blob stream: one dark ellipse under every entity,
/// sized by the silhouette's derived ground radius.
fn build_shadows(
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
            color: [0.0, 0.0, 0.0, 0.32],
        });
    }
    decals
}

/// The selection ring color (own-team green — Generals convention).
const RING_COLOR: [f32; 4] = [0.35, 1.0, 0.45, 0.85];

/// Builds the selection ring stream: one ground ellipse around every
/// selected entity, sized to its silhouette.
fn build_rings(
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
fn death_instance(cue: &DyingCue, shape: &KindShape) -> EntityInstance {
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
    }
}

impl Renderer for WgpuRenderer {
    fn render(&mut self, frame: Frame<'_>) {
        // Camera uniform.
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::bytes_of(&CameraUniform {
                view_projection: frame.view_projection.to_cols_array_2d(),
            }),
        );
        // The fog pass shares the camera; it also carries the map size for
        // the world-space UV mapping.
        self.queue.write_buffer(
            &self.fog_uniform_buf,
            0,
            bytemuck::bytes_of(&FogUniform {
                view_projection: frame.view_projection.to_cols_array_2d(),
                map_size: [self.fog_map_size.0, self.fog_map_size.1],
                _pad: [0.0; 2],
            }),
        );

        // Entity instances from the interpolated snapshot (ascending id),
        // composed into per-kind silhouettes by the pure instance builder.
        let mut instances = build_instances(
            frame.snapshot,
            frame.selection,
            frame.flashes,
            &self.kind_specs,
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
        let needed = instances.len() as u64;
        if needed > 0 {
            let bytes = bytemuck::cast_slice(&instances);
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
            let shadows = build_shadows(frame.snapshot, &self.kind_specs, decal_capacity);
            let shadow_count = shadows.len() as u32;
            let mut decals = shadows;
            // The ghost rides above shadows: one quad at the footprint.
            let ghost_count = if let Some(ghost) = self.ghost {
                if decals.len() < decal_capacity {
                    decals.push(DecalInstance {
                        position: [ghost.center[0], DECAL_LIFT * 1.5, ghost.center[2]],
                        scale: ghost.extent,
                        color: if ghost.legal {
                            [0.30, 1.0, 0.40, 0.30]
                        } else {
                            [1.0, 0.30, 0.25, 0.38]
                        },
                    });
                    1u32
                } else {
                    0
                }
            } else {
                0
            };
            let rings = build_rings(
                frame.snapshot,
                frame.selection,
                &self.kind_specs,
                decal_capacity - decals.len(),
            );
            let ring_count = rings.len() as u32;
            decals.extend(rings);
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
                if ring_count > 0 {
                    pass.set_vertex_buffer(0, self.ring_vertex_buf.slice(..));
                    pass.draw(
                        0..self.ring_vertex_count,
                        (shadow_count + ghost_count)..(shadow_count + ghost_count + ring_count),
                    );
                }
            }
            pass.set_pipeline(&self.entity_pipeline);
            pass.set_vertex_buffer(0, self.entity_vertex_buf.slice(..));
            pass.set_vertex_buffer(1, self.entity_instance_buf.slice(..));
            pass.draw(0..36, 0..needed.min(self.entity_instance_capacity) as u32);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every embedded WGSL module parses (and keeps its expected entry
    /// points). The GPU path cannot run on a headless CI machine, so the
    /// shaders are machine-checked to the extent the environment allows —
    /// a parse error would otherwise only surface as a runtime panic in
    /// `WgpuRenderer::new` on a machine with a display.
    #[test]
    fn every_embedded_shader_parses_with_expected_entry_points() {
        for (name, source) in [
            ("terrain", TERRAIN_SHADER),
            ("entity", ENTITY_SHADER_SRC),
            ("ui", UI_SHADER_SRC),
            ("fog", FOG_SHADER_SRC),
        ] {
            let module = wgpu::naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|error| panic!("{name} shader does not parse: {error}"));
            for entry in ["vs_main", "fs_main"] {
                assert!(
                    module.entry_points.iter().any(|point| point.name == entry),
                    "{name} shader lost its {entry} entry point"
                );
            }
        }
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
        let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/render.rs"))
            .expect("render.rs sits at the crate's src/");
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
        build_entity_instances(&e, &shape, false, false, &mut out);
        assert_eq!(out.len(), 3, "body + head + rifle");
        let yaw = out[0].yaw;
        assert!(
            (yaw - 0.0).abs() < 1e-4,
            "an east-facing unit yaws to 0, got {yaw}"
        );
        // The rifle rides ahead of the body center along the facing.
        assert!(out[2].position[0] > out[0].position[0], "rifle forward");
        // A moving unit squats: its whole silhouette is shorter.
        let mut moving = entity(1, 0, PlayerId(0), MoveState::Moving);
        moving.facing = Vec3::new(1.0, 0.0, 0.0);
        let mut out2 = Vec::new();
        build_entity_instances(&moving, &shape, false, false, &mut out2);
        assert!(out2[0].scale[1] < out[0].scale[1], "moving squats the body");
        assert!(
            out2[2].position[1] < out[2].position[1],
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
        build_entity_instances(&e, &shape, false, false, &mut out);
        assert!(
            (out[0].yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
            "a north-facing unit yaws to +90 degrees"
        );
        assert!(
            out[2].position[2] > out[0].position[2],
            "rifle forward (+z)"
        );
        assert!(
            out[2].position[0] < out[0].position[0],
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
            &mut plain,
        );
        let mut selected = Vec::new();
        build_entity_instances(
            &entity(1, 0, PlayerId(0), MoveState::Idle),
            &unit_shape(),
            true,
            false,
            &mut selected,
        );
        let mut flashing = Vec::new();
        build_entity_instances(
            &entity(1, 0, PlayerId(0), MoveState::Idle),
            &unit_shape(),
            false,
            true,
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
        let instances = build_instances(&snapshot, &[], &[], &specs, 6);
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
        let instances = build_instances(&snapshot, &[], &[], &[], 64);
        assert_eq!(instances.len(), 1, "the fallback silhouette still renders");
    }
}

#[cfg(test)]
mod decal_tests {
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
            assert_eq!(shadow.color[3], 0.32, "uniform shadow opacity");
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
