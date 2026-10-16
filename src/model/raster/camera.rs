//! Where the thumbnail's camera looks from, and how the projected model is framed in the square.

use super::MARGIN;
use crate::model::vector::{cross, dot, normalize, sub};
use crate::model::{Model, shading};

/// View space as in `shading`: looking down -Z, +Y up, +X right.
pub(super) struct Camera {
    right: [f32; 3],
    up: [f32; 3],
    back: [f32; 3],
    eye: [f32; 3],
}

impl Camera {
    pub(super) fn new(model: &Model) -> Self {
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

    pub(super) fn to_view(&self, p: [f32; 3]) -> [f32; 3] {
        self.direction_to_view(sub(p, self.eye))
    }

    pub(super) fn direction_to_view(&self, d: [f32; 3]) -> [f32; 3] {
        [dot(d, self.right), dot(d, self.up), dot(d, self.back)]
    }
}

/// Maps view-space points to sample coordinates, fitted to what's drawn.
pub(super) struct Framing {
    scale: f32,
    center: [f32; 2],
    half: f32,
}

impl Framing {
    /// Fits the projection of `points` (view space, all in front of the camera) into a square of
    /// `side` samples.
    pub(super) fn fit(points: impl Iterator<Item = [f32; 3]>, side: usize) -> Self {
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
    pub(super) fn project(&self, p: [f32; 3]) -> [f32; 3] {
        let inv_w = 1.0 / -p[2];
        [
            (p[0] * inv_w - self.center[0]) * self.scale + self.half,
            (self.center[1] - p[1] * inv_w) * self.scale + self.half,
            inv_w,
        ]
    }
}
