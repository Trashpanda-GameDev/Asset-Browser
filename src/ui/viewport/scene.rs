//! A model on the GPU: its buffers, its materials with their textures (and mip chains), the
//! skeleton's lines, and the wireframe's edges once they're wanted.

use std::collections::HashSet;

use eframe::wgpu::{self, util::DeviceExt};

use crate::model::{Model, Vertex, shading};

use super::gpu::{COLOR_FORMAT, Gpu, MaterialUniform};

/// Textures this small are pixel art: magnified with sharp pixels.
const PIXEL_ART_MAX: u32 = 256;

pub(super) struct Scene {
    pub(super) key: i64,
    pub(super) vertices: Option<wgpu::Buffer>,
    pub(super) indices: Option<wgpu::Buffer>,
    pub(super) parts: Vec<(std::ops::Range<u32>, wgpu::BindGroup)>,
    /// Built the first time wireframe is switched on.
    pub(super) edges: Option<(wgpu::Buffer, u32)>,
    pub(super) bones: Option<(wgpu::Buffer, u32)>,
    pub(super) center: [f32; 3],
    pub(super) radius: f32,
}

impl Scene {
    pub(super) fn upload(gpu: &Gpu, key: i64, model: &Model) -> Self {
        let has_triangles = !model.indices.is_empty() && !model.vertices.is_empty();
        let buffer = |label: &str, contents: &[u8], usage: wgpu::BufferUsages| {
            gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage })
        };
        let vertices = has_triangles
            .then(|| buffer("model vertices", bytemuck::cast_slice(&model.vertices), wgpu::BufferUsages::VERTEX));
        let indices = has_triangles
            .then(|| buffer("model indices", bytemuck::cast_slice(&model.indices), wgpu::BufferUsages::INDEX));
        let parts = if has_triangles { material_parts(gpu, model) } else { Vec::new() };
        let bones = bone_lines(&gpu.device, model);
        let (center, radius) = (model.bounds.center(), model.bounds.radius().max(1e-4));
        Self { key, vertices, indices, parts, edges: None, bones, center, radius }
    }
}

/// The model's parts, each with its material ready to draw: its colour, and its texture (with
/// crisp sampling for small pixel art) or else plain white. A model without parts is one part in
/// the default clay.
fn material_parts(gpu: &Gpu, model: &Model) -> Vec<(std::ops::Range<u32>, wgpu::BindGroup)> {
    let textures: Vec<(wgpu::TextureView, bool)> = model
        .textures
        .iter()
        .map(|texture| {
            let view =
                upload_texture(&gpu.device, &gpu.queue, &texture.name, [texture.width, texture.height], &texture.rgba);
            (view, texture.width.max(texture.height) <= PIXEL_ART_MAX)
        })
        .collect();
    let default_material = crate::model::Material {
        name: String::new(),
        color: [shading::DEFAULT_ALBEDO[0], shading::DEFAULT_ALBEDO[1], shading::DEFAULT_ALBEDO[2], 1.0],
        texture: None,
        alpha_cutoff: None,
    };
    if model.parts.is_empty() {
        return vec![(0..model.indices.len() as u32, bind_material(gpu, &textures, &default_material))];
    }
    model
        .parts
        .iter()
        .map(|part| {
            let material = model.materials.get(part.material as usize).unwrap_or(&default_material);
            (part.first..part.first + part.count, bind_material(gpu, &textures, material))
        })
        .collect()
}

/// One material's colour, alpha cut-off and texture, bound for the shader.
fn bind_material(
    gpu: &Gpu,
    textures: &[(wgpu::TextureView, bool)],
    material: &crate::model::Material,
) -> wgpu::BindGroup {
    let texture = material.texture.and_then(|index| textures.get(index as usize));
    let cutoff = material.alpha_cutoff.unwrap_or(-1.0);
    let uniform = MaterialUniform { color: material.color, flags: [cutoff, 0.0, 0.0, 0.0] };
    let buffer = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("model material"),
        contents: bytemuck::bytes_of(&uniform),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let (view, sampler) = match texture {
        Some((view, true)) => (view, &gpu.crisp),
        Some((view, false)) => (view, &gpu.smooth),
        None => (&gpu.white, &gpu.smooth),
    };
    gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("model material"),
        layout: &gpu.material_layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}

/// The skeleton's bones as lines, when the model has one.
fn bone_lines(device: &wgpu::Device, model: &Model) -> Option<(wgpu::Buffer, u32)> {
    if model.bones.is_empty() {
        return None;
    }
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
    Some((buffer, points.len() as u32))
}

/// A texture with its mip chain, so shrunk-down textures don't shimmer.
pub(super) fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    [w, h]: [u32; 2],
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
pub(super) fn edges(gpu: &Gpu, model: &Model) -> Option<(wgpu::Buffer, u32)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mips_go_down_to_one_pixel() {
        let levels = mip_chain(5, 3, &[200; 5 * 3 * 4]);
        assert_eq!(levels.iter().map(|(w, h, _)| (*w, *h)).collect::<Vec<_>>(), [(5, 3), (2, 1), (1, 1)]);
        assert!(levels.iter().all(|(w, h, pixels)| pixels.len() == (w * h * 4) as usize));
    }
}
