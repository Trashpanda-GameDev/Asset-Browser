//! A picture in the pane: shown large to zoom and pan, played as a sprite sheet or GIF, and its
//! details.

use eframe::egui::{self, Align2, Color32, FontId, RichText, Sense, Ui, pos2, vec2};
use egui_phosphor::regular as icon;

use crate::library::{Entry, Library};
use crate::sheets::{Saved, Sheet, Source};
use crate::ui::browse::Look;
use crate::ui::format::{dimensions, format_time, grouped};
use crate::ui::image_view::{Picture, Place};
use crate::ui::previews::Previews;
use crate::ui::{display, theme, widgets};

use super::{DetailState, Inspector, overlay, pending, placeholder, spinner_badge};

impl Inspector {
    /// Hands the sprite sheet player the focused picture once it's loaded: what was detected in
    /// it, and what the user chose for it before.
    pub(super) fn load_sheet(&mut self, library: &Library, entry: &Entry, saved: Option<Saved>, now: f64) {
        let Some((key, DetailState::Image { pixels, sheet, .. })) = &self.detail else { return };
        let path_key = crate::groups::path_key(&library.path_of(entry));
        if *key != entry.key || self.sheet.is_for(&path_key) {
            return;
        }
        self.sheet.load(path_key, sheet.clone(), now);
        if let Some(saved) = saved {
            self.sheet.apply(saved, pixels);
        }
    }

    pub(super) fn detail_size(&self, key: i64) -> Option<[u32; 2]> {
        match &self.detail {
            Some((detail_key, DetailState::Image { size, sheet, .. })) if *detail_key == key => Some(match sheet {
                // What's loaded of a GIF is its frames side by side, not its size.
                Some(Sheet { source: Source::Animated { width, height, .. }, .. }) => [*width, *height],
                _ => *size,
            }),
            _ => None,
        }
    }

    /// A GIF's frames for its details ("24 · 2.4 s", "1" for a still one, "…" while they load);
    /// `None` for other pictures.
    pub(super) fn gif_frames(&self, entry: &Entry) -> Option<RichText> {
        if !entry.extension().eq_ignore_ascii_case("gif") {
            return None;
        }
        Some(match &self.detail {
            Some((key, DetailState::Image { sheet, .. })) if *key == entry.key => match sheet {
                Some(sheet) if matches!(sheet.source, Source::Animated { .. }) => {
                    let total = match sheet.source {
                        Source::Animated { total, .. } => total,
                        _ => sheet.frames.len(),
                    };
                    let ms: u32 = sheet.frames.iter().filter_map(|frame| frame.duration_ms).sum();
                    RichText::new(format!("{} · {}", grouped(total), format_time(f64::from(ms) / 1000.0)))
                }
                _ => RichText::new("1"),
            },
            Some((key, DetailState::Failed(_))) if *key == entry.key => RichText::new("–"),
            _ => pending(),
        })
    }

    /// The sprite sheet player's controls, once the picture's frames are loaded and playing.
    pub(super) fn animation_section(&mut self, ui: &mut Ui, entry: &Entry) {
        let Some((key, DetailState::Image { texture, size, pixels, .. })) = &self.detail else { return };
        if *key != entry.key || !self.sheet.animate || self.sheet.sheet.is_none() {
            return;
        }
        let sheet_picture = Picture { texture: texture.id(), size: *size, pixel_art: false };
        let pixels = pixels.clone();
        ui.add_space(10.0);
        widgets::section_label(ui, "Animation");
        self.sheet.controls(ui, &sheet_picture, &pixels);
    }

    pub(super) fn image_section(&mut self, ui: &mut Ui, entry: &Entry, previews: &Previews, width: f32) {
        let height = (width * 0.85).clamp(180.0, 440.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let place = Place { rect, key: entry.key, backdrop: self.display.backdrop, checker: previews.checker.id() };
        let now = ui.input(|input| input.time);
        // The full picture, once loaded: what the sprite sheet player draws from.
        let loaded = match &self.detail {
            Some((key, DetailState::Image { texture, size, pixels, .. })) if *key == entry.key => {
                Some((texture.id(), *size, pixels.clone()))
            }
            _ => None,
        };
        let animating = self.sheet.animate && self.sheet.sheet.is_some() && loaded.is_some();

        if let (Some((texture, size, _)), true) = (&loaded, animating) {
            let sheet_picture = Picture { texture: *texture, size: *size, pixel_art: false };
            self.sheet.draw(ui, &mut self.image, &place, &sheet_picture);
        } else {
            let picture = match (&loaded, previews.texture(entry.key)) {
                (Some((texture, size, _)), _) => Some(Picture { texture: *texture, size: *size, pixel_art: false }),
                (None, Some(thumbnail)) => Some(Picture {
                    texture: thumbnail.handle.id(),
                    size: thumbnail.src_size,
                    pixel_art: thumbnail.unscaled,
                }),
                (None, None) => None,
            };
            match picture {
                Some(picture) => self.image.show(ui, &place, &picture),
                None => placeholder(ui, rect, entry, previews, Look::new(ui, self.display.backdrop)),
            }
        }
        if matches!(&self.detail, Some((key, DetailState::Loading)) if *key == entry.key) {
            spinner_badge(ui, rect);
        }
        let (sheet, backdrop) = (&mut self.sheet, &mut self.display.backdrop);
        overlay(ui, rect, |ui| {
            display::backdrop_buttons(ui, backdrop);
            if let Some((_, _, pixels)) = &loaded {
                widgets::divider(ui);
                if animation_button(ui, sheet.animate, sheet.frame_count()).clicked() {
                    sheet.toggle(pixels, now);
                }
            }
        });
    }
}

/// `frames` is a GIF's frame count and length (`Inspector::gif_frames`).
pub(super) fn image_properties(
    ui: &mut Ui,
    entry: &Entry,
    previews: &Previews,
    detail: Option<[u32; 2]>,
    frames: Option<RichText>,
) {
    if let Some([w, h]) = detail.or(previews.dimensions(entry.key)) {
        widgets::property(ui, "Dimensions", format!("{} px", dimensions([w, h])));
    } else if let Some(message) = previews.failed.get(&entry.key) {
        widgets::property(ui, "Preview", RichText::new(message).color(theme::ERROR));
    } else {
        widgets::property(ui, "Dimensions", pending());
    }
    if let Some(frames) = frames {
        widgets::property(ui, "Frames", frames);
    }
    let format = entry.extension().to_uppercase();
    let format = if format == "PSD" { "PSD (flattened picture)".to_owned() } else { format };
    widgets::property(ui, "Format", format);
}

/// Switches the picture to its frames played and back. Lit up with the frame count when frames
/// were found but aren't shown, as an invitation.
fn animation_button(ui: &mut Ui, animating: bool, frames: Option<usize>) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(if frames.is_some() && !animating { 52.0 } else { 24.0 }, 24.0), Sense::click());
    let painter = ui.painter();
    let invite = frames.is_some() && !animating;
    let fill = if animating {
        theme::ACCENT_SOFT
    } else if response.hovered() {
        theme::CARD_HOVER
    } else if invite {
        theme::ACCENT.gamma_multiply(0.25)
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, theme::RADIUS, fill);
    let color = if animating || invite { theme::ACCENT } else { theme::TEXT_WEAK };
    if invite {
        painter.text(
            pos2(rect.left() + 13.0, rect.center().y),
            Align2::CENTER_CENTER,
            icon::FILM_STRIP,
            FontId::proportional(theme::size::MEDIUM),
            color,
        );
        let count = frames.map(|n| n.to_string()).unwrap_or_default();
        painter.text(
            pos2(rect.right() - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            count,
            FontId::proportional(theme::size::CAPTION),
            color,
        );
    } else {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            icon::FILM_STRIP,
            FontId::proportional(theme::size::MEDIUM),
            color,
        );
    }
    let tip = match (animating, frames) {
        (true, _) => "Show the whole picture".to_owned(),
        (false, Some(n)) => format!("Play its {n} frames as an animation"),
        (false, None) => "Play it as a sprite sheet (set the grid after)".to_owned(),
    };
    response.on_hover_text(tip)
}
