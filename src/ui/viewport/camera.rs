//! Where the 3D view looks from: the orbit the user turns, pans and zooms, and the matrices it
//! makes.

use eframe::egui::{self, Rect, Ui};

use crate::model::shading;
use crate::model::vector::{cross, dot, normalize};
use crate::ui::{theme, widgets};

use super::gpu::Globals;
use super::scene::Scene;

#[derive(Clone, Copy)]
pub(super) struct Orbit {
    pub(super) yaw: f32,
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

impl Orbit {
    /// Dragging turns the model, right- or middle-dragging pans, scrolling zooms, a double-click
    /// goes back to the start. Whether the user is moving the view.
    pub(super) fn take_input(&mut self, ui: &Ui, response: &egui::Response, rect: Rect, radius: f32) -> bool {
        let mut interacting = false;
        if response.dragged_by(egui::PointerButton::Primary) {
            let delta = response.drag_delta();
            self.yaw -= delta.x * 0.01;
            self.pitch = (self.pitch + delta.y * 0.01).clamp(-1.55, 1.55);
            interacting = true;
        }
        if response.dragged_by(egui::PointerButton::Secondary) || response.dragged_by(egui::PointerButton::Middle) {
            let delta = response.drag_delta();
            let (right, up, _) = camera_axes(self);
            let distance = fit_distance(radius, rect.aspect_ratio()) * self.zoom;
            let per_point = 2.0 * distance * (shading::FOV_Y / 2.0).tan() / rect.height().max(1.0);
            for axis in 0..3 {
                self.pan[axis] -= (right[axis] * delta.x - up[axis] * delta.y) * per_point;
            }
            interacting = true;
        }
        if response.hovered() {
            let scroll = widgets::take_scroll(ui);
            if scroll != 0.0 {
                self.zoom = (self.zoom * (-scroll * 0.0015).exp()).clamp(0.05, 20.0);
            }
        }
        if response.double_clicked() {
            *self = Self::default();
        }
        interacting
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
    // Looking straight up or down, the up axis gives no right; any level direction will do.
    let right = if right == [0.0; 3] { [1.0, 0.0, 0.0] } else { right };
    let up = cross(back, right);
    (right, up, back)
}

pub(super) fn globals(orbit: &Orbit, scene: &Scene, size: [u32; 2]) -> Globals {
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

#[cfg(test)]
mod tests {
    use super::*;

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
