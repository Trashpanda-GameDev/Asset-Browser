//! "What's new": the changelog's notes, shown once after an update for the versions since the
//! last one seen, or for every version from the settings.

use eframe::egui::{self, Align, Color32, FontId, Layout, RichText, Sense, Ui, UiBuilder, text::LayoutJob, vec2};
use semver::Version;

use crate::changelog::{self, Block, Release};

use super::{theme, widgets};

pub struct WhatsNew {
    /// The version running.
    current: Version,
    /// Notes after this version; `None` for every version.
    since: Option<Version>,
}

impl WhatsNew {
    /// After an update: the versions since `seen`.
    pub fn since(seen: Version, current: Version) -> Self {
        Self { current, since: Some(seen) }
    }

    /// Every version's notes.
    pub fn every_version(current: Version) -> Self {
        Self { current, since: None }
    }

    /// Draws it; false once it's closed.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        let mut open = true;
        let modal = widgets::modal_window("whats-new", 20);
        let response = modal.show(ctx, |ui| {
            // A fixed width, so the notes wrap to it, laid out top to bottom.
            let (rect, _) = ui.allocate_exact_size(vec2(540.0, 500.0), Sense::hover());
            let mut ui = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)));
            let title = if self.since.is_some() { "What's new" } else { "Every version" };
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(title)
                        .font(FontId::new(theme::size::WINDOW_TITLE, theme::semibold()))
                        .color(theme::TEXT),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, egui_phosphor::regular::X, "Close (Esc)", false).clicked() {
                        open = false;
                    }
                });
            });
            ui.label(RichText::new(format!("Asset Browser {}", self.current)).color(theme::TEXT_WEAK));
            ui.add_space(8.0);
            let releases: Vec<&Release> = match &self.since {
                Some(seen) => changelog::since(changelog::releases(), seen, &self.current),
                None => changelog::releases().iter().collect(),
            };
            // Leaves room for the buttons at the bottom.
            let height = ui.available_height() - 40.0;
            egui::ScrollArea::vertical().max_height(height).auto_shrink([false, false]).show(&mut ui, |ui| {
                for release in releases {
                    self.release(ui, release);
                    ui.add_space(12.0);
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if self.since.is_some() && widgets::button(ui, "", "Every version").clicked() {
                    self.since = None;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let close = egui::Button::new(RichText::new("Close").color(Color32::WHITE))
                        .fill(theme::ACCENT)
                        .min_size(vec2(88.0, 28.0));
                    if ui.add(close).clicked() {
                        open = false;
                    }
                });
            });
        });
        open && !response.should_close()
    }

    fn release(&self, ui: &mut Ui, release: &Release) {
        let name = match &release.version {
            Some(version) => version.to_string(),
            None => "Coming next".to_owned(),
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).font(FontId::new(theme::size::TITLE, theme::semibold())).color(theme::TEXT));
            if let Some(date) = &release.date {
                ui.label(RichText::new(date).color(theme::TEXT_FAINT));
            }
            if release.version.as_ref() == Some(&self.current) {
                widgets::badge_widget(ui, "THIS VERSION", theme::ACCENT);
            }
        });
        ui.add_space(2.0);
        for block in &release.blocks {
            match block {
                Block::Heading(text) => {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(text)
                            .font(FontId::new(theme::size::BODY, theme::semibold()))
                            .color(theme::TEXT_WEAK),
                    );
                }
                Block::Item(text) => {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.label(RichText::new("•").color(theme::ACCENT));
                        rich_text(ui, text);
                    });
                }
                Block::Text(text) => rich_text(ui, text),
            }
        }
    }
}

/// A line of notes, with its `**bold**` parts in the title font.
fn rich_text(ui: &mut Ui, text: &str) {
    let mut job = LayoutJob::default();
    for (part, bold) in changelog::runs(text) {
        let font = if bold {
            FontId::new(theme::size::EMPHASIS, theme::semibold())
        } else {
            FontId::proportional(theme::size::EMPHASIS)
        };
        let color = if bold { theme::TEXT } else { theme::TEXT_WEAK };
        job.append(&part, 0.0, egui::TextFormat { font_id: font, color, ..Default::default() });
    }
    // Wrapped to the window: in a row (after a bullet) a label would otherwise run on, and widen
    // everything after it.
    ui.add(egui::Label::new(job).wrap());
}
