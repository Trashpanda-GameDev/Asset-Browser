//! The 3D view's GPU side: the pipelines, samplers and buffers made once, the picture it renders
//! into, and the render pass.

use std::sync::Arc;

use eframe::egui::{self, TextureId};
use eframe::egui_wgpu::{self, RenderState};
use eframe::wgpu;

use crate::model::Vertex;

use super::scene::{Scene, upload_texture};
use super::shader::shader_source;

pub(super) const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// egui reads textures as gamma-encoded bytes, so it gets a plain (non-sRGB) view of the picture.
const EGUI_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct Globals {
    pub(super) view_proj: [[f32; 4]; 4],
    pub(super) view: [[f32; 4]; 4],
    pub(super) line_color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct MaterialUniform {
    pub(super) color: [f32; 4],
    /// x: alpha below this is cut out (-1 for opaque materials, whose alpha is ignored).
    pub(super) flags: [f32; 4],
}

pub(super) struct Gpu {
    pub(super) device: wgpu::Device,
    pub(super) queue: wgpu::Queue,
    pub(super) renderer: Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>,
    samples: u32,
    pub(super) material_layout: wgpu::BindGroupLayout,
    /// Front faces, then back faces a hair farther away (see `Gpu::new`).
    front_faces: wgpu::RenderPipeline,
    back_faces: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    pub(super) globals: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    pub(super) smooth: wgpu::Sampler,
    pub(super) crisp: wgpu::Sampler,
    pub(super) white: wgpu::TextureView,
}

pub(super) struct Target {
    pub(super) size: [u32; 2],
    multisampled: Option<wgpu::TextureView>,
    depth: wgpu::TextureView,
    resolve: wgpu::TextureView,
    _texture: wgpu::Texture,
    pub(super) id: TextureId,
}

impl Gpu {
    pub(super) fn new(state: &RenderState) -> Self {
        let device = state.device.clone();
        let samples = sample_count(state);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("model"),
            source: wgpu::ShaderSource::Wgsl(shader_source().into()),
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model globals"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let material_layout = material_layout(&device);
        let shaded_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("model shaded"),
            bind_group_layouts: &[Some(&globals_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let lines_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("model lines"),
            bind_group_layouts: &[Some(&globals_layout)],
            immediate_size: 0,
        });
        let parts = PipelineParts { device: &device, module: &module, samples };
        let front_faces =
            parts.shaded(&shaded_layout, "model front faces", wgpu::Face::Back, wgpu::DepthBiasState::default());
        let back_bias = wgpu::DepthBiasState { constant: 16, slope_scale: 1.0, clamp: 0.0 };
        let back_faces = parts.shaded(&shaded_layout, "model back faces", wgpu::Face::Front, back_bias);
        let lines = parts.lines(&lines_layout);
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("model globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("model globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let (smooth, crisp) = (sampler(&device, wgpu::FilterMode::Linear), sampler(&device, wgpu::FilterMode::Nearest));
        let white = upload_texture(&device, &state.queue, "white", [1, 1], &[255; 4]);
        Self {
            device,
            queue: state.queue.clone(),
            renderer: state.renderer.clone(),
            samples,
            material_layout,
            front_faces,
            back_faces,
            lines,
            globals,
            globals_bind,
            smooth,
            crisp,
            white,
        }
    }

    pub(super) fn render(&self, scene: &Scene, target: &Target, wireframe: bool) {
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("model") });
        {
            let (view, resolve_target, store) = match &target.multisampled {
                Some(multisampled) => (multisampled, Some(&target.resolve), wgpu::StoreOp::Discard),
                None => (&target.resolve, None, wgpu::StoreOp::Store),
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("model"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bind, &[]);
            if let (Some(vertices), Some(indices)) = (&scene.vertices, &scene.indices) {
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                for pipeline in [&self.front_faces, &self.back_faces] {
                    pass.set_pipeline(pipeline);
                    for (range, material) in &scene.parts {
                        pass.set_bind_group(1, material, &[]);
                        pass.draw_indexed(range.clone(), 0, 0..1);
                    }
                }
                if wireframe && let Some((edges, count)) = &scene.edges {
                    pass.set_pipeline(&self.lines);
                    pass.set_index_buffer(edges.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..*count, 0, 0..1);
                }
            }
            if let Some((bones, count)) = &scene.bones {
                pass.set_pipeline(&self.lines);
                pass.set_vertex_buffer(0, bones.slice(..));
                pass.draw(0..*count, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
    }
}

impl Target {
    pub(super) fn new(gpu: &Gpu, size: [u32; 2], reuse_id: Option<TextureId>) -> Self {
        let extent = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
        let texture = |label: &str,
                       format: wgpu::TextureFormat,
                       samples: u32,
                       usage: wgpu::TextureUsages,
                       views: &[wgpu::TextureFormat]| {
            gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: views,
            })
        };
        let resolve = texture(
            "model picture",
            COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            &[EGUI_FORMAT],
        );
        let multisampled = (gpu.samples > 1).then(|| {
            texture("model msaa", COLOR_FORMAT, gpu.samples, wgpu::TextureUsages::RENDER_ATTACHMENT, &[])
                .create_view(&Default::default())
        });
        let depth = texture("model depth", DEPTH_FORMAT, gpu.samples, wgpu::TextureUsages::RENDER_ATTACHMENT, &[])
            .create_view(&Default::default());
        let resolve_view = resolve.create_view(&Default::default());
        let egui_view =
            resolve.create_view(&wgpu::TextureViewDescriptor { format: Some(EGUI_FORMAT), ..Default::default() });
        let mut renderer = gpu.renderer.write();
        let id = match reuse_id {
            Some(id) => {
                renderer.update_egui_texture_from_wgpu_texture(&gpu.device, &egui_view, wgpu::FilterMode::Linear, id);
                id
            }
            None => renderer.register_native_texture(&gpu.device, &egui_view, wgpu::FilterMode::Linear),
        };
        Self { size, multisampled, depth, resolve: resolve_view, _texture: resolve, id }
    }
}

/// Multisampling, 4 or 2 samples where the GPU can for both colour and depth.
fn sample_count(state: &RenderState) -> u32 {
    [4, 2]
        .into_iter()
        .find(|&count| {
            state.adapter.get_texture_format_features(COLOR_FORMAT).flags.sample_count_supported(count)
                && state.adapter.get_texture_format_features(DEPTH_FORMAT).flags.sample_count_supported(count)
        })
        .unwrap_or(1)
}

/// What a material binds: its uniform, its texture and the sampler.
fn material_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("model material"),
        entries: &[
            uniform_entry(0, wgpu::ShaderStages::FRAGMENT),
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

/// A sampler for textures that repeat, magnified smoothly or by `magnify`.
fn sampler(device: &wgpu::Device, magnify: wgpu::FilterMode) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("model texture"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: magnify,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    })
}

/// What the render pipelines are made from.
struct PipelineParts<'a> {
    device: &'a wgpu::Device,
    module: &'a wgpu::ShaderModule,
    samples: u32,
}

impl PipelineParts<'_> {
    /// A pipeline for one side of the faces. Models are often open or one-sided, so both sides are
    /// drawn and lit, in two passes: the back faces go a hair farther away, so where a model has a
    /// front and a back face in the same place (a petal's two coloured sides) the front one shows
    /// instead of both fighting. The thumbnails do the same (`model::raster`).
    fn shaded(
        &self,
        layout: &wgpu::PipelineLayout,
        label: &str,
        cull: wgpu::Face,
        bias: wgpu::DepthBiasState,
    ) -> wgpu::RenderPipeline {
        let full_vertex = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4],
        };
        self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: self.module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(full_vertex)],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: Some(cull),
                ..Default::default()
            },
            depth_stencil: Some(depth_state(true, bias)),
            multisample: wgpu::MultisampleState { count: self.samples, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: self.module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }

    /// The pipeline for lines: the wireframe and the skeleton.
    fn lines(&self, layout: &wgpu::PipelineLayout) -> wgpu::RenderPipeline {
        let position_only = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3],
        };
        self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("model lines"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: self.module,
                entry_point: Some("vs_line"),
                compilation_options: Default::default(),
                buffers: &[Some(position_only)],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::LineList, ..Default::default() },
            depth_stencil: Some(depth_state(false, wgpu::DepthBiasState::default())),
            multisample: wgpu::MultisampleState { count: self.samples, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: self.module,
                entry_point: Some("fs_line"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }
}

/// Depth testing, nearer or level passing; written by the faces, not by the lines over them.
fn depth_state(write: bool, bias: wgpu::DepthBiasState) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(write),
        depth_compare: Some(wgpu::CompareFunction::LessEqual),
        stencil: wgpu::StencilState::default(),
        bias,
    }
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
