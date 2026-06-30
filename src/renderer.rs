use crate::parser::{Mesh, Vertex};
use anyhow::{anyhow, Result};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

const COLOR_FORMAT:  wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const NORMAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const DEPTH_FORMAT:  wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const BYTES_PER_PIXEL: u32 = 4;

#[derive(Copy, Clone, Debug)]
pub enum RenderMode {
    /// Single-pass Lambert shading (the original look).
    Plain,
    /// Two-pass: write world-space normals + depth to offscreen targets, then
    /// a full-screen post pass detects edges (normal/depth discontinuities)
    /// and overlays them as black lines on flat white-ish faces.
    Edges,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    model:     [[f32; 4]; 4],
}

pub struct Renderer {
    device: wgpu::Device,
    queue:  wgpu::Queue,

    // Mesh + uniform resources (shared by plain and edges-geometry pipelines).
    vertex_buffer:   wgpu::Buffer,
    index_buffer:    wgpu::Buffer,
    index_count:     u32,
    uniform_buffer:  wgpu::Buffer,
    geom_bind_group: wgpu::BindGroup,

    // Pipelines.
    plain_pipeline:           wgpu::RenderPipeline,
    edges_geometry_pipeline:  wgpu::RenderPipeline,
    edges_composite_pipeline: wgpu::RenderPipeline,
    post_bind_group:          wgpu::BindGroup,

    // Render targets.
    color_texture: wgpu::Texture,
    color_view:    wgpu::TextureView,
    normal_view:   wgpu::TextureView,
    depth_view:    wgpu::TextureView,

    // Readback.
    output_buffer: wgpu::Buffer,
    width:  u32,
    height: u32,
    padded_bytes_per_row:   u32,
    unpadded_bytes_per_row: u32,

    model_matrix: Mat4,
}

/// Round `value` up to the nearest multiple of `alignment`.
/// Used to satisfy `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT` (256).
fn align_up(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}

impl Renderer {
    pub async fn new(width: u32, height: u32, mesh: &Mesh) -> Result<Self> {
        // ────────────────────────────────────────────────────────────────
        // 1. Instance / Adapter / Device — headless: no surface, no window.
        // ────────────────────────────────────────────────────────────────
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| anyhow!("no suitable GPU adapter"))?;

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("stl-gallery device"),
                    required_features: wgpu::Features::empty(),
                    required_limits:   wgpu::Limits::default(),
                    memory_hints:      wgpu::MemoryHints::Performance,
                },
                None,
            )
            .await?;

        // ────────────────────────────────────────────────────────────────
        // 2. Auto-center & uniform-scale the model into a [-1, 1] cube.
        // ────────────────────────────────────────────────────────────────
        let center = Vec3::from(mesh.bounds.center());
        let extent = mesh.bounds.longest_extent().max(1e-6);
        let scale  = 2.0 / extent;
        let model_matrix =
            Mat4::from_scale(Vec3::splat(scale)) * Mat4::from_translation(-center);

        // ────────────────────────────────────────────────────────────────
        // 3. Shared mesh + uniform buffers.
        // ────────────────────────────────────────────────────────────────
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertices"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size:  std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("uniform bgl"),
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
        let geom_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniform bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // ────────────────────────────────────────────────────────────────
        // 4. Shaders: plain (lit color), normal-write (G-buffer), composite.
        // ────────────────────────────────────────────────────────────────
        let plain_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("plain shader"),
            source: wgpu::ShaderSource::Wgsl(PLAIN_SHADER.into()),
        });
        let normal_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("normal shader"),
            source: wgpu::ShaderSource::Wgsl(NORMAL_SHADER.into()),
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite shader"),
            source: wgpu::ShaderSource::Wgsl(COMPOSITE_SHADER.into()),
        });

        // Geometry pipelines share the uniform BGL and vertex layout; they
        // differ only in fragment output format (color vs. encoded normals).
        let geom_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("geom layout"),
            bind_group_layouts: &[&uniform_bgl],
            push_constant_ranges: &[],
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
        };

        let primitive = wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        };
        let depth_state = wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: Default::default(),
            bias:    Default::default(),
        };

        let plain_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("plain pipeline"),
            layout: Some(&geom_layout),
            vertex: wgpu::VertexState {
                module: &plain_shader,
                entry_point: "vs_main",
                buffers: &[vertex_layout.clone()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &plain_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive,
            depth_stencil: Some(depth_state.clone()),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let edges_geometry_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("edges-geom pipeline"),
            layout: Some(&geom_layout),
            vertex: wgpu::VertexState {
                module: &normal_shader,
                entry_point: "vs_main",
                buffers: &[vertex_layout],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &normal_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    // Float16 RGBA — needed because world-space normals carry
                    // negative components (Rgba8Unorm would clamp to [0,1]).
                    format: NORMAL_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive,
            depth_stencil: Some(depth_state),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Composite (post-process) pass: full-screen triangle, samples normal
        // + depth G-buffers, no vertex inputs, no depth attachment.
        let post_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post bgl"),
            entries: &[
                // Normal G-buffer — sampled with `textureLoad`, no sampler.
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // Depth buffer — bound as `texture_depth_2d`.
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("composite layout"),
            bind_group_layouts: &[&post_bgl],
            push_constant_ranges: &[],
        });
        let edges_composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite pipeline"),
            layout: Some(&composite_layout),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: "vs_main",
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // ────────────────────────────────────────────────────────────────
        // 5. Render targets.
        //    - color_texture: final image (always copied to readback buffer).
        //    - normal_texture: G-buffer for edges mode (RENDER_ATTACHMENT
        //      while drawing geometry, TEXTURE_BINDING while compositing).
        //    - depth_texture: same dual usage as normal_texture.
        // ────────────────────────────────────────────────────────────────
        let color_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("color"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let color_view = color_texture.create_view(&Default::default());

        let normal_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("normal gbuffer"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: NORMAL_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let normal_view = normal_texture.create_view(&Default::default());

        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            // TEXTURE_BINDING so the composite pass can sample it.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_view = depth_texture.create_view(&Default::default());

        let post_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post bg"),
            layout: &post_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&normal_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&depth_view) },
            ],
        });

        // ────────────────────────────────────────────────────────────────
        // 6. Readback buffer (256-byte row alignment, see align_up).
        // ────────────────────────────────────────────────────────────────
        let unpadded_bytes_per_row = width * BYTES_PER_PIXEL;
        let padded_bytes_per_row   = align_up(unpadded_bytes_per_row,
                                              wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size:  (padded_bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            device, queue,
            vertex_buffer, index_buffer,
            index_count: mesh.indices.len() as u32,
            uniform_buffer, geom_bind_group,
            plain_pipeline, edges_geometry_pipeline, edges_composite_pipeline,
            post_bind_group,
            color_texture, color_view, normal_view, depth_view,
            output_buffer,
            width, height,
            padded_bytes_per_row, unpadded_bytes_per_row,
            model_matrix,
        })
    }

    /// Render a single frame and return a tightly packed RGBA8 buffer
    /// (`width * height * 4` bytes). Encoding to PNG is the caller's job —
    /// keeping it out of this hot path lets the caller fan it out across
    /// CPU cores while the GPU renders the next frame.
    pub async fn render_to_pixels(
        &mut self,
        view_proj: Mat4,
        mode: RenderMode,
    ) -> Result<Vec<u8>> {
        let uniforms = Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
            model:     self.model_matrix.to_cols_array_2d(),
        };
        self.queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });

        match mode {
            RenderMode::Plain => {
                let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("plain rpass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.color_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load:  wgpu::LoadOp::Clear(wgpu::Color { r: 0.07, g: 0.07, b: 0.09, a: 1.0 }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load:  wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                rpass.set_pipeline(&self.plain_pipeline);
                rpass.set_bind_group(0, &self.geom_bind_group, &[]);
                rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                rpass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                rpass.draw_indexed(0..self.index_count, 0, 0..1);
            }
            RenderMode::Edges => {
                // Pass 1 — geometry → normal G-buffer + depth.
                {
                    let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("edges-geom rpass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &self.normal_view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                // Background sentinel: zero normal. Composite
                                // pass detects background via depth==1.0 anyway.
                                load:  wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &self.depth_view,
                            depth_ops: Some(wgpu::Operations {
                                load:  wgpu::LoadOp::Clear(1.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    rpass.set_pipeline(&self.edges_geometry_pipeline);
                    rpass.set_bind_group(0, &self.geom_bind_group, &[]);
                    rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                    rpass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    rpass.draw_indexed(0..self.index_count, 0, 0..1);
                }
                // Pass 2 — full-screen composite → final color.
                {
                    let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("composite rpass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &self.color_view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load:  wgpu::LoadOp::Clear(wgpu::Color { r: 0.07, g: 0.07, b: 0.09, a: 1.0 }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    rpass.set_pipeline(&self.edges_composite_pipeline);
                    rpass.set_bind_group(0, &self.post_bind_group, &[]);
                    rpass.draw(0..3, 0..1);
                }
            }
        }

        // Texture → padded buffer copy (same regardless of mode).
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &self.color_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &self.output_buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row:  Some(self.padded_bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );

        self.queue.submit(Some(encoder.finish()));

        // ── GPU → CPU readback (see original commentary in git history) ──
        let buffer_slice = self.output_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;

        let padded = buffer_slice.get_mapped_range();

        // Fast path: when width*4 already aligns to 256 (e.g. 1024×1024) the
        // padded and tight layouts are identical — one bulk copy beats a
        // per-row loop.
        let pixels = if self.padded_bytes_per_row == self.unpadded_bytes_per_row {
            padded.to_vec()
        } else {
            let mut tight = Vec::with_capacity((self.unpadded_bytes_per_row * self.height) as usize);
            for row in 0..self.height as usize {
                let start = row * self.padded_bytes_per_row as usize;
                let end   = start + self.unpadded_bytes_per_row as usize;
                tight.extend_from_slice(&padded[start..end]);
            }
            tight
        };

        drop(padded);
        self.output_buffer.unmap();

        Ok(pixels)
    }
}

// ─── Shaders ────────────────────────────────────────────────────────────

const PLAIN_SHADER: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
    model:     mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsIn  { @location(0) position: vec3<f32>, @location(1) normal: vec3<f32> };
struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) world_normal: vec3<f32> };

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = u.model * vec4<f32>(in.position, 1.0);
    out.clip  = u.view_proj * world;
    out.world_normal = normalize((u.model * vec4<f32>(in.normal, 0.0)).xyz);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let light_dir = normalize(vec3<f32>(0.4, 0.8, 0.6));
    let n         = normalize(in.world_normal);
    let diffuse   = max(dot(n, light_dir), 0.0);
    let base      = vec3<f32>(0.78, 0.82, 0.88);
    return vec4<f32>(base * (0.25 + diffuse * 0.85), 1.0);
}
"#;

// Geometry pass for edges mode: writes world-space normal as RGB.
// Normal is stored unencoded (i.e. with sign), which is why the target format
// is Rgba16Float — Rgba8Unorm would clip negative components to zero.
const NORMAL_SHADER: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
    model:     mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsIn  { @location(0) position: vec3<f32>, @location(1) normal: vec3<f32> };
struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) world_normal: vec3<f32> };

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = u.model * vec4<f32>(in.position, 1.0);
    out.clip  = u.view_proj * world;
    out.world_normal = normalize((u.model * vec4<f32>(in.normal, 0.0)).xyz);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(normalize(in.world_normal), 1.0);
}
"#;

// Full-screen composite pass for edges mode.
//
// Vertex stage emits a single oversized triangle (no vertex buffer needed) —
// vertex_index 0,1,2 → NDC corners (-1,-1), (3,-1), (-1,3). The triangle
// covers [-1,1]^2 exactly once; areas outside the screen are clipped.
//
// Fragment stage reads the world-space normal G-buffer and the depth buffer
// at the current pixel and at its 8 neighbors, then declares an EDGE pixel
// when either:
//   - the normal differs sharply from the center (crease detection), or
//   - the depth differs sharply (silhouette/contour detection).
// Otherwise it shades the pixel with simple Lambert lighting in white.
const COMPOSITE_SHADER: &str = r#"
@group(0) @binding(0) var normal_tex: texture_2d<f32>;
@group(0) @binding(1) var depth_tex:  texture_depth_2d;

struct VsOut { @builtin(position) clip: vec4<f32> };

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    var out: VsOut;
    out.clip = vec4<f32>(positions[i], 0.0, 1.0);
    return out;
}

// Edge-detection thresholds. Tuned by eye on a model with mixed flat faces
// and curved/diamond perforations:
//   NORMAL_THRESH ≈ 1 - cos(θ) where θ is the smallest crease angle to flag.
//     0.05  → flags creases of ~18° or more.
//   DEPTH_THRESH is in normalized (0..1) device depth. Smaller catches
//     finer silhouettes but risks noise on near-coplanar surfaces.
const NORMAL_THRESH: f32 = 0.05;
const DEPTH_THRESH:  f32 = 0.0008;
const BG_COLOR: vec3<f32> = vec3<f32>(0.07, 0.07, 0.09);

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let dims  = vec2<i32>(textureDimensions(normal_tex));
    let coord = vec2<i32>(in.clip.xy);

    let n_c = textureLoad(normal_tex, coord, 0).xyz;
    let d_c = textureLoad(depth_tex,  coord, 0);

    // Background pixels: depth was never written, stays at the cleared 1.0.
    if (d_c >= 0.9999) {
        return vec4<f32>(BG_COLOR, 1.0);
    }

    var max_normal_diff = 0.0;
    var max_depth_diff  = 0.0;
    for (var dy: i32 = -1; dy <= 1; dy = dy + 1) {
        for (var dx: i32 = -1; dx <= 1; dx = dx + 1) {
            if (dx == 0 && dy == 0) { continue; }
            let p = clamp(coord + vec2<i32>(dx, dy),
                          vec2<i32>(0, 0),
                          dims - vec2<i32>(1, 1));
            let n = textureLoad(normal_tex, p, 0).xyz;
            let d = textureLoad(depth_tex,  p, 0);
            // 1 - dot(a, b) is small when normals agree, large when they don't.
            max_normal_diff = max(max_normal_diff, 1.0 - dot(n_c, n));
            max_depth_diff  = max(max_depth_diff,  abs(d_c - d));
        }
    }

    let is_edge = max(step(NORMAL_THRESH, max_normal_diff),
                      step(DEPTH_THRESH,  max_depth_diff));

    // Flat-ish white face shading so geometry still reads even where edges
    // are sparse. Faces are intentionally bright so black edges pop.
    let light_dir = normalize(vec3<f32>(0.4, 0.8, 0.6));
    let n         = normalize(n_c);
    let diffuse   = max(dot(n, light_dir), 0.0);
    let face      = vec3<f32>(0.97) * (0.55 + diffuse * 0.45);

    let color = mix(face, vec3<f32>(0.0), is_edge);
    return vec4<f32>(color, 1.0);
}
"#;
