//! The inspector's 3D view. The selected model is drawn on the GPU egui already uses, into a
//! texture egui then shows like any picture: drag to orbit, right- or middle-drag to pan, scroll to
//! zoom, double-click to reset.
//!
//! Lighting comes from `model::shading`, the same constants the thumbnails are rendered with, so a
//! model looks the same in its tile and here. The picture is redrawn only when something changed.

use std::collections::HashSet;
use std::sync::Arc;

use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, TextureId, Ui, pos2, vec2};
use eframe::egui_wgpu::{self, RenderState};
use eframe::wgpu::{self, util::DeviceExt};
use egui_phosphor::regular as icon;

use crate::model::{Model, Vertex, shading};

use super::{theme, widgets};

const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// egui reads textures as gamma-encoded bytes, so it gets a plain (non-sRGB) view of the picture.
const EGUI_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Textures this small are pixel art: magnified with sharp pixels.
const PIXEL_ART_MAX: u32 = 256;
const AUTO_ROTATE_SPEED: f32 = 0.5; // radians per second

#[derive(Default)]
pub struct Viewport {
    gpu: Option<Gpu>,
    scene: Option<Scene>,
    target: Option<Target>,
    orbit: Orbit,
    pub auto_rotate: bool,
    pub wireframe: bool,
    /// What the current picture shows; it's redrawn when this changes.
    drawn: Option<Globals>,
    drawn_wireframe: bool,
}

#[derive(Clone, Copy)]
struct Orbit {
    yaw: f32,
    pitch: f32,
    /// Distance from the target, in multiples of the distance that fits the model.
    zoom: f32,
    /// Offset of the point looked at from the model's centre, in model units.
    pan: [f32; 3],
}

impl Default for Orbit {
    fn default() -> Self {
        Self { yaw: shading::VIEW_YAW, pitch: shading::VIEW_PITCH, zoom: 1.0, pan: [0.0; 3] }
    }
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    line_color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MaterialUniform {
    color: [f32; 4],
    /// x: alpha below this is cut out (-1 for opaque materials, whose alpha is ignored).
    flags: [f32; 4],
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>,
    samples: u32,
    material_layout: wgpu::BindGroupLayout,
    /// Front faces, then back faces a hair farther away (see `Gpu::new`).
    front_faces: wgpu::RenderPipeline,
    back_faces: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    smooth: wgpu::Sampler,
    crisp: wgpu::Sampler,
    white: wgpu::TextureView,
}

struct Scene {
    key: i64,
    vertices: Option<wgpu::Buffer>,
    indices: Option<wgpu::Buffer>,
    parts: Vec<(std::ops::Range<u32>, wgpu::BindGroup)>,
    /// Built the first time wireframe is switched on.
    edges: Option<(wgpu::Buffer, u32)>,
    bones: Option<(wgpu::Buffer, u32)>,
    center: [f32; 3],
    radius: f32,
}

struct Target {
    size: [u32; 2],
    multisampled: Option<wgpu::TextureView>,
    depth: wgpu::TextureView,
    resolve: wgpu::TextureView,
    _texture: wgpu::Texture,
    id: TextureId,
}

impl Viewport {
    /// Whether models can be shown in 3D at all (the window runs on wgpu).
    pub fn available(render_state: Option<&RenderState>) -> bool {
        render_state.is_some()
    }

    /// Draws `model` (with cache key `key`) in `rect`. Returns false if there's no GPU to draw with.
    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "refactor: take fewer, grouped arguments; split into named steps"
    )]
    pub fn show(&mut self, ui: &mut Ui, rect: Rect, render_state: &RenderState, key: i64, model: &Model) -> bool {
        let gpu = self.gpu.get_or_insert_with(|| Gpu::new(render_state));
        if self.scene.as_ref().is_none_or(|scene| scene.key != key) {
            self.scene = Some(Scene::upload(gpu, key, model));
            self.orbit = Orbit::default();
            self.drawn = None;
        }
        let Some(scene) = self.scene.as_mut() else { return false };

        // Orbit, pan, zoom.
        let response = ui.interact(rect, ui.id().with("viewport"), Sense::click_and_drag());
        let mut interacting = false;
        if response.dragged_by(egui::PointerButton::Primary) {
            let delta = response.drag_delta();
            self.orbit.yaw -= delta.x * 0.01;
            self.orbit.pitch = (self.orbit.pitch + delta.y * 0.01).clamp(-1.55, 1.55);
            interacting = true;
        }
        if response.dragged_by(egui::PointerButton::Secondary) || response.dragged_by(egui::PointerButton::Middle) {
            let delta = response.drag_delta();
            let (right, up, _) = camera_axes(&self.orbit);
            let distance = fit_distance(scene.radius, rect.aspect_ratio()) * self.orbit.zoom;
            let per_point = 2.0 * distance * (shading::FOV_Y / 2.0).tan() / rect.height().max(1.0);
            for axis in 0..3 {
                self.orbit.pan[axis] -= (right[axis] * delta.x - up[axis] * delta.y) * per_point;
            }
            interacting = true;
        }
        if response.hovered() {
            let scroll = ui.input_mut(|input| std::mem::take(&mut input.smooth_scroll_delta.y));
            if scroll != 0.0 {
                self.orbit.zoom = (self.orbit.zoom * (-scroll * 0.0015).exp()).clamp(0.05, 20.0);
            }
        }
        if response.double_clicked() {
            self.orbit = Orbit::default();
        }
        if self.auto_rotate && !interacting {
            let dt = ui.input(|input| input.stable_dt).min(0.1);
            self.orbit.yaw += AUTO_ROTATE_SPEED * dt;
            ui.ctx().request_repaint();
        }

        // Render at the screen's pixel density.
        let pixels_per_point = ui.ctx().pixels_per_point();
        let max_side = gpu.device.limits().max_texture_dimension_2d.min(4096);
        let size = [
            ((rect.width() * pixels_per_point).round() as u32).clamp(1, max_side),
            ((rect.height() * pixels_per_point).round() as u32).clamp(1, max_side),
        ];
        if self.target.as_ref().is_none_or(|target| target.size != size) {
            let old = self.target.take();
            self.target = Some(Target::new(gpu, size, old.map(|target| target.id)));
            self.drawn = None;
        }
        let Some(target) = &self.target else { return false };

        let globals = globals(&self.orbit, scene, size);
        if self.drawn != Some(globals) || self.drawn_wireframe != self.wireframe {
            if self.wireframe && scene.edges.is_none() {
                scene.edges = edges(gpu, model);
            }
            gpu.queue.write_buffer(&gpu.globals, 0, bytemuck::bytes_of(&globals));
            gpu.render(scene, target, self.wireframe);
            self.drawn = Some(globals);
            self.drawn_wireframe = self.wireframe;
        }

        let painter = ui.painter_at(rect);
        widgets::vertical_gradient(&painter, rect, Color32::from_rgb(34, 37, 44), theme::WELL);
        painter.image(target.id, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        painter.rect_stroke(rect, theme::CARD_RADIUS, Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
        if response.hovered() && !interacting {
            painter.text(
                rect.left_bottom() + vec2(10.0, -8.0),
                Align2::LEFT_BOTTOM,
                "drag to orbit · right-drag to pan · scroll to zoom",
                FontId::proportional(11.0),
                theme::TEXT_FAINT,
            );
        }
        true
    }

    /// The overlay buttons: auto-rotate, wireframe, reset.
    pub fn controls(&mut self, ui: &mut Ui) {
        if widgets::icon_button_sized(ui, icon::ARROWS_CLOCKWISE, "Turn the model slowly", self.auto_rotate, 26.0)
            .clicked()
        {
            self.auto_rotate = !self.auto_rotate;
        }
        if widgets::icon_button_sized(ui, icon::POLYGON, "Show the triangles", self.wireframe, 26.0).clicked() {
            self.wireframe = !self.wireframe;
        }
        if widgets::icon_button_sized(ui, icon::CORNERS_OUT, "Reset the view (double-click)", false, 26.0).clicked() {
            self.orbit = Orbit::default();
        }
    }

    /// Frees the GPU copy of the model shown last, e.g. when the selection moves to something else.
    pub fn release(&mut self) {
        self.scene = None;
        self.drawn = None;
    }
}

impl Drop for Viewport {
    fn drop(&mut self) {
        if let (Some(gpu), Some(target)) = (&self.gpu, &self.target) {
            gpu.renderer.write().free_texture(&target.id);
        }
    }
}

impl Gpu {
    #[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
    fn new(state: &RenderState) -> Self {
        let device = state.device.clone();
        let samples = [4, 2]
            .into_iter()
            .find(|&count| {
                state.adapter.get_texture_format_features(COLOR_FORMAT).flags.sample_count_supported(count)
                    && state.adapter.get_texture_format_features(DEPTH_FORMAT).flags.sample_count_supported(count)
            })
            .unwrap_or(1);

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("model"),
            source: wgpu::ShaderSource::Wgsl(shader_source().into()),
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model globals"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
        });
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

        let full_vertex = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4],
        };
        let position_only = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3],
        };
        let depth = |write: bool, bias: wgpu::DepthBiasState| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState::default(),
            bias,
        };
        let multisample = wgpu::MultisampleState { count: samples, ..Default::default() };
        // Models are often open or one-sided, so both sides are drawn and lit, in two passes: the
        // back faces go a hair farther away, so where a model has a front and a back face in the
        // same place (a petal's two coloured sides) the front one shows instead of both fighting.
        // The thumbnails do the same (`model::raster`).
        let shaded = |label: &str, cull: wgpu::Face, bias: wgpu::DepthBiasState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&shaded_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(full_vertex.clone())],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: Some(cull),
                    ..Default::default()
                },
                depth_stencil: Some(depth(true, bias)),
                multisample,
                fragment: Some(wgpu::FragmentState {
                    module: &module,
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
        };
        let front_faces = shaded("model front faces", wgpu::Face::Back, wgpu::DepthBiasState::default());
        let back_faces = shaded(
            "model back faces",
            wgpu::Face::Front,
            wgpu::DepthBiasState { constant: 16, slope_scale: 1.0, clamp: 0.0 },
        );
        let lines = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("model lines"),
            layout: Some(&lines_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_line"),
                compilation_options: Default::default(),
                buffers: &[Some(position_only)],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::LineList, ..Default::default() },
            depth_stencil: Some(depth(false, wgpu::DepthBiasState::default())),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &module,
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
        });

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
        let sampler = |magnify: wgpu::FilterMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("model texture"),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                mag_filter: magnify,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            })
        };
        let smooth = sampler(wgpu::FilterMode::Linear);
        let crisp = sampler(wgpu::FilterMode::Nearest);
        let white = upload_texture(&device, &state.queue, "white", 1, 1, &[255; 4]);

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

    fn render(&self, scene: &Scene, target: &Target, wireframe: bool) {
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

impl Scene {
    #[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
    fn upload(gpu: &Gpu, key: i64, model: &Model) -> Self {
        let device = &gpu.device;
        let center = model.bounds.center();
        let radius = model.bounds.radius().max(1e-4);

        let has_triangles = !model.indices.is_empty() && !model.vertices.is_empty();
        let vertices = has_triangles.then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("model vertices"),
                contents: bytemuck::cast_slice(&model.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        let indices = has_triangles.then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("model indices"),
                contents: bytemuck::cast_slice(&model.indices),
                usage: wgpu::BufferUsages::INDEX,
            })
        });

        let textures: Vec<(wgpu::TextureView, bool)> = model
            .textures
            .iter()
            .map(|texture| {
                let view =
                    upload_texture(device, &gpu.queue, &texture.name, texture.width, texture.height, &texture.rgba);
                (view, texture.width.max(texture.height) <= PIXEL_ART_MAX)
            })
            .collect();
        let default_material = crate::model::Material {
            name: String::new(),
            color: [shading::DEFAULT_ALBEDO[0], shading::DEFAULT_ALBEDO[1], shading::DEFAULT_ALBEDO[2], 1.0],
            texture: None,
            alpha_cutoff: None,
        };
        let bind_material = |material: &crate::model::Material| {
            let texture = material.texture.and_then(|index| textures.get(index as usize));
            let cutoff = material.alpha_cutoff.unwrap_or(-1.0);
            let uniform = MaterialUniform { color: material.color, flags: [cutoff, 0.0, 0.0, 0.0] };
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("model material"),
                contents: bytemuck::bytes_of(&uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let (view, sampler) = match texture {
                Some((view, true)) => (view, &gpu.crisp),
                Some((view, false)) => (view, &gpu.smooth),
                None => (&gpu.white, &gpu.smooth),
            };
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("model material"),
                layout: &gpu.material_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
                ],
            })
        };
        let parts = if !has_triangles {
            Vec::new()
        } else if model.parts.is_empty() {
            vec![(0..model.indices.len() as u32, bind_material(&default_material))]
        } else {
            model
                .parts
                .iter()
                .map(|part| {
                    let material = model.materials.get(part.material as usize).unwrap_or(&default_material);
                    (part.first..part.first + part.count, bind_material(material))
                })
                .collect()
        };

        let bones = (!model.bones.is_empty()).then(|| {
            let points: Vec<Vertex> = model
                .bones
                .iter()
                .flatten()
                .map(|&position| Vertex { position, normal: [0.0; 3], uv: [0.0; 2], color: [1.0; 4] })
                .collect();
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("model bones"),
                contents: bytemuck::cast_slice(&points),
                usage: wgpu::BufferUsages::VERTEX,
            });
            (buffer, points.len() as u32)
        });

        Self { key, vertices, indices, parts, edges: None, bones, center, radius }
    }
}

impl Target {
    fn new(gpu: &Gpu, size: [u32; 2], reuse_id: Option<TextureId>) -> Self {
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

/// A texture with its mip chain, so shrunk-down textures don't shimmer.
#[expect(clippy::too_many_arguments, reason = "refactor: take fewer, grouped arguments")]
fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    w: u32,
    h: u32,
    rgba: &[u8],
) -> wgpu::TextureView {
    let levels = mip_chain(w, h, rgba);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, (lw, lh, pixels)) in levels.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(lw * 4), rows_per_image: Some(*lh) },
            wgpu::Extent3d { width: *lw, height: *lh, depth_or_array_layers: 1 },
        );
    }
    texture.create_view(&Default::default())
}

/// The picture at every half size down to 1×1, by averaging 2×2 blocks.
fn mip_chain(w: u32, h: u32, rgba: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    let mut levels = vec![(w, h, rgba.to_vec())];
    while let Some((w, h, pixels)) = levels.last()
        && (*w > 1 || *h > 1)
    {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                for channel in 0..4 {
                    let sample = |sx: u32, sy: u32| {
                        let (sx, sy) = (sx.min(w - 1), sy.min(h - 1));
                        pixels[((sy * w + sx) * 4 + channel) as usize] as u32
                    };
                    let sum = sample(x * 2, y * 2)
                        + sample(x * 2 + 1, y * 2)
                        + sample(x * 2, y * 2 + 1)
                        + sample(x * 2 + 1, y * 2 + 1);
                    next[((y * nw + x) * 4 + channel) as usize] = ((sum + 2) / 4) as u8;
                }
            }
        }
        levels.push((nw, nh, next));
    }
    levels
}

/// Every triangle edge once, as a line list into the model's vertices.
fn edges(gpu: &Gpu, model: &Model) -> Option<(wgpu::Buffer, u32)> {
    let mut seen = HashSet::with_capacity(model.indices.len());
    let mut lines = Vec::with_capacity(model.indices.len() * 2);
    for triangle in model.indices.as_chunks::<3>().0 {
        for (a, b) in [(triangle[0], triangle[1]), (triangle[1], triangle[2]), (triangle[2], triangle[0])] {
            if seen.insert((a.min(b), a.max(b))) {
                lines.extend([a, b]);
            }
        }
    }
    if lines.is_empty() {
        return None;
    }
    let buffer = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("model edges"),
        contents: bytemuck::cast_slice(&lines),
        usage: wgpu::BufferUsages::INDEX,
    });
    Some((buffer, lines.len() as u32))
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

/// How far the camera must be for the whole bounding sphere to fit, horizontally and vertically.
fn fit_distance(radius: f32, aspect: f32) -> f32 {
    let half_y = shading::FOV_Y / 2.0;
    let half_x = (half_y.tan() * aspect.max(0.01)).atan();
    radius * 1.08 / half_y.min(half_x).sin()
}

/// The camera's right, up and backward (toward the camera) directions.
fn camera_axes(orbit: &Orbit) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let (sin_yaw, cos_yaw) = orbit.yaw.sin_cos();
    let (sin_pitch, cos_pitch) = orbit.pitch.sin_cos();
    let back = [cos_pitch * sin_yaw, sin_pitch, cos_pitch * cos_yaw];
    let right = normalize(cross([0.0, 1.0, 0.0], back));
    let up = cross(back, right);
    (right, up, back)
}

fn globals(orbit: &Orbit, scene: &Scene, size: [u32; 2]) -> Globals {
    let aspect = size[0] as f32 / size[1].max(1) as f32;
    let distance = fit_distance(scene.radius, aspect) * orbit.zoom;
    let (right, up, back) = camera_axes(orbit);
    let target = [0, 1, 2].map(|axis| scene.center[axis] + orbit.pan[axis]);
    let eye = [0, 1, 2].map(|axis| target[axis] + back[axis] * distance);
    let view = [
        [right[0], up[0], back[0], 0.0],
        [right[1], up[1], back[1], 0.0],
        [right[2], up[2], back[2], 0.0],
        [-dot(right, eye), -dot(up, eye), -dot(back, eye), 1.0],
    ];
    let near = (distance - scene.radius * 2.0).max(distance * 0.01).max(1e-4);
    let far = distance + scene.radius * 2.0;
    let f = 1.0 / (shading::FOV_Y / 2.0).tan();
    let projection = [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, far / (near - far), -1.0],
        [0.0, 0.0, near * far / (near - far), 0.0],
    ];
    let accent = theme::ACCENT;
    let alpha = 0.55;
    let line = |channel: u8| egui::ecolor::linear_f32_from_gamma_u8(channel) * alpha;
    Globals {
        view_proj: multiply(&projection, &view),
        view,
        line_color: [line(accent.r()), line(accent.g()), line(accent.b()), alpha],
    }
}

/// Column-major 4×4 product `a × b`.
fn multiply(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    for (column, out_column) in out.iter_mut().enumerate() {
        for (row, value) in out_column.iter_mut().enumerate() {
            *value = (0..4).map(|k| a[k][row] * b[column][k]).sum();
        }
    }
    out
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt();
    if length > 1e-12 { v.map(|c| c / length) } else { [1.0, 0.0, 0.0] }
}

/// The WGSL, with the shared lighting constants filled in.
#[expect(clippy::too_many_lines, reason = "refactor: the shader's text goes into a .wgsl file of its own")]
fn shader_source() -> String {
    let v = |c: [f32; 3]| format!("vec3<f32>({:?}, {:?}, {:?})", c[0], c[1], c[2]);
    let n = |c: [f32; 3]| v(normalize(c));
    format!(
        r#"
const KEY_DIR = {key_dir};
const KEY_COLOR = {key_color};
const FILL_DIR = {fill_dir};
const FILL_COLOR = {fill_color};
const RIM_DIR = {rim_dir};
const RIM_COLOR = {rim_color};
const SKY = {sky};
const GROUND = {ground};
const SPECULAR = {specular:?};
const SHININESS = {shininess:?};

struct Globals {{
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    line_color: vec4<f32>,
}};
@group(0) @binding(0) var<uniform> globals: Globals;

struct Material {{
    color: vec4<f32>,
    flags: vec4<f32>,
}};
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var base_texture: texture_2d<f32>;
@group(1) @binding(2) var base_sampler: sampler;

struct VertexIn {{
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
}};

struct VertexOut {{
    // Invariant: the front and back passes must put a shared position at exactly the same depth.
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
}};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {{
    var out: VertexOut;
    out.clip = globals.view_proj * vec4<f32>(in.position, 1.0);
    out.normal = (globals.view * vec4<f32>(in.normal, 0.0)).xyz;
    out.uv = in.uv;
    out.color = in.color;
    return out;
}}

fn shade(albedo: vec3<f32>, n: vec3<f32>) -> vec3<f32> {{
    var light = mix(GROUND, SKY, n.y * 0.5 + 0.5);
    light += KEY_COLOR * max(dot(n, KEY_DIR), 0.0);
    light += FILL_COLOR * max(dot(n, FILL_DIR), 0.0);
    light += RIM_COLOR * max(dot(n, RIM_DIR), 0.0);
    let half_vector = normalize(KEY_DIR + vec3<f32>(0.0, 0.0, 1.0));
    let specular = SPECULAR * KEY_COLOR * pow(max(dot(n, half_vector), 0.0), SHININESS);
    return albedo * light + specular;
}}

@fragment
fn fs_main(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {{
    let base = material.color * in.color * textureSample(base_texture, base_sampler, in.uv);
    if (base.a < material.flags.x) {{
        discard;
    }}
    // Both sides are lit: a back face (clockwise on screen) is shaded with its normal flipped. A
    // missing normal faces the camera.
    var n = vec3<f32>(0.0, 0.0, 1.0);
    if (dot(in.normal, in.normal) > 1e-12) {{
        n = normalize(in.normal);
        if (!front) {{
            n = -n;
        }}
    }}
    return vec4<f32>(shade(base.rgb, n), 1.0);
}}

@vertex
fn vs_line(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {{
    var clip = globals.view_proj * vec4<f32>(position, 1.0);
    // Pulled slightly toward the camera so edges win over the surface they lie on.
    clip.z = clip.z - 0.0006 * clip.w;
    return clip;
}}

@fragment
fn fs_line() -> @location(0) vec4<f32> {{
    return globals.line_color;
}}
"#,
        key_dir = n(shading::KEY_DIR),
        key_color = v(shading::KEY_COLOR),
        fill_dir = n(shading::FILL_DIR),
        fill_color = v(shading::FILL_COLOR),
        rim_dir = n(shading::RIM_DIR),
        rim_color = v(shading::RIM_COLOR),
        sky = v(shading::SKY),
        ground = v(shading::GROUND),
        specular = shading::SPECULAR,
        shininess = shading::SHININESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks the WGSL without a GPU: it parses and passes naga's validation.
    #[test]
    fn shader_is_valid() {
        let module = naga::front::wgsl::parse_str(&shader_source()).expect("WGSL parses");
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .expect("WGSL validates");
        assert!(module.entry_points.iter().any(|entry| entry.name == "fs_main"));
    }

    #[test]
    fn mips_go_down_to_one_pixel() {
        let levels = mip_chain(5, 3, &[200; 5 * 3 * 4]);
        assert_eq!(levels.iter().map(|(w, h, _)| (*w, *h)).collect::<Vec<_>>(), [(5, 3), (2, 1), (1, 1)]);
        assert!(levels.iter().all(|(w, h, pixels)| pixels.len() == (w * h * 4) as usize));
    }

    #[test]
    fn camera_looks_at_the_target() {
        let scene = Scene {
            key: 0,
            vertices: None,
            indices: None,
            parts: Vec::new(),
            edges: None,
            bones: None,
            center: [1.0, 2.0, 3.0],
            radius: 1.0,
        };
        let globals = globals(&Orbit::default(), &scene, [100, 100]);
        // The centre projects to the middle of the picture, in front of the camera.
        let p = [1.0f32, 2.0, 3.0, 1.0];
        let clip: Vec<f32> = (0..4).map(|row| (0..4).map(|k| globals.view_proj[k][row] * p[k]).sum()).collect();
        assert!((clip[0] / clip[3]).abs() < 1e-4 && (clip[1] / clip[3]).abs() < 1e-4);
        assert!(clip[2] / clip[3] > 0.0 && clip[2] / clip[3] < 1.0);
    }
}
