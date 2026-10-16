//! The inspector's picture view: fit to the pane, scroll to zoom at the cursor, drag to pan,
//! double-click to switch between fit and actual pixels. The sprite sheet player shows its frames
//! through it too, so an animation zooms the same way.

use eframe::egui::{
    Align2, Color32, FontId, Painter, Pos2, Rect, Response, Sense, Stroke, StrokeKind, TextureId, Ui, Vec2, pos2, vec2,
};

use super::display::Backdrop;
use super::{theme, widgets};

/// What to draw: a texture showing the whole picture, and the picture's real size.
pub struct Picture {
    pub texture: TextureId,
    pub size: [u32; 2],
    /// Small pixel art: fit by whole pixels so every pixel stays square and sharp.
    pub pixel_art: bool,
}

/// Where the view is, for which asset, and what's behind the picture.
#[derive(Clone, Copy)]
pub struct Place {
    pub rect: Rect,
    /// The asset shown: its zoom is kept while this and the size shown stay.
    pub key: i64,
    pub backdrop: Backdrop,
    /// The checkerboard's texture, for `Backdrop::Checker`.
    pub checker: TextureId,
}

/// What's shown: something `size` pixels big.
#[derive(Clone, Copy)]
pub struct Content {
    pub size: [u32; 2],
    /// Small pixel art: fit by whole pixels so every pixel stays square and sharp.
    pub pixel_art: bool,
}

impl Content {
    fn size(self) -> Vec2 {
        let [w, h] = self.size.map(|side| side.max(1) as f32);
        vec2(w, h)
    }
}

pub struct ImageView {
    /// What's shown: the asset, and the size of what's drawn of it (the whole picture, or an
    /// animation's frames). A change of either starts over at fit.
    key: Option<(i64, [u32; 2])>,
    /// Screen points per picture pixel; `None` fits the picture in the view.
    zoom: Option<f32>,
    /// The picture point (0..1 on each axis) at the centre of the view.
    center: Vec2,
}

impl Default for ImageView {
    fn default() -> Self {
        Self { key: None, zoom: None, center: Vec2::splat(0.5) }
    }
}

impl ImageView {
    pub fn show(&mut self, ui: &mut Ui, place: &Place, picture: &Picture) {
        let texture = picture.texture;
        let content = Content { size: picture.size, pixel_art: picture.pixel_art };
        self.show_with(ui, place, content, |painter, area| {
            painter.image(texture, area, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        });
    }

    /// Shows `content`, zoomed and panned like a picture: `draw` paints it into the area it
    /// covers on screen (clipped to the place's rect).
    pub fn show_with(&mut self, ui: &mut Ui, place: &Place, content: Content, draw: impl FnOnce(&Painter, Rect)) {
        self.follow(place.key, content.size);
        let rect = place.rect;
        let response = ui.interact(rect, ui.id().with("image-view"), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, theme::CARD_RADIUS, place.backdrop.fill());
        let scale = self.take_input(ui, &response, rect, content);
        let pixels_per_point = ui.ctx().pixels_per_point();
        let image_rect = self.picture_rect(rect, content.size() * scale, content.pixel_art, pixels_per_point);
        if place.backdrop == Backdrop::Checker {
            checkerboard(&painter, rect, image_rect, place.checker);
        }
        draw(&painter, image_rect);
        self.readout(&painter, &response, place, scale * pixels_per_point);
        painter.rect_stroke(rect, theme::CARD_RADIUS, Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
    }

    /// Something new to show, or a new size of it (an animation's frames instead of the whole
    /// picture), starts over at fit.
    fn follow(&mut self, key: i64, size: [u32; 2]) {
        if self.key != Some((key, size)) {
            self.key = Some((key, size));
            self.zoom = None;
            self.center = Vec2::splat(0.5);
        }
    }

    /// Scrolling zooms around the pointer, dragging pans, a double-click switches between fit and
    /// 1:1. The scale to draw at, in points per picture pixel.
    fn take_input(&mut self, ui: &Ui, response: &Response, rect: Rect, content: Content) -> f32 {
        let size = content.size();
        let pixels_per_point = ui.ctx().pixels_per_point();
        let fit = Self::fit_scale(rect, size, content.pixel_art, pixels_per_point);
        let mut scale = self.zoom.unwrap_or(fit);
        // The picture point under the pointer stays put.
        if let Some(pointer) = response.hover_pos() {
            let scroll = widgets::take_scroll(ui);
            if scroll != 0.0 {
                let new_scale = (scale * (scroll * 0.0025).exp()).clamp(fit.min(1.0) * 0.5, 64.0);
                let under = self.picture_point(rect, pointer, scale, size);
                self.zoom = Some(new_scale);
                self.center = under - (pointer - rect.center()) / (size * new_scale);
                scale = new_scale;
            }
        }
        if response.dragged() {
            self.center -= response.drag_delta() / (size * scale);
            self.zoom = Some(self.zoom.unwrap_or(scale));
        }
        if response.double_clicked() {
            if self.zoom.is_some() {
                self.zoom = None;
            } else if let Some(pointer) = response.interact_pointer_pos() {
                self.center = self.picture_point(rect, pointer, scale, size);
                self.zoom = Some(1.0 / pixels_per_point);
            }
            scale = self.zoom.unwrap_or(fit);
        }
        self.center =
            if self.zoom.is_some() { self.center.clamp(Vec2::ZERO, Vec2::splat(1.0)) } else { Vec2::splat(0.5) };
        scale
    }

    /// Where the picture is on screen, `shown` big: on whole physical pixels when it's fitted or
    /// pixel art, so pixel art doesn't shimmer at its seams.
    fn picture_rect(&self, rect: Rect, shown: Vec2, pixel_art: bool, pixels_per_point: f32) -> Rect {
        let mut min = rect.center() - self.center * shown;
        if self.zoom.is_none() || pixel_art {
            min = ((min.to_vec2() * pixels_per_point).round() / pixels_per_point).to_pos2();
        }
        Rect::from_min_size(min, shown)
    }

    /// The zoom ("Fit" while fitted), and how to change it while the pointer is over the view.
    fn readout(&self, painter: &Painter, response: &Response, place: &Place, physical_scale: f32) {
        let percent = physical_scale * 100.0;
        let label = if self.zoom.is_none() { format!("Fit · {percent:.0}%") } else { format!("{percent:.0}%") };
        let rect = place.rect;
        widgets::badge(painter, rect.left_bottom() + vec2(8.0, -8.0), Align2::LEFT_BOTTOM, &label, theme::TEXT);
        if response.hovered() {
            painter.text(
                rect.right_bottom() + vec2(-10.0, -10.0),
                Align2::RIGHT_BOTTOM,
                "scroll to zoom · double-click for 1:1",
                FontId::proportional(theme::size::FINE),
                if place.backdrop == Backdrop::Light { Backdrop::LIGHT_TEXT } else { theme::TEXT_FAINT },
            );
        }
    }

    /// Screen points per picture pixel that fit the whole picture in `rect`.
    fn fit_scale(rect: Rect, size: Vec2, pixel_art: bool, pixels_per_point: f32) -> f32 {
        let room = rect.shrink(12.0);
        let scale = (room.width() / size.x).min(room.height() / size.y);
        if pixel_art && scale * pixels_per_point >= 1.0 {
            (scale * pixels_per_point).floor() / pixels_per_point
        } else {
            scale
        }
    }

    /// The picture point (0..1) under a screen position.
    fn picture_point(&self, rect: Rect, at: Pos2, scale: f32, size: Vec2) -> Vec2 {
        self.center + (at - rect.center()) / (size * scale)
    }
}

/// The checkerboard behind the part of the picture that's in view.
fn checkerboard(painter: &Painter, rect: Rect, image_rect: Rect, checker: TextureId) {
    let shown = image_rect.intersect(rect);
    let uv = Rect::from_min_max(
        ((shown.min - image_rect.min) / 12.0).to_pos2(),
        ((shown.max - image_rect.min) / 12.0).to_pos2(),
    );
    painter.image(checker, shown, uv, Color32::WHITE);
}
