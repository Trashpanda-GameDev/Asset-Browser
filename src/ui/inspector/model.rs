//! A 3D model in the pane: turned in the 3D view (the thumbnail stands in until it's loaded), and its
//! details.

use eframe::egui::{Align2, FontId, RichText, Sense, Ui, vec2};

use crate::library::Entry;
use crate::ui::browse::{Look, PreviewOf, draw_preview};
use crate::ui::format::{format_time, grouped};
use crate::ui::previews::Previews;
use crate::ui::viewport::ShownModel;
use crate::ui::{theme, widgets};

use super::{DetailState, Inspector, InspectorArgs, overlay, pending, spinner_badge};

impl Inspector {
    pub(super) fn model_section(&mut self, ui: &mut Ui, entry: &Entry, args: &InspectorArgs<'_>, width: f32) {
        let (previews, render_state) = (&*args.previews, args.render_state);
        let height = (width * 0.9).clamp(200.0, 460.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let model = match &self.detail {
            Some((key, DetailState::Model(model))) if *key == entry.key => Some(model.clone()),
            _ => None,
        };
        let shown_3d = match (model, render_state) {
            (Some(model), Some(render_state)) => {
                self.viewport.show(ui, rect, render_state, ShownModel { key: entry.key, model: &model })
            }
            _ => false,
        };
        if shown_3d {
            overlay(ui, rect, |ui| self.viewport.controls(ui));
            return;
        }
        // Until the model is loaded (or when it can't be shown in 3D), the thumbnail stands in.
        ui.painter().rect_filled(rect, theme::CARD_RADIUS, theme::WELL);
        let inner = rect.shrink(16.0);
        let preview = PreviewOf { entry, previews, playing: None, large: true };
        draw_preview(ui.painter(), inner, &preview, Look::new(ui, self.display.backdrop));
        let note = match &self.detail {
            Some((key, DetailState::Loading)) if *key == entry.key => {
                spinner_badge(ui, rect);
                None
            }
            Some((key, DetailState::Failed(message))) if *key == entry.key => Some(message.as_str()),
            _ if entry.extension().eq_ignore_ascii_case("blend") => Some("The preview Blender saved in the file"),
            _ if render_state.is_none() => Some("3D view needs the wgpu renderer"),
            _ => None,
        };
        if let Some(note) = note {
            ui.painter().text(
                rect.center_bottom() + vec2(0.0, -10.0),
                Align2::CENTER_BOTTOM,
                note,
                FontId::proportional(theme::size::SMALL),
                theme::TEXT_FAINT,
            );
        }
    }

    pub(super) fn model_properties(&self, ui: &mut Ui, entry: &Entry, previews: &Previews) {
        let loaded = match &self.detail {
            Some((key, DetailState::Model(detail))) if *key == entry.key => Some(&detail.model.info),
            _ => None,
        };
        let Some(info) = loaded.or(previews.models.get(&entry.key).map(|info| info.as_ref())) else {
            if let Some(message) = previews.failed.get(&entry.key) {
                widgets::property(ui, "Preview", RichText::new(message).color(theme::ERROR));
            } else if entry.extension().eq_ignore_ascii_case("blend") {
                widgets::property(ui, "Format", pending());
            } else {
                for name in ["Format", "Triangles", "Vertices", "Meshes", "Materials", "Textures", "Dimensions"] {
                    widgets::property(ui, name, pending());
                }
            }
            return;
        };
        widgets::property(ui, "Format", &info.format);
        if info.geometry {
            widgets::property(ui, "Triangles", grouped(info.triangles as usize));
            widgets::property(ui, "Vertices", grouped(info.vertices as usize));
            widgets::property(ui, "Meshes", grouped(info.meshes as usize));
            widgets::property(ui, "Materials", grouped(info.materials as usize));
            let textures = match (info.textures, info.missing_textures) {
                (0, 0) => "none".to_owned(),
                (found, 0) => grouped(found as usize),
                (found, missing) => format!("{found} ({missing} not found)"),
            };
            widgets::property(ui, "Textures", textures);
            let [x, y, z] = info.size;
            widgets::property(ui, "Dimensions", format!("{} × {} × {} m", metres(x), metres(y), metres(z)));
        }
        for (name, seconds) in &info.animations {
            let name = if name.is_empty() { "(unnamed)" } else { name.as_str() };
            widgets::property(ui, "Animation", format!("{name} · {}", format_time(*seconds as f64)));
        }
    }
}

fn metres(value: f32) -> String {
    if value >= 10.0 {
        format!("{value:.1}")
    } else if value >= 0.1 {
        format!("{value:.2}")
    } else {
        format!("{value:.3}")
    }
}
