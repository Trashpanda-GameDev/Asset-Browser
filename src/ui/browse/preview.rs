//! An asset's preview, wherever it's drawn: its thumbnail, a sound's waveform, or a stand-in while
//! it's made.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, TextureId, pos2, vec2};
use egui_phosphor::regular as icon;

use crate::asset::AssetKind;
use crate::audio::NowPlaying;
use crate::library::Entry;
use crate::ui::display::Backdrop;
use crate::ui::format::format_time;
use crate::ui::previews::{CachedTexture, Previews};
use crate::ui::sound::draw_waveform;
use crate::ui::{theme, widgets};

use super::Look;

/// An asset's preview to draw, and what goes with it.
#[derive(Clone, Copy)]
pub struct PreviewOf<'a> {
    pub entry: &'a Entry,
    pub previews: &'a Previews,
    /// It's the sound playing: its waveform shows how far.
    pub playing: Option<&'a NowPlaying>,
    /// There's room for a sound's length (and a speaker while it plays) under its waveform.
    pub large: bool,
}

/// Draws an asset's preview into `rect`: its thumbnail, waveform, or a placeholder while it's made.
pub fn draw_preview(painter: &egui::Painter, rect: Rect, preview: &PreviewOf<'_>, look: Look) {
    let PreviewOf { entry, previews, playing, large } = *preview;
    if entry.kind == AssetKind::Model {
        widgets::vertical_gradient(painter, rect, theme::MODEL_TILE[0], theme::MODEL_TILE[1]);
    }
    if let Some(texture) = previews.texture(entry.key) {
        let backdrop = (entry.kind == AssetKind::Image).then_some((look.backdrop, previews.checker.id()));
        draw_texture(painter, rect, texture, backdrop, look.pixels_per_point);
    } else if let Some(waveform) = previews.waveforms.get(&entry.key) {
        let color = theme::kind_color(entry.kind);
        let progress = playing.map(|now| now.progress(waveform.duration as f64));
        let wave_rect = if large { Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y - 16.0)) } else { rect };
        draw_waveform(painter, wave_rect, waveform, [color.gamma_multiply(0.5), color], progress);
        if large {
            painter.text(
                rect.right_bottom(),
                Align2::RIGHT_BOTTOM,
                format_time(waveform.duration as f64),
                FontId::proportional(theme::size::FINE),
                theme::TEXT_WEAK,
            );
            if playing.is_some() {
                painter.text(
                    rect.left_bottom(),
                    Align2::LEFT_BOTTOM,
                    icon::SPEAKER_HIGH,
                    FontId::proportional(theme::size::BODY),
                    color,
                );
            }
        }
    } else if previews.failed.contains_key(&entry.key) {
        let size = (rect.height() * 0.28).clamp(12.0, 34.0);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            icon::WARNING_CIRCLE,
            FontId::proportional(size),
            theme::ERROR.gamma_multiply(0.8),
        );
    } else {
        let size = (rect.height() * 0.3).clamp(12.0, 40.0);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            theme::kind_icon(entry.kind),
            FontId::proportional(size),
            theme::kind_color(entry.kind).gamma_multiply(0.35),
        );
    }
}

/// A thumbnail fitted into `area`, a picture's with its backdrop (and the checkerboard's texture)
/// behind it.
fn draw_texture(
    painter: &egui::Painter,
    area: Rect,
    texture: &CachedTexture,
    backdrop: Option<(Backdrop, TextureId)>,
    pixels_per_point: f32,
) {
    let [w, h] = texture.handle.size();
    let fitted = fit_rect(area, w as f32, h as f32, texture.unscaled, pixels_per_point);
    match backdrop {
        Some((Backdrop::Checker, checker)) => {
            let checker_uv = Rect::from_min_max(Pos2::ZERO, pos2(fitted.width() / 12.0, fitted.height() / 12.0));
            painter.image(checker, fitted, checker_uv, Color32::WHITE);
        }
        Some((Backdrop::Light, _)) => {
            painter.rect_filled(fitted, 0.0, Backdrop::LIGHT);
        }
        Some((Backdrop::Dark, _)) | None => {}
    }
    painter.image(texture.handle.id(), fitted, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
}

/// Fits `w` × `h` inside `area`, keeping its shape. Unscaled thumbnails (pixel art) are scaled by
/// whole physical pixels and snapped to the pixel grid, so every pixel stays the same size.
pub fn fit_rect(area: Rect, w: f32, h: f32, pixel_exact: bool, pixels_per_point: f32) -> Rect {
    let mut scale = (area.width() / w).min(area.height() / h);
    let physical = scale * pixels_per_point;
    if pixel_exact && physical >= 1.0 {
        scale = physical.floor() / pixels_per_point;
    }
    let size = vec2(w * scale, h * scale);
    let rect = Rect::from_center_size(area.center(), size);
    if pixel_exact {
        let min = (rect.min.to_vec2() * pixels_per_point).round() / pixels_per_point;
        Rect::from_min_size(min.to_pos2(), size)
    } else {
        rect
    }
}
