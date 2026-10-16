//! The rasteriser: which triangle is nearest at each sample, then each pixel shaded once, or sample
//! by sample along edges.

use super::camera::{Camera, Framing};
use super::colour::{self, linear_to_srgb8, srgb8_to_linear};
use super::{BACK_FACE_DEPTH, EDGE_SLACK, NO_TRIANGLE, SAMPLES, SUPERSAMPLE};
use crate::model::vector::normalize;
use crate::model::{Material, Model, Part, Texture, Vertex, shading};

pub(super) fn draw_triangles(model: &Model, camera: &Camera, size: usize) -> Vec<u8> {
    let side = size * SUPERSAMPLE;
    let screen = screen_positions(model, camera, side);
    let nearest = nearest_triangles(model, &screen, side);
    let mut shading = Shading::new(model, camera, &screen);
    let table = linear_to_srgb8();
    let mut out = vec![0u8; size * size * 4];
    for (index, pixel) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (px, py) = (index % size, index / size);
        if let Some((color, count)) = shading.pixel(px, py, &nearest, side)
            && count > 0
        {
            *pixel = colour::pixel(table, color, count);
        }
    }
    out
}

/// Every vertex on the screen: x and y in samples, and 1 / depth. The vertices drawn fill the
/// picture.
fn screen_positions(model: &Model, camera: &Camera, side: usize) -> Vec<[f32; 3]> {
    let view: Vec<[f32; 3]> = model.vertices.iter().map(|vertex| camera.to_view(vertex.position)).collect();
    let mut used = vec![false; view.len()];
    for &index in &model.indices {
        if let Some(used) = used.get_mut(index as usize) {
            *used = true;
        }
    }
    let framing = Framing::fit(view.iter().zip(&used).filter(|(_, used)| **used).map(|(p, _)| *p), side);
    view.iter().map(|&p| framing.project(p)).collect()
}

/// Visibility: the nearest triangle at each sample, by 1 / depth (larger is nearer).
fn nearest_triangles(model: &Model, screen: &[[f32; 3]], side: usize) -> Vec<u32> {
    let mut depth = vec![0.0f32; side * side];
    let mut nearest = vec![NO_TRIANGLE; side * side];
    for (id, corners, material) in triangles(model) {
        let [a, b, c] = corners.map(|corner| screen[corner]);
        let cutout = material.alpha_cutoff.map(|cutoff| Cutout {
            cutoff,
            alpha: material.color[3],
            texture: material.texture.and_then(|index| model.textures.get(index as usize)),
            corners: corners.map(|v| &model.vertices[v]),
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
            if cutout.as_ref().is_some_and(|cutout| cutout.cut_away([a, b, c], l, inv_w)) {
                return;
            }
            depth[sample] = inv_w * bias;
            nearest[sample] = id;
        });
    }
    nearest
}

/// A cut-out material on one triangle: leaves, fences and hair cards have holes where their alpha
/// is below the cut-off.
struct Cutout<'a> {
    cutoff: f32,
    /// The material's own alpha.
    alpha: f32,
    texture: Option<&'a Texture>,
    corners: [&'a Vertex; 3],
}

impl Cutout<'_> {
    /// Whether the triangle has a hole at a sample: `l` are its screen weights there and `inv_w`
    /// its 1 / depth.
    fn cut_away(&self, screen: [[f32; 3]; 3], l: [f32; 3], inv_w: f32) -> bool {
        let [a, b, c] = screen;
        let [va, vb, vc] = self.corners;
        let weights = [l[0] * a[2] / inv_w, l[1] * b[2] / inv_w, l[2] * c[2] / inv_w];
        let mut alpha = self.alpha * mix(weights, [va.color[3]], [vb.color[3]], [vc.color[3]])[0];
        if let Some(texture) = self.texture {
            alpha *= sample_texture(texture, mix(weights, va.uv, vb.uv, vc.uv))[3] as f32 / 255.0;
        }
        alpha < self.cutoff
    }
}

/// Shades pixels from the nearest triangle at each of their samples, keeping the last
/// triangle's setup: neighbouring pixels mostly share it.
struct Shading<'a> {
    model: &'a Model,
    screen: &'a [[f32; 3]],
    /// Every vertex's normal in view space.
    normals: Vec<[f32; 3]>,
    srgb: &'static [f32; 256],
    current: Option<Setup<'a>>,
}

impl<'a> Shading<'a> {
    fn new(model: &'a Model, camera: &Camera, screen: &'a [[f32; 3]]) -> Self {
        let normals = model.vertices.iter().map(|v| camera.direction_to_view(v.normal)).collect();
        Self { model, screen, normals, srgb: srgb8_to_linear(), current: None }
    }

    /// The pixel's linear colour and how many of its samples are covered; `None` when nothing is
    /// drawn there.
    fn pixel(&mut self, px: usize, py: usize, nearest: &[u32], side: usize) -> Option<([f32; 3], usize)> {
        let mut ids = [NO_TRIANGLE; SAMPLES];
        let mut centers = [[0.0f32; 2]; SAMPLES];
        for (k, (id, center)) in ids.iter_mut().zip(&mut centers).enumerate() {
            let (sx, sy) = (px * SUPERSAMPLE + k % SUPERSAMPLE, py * SUPERSAMPLE + k / SUPERSAMPLE);
            *id = nearest[sy * side + sx];
            *center = [sx as f32 + 0.5, sy as f32 + 0.5];
        }
        if ids.iter().all(|&id| id == ids[0]) {
            if ids[0] == NO_TRIANGLE {
                return None;
            }
            let middle = [
                (px * SUPERSAMPLE) as f32 + SUPERSAMPLE as f32 * 0.5,
                (py * SUPERSAMPLE) as f32 + SUPERSAMPLE as f32 * 0.5,
            ];
            self.inside(ids[0], middle, &centers).map(|color| (color, SAMPLES))
        } else {
            Some(self.on_edge(&ids, &centers))
        }
    }

    /// A pixel inside one triangle: shaded once, at its middle, with its texture sampled at every
    /// sample.
    fn inside(&mut self, id: u32, middle: [f32; 2], centers: &[[f32; 2]; SAMPLES]) -> Option<[f32; 3]> {
        let setup = Setup::cached(&mut self.current, id, self.model, self.screen, &self.normals)?;
        let weights = setup.weights(middle);
        let mut albedo = setup.albedo(weights);
        if let Some(texture) = setup.texture {
            let mut texels = [0.0f32; 3];
            for &center in centers {
                let texel = sample_texture(texture, setup.uv(setup.weights(center)));
                texels = [0, 1, 2].map(|k| texels[k] + self.srgb[texel[k] as usize]);
            }
            albedo = [0, 1, 2].map(|k| albedo[k] * texels[k] / SAMPLES as f32);
        }
        Some(shading::shade(albedo, setup.normal(weights)).map(|c| c.clamp(0.0, 1.0)))
    }

    /// A pixel on an edge: every covered sample shaded on its own, then averaged; with how many
    /// are covered.
    fn on_edge(&mut self, ids: &[u32; SAMPLES], centers: &[[f32; 2]; SAMPLES]) -> ([f32; 3], usize) {
        let (mut sum, mut count) = ([0.0f32; 3], 0);
        for (&id, &center) in ids.iter().zip(centers) {
            if id == NO_TRIANGLE {
                continue;
            }
            let Some(setup) = Setup::cached(&mut self.current, id, self.model, self.screen, &self.normals) else {
                continue;
            };
            let weights = setup.weights(center);
            let mut albedo = setup.albedo(weights);
            if let Some(texture) = setup.texture {
                let texel = sample_texture(texture, setup.uv(weights));
                albedo = [0, 1, 2].map(|k| albedo[k] * self.srgb[texel[k] as usize]);
            }
            let color = shading::shade(albedo, setup.normal(weights)).map(|c| c.clamp(0.0, 1.0));
            sum = [0, 1, 2].map(|k| sum[k] + color[k]);
            count += 1;
        }
        (sum.map(|c| c / count.max(1) as f32), count)
    }
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

/// Every triangle with its id (its position in `indices` / 3), corners and material.
fn triangles(model: &Model) -> impl Iterator<Item = (u32, [usize; 3], &Material)> {
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
