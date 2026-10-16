//! Draws a model into a small square picture on the CPU: the grid's thumbnails.
//!
//! The camera looks from the default direction (`shading::VIEW_YAW`, `VIEW_PITCH`) at the centre of
//! the bounds, from far enough away for the bounding sphere to fit `FOV_Y`. The picture is then
//! framed tightly: every vertex is projected, and the projected extent is scaled and centred to
//! fill the square with a small margin, keeping its aspect.
//!
//! Triangles are rasterised at 3 × 3 samples per pixel into a depth and triangle-id buffer first,
//! so overdraw costs no lighting. Then each pixel is shaded: once, as MSAA does on a GPU, where one
//! triangle covers all its samples (the texture is still read at every sample, so a texture drawn
//! smaller than its size stays smooth), and sample by sample along edges, where the coverage
//! becomes alpha. Measured over the test library's 503 renderable models (`bench`, one thread):
//! a median 3.3 ms per 192-pixel thumbnail, from 8.6 ms when every sample was shaded and each
//! triangle's whole bounding box was walked. Skeleton-only models are drawn as a stick figure.

mod camera;
mod canvas;
mod colour;
mod skeleton;
mod triangles;

use super::Model;
use camera::Camera;
use canvas::Canvas;
use skeleton::draw_skeleton;
use triangles::draw_triangles;

/// Samples per pixel along each axis.
const SUPERSAMPLE: usize = 3;

const SAMPLES: usize = SUPERSAMPLE * SUPERSAMPLE;

/// Empty border around the model, as a fraction of the picture's side.
const MARGIN: f32 = 0.06;

const NO_TRIANGLE: u32 = u32::MAX;

/// How far outside a triangle a sample may lie and still count as inside, in barycentric units,
/// so rounding never opens a crack along an edge two triangles share.
const EDGE_SLACK: f32 = -1e-5;

/// Back faces' 1 / depth is scaled by this: 1/100,000 farther, far more than rounding between two
/// coplanar triangles (about 1e-7) and far less than any real gap between surfaces.
const BACK_FACE_DEPTH: f32 = 1.0 - 1e-5;

/// `size` × `size` sRGB pixels with straight alpha, transparent where the model isn't.
pub fn render_thumbnail(model: &Model, size: u32) -> Vec<u8> {
    let size = size.max(1) as usize;
    let camera = Camera::new(model);
    if model.indices.is_empty() {
        let mut canvas = Canvas::new(size * SUPERSAMPLE);
        draw_skeleton(model, &camera, &mut canvas);
        canvas.downsample(size)
    } else {
        draw_triangles(model, &camera, size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Bounds, Material, ModelInfo, Part, Texture, Vertex};

    /// A cube of `half` around the origin with one red material, wound counter-clockwise from outside.
    fn cube(half: f32) -> Model {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for axis in 0..3 {
            for sign in [-1.0f32, 1.0] {
                let mut normal = [0.0; 3];
                normal[axis] = sign;
                let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                let base = vertices.len() as u32;
                for (du, dv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                    let mut position = [0.0; 3];
                    position[axis] = sign * half;
                    position[u] = du * half;
                    position[v] = dv * half;
                    vertices.push(Vertex { position, normal, uv: [0.0; 2], color: [1.0; 4] });
                }
                let quad = if sign > 0.0 { [0, 1, 2, 0, 2, 3] } else { [0, 2, 1, 0, 3, 2] };
                indices.extend(quad.map(|i| base + i));
            }
        }
        let count = indices.len() as u32;
        Model {
            vertices,
            indices,
            parts: vec![Part { first: 0, count, material: 0 }],
            materials: vec![Material {
                name: "red".into(),
                color: [0.8, 0.05, 0.05, 1.0],
                texture: None,
                alpha_cutoff: None,
            }],
            textures: Vec::new(),
            bones: Vec::new(),
            bounds: Bounds { min: [-half; 3], max: [half; 3] },
            info: ModelInfo::default(),
        }
    }

    fn pixel(rgba: &[u8], size: usize, x: usize, y: usize) -> [u8; 4] {
        let at = (y * size + x) * 4;
        [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
    }

    #[test]
    fn cube_fills_the_middle_and_leaves_corners_clear() {
        let model = cube(0.5);
        let rgba = render_thumbnail(&model, 64);
        assert_eq!(rgba.len(), 64 * 64 * 4);
        let center = pixel(&rgba, 64, 32, 32);
        assert_eq!(center[3], 255);
        assert!(center[0] > center[1] * 2, "red: {center:?}");
        for (x, y) in [(0, 0), (63, 0), (0, 63), (63, 63)] {
            assert_eq!(pixel(&rgba, 64, x, y)[3], 0);
        }
        // Framed tightly: the cube reaches into the margin on some side but not past it.
        let opaque_columns: Vec<usize> = (0..64).filter(|&x| (0..64).any(|y| pixel(&rgba, 64, x, y)[3] > 0)).collect();
        let width = opaque_columns.last().unwrap() - opaque_columns.first().unwrap() + 1;
        let opaque_rows: Vec<usize> = (0..64).filter(|&y| (0..64).any(|x| pixel(&rgba, 64, x, y)[3] > 0)).collect();
        let height = opaque_rows.last().unwrap() - opaque_rows.first().unwrap() + 1;
        assert!(width.max(height) >= 56 && width.max(height) <= 60, "{width} × {height}");
    }

    #[test]
    fn top_is_lit_more_than_the_shadowed_side() {
        let model = cube(1.0);
        let rgba = render_thumbnail(&model, 96);
        // Seen from above-front-right: the top face is near the top of the picture, the +X face
        // (away from the key light on the left) on the right.
        let top = pixel(&rgba, 96, 48, 14);
        let right = pixel(&rgba, 96, 72, 60);
        assert_eq!((top[3], right[3]), (255, 255));
        assert!(top[0] > right[0], "top {top:?} right {right:?}");
    }

    #[test]
    fn inside_out_cube_looks_the_same() {
        // Reversed winding with normals pointing in: the faces toward the camera are back faces,
        // and double-sided shading flips their normals back out.
        let mut model = cube(1.0);
        for triangle in model.indices.as_chunks_mut::<3>().0 {
            triangle.swap(1, 2);
        }
        for vertex in &mut model.vertices {
            vertex.normal = vertex.normal.map(|c| -c);
        }
        let normal = render_thumbnail(&cube(1.0), 48);
        let flipped = render_thumbnail(&model, 48);
        let differ = normal.iter().zip(&flipped).filter(|(a, b)| a.abs_diff(**b) > 2).count();
        assert!(differ < 16, "{differ} channels differ");
    }

    #[test]
    fn degenerate_and_broken_triangles_are_skipped() {
        let mut model = cube(1.0);
        model.vertices.push(Vertex { position: [0.0; 3], normal: [0.0; 3], uv: [0.0; 2], color: [1.0; 4] });
        let last = model.vertices.len() as u32 - 1;
        model.indices.extend([last, last, last]);
        model.parts[0].count += 3;
        let rgba = render_thumbnail(&model, 32);
        assert_eq!(pixel(&rgba, 32, 16, 16)[3], 255);
    }

    #[test]
    fn cutout_texture_punches_holes() {
        let mut model = cube(1.0);
        // Left half of the texture clear, right half opaque; the cube's faces span it all.
        let mut rgba = Vec::new();
        for _y in 0..2 {
            rgba.extend([255, 255, 255, 0, 255, 255, 255, 255]);
        }
        model.textures.push(Texture { name: "mask".into(), width: 2, height: 2, rgba });
        model.materials[0].texture = Some(0);
        model.materials[0].alpha_cutoff = Some(0.5);
        for vertex in &mut model.vertices {
            vertex.uv =
                [if vertex.position[0] + vertex.position[1] + vertex.position[2] > 0.0 { 0.9 } else { 0.1 }, 0.5];
        }
        let solid = render_thumbnail(&cube(1.0), 64);
        let holey = render_thumbnail(&model, 64);
        let coverage = |rgba: &[u8]| rgba.as_chunks::<4>().0.iter().filter(|p| p[3] > 0).count();
        assert!(coverage(&holey) < coverage(&solid));
        assert!(coverage(&holey) > 0);
    }

    #[test]
    fn skeleton_draws_a_stick_figure() {
        let mut model = cube(1.0);
        model.indices.clear();
        model.parts.clear();
        model.bones = vec![[[0.0, 0.0, 0.0], [0.0, 1.0, 0.0]], [[0.0, 1.0, 0.0], [0.5, 1.5, 0.0]]];
        model.bounds = Bounds { min: [0.0; 3], max: [0.5, 1.5, 0.0] };
        let rgba = render_thumbnail(&model, 64);
        let drawn = rgba.as_chunks::<4>().0.iter().filter(|p| p[3] > 0).count();
        assert!(drawn > 64 && drawn < 64 * 64 / 2, "{drawn}");
        assert_eq!(pixel(&rgba, 64, 0, 0)[3], 0);
    }
}
