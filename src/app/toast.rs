//! The brief confirmations at the bottom of the window ("Added 3 to Forest props").

use std::time::{Duration, Instant};

use eframe::egui::{self, Frame, Margin, RichText, vec2};
use egui_phosphor::regular as icon;

use crate::ui::theme;

use super::BrowserApp;

/// How long a confirmation ("Added 3 to Forest props") stays up.
const TOAST_TIME: Duration = Duration::from_millis(2600);

impl BrowserApp {
    pub(super) fn notify(&mut self, text: String) {
        self.toast = Some((text, Instant::now()));
    }

    /// The brief confirmation at the bottom of the window.
    pub(super) fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some((text, at)) = &self.toast else { return };
        let age = at.elapsed();
        if age > TOAST_TIME {
            self.toast = None;
            return;
        }
        let fade = 1.0 - ((age.as_secs_f32() - (TOAST_TIME.as_secs_f32() - 0.4)) / 0.4).clamp(0.0, 1.0);
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::new("toast"))
            .order(egui::Order::Foreground)
            .interactable(false)
            .pivot(egui::Align2::CENTER_BOTTOM)
            .fixed_pos(screen.center_bottom() + vec2(0.0, -48.0))
            .show(ctx, |ui| {
                ui.set_opacity(fade);
                Frame::new()
                    .fill(egui::Color32::from_rgb(40, 43, 51))
                    .stroke(egui::Stroke::new(1.0, theme::BORDER))
                    .corner_radius(10)
                    .inner_margin(Margin::symmetric(14, 8))
                    .shadow(ui.visuals().popup_shadow)
                    .show(ui, |ui| {
                        let text = RichText::new(format!("{}  {text}", icon::CHECK_CIRCLE)).color(theme::TEXT);
                        ui.add(egui::Label::new(text).extend());
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(50));
    }
}
