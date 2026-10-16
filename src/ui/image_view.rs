//! The inspector's picture view: fit to the pane, scroll to zoom at the cursor, drag to pan,
//! double-click to switch between fit and actual pixels. The sprite sheet player shows its frames
//! through it too, so an animation zooms the same way.

use eframe::egui::{
    Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, TextureId, Ui, Vec2, pos2, vec2,
};
use egui_phosphor::regular as icon;

use super::{theme, widgets};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backdrop {
    Checker,
    Dark,
    Light,
}

impl Backdrop {
    pub const ALL: [Backdrop; 3] = [Self::Checker, Self::Dark, Self::Light];
    pub const LIGHT: Color32 = Color32::from_gray(222);

    pub fn key(self) -> &'static str {
        match self {
            Self::Checker => "checker",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|backdrop| backdrop.key() == key)
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Self::Checker => icon::CHECKERBOARD,
            Self::Dark => icon::MOON,
            Self::Light => icon::SUN,
        }
    }

    pub fn tooltip(self) -> &'static str {
        match self {
            Self::Checker => "Checkerboard behind transparency",
            Self::Dark => "Dark background",
            Self::Light => "Light background",
        }
    }
}

/// What to draw: a texture showing the whole picture, and the picture's real size.
pub struct Picture {
    pub texture: TextureId,
    pub size: [u32; 2],
    /// Small pixel art: fit by whole pixels so every pixel stays square and sharp.
    pub pixel_art: bool,
}

pub struct ImageView {
    /// What's shown: the asset, and the size of what's drawn of it (the whole picture, or an
    /// animation's frames). A change of either starts over at fit.
    key: Option<(i64, [u32; 2])>,
    /// Screen points per picture pixel; `None` fits the picture in the view.
    zoom: Option<f32>,
    /// The picture point (0..1 on each axis) at the centre of the view.
    center: Vec2,
    pub backdrop: Backdrop,
}

impl Default for ImageView {
    fn default() -> Self {
        Self { key: None, zoom: None, center: Vec2::splat(0.5), backdrop: Backdrop::Checker }
    }
}

impl ImageView {
    #[expect(clippy::too_many_arguments, reason = "refactor: take fewer, grouped arguments")]
    pub fn show(&mut self, ui: &mut Ui, rect: Rect, key: i64, picture: &Picture, checker: TextureId) {
        let texture = picture.texture;
        self.show_with(ui, rect, key, picture.size, picture.pixel_art, checker, |painter, area| {
            painter.image(texture, area, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        });
    }

    /// Shows `size` pixels of something, zoomed and panned like a picture: `draw` paints it into
    /// the area it covers on screen (clipped to `rect`).
    #[expect(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        clippy::too_many_arguments,
        reason = "refactor: split into named steps; take fewer, grouped arguments"
    )]
    pub fn show_with(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        key: i64,
        size: [u32; 2],
        pixel_art: bool,
        checker: TextureId,
        draw: impl FnOnce(&Painter, Rect),
    ) {
        if self.key != Some((key, size)) {
            self.key = Some((key, size));
            self.zoom = None;
            self.center = Vec2::splat(0.5);
        }
        let response = ui.interact(rect, ui.id().with("image-view"), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let background = match self.backdrop {
            Backdrop::Light => Backdrop::LIGHT,
            _ => theme::WELL,
        };
        painter.rect_filled(rect, theme::CARD_RADIUS, background);

        let [w, h] = size.map(|side| side.max(1) as f32);
        let pixels_per_point = ui.ctx().pixels_per_point();
        let fit = Self::fit_scale(rect, w, h, pixel_art, pixels_per_point);
        let mut scale = self.zoom.unwrap_or(fit);

        // Scroll to zoom around the pointer; the picture point under it stays put.
        if let Some(pointer) = response.hover_pos() {
            let scroll = ui.input_mut(|input| std::mem::take(&mut input.smooth_scroll_delta.y));
            if scroll != 0.0 {
                let new_scale = (scale * (scroll * 0.0025).exp()).clamp(fit.min(1.0) * 0.5, 64.0);
                let under = self.picture_point(rect, pointer, scale, w, h);
                self.zoom = Some(new_scale);
                self.center = under - (pointer - rect.center()) / vec2(w * new_scale, h * new_scale);
                scale = new_scale;
            }
        }
        if response.dragged() {
            self.center -= response.drag_delta() / vec2(w * scale, h * scale);
            if self.zoom.is_none() {
                self.zoom = Some(scale);
            }
        }
        if response.double_clicked() {
            if self.zoom.is_some() {
                self.zoom = None;
                self.center = Vec2::splat(0.5);
            } else if let Some(pointer) = response.interact_pointer_pos() {
                let under = self.picture_point(rect, pointer, scale, w, h);
                self.zoom = Some(1.0 / pixels_per_point);
                self.center = under;
            }
            scale = self.zoom.unwrap_or(fit);
        }
        self.center = self.center.clamp(Vec2::ZERO, Vec2::splat(1.0));
        if self.zoom.is_none() {
            self.center = Vec2::splat(0.5);
        }

        let shown_size = vec2(w * scale, h * scale);
        let mut min = rect.center() - self.center * shown_size;
        if self.zoom.is_none() || pixel_art {
            // Whole physical pixels: no shimmering seams in pixel art.
            min = ((min.to_vec2() * pixels_per_point).round() / pixels_per_point).to_pos2();
        }
        let image_rect = Rect::from_min_size(min, shown_size);
        if self.backdrop == Backdrop::Checker {
            let shown = image_rect.intersect(rect);
            let uv = Rect::from_min_max(
                ((shown.min - image_rect.min) / 12.0).to_pos2(),
                ((shown.max - image_rect.min) / 12.0).to_pos2(),
            );
            painter.image(checker, shown, uv, Color32::WHITE);
        }
        draw(&painter, image_rect);

        // Zoom readout and the fit / 1:1 switch.
        let percent = scale * pixels_per_point * 100.0;
        let label = if self.zoom.is_none() { format!("Fit · {percent:.0}%") } else { format!("{percent:.0}%") };
        widgets::badge(&painter, rect.left_bottom() + vec2(8.0, -8.0), Align2::LEFT_BOTTOM, &label, theme::TEXT);
        if response.hovered() {
            painter.text(
                rect.right_bottom() + vec2(-10.0, -10.0),
                Align2::RIGHT_BOTTOM,
                "scroll to zoom · double-click for 1:1",
                FontId::proportional(11.0),
                if self.backdrop == Backdrop::Light { Color32::from_gray(90) } else { theme::TEXT_FAINT },
            );
        }
        painter.rect_stroke(rect, theme::CARD_RADIUS, Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
    }

    /// Screen points per picture pixel that fit the whole picture in `rect`.
    fn fit_scale(rect: Rect, w: f32, h: f32, pixel_art: bool, pixels_per_point: f32) -> f32 {
        let room = rect.shrink(12.0);
        let scale = (room.width() / w).min(room.height() / h);
        if pixel_art && scale * pixels_per_point >= 1.0 {
            (scale * pixels_per_point).floor() / pixels_per_point
        } else {
            scale
        }
    }

    /// The picture point (0..1) under a screen position.
    #[expect(clippy::too_many_arguments, reason = "refactor: take fewer, grouped arguments")]
    fn picture_point(&self, rect: Rect, at: Pos2, scale: f32, w: f32, h: f32) -> Vec2 {
        self.center + (at - rect.center()) / vec2(w * scale, h * scale)
    }
}

/// The backdrop switch shown above the picture.
pub fn backdrop_buttons(ui: &mut Ui, backdrop: &mut Backdrop) {
    for option in Backdrop::ALL {
        if widgets::icon_button_sized(ui, option.glyph(), option.tooltip(), *backdrop == option, 24.0).clicked() {
            *backdrop = option;
        }
    }
}
