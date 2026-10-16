//! The made-up pictures' drawing kit: colours, paints, a canvas that draws shapes with smooth
//! edges, the shapes, and noise for textures.

use std::f32::consts::PI;
use std::io;
use std::path::Path;

/// Red, green, blue and alpha, from 0 to 1, not premultiplied.
pub(super) type Rgba = [f32; 4];

pub(super) const WHITE: Rgba = [1.0; 4];

pub(super) const BLACK: Rgba = [0.0, 0.0, 0.0, 1.0];

pub(super) const CLEAR: Rgba = [0.0; 4];

pub(super) fn hex(rgb: u32) -> Rgba {
    let channel = |shift: u32| ((rgb >> shift) & 255) as f32 / 255.0;
    [channel(16), channel(8), channel(0), 1.0]
}

pub(super) fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

pub(super) fn alpha(color: Rgba, alpha: f32) -> Rgba {
    [color[0], color[1], color[2], color[3] * alpha]
}

pub(super) fn lighter(color: Rgba, t: f32) -> Rgba {
    mix(color, [1.0, 1.0, 1.0, color[3]], t)
}

pub(super) fn darker(color: Rgba, t: f32) -> Rgba {
    mix(color, [0.0, 0.0, 0.0, color[3]], t)
}

pub(super) fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub(super) fn solid(color: Rgba) -> impl Fn(f32, f32) -> Rgba {
    move |_, _| color
}

/// `top` at `y0`, `bottom` at `y1`.
pub(super) fn vertical(y0: f32, y1: f32, top: Rgba, bottom: Rgba) -> impl Fn(f32, f32) -> Rgba {
    move |_, y| mix(top, bottom, (y - y0) / (y1 - y0))
}

/// Lit from (`x0`, `y0`): `light` there, `dark` `reach` away.
pub(super) fn radial(x0: f32, y0: f32, reach: f32, light: Rgba, dark: Rgba) -> impl Fn(f32, f32) -> Rgba {
    move |x, y| mix(light, dark, (x - x0).hypot(y - y0) / reach)
}

pub(super) struct Canvas {
    w: usize,
    h: usize,
    px: Vec<Rgba>,
}

impl Canvas {
    pub(super) fn new(w: usize, h: usize) -> Self {
        Self { w, h, px: vec![CLEAR; w * h] }
    }

    /// Every pixel from `color` at its centre: for textures and backgrounds.
    pub(super) fn paint(w: usize, h: usize, color: impl Fn(f32, f32) -> Rgba) -> Self {
        let mut canvas = Self::new(w, h);
        for y in 0..h {
            for x in 0..w {
                canvas.px[y * w + x] = color(x as f32 + 0.5, y as f32 + 0.5);
            }
        }
        canvas
    }

    /// `color` over where `inside` holds, with smooth edges (16 samples a pixel).
    pub(super) fn fill(&mut self, inside: impl Fn(f32, f32) -> bool, color: impl Fn(f32, f32) -> Rgba) {
        self.fill_in([0.0, 0.0, self.w as f32, self.h as f32], inside, color);
    }

    /// `fill`, looking only within `[x0, y0, x1, y1]`.
    pub(super) fn fill_in(
        &mut self,
        area: [f32; 4],
        inside: impl Fn(f32, f32) -> bool,
        color: impl Fn(f32, f32) -> Rgba,
    ) {
        let clamp = |v: f32, max: usize| (v.max(0.0) as usize).min(max);
        let (x0, y0) = (clamp(area[0].floor(), self.w), clamp(area[1].floor(), self.h));
        let (x1, y1) = (clamp(area[2].ceil(), self.w), clamp(area[3].ceil(), self.h));
        for y in y0..y1 {
            for x in x0..x1 {
                let mut hits = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        if inside(x as f32 + (sx as f32 + 0.5) / 4.0, y as f32 + (sy as f32 + 0.5) / 4.0) {
                            hits += 1;
                        }
                    }
                }
                if hits > 0 {
                    let color = color(x as f32 + 0.5, y as f32 + 0.5);
                    self.blend(x, y, color, hits as f32 / 16.0);
                }
            }
        }
    }

    pub(super) fn blend(&mut self, x: usize, y: usize, color: Rgba, coverage: f32) {
        let a = color[3] * coverage;
        let under = &mut self.px[y * self.w + x];
        let (keep, out) = (under[3] * (1.0 - a), a + under[3] * (1.0 - a));
        if out > 0.0 {
            for (channel, value) in under.iter_mut().zip(color).take(3) {
                *channel = (value * a + *channel * keep) / out;
            }
            under[3] = out;
        }
    }

    pub(super) fn rgba8(&self) -> Vec<u8> {
        self.px.iter().flat_map(|color| color.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)).collect()
    }

    pub(super) fn save(&self, path: &Path) -> io::Result<()> {
        image::save_buffer(path, &self.rgba8(), self.w as u32, self.h as u32, image::ExtendedColorType::Rgba8)
            .map_err(io::Error::other)
    }
}

pub(super) fn circle(cx: f32, cy: f32, r: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

pub(super) fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| ((x - cx) / rx).powi(2) + ((y - cy) / ry).powi(2) <= 1.0
}

pub(super) fn ring(cx: f32, cy: f32, outer: f32, inner: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| {
        let d = (x - cx).hypot(y - cy);
        d <= outer && d >= inner
    }
}

pub(super) fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| x >= x0 && x <= x1 && y >= y0 && y <= y1
}

pub(super) fn rounded(x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| {
        let dx = (x0 + r - x).max(x - (x1 - r)).max(0.0);
        let dy = (y0 + r - y).max(y - (y1 - r)).max(0.0);
        x >= x0 && x <= x1 && y >= y0 && y <= y1 && dx * dx + dy * dy <= r * r
    }
}

pub(super) fn capsule(ax: f32, ay: f32, bx: f32, by: f32, r: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| {
        let (px, py, vx, vy) = (x - ax, y - ay, bx - ax, by - ay);
        let t = ((px * vx + py * vy) / (vx * vx + vy * vy)).clamp(0.0, 1.0);
        (px - vx * t).powi(2) + (py - vy * t).powi(2) <= r * r
    }
}

pub(super) fn polygon(points: Vec<(f32, f32)>) -> impl Fn(f32, f32) -> bool {
    move |x, y| {
        let mut inside = false;
        let mut j = points.len() - 1;
        for (i, &(xi, yi)) in points.iter().enumerate() {
            let (xj, yj) = points[j];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                inside = !inside;
            }
            j = i;
        }
        inside
    }
}

/// A star's corners, `count` points, the first straight up (turned by `turn`).
#[expect(clippy::too_many_arguments, reason = "refactor: take fewer, grouped arguments")]
pub(super) fn star_points(cx: f32, cy: f32, outer: f32, inner: f32, count: usize, turn: f32) -> Vec<(f32, f32)> {
    (0..count * 2)
        .map(|i| {
            let r = if i % 2 == 0 { outer } else { inner };
            let a = turn + i as f32 * PI / count as f32 - PI / 2.0;
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

/// `shape` turned by `angle` around (`cx`, `cy`).
pub(super) fn turned(cx: f32, cy: f32, angle: f32, shape: impl Fn(f32, f32) -> bool) -> impl Fn(f32, f32) -> bool {
    let (sin, cos) = angle.sin_cos();
    move |x, y| {
        let (dx, dy) = (x - cx, y - cy);
        shape(cx + dx * cos + dy * sin, cy - dx * sin + dy * cos)
    }
}

pub(super) fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h =
        (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

/// Smooth noise from 0 to 1 that repeats every `period` cells, so tiles join up.
pub(super) fn noise(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - xi as f32, y - yi as f32);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let at = |dx: i32, dy: i32| hash((xi + dx).rem_euclid(period), (yi + dy).rem_euclid(period), seed);
    let top = at(0, 0) + (at(1, 0) - at(0, 0)) * sx;
    let bottom = at(0, 1) + (at(1, 1) - at(0, 1)) * sx;
    top + (bottom - top) * sy
}

/// Layers of `noise`, each finer and fainter.
pub(super) fn fbm(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut total, mut scale) = (0.0, 1.0, 0.0, 1);
    for octave in 0..4 {
        sum += noise(x * scale as f32, y * scale as f32, period * scale, seed + octave) * amp;
        total += amp;
        amp *= 0.5;
        scale *= 2;
    }
    sum / total
}

/// Distances to the nearest and next nearest of points scattered one a cell (repeating every
/// `period` cells), and the nearest one's cell.
pub(super) fn cells(x: f32, y: f32, period: i32, seed: u32) -> (f32, f32, i32) {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (mut d1, mut d2, mut id) = (f32::MAX, f32::MAX, 0);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (cx, cy) = (xi + dx, yi + dy);
            let (wx, wy) = (cx.rem_euclid(period), cy.rem_euclid(period));
            let px = cx as f32 + 0.15 + 0.7 * hash(wx, wy, seed);
            let py = cy as f32 + 0.15 + 0.7 * hash(wx, wy, seed + 1);
            let d = (x - px).hypot(y - py);
            if d < d1 {
                (d2, d1, id) = (d1, d, wy * period + wx);
            } else if d < d2 {
                d2 = d;
            }
        }
    }
    (d1, d2, id)
}
