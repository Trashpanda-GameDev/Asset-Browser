//! A model with bones and no mesh, drawn as a stick figure.

use super::camera::{Camera, Framing};
use super::canvas::Canvas;
use crate::model::Model;

pub(super) fn draw_skeleton(model: &Model, camera: &Camera, target: &mut Canvas) {
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
