//! The inspector's 3D view. The selected model is drawn on the GPU egui already uses, into a
//! texture egui then shows like any picture: drag to orbit, right- or middle-drag to pan, scroll to
//! zoom, double-click to reset.
//!
//! Lighting comes from `model::shading`, the same constants the thumbnails are rendered with, so a
//! model looks the same in its tile and here. The picture is redrawn only when something changed.

use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, TextureId, Ui, pos2, vec2};
use eframe::egui_wgpu::RenderState;
use egui_phosphor::regular as icon;

use crate::detail::DetailModel;

use super::{theme, widgets};

mod camera;
mod gpu;
mod scene;
mod shader;

use camera::{Orbit, globals};
use gpu::{Globals, Gpu, Target};
use scene::{Scene, edge_buffer};

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

/// A model to show, by the key it's cached under: a new key is a new model, viewed afresh.
#[derive(Clone, Copy)]
pub struct ShownModel<'a> {
    pub key: i64,
    pub model: &'a DetailModel,
}

impl Viewport {
    /// Draws the model in `rect`. Returns false if there's no GPU to draw with.
    pub fn show(&mut self, ui: &mut Ui, rect: Rect, render_state: &RenderState, shown: ShownModel<'_>) -> bool {
        let gpu = self.gpu.get_or_insert_with(|| Gpu::new(render_state));
        if self.scene.as_ref().is_none_or(|scene| scene.key != shown.key) {
            self.scene = Some(Scene::upload(gpu, shown.key, shown.model));
            self.orbit = Orbit::default();
            self.drawn = None;
        }
        let Some(scene) = self.scene.as_mut() else { return false };
        let response = ui.interact(rect, ui.id().with("viewport"), Sense::click_and_drag());
        let interacting = self.orbit.take_input(ui, &response, rect, scene.radius);
        if self.auto_rotate && !interacting {
            let dt = ui.input(|input| input.stable_dt).min(0.1);
            self.orbit.yaw += AUTO_ROTATE_SPEED * dt;
            ui.ctx().request_repaint();
        }
        let size = render_size(ui, rect, gpu);
        if self.target.as_ref().is_none_or(|target| target.size != size) {
            let old = self.target.take();
            self.target = Some(Target::new(gpu, size, old.map(|target| target.id)));
            self.drawn = None;
        }
        let Some(target) = &self.target else { return false };
        let globals = globals(&self.orbit, scene, size);
        if self.drawn != Some(globals) || self.drawn_wireframe != self.wireframe {
            if self.wireframe && scene.edges.is_none() {
                scene.edges = edge_buffer(gpu, &shown.model.edges);
            }
            gpu.queue.write_buffer(&gpu.globals, 0, bytemuck::bytes_of(&globals));
            gpu.render(scene, target, self.wireframe);
            self.drawn = Some(globals);
            self.drawn_wireframe = self.wireframe;
        }
        paint(ui, rect, target.id, &response, interacting);
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

/// The view's size in physical pixels, within what the GPU can draw.
fn render_size(ui: &Ui, rect: Rect, gpu: &Gpu) -> [u32; 2] {
    let pixels_per_point = ui.ctx().pixels_per_point();
    let max_side = gpu.device.limits().max_texture_dimension_2d.min(4096);
    [
        ((rect.width() * pixels_per_point).round() as u32).clamp(1, max_side),
        ((rect.height() * pixels_per_point).round() as u32).clamp(1, max_side),
    ]
}

/// The rendered picture over the view's backdrop, its border, and how to move the view while the
/// pointer is over it.
fn paint(ui: &Ui, rect: Rect, picture: TextureId, response: &egui::Response, interacting: bool) {
    let painter = ui.painter_at(rect);
    widgets::vertical_gradient(&painter, rect, theme::VIEWPORT_TOP, theme::WELL);
    painter.image(picture, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    painter.rect_stroke(rect, theme::CARD_RADIUS, Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
    if response.hovered() && !interacting {
        painter.text(
            rect.left_bottom() + vec2(10.0, -8.0),
            Align2::LEFT_BOTTOM,
            "drag to orbit · right-drag to pan · scroll to zoom",
            FontId::proportional(theme::size::FINE),
            theme::TEXT_FAINT,
        );
    }
}
