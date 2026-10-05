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

/// One entity instance: where the placeholder box sits and its color.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct EntityInstance {
    /// Instance center (the box's bottom sits on the ground).
    position: [f32; 3],
    /// Box half-extent in tiles.
    half_extent: f32,
    /// Placeholder color (team color, brighter when selected).
    color: [f32; 3],
    /// Padding to 16-byte multiples (keep buffers aligned).
    _pad: [f32; 3],
}

const TERRAIN_SHADER: &str = r#"
struct CameraUniform { view_projection: mat4x4<f32> };
@group(0) @binding(0) var<uniform> camera: CameraUniform;

struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) color: vec3<f32> };

@vertex fn vs_main(@location(0) position: vec3<f32>, @location(1) color: vec3<f32>) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_projection * vec4<f32>(position, 1.0);
    out.color = color;
    return out;
}

@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
"#;

const ENTITY_SHADER_SRC: &str = r#"
struct CameraUniform { view_projection: mat4x4<f32> };
@group(0) @binding(0) var<uniform> camera: CameraUniform;

struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) color: vec3<f32> };

@vertex fn vs_main(
    @location(0) corner: vec3<f32>,
    @location(1) instance_position: vec3<f32>,
    @location(2) instance_half_extent: f32,
    @location(3) instance_color: vec3<f32>,
) -> VsOut {
    var out: VsOut;
    let world = corner * instance_half_extent + instance_position;
    out.clip = camera.view_projection * vec4<f32>(world, 1.0);
    out.color = instance_color;
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

impl From<TerrainVertex> for GpuTerrainVertex {
    fn from(vertex: TerrainVertex) -> Self {
        Self {
            position: vertex.position,
            color: vertex.color,
        }
    }
}

/// The unit cube's 36 corner vertices (12 triangles), centered at the origin,
/// half-extent 0.5 on each axis.
fn cube_corners() -> Vec<[f32; 3]> {
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
    for face in FACES {
        let quad = [
            corners[face[0]],
            corners[face[1]],
            corners[face[2]],
            corners[face[3]],
        ];
        for corner in [quad[0], quad[1], quad[2], quad[0], quad[2], quad[3]] {
            vertices.push(corner);
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
}

impl WgpuRenderer {
    /// Creates the renderer for a window: device, queues, pipelines, and the
    /// static terrain buffers.
    pub fn new(
        window: std::sync::Arc<winit::window::Window>,
        terrain: &TerrainMesh,
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
                        array_stride: 12,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        }],
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

        let corners = cube_corners();
        let entity_vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("entity cube"),
            contents: bytemuck::cast_slice(&corners),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let entity_instance_capacity = 512u64;
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

        // Entity instances from the interpolated snapshot (ascending id).
        let selection: &[pandemonium_sim_api::EntityId] = frame.selection;
        let instances: Vec<EntityInstance> = frame
            .snapshot
            .entities
            .iter()
            .map(|entity| {
                let selected = selection.contains(&entity.id);
                let flashing = frame.flashes.contains(&entity.id);
                let moving = entity.move_state == MoveState::Moving;
                let half_extent = if moving { 0.28 } else { 0.34 };
                EntityInstance {
                    position: [entity.pos.x, entity.pos.y + half_extent, entity.pos.z],
                    half_extent,
                    color: team_color(entity.owner, selected, flashing),
                    _pad: [0.0; 3],
                }
            })
            .collect();
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
            pass.set_pipeline(&self.entity_pipeline);
            pass.set_vertex_buffer(0, self.entity_vertex_buf.slice(..));
            pass.set_vertex_buffer(1, self.entity_instance_buf.slice(..));
            pass.draw(0..36, 0..needed.min(self.entity_instance_capacity) as u32);
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
}
