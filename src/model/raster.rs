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

use std::sync::OnceLock;

use super::geometry::{cross, dot, normalize, sub};
use super::{Material, Model, Part, Texture, shading};

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

/// View space as in `shading`: looking down -Z, +Y up, +X right.
struct Camera {
    right: [f32; 3],
    up: [f32; 3],
    back: [f32; 3],
    eye: [f32; 3],
}

impl Camera {
    fn new(model: &Model) -> Self {
        let back = shading::view_direction(shading::VIEW_YAW, shading::VIEW_PITCH);
        let right = normalize(cross([0.0, 1.0, 0.0], back));
        let up = cross(back, right);
        let bounds = &model.bounds;
        let radius = bounds.radius().max(1e-6);
        let distance = radius / (shading::FOV_Y * 0.5).sin();
        let center = bounds.center();
        let eye = [0, 1, 2].map(|axis| center[axis] + back[axis] * distance);
        Camera { right, up, back, eye }
    }

    fn to_view(&self, p: [f32; 3]) -> [f32; 3] {
        self.direction_to_view(sub(p, self.eye))
    }

    fn direction_to_view(&self, d: [f32; 3]) -> [f32; 3] {
        [dot(d, self.right), dot(d, self.up), dot(d, self.back)]
    }
}

/// Maps view-space points to sample coordinates, fitted to what's drawn.
struct Framing {
    scale: f32,
    center: [f32; 2],
    half: f32,
}

impl Framing {
    /// Fits the projection of `points` (view space, all in front of the camera) into a square of
    /// `side` samples.
    fn fit(points: impl Iterator<Item = [f32; 3]>, side: usize) -> Self {
        let (mut min, mut max) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for p in points {
            let s = [p[0] / -p[2], p[1] / -p[2]];
            if s[0].is_finite() && s[1].is_finite() {
                for axis in 0..2 {
                    min[axis] = min[axis].min(s[axis]);
                    max[axis] = max[axis].max(s[axis]);
                }
            }
        }
        if min[0] > max[0] {
            (min, max) = ([-1.0; 2], [1.0; 2]);
        }
        let extent = (max[0] - min[0]).max(max[1] - min[1]).max(1e-9);
        Framing {
            scale: side as f32 * (1.0 - 2.0 * MARGIN) / extent,
            center: [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5],
            half: side as f32 * 0.5,
        }
    }

    /// (x, y, 1 / depth): sample coordinates with y down, and the reciprocal of the distance in
    /// front of the camera, which interpolates linearly across the screen.
    fn project(&self, p: [f32; 3]) -> [f32; 3] {
        let inv_w = 1.0 / -p[2];
        [
            (p[0] * inv_w - self.center[0]) * self.scale + self.half,
            (self.center[1] - p[1] * inv_w) * self.scale + self.half,
            inv_w,
        ]
    }
}

fn draw_triangles(model: &Model, camera: &Camera, size: usize) -> Vec<u8> {
    let side = size * SUPERSAMPLE;
    let view: Vec<[f32; 3]> = model.vertices.iter().map(|vertex| camera.to_view(vertex.position)).collect();
    let mut used = vec![false; view.len()];
    for &index in &model.indices {
        if let Some(used) = used.get_mut(index as usize) {
            *used = true;
        }
    }
    let framing = Framing::fit(view.iter().zip(&used).filter(|(_, used)| **used).map(|(p, _)| *p), side);
    let screen: Vec<[f32; 3]> = view.iter().map(|&p| framing.project(p)).collect();

    // Visibility: the nearest triangle at each sample, by 1 / depth (larger is nearer).
    let mut depth = vec![0.0f32; side * side];
    let mut nearest = vec![NO_TRIANGLE; side * side];
    for (id, corners, material) in triangles(model) {
        let [a, b, c] = corners.map(|corner| screen[corner]);
        let cutout = material.alpha_cutoff.map(|cutoff| {
            let texture = material.texture.and_then(|index| model.textures.get(index as usize));
            (cutoff, material.color[3], texture, corners.map(|v| &model.vertices[v]))
        });
        // A back face counts as a hair farther away, so where a model has a front and a back face
        // in the same place (a petal's two coloured sides) the front one shows rather than both
        // fighting sample by sample. Counter-clockwise in view space is a negative area here.
        let bias = if edge(a, b, c) < 0.0 { 1.0 } else { BACK_FACE_DEPTH };
        covered_samples(side, [a, b, c], |sample, l| {
            let inv_w = l[0] * a[2] + l[1] * b[2] + l[2] * c[2];
            if inv_w * bias <= depth[sample] {
                return;
            }
            if let Some((cutoff, alpha, texture, [va, vb, vc])) = cutout {
                let weights = [l[0] * a[2] / inv_w, l[1] * b[2] / inv_w, l[2] * c[2] / inv_w];
                let mut alpha = alpha * mix(weights, [va.color[3]], [vb.color[3]], [vc.color[3]])[0];
                if let Some(texture) = texture {
                    alpha *= sample_texture(texture, mix(weights, va.uv, vb.uv, vc.uv))[3] as f32 / 255.0;
                }
                if alpha < cutoff {
                    return;
                }
            }
            depth[sample] = inv_w * bias;
            nearest[sample] = id;
        });
    }

    // Shading, pixel by pixel.
    let normals: Vec<[f32; 3]> = model.vertices.iter().map(|v| camera.direction_to_view(v.normal)).collect();
    let srgb = srgb8_to_linear();
    let lut = linear_to_srgb8();
    let mut current: Option<Setup> = None;
    let mut out = vec![0u8; size * size * 4];
    for (index, pixel) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (px, py) = (index % size, index / size);
        let mut ids = [NO_TRIANGLE; SAMPLES];
        let mut centers = [[0.0f32; 2]; SAMPLES];
        for (k, (id, center)) in ids.iter_mut().zip(&mut centers).enumerate() {
            let (sx, sy) = (px * SUPERSAMPLE + k % SUPERSAMPLE, py * SUPERSAMPLE + k / SUPERSAMPLE);
            *id = nearest[sy * side + sx];
            *center = [sx as f32 + 0.5, sy as f32 + 0.5];
        }
        // (linear colour, samples covered)
        let (color, count) = if ids.iter().all(|&id| id == ids[0]) {
            if ids[0] == NO_TRIANGLE {
                continue;
            }
            let Some(setup) = Setup::cached(&mut current, ids[0], model, &screen, &normals) else { continue };
            let middle = [
                (px * SUPERSAMPLE) as f32 + SUPERSAMPLE as f32 * 0.5,
                (py * SUPERSAMPLE) as f32 + SUPERSAMPLE as f32 * 0.5,
            ];
            let weights = setup.weights(middle);
            let mut albedo = setup.albedo(weights);
            if let Some(texture) = setup.texture {
                let mut texels = [0.0f32; 3];
                for center in centers {
                    let texel = sample_texture(texture, setup.uv(setup.weights(center)));
                    texels = [0, 1, 2].map(|k| texels[k] + srgb[texel[k] as usize]);
                }
                albedo = [0, 1, 2].map(|k| albedo[k] * texels[k] / SAMPLES as f32);
            }
            (shading::shade(albedo, setup.normal(weights)).map(|c| c.clamp(0.0, 1.0)), SAMPLES)
        } else {
            let (mut sum, mut count) = ([0.0f32; 3], 0);
            for (&id, &center) in ids.iter().zip(&centers) {
                if id == NO_TRIANGLE {
                    continue;
                }
                let Some(setup) = Setup::cached(&mut current, id, model, &screen, &normals) else { continue };
                let weights = setup.weights(center);
                let mut albedo = setup.albedo(weights);
                if let Some(texture) = setup.texture {
                    let texel = sample_texture(texture, setup.uv(weights));
                    albedo = [0, 1, 2].map(|k| albedo[k] * srgb[texel[k] as usize]);
                }
                let color = shading::shade(albedo, setup.normal(weights)).map(|c| c.clamp(0.0, 1.0));
                sum = [0, 1, 2].map(|k| sum[k] + color[k]);
                count += 1;
            }
            (sum.map(|c| c / count.max(1) as f32), count)
        };
        if count > 0 {
            let rgb = color.map(|c| lut[(c * 4095.0 + 0.5) as usize & 4095]);
            *pixel = [rgb[0], rgb[1], rgb[2], (count as f32 / SAMPLES as f32 * 255.0).round() as u8];
        }
    }
    out
}

/// What shading needs of one triangle, worked out once for all its pixels.
struct Setup<'a> {
    id: u32,
    screen: [[f32; 3]; 3],
    inv_area: f32,
    /// View space, flipped for a back face.
    normals: [[f32; 3]; 3],
    colors: [[f32; 4]; 3],
    uvs: [[f32; 2]; 3],
    material: &'a Material,
    texture: Option<&'a Texture>,
}

impl<'a> Setup<'a> {
    /// The setup for triangle `id`, reusing the last one: neighbouring pixels mostly share it.
    fn cached<'c>(
        current: &'c mut Option<Setup<'a>>,
        id: u32,
        model: &'a Model,
        screen: &[[f32; 3]],
        normals: &[[f32; 3]],
    ) -> Option<&'c Setup<'a>> {
        if current.as_ref().is_none_or(|setup| setup.id != id) {
            *current = Setup::new(id, model, screen, normals);
        }
        current.as_ref()
    }

    fn new(id: u32, model: &'a Model, screen: &[[f32; 3]], normals: &[[f32; 3]]) -> Option<Self> {
        let first = id as usize * 3;
        let corners: [usize; 3] = std::array::from_fn(|k| model.indices[first + k] as usize);
        let [a, b, c] = corners.map(|corner| screen[corner]);
        let area = edge(a, b, c);
        if area.is_nan() || area.abs() <= 1e-12 {
            return None;
        }
        // Screen y points down, so counter-clockwise in view space is a negative area here; back
        // faces are lit with their normals flipped.
        let facing = if area < 0.0 { 1.0 } else { -1.0 };
        let material = &model.materials[part_of(&model.parts, first as u32).material as usize];
        Some(Setup {
            id,
            screen: [a, b, c],
            inv_area: 1.0 / area,
            normals: corners.map(|corner| normals[corner].map(|n| n * facing)),
            colors: corners.map(|corner| model.vertices[corner].color),
            uvs: corners.map(|corner| model.vertices[corner].uv),
            material,
            texture: material.texture.and_then(|index| model.textures.get(index as usize)),
        })
    }

    /// Perspective-correct barycentric weights at a point inside the triangle.
    fn weights(&self, p: [f32; 2]) -> [f32; 3] {
        let [a, b, c] = self.screen;
        let l = [edge2(b, c, p), edge2(c, a, p), edge2(a, b, p)].map(|e| e * self.inv_area);
        let q = [l[0] * a[2], l[1] * b[2], l[2] * c[2]];
        let sum = q[0] + q[1] + q[2];
        if sum > 0.0 { q.map(|q| q / sum) } else { [1.0 / 3.0; 3] }
    }

    fn normal(&self, weights: [f32; 3]) -> [f32; 3] {
        let [a, b, c] = self.normals;
        match normalize(mix(weights, a, b, c)) {
            n if n == [0.0; 3] => [0.0, 0.0, 1.0],
            n => n,
        }
    }

    fn uv(&self, weights: [f32; 3]) -> [f32; 2] {
        let [a, b, c] = self.uvs;
        mix(weights, a, b, c)
    }

    /// Material × vertex colour; the texture is the caller's, to sample once or per sample.
    fn albedo(&self, weights: [f32; 3]) -> [f32; 3] {
        let [a, b, c] = self.colors;
        let tint = mix(weights, a, b, c);
        [0, 1, 2].map(|k| self.material.color[k] * tint[k])
    }
}

/// The part holding the triangle that starts at `index` in `indices`.
fn part_of(parts: &[Part], index: u32) -> &Part {
    let at = parts.partition_point(|part| part.first <= index);
    &parts[at.saturating_sub(1)]
}

/// Per sample colour, for the stick figures.
struct Canvas {
    side: usize,
    /// Linear, 0..1.
    color: Vec<[f32; 3]>,
    covered: Vec<bool>,
}

impl Canvas {
    fn new(side: usize) -> Self {
        Canvas { side, color: vec![[0.0; 3]; side * side], covered: vec![false; side * side] }
    }

    /// Averages each block of samples into one pixel: coverage becomes alpha.
    fn downsample(&self, size: usize) -> Vec<u8> {
        let lut = linear_to_srgb8();
        let mut out = vec![0u8; size * size * 4];
        for (index, pixel) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (px, py) = (index % size, index / size);
            let mut sum = [0.0f32; 3];
            let mut count = 0usize;
            for sy in 0..SUPERSAMPLE {
                let row = (py * SUPERSAMPLE + sy) * self.side + px * SUPERSAMPLE;
                for sample in row..row + SUPERSAMPLE {
                    if self.covered[sample] {
                        let c = self.color[sample];
                        sum = [sum[0] + c[0], sum[1] + c[1], sum[2] + c[2]];
                        count += 1;
                    }
                }
            }
            if count > 0 {
                let rgb = sum.map(|c| lut[((c / count as f32) * 4095.0 + 0.5) as usize & 4095]);
                *pixel = [rgb[0], rgb[1], rgb[2], (count as f32 / SAMPLES as f32 * 255.0).round() as u8];
            }
        }
        out
    }
}

/// Every triangle with its id (its position in `indices` / 3), corners and material.
fn triangles(model: &Model) -> impl Iterator<Item = (u32, [usize; 3], &super::Material)> {
    model.parts.iter().flat_map(move |part| {
        let material = &model.materials[part.material as usize];
        let triangles = model.indices[part.first as usize..][..part.count as usize].as_chunks::<3>().0;
        let first = part.first / 3;
        triangles.iter().enumerate().map(move |(i, t)| (first + i as u32, t.map(|v| v as usize), material))
    })
}

/// Calls `visit` with each sample inside the triangle (screen coordinates) and the sample's
/// barycentric weights. Skips degenerate triangles, NaN included.
///
/// Each row visits only the span the three edges leave open, worked out from their equations,
/// rather than the whole bounding box: half of a big triangle's box is outside it.
fn covered_samples(side: usize, [a, b, c]: [[f32; 3]; 3], mut visit: impl FnMut(usize, [f32; 3])) {
    let area = edge(a, b, c);
    if area.is_nan() || area.abs() <= 1e-12 {
        return;
    }
    let inv_area = 1.0 / area;
    let limit = side as f32;
    let min_x = a[0].min(b[0]).min(c[0]);
    let max_x = a[0].max(b[0]).max(c[0]);
    let min_y = a[1].min(b[1]).min(c[1]);
    let max_y = a[1].max(b[1]).max(c[1]);
    if !(max_x >= 0.0 && max_y >= 0.0 && min_x < limit && min_y < limit) {
        return;
    }
    // Sample centres sit at half-integer coordinates.
    let x0 = (min_x - 0.5).ceil().max(0.0);
    let y0 = (min_y - 0.5).ceil().max(0.0) as usize;
    let x1 = (max_x - 0.5).floor().min(limit - 1.0);
    let y1 = ((max_y - 0.5).floor().min(limit - 1.0)) as usize;
    // Each weight as a linear function of the sample position: l = dx·x + dy·y + base.
    let weight = |p: [f32; 3], q: [f32; 3]| {
        let (dx, dy) = (-(q[1] - p[1]) * inv_area, (q[0] - p[0]) * inv_area);
        (dx, dy, -(dx * p[0] + dy * p[1]))
    };
    let weights = [weight(b, c), weight(c, a), weight(a, b)];
    for y in y0..=y1 {
        let py = y as f32 + 0.5;
        let (mut from, mut to) = (x0 + 0.5, x1 + 0.5);
        for &(dx, dy, base) in &weights {
            let at_row = dy * py + base;
            if dx > 0.0 {
                from = from.max((EDGE_SLACK - at_row) / dx);
            } else if dx < 0.0 {
                to = to.min((EDGE_SLACK - at_row) / dx);
            } else if at_row < EDGE_SLACK {
                to = f32::NEG_INFINITY;
            }
        }
        if from.is_nan() || to.is_nan() || from > to {
            continue;
        }
        // One sample of slack each side for rounding; the exact test below decides.
        let first = ((from - 0.5).ceil() - 1.0).max(x0) as usize;
        let last = ((to - 0.5).floor() + 1.0).min(x1) as usize;
        for x in first..=last {
            let px = x as f32 + 0.5;
            let l = weights.map(|(dx, dy, base)| dx * px + dy * py + base);
            if l[0] >= EDGE_SLACK && l[1] >= EDGE_SLACK && l[2] >= EDGE_SLACK {
                visit(y * side + x, l);
            }
        }
    }
}

/// Weighted sum of three vertices' attribute.
fn mix<const N: usize>(weights: [f32; 3], a: [f32; N], b: [f32; N], c: [f32; N]) -> [f32; N] {
    std::array::from_fn(|i| weights[0] * a[i] + weights[1] * b[i] + weights[2] * c[i])
}

/// Twice the signed area of (a, b, c) in screen space.
fn edge(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
    edge2(a, b, [c[0], c[1]])
}

fn edge2(a: [f32; 3], b: [f32; 3], p: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

/// Nearest texel, repeating outside 0..1.
fn sample_texture(texture: &Texture, uv: [f32; 2]) -> [u8; 4] {
    let (w, h) = (texture.width as usize, texture.height as usize);
    if w == 0 || h == 0 || texture.rgba.len() < w * h * 4 {
        return [255; 4];
    }
    // The fraction repeats the texture; `as` turns NaN into 0 and the `min` catches rounding up.
    // `floor` by hand: on x86-64 without SSE4.1 the library call costs more than the lookup.
    let floor = |t: f32| {
        let whole = t as i32 as f32;
        if whole > t { whole - 1.0 } else { whole }
    };
    let wrap = |t: f32, n: usize| (((t - floor(t)) * n as f32) as usize).min(n - 1);
    let at = (wrap(uv[1], h) * w + wrap(uv[0], w)) * 4;
    [texture.rgba[at], texture.rgba[at + 1], texture.rgba[at + 2], texture.rgba[at + 3]]
}

fn draw_skeleton(model: &Model, camera: &Camera, target: &mut Canvas) {
    let segments: Vec<[[f32; 3]; 2]> =
        model.bones.iter().map(|segment| segment.map(|joint| camera.to_view(joint))).collect();
    let framing = Framing::fit(segments.iter().flatten().copied(), target.side);
    let mut screen: Vec<[[f32; 3]; 2]> =
        segments.iter().map(|segment| segment.map(|joint| framing.project(joint))).collect();
    // Far to near, so nearer bones are drawn over farther ones.
    screen.sort_by(|a, b| (a[0][2] + a[1][2]).total_cmp(&(b[0][2] + b[1][2])));
    let near = screen.iter().flatten().map(|p| p[2]).fold(0.0f32, f32::max);
    let far = screen.iter().flatten().map(|p| p[2]).fold(f32::INFINITY, f32::min);
    // Nearer is lighter, so the figure reads as 3D.
    let shade = |inv_w: f32| 0.55 + 0.45 * ((inv_w - far) / (near - far).max(1e-12)).clamp(0.0, 1.0);

    let width = target.side as f32 * 0.012;
    for [a, b] in &screen {
        let light = shade((a[2] + b[2]) * 0.5);
        fill_capsule(target, [a[0], a[1]], [b[0], b[1]], width, [0.78 * light, 0.8 * light, 0.86 * light]);
    }
    for [a, b] in &screen {
        for joint in [a, b] {
            let light = shade(joint[2]);
            fill_capsule(
                target,
                [joint[0], joint[1]],
                [joint[0], joint[1]],
                width * 1.9,
                [light, 0.62 * light, 0.2 * light],
            );
        }
    }
}

/// Fills every sample within `radius` of the segment a-b.
fn fill_capsule(target: &mut Canvas, a: [f32; 2], b: [f32; 2], radius: f32, color: [f32; 3]) {
    if !a.iter().chain(&b).all(|c| c.is_finite()) {
        return;
    }
    let side = target.side as f32;
    let x0 = (a[0].min(b[0]) - radius - 0.5).ceil().clamp(0.0, side) as usize;
    let y0 = (a[1].min(b[1]) - radius - 0.5).ceil().clamp(0.0, side) as usize;
    let x1 = (a[0].max(b[0]) + radius - 0.5).floor().clamp(-1.0, side - 1.0);
    let y1 = (a[1].max(b[1]) + radius - 0.5).floor().clamp(-1.0, side - 1.0);
    if x1 < 0.0 || y1 < 0.0 {
        return;
    }
    let ab = [b[0] - a[0], b[1] - a[1]];
    let length2 = ab[0] * ab[0] + ab[1] * ab[1];
    for y in y0..=y1 as usize {
        for x in x0..=x1 as usize {
            let p = [x as f32 + 0.5 - a[0], y as f32 + 0.5 - a[1]];
            let t = if length2 > 0.0 { ((p[0] * ab[0] + p[1] * ab[1]) / length2).clamp(0.0, 1.0) } else { 0.0 };
            let d = [p[0] - ab[0] * t, p[1] - ab[1] * t];
            if d[0] * d[0] + d[1] * d[1] <= radius * radius {
                let sample = y * target.side + x;
                target.color[sample] = color.map(|c| c.clamp(0.0, 1.0));
                target.covered[sample] = true;
            }
        }
    }
}

fn srgb8_to_linear() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| std::array::from_fn(|i| super::srgb_to_linear(i as f32 / 255.0)))
}

/// Linear 0..1 in 4,096 steps to 8-bit sRGB: fine enough that no step is visible.
fn linear_to_srgb8() -> &'static [u8; 4096] {
    static LUT: OnceLock<[u8; 4096]> = OnceLock::new();
    LUT.get_or_init(|| std::array::from_fn(|i| (super::linear_to_srgb(i as f32 / 4095.0) * 255.0).round() as u8))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Bounds, Material, ModelInfo, Part, Vertex};

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
