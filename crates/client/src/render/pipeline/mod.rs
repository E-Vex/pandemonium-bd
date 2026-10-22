// `pipeline/` (ADR-0100 part c, the ARCHITECTURE.md refactor map): the
// device, surface, and common bind groups — the constructor and the
// surface-lifecycle half of `WgpuRenderer`. The shared instance buffers
// (the decal stream included) are born here and stay referenced by the
// frame pass in `super` — the AI-Handoff §9 ordering gotcha rides on this
// buffer being one shared, order-dependent stream.
use super::entities::ENTITY_SHADER_SRC;
use super::minimap::{cube_vertices, CubeVertex, MinimapVertex, MINIMAP_SHADER_SRC};
use super::overlay::{quad_vertices, ring_vertices, DecalInstance, DECAL_SHADER_SRC};
#[allow(clippy::wildcard_imports)]
use super::*;

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
                eye: [0.0; 3],
                time: 0.0,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        // The camera layout is visible to the fragment stage too: the entity
        // and terrain fragments read the eye position (specular, rim,
        // distance haze) and the decal-family shaders may read the time.
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
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
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32,
                            offset: 36,
                            shader_location: 3,
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
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32,
                                offset: 44,
                                shader_location: 6,
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
                time: 0.0,
                _pad: 0.0,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let fog_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fog layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    // Fragment-visible too: the fog fragment reads the
                    // animation clock that drifts the fog's edge noise.
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
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
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32,
                                offset: 36,
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
            terrain_heights: terrain_height_grid(terrain, fog_texture_size.0, fog_texture_size.1),
            terrain_height_size: fog_texture_size,
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
            fog_layout,
            fog_sampler,
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
            minimap_layout,
            minimap_sampler,
            minimap_bind_group,
            minimap_pipeline,
            minimap_vertex_buf,
            minimap_rect: None,
            minimap_base,
            minimap_scratch,
            minimap_size,
            start: std::time::Instant::now(),
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
}
