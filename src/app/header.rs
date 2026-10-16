//! The window's top: the library's folders, the modes with their counts, and the view's buttons
//! and search.

use eframe::egui::{self, Align, Frame, Layout, Margin, RichText, vec2};
use egui_phosphor::regular as icon;

use crate::library::folder_name;
use crate::platform;
use crate::ui::folders::FolderAction;
use crate::ui::format::grouped;
use crate::ui::keymap::{self, Action};
use crate::ui::mode::{Layout as ViewLayout, Mode};
use crate::ui::{theme, widgets};

use super::BrowserApp;

impl BrowserApp {
    #[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
    pub(super) fn header(&mut self, ui: &mut egui::Ui) {
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(12, 8));
        let mut action = None;
        egui::Panel::top("header").frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                // The library's folders: click for the list, to go to one, add one or take one out.
                let (glyph, name) = match self.folders.roots.as_slice() {
                    [] => (icon::FOLDER_PLUS, "Add a folder".to_owned()),
                    [root] => (icon::FOLDER_SIMPLE, folder_name(root)),
                    roots => (icon::FOLDERS, format!("{} folders", roots.len())),
                };
                let label = format!("{glyph}  {name}  {}", icon::CARET_DOWN);
                let button = egui::Button::new(RichText::new(label).font(egui::FontId::new(14.0, theme::semibold())))
                    .frame_when_inactive(false)
                    .min_size(vec2(0.0, 30.0));
                let response = ui.add(button);
                match &self.library {
                    Some(library) => {
                        let paths: Vec<String> = self.folders.roots.iter().map(|root| platform::native(root)).collect();
                        let response = response.on_hover_text(format!(
                            "{}\nClick to go to a folder, add one or take one out",
                            paths.join("\n")
                        ));
                        let kinds = self.browsing.mode.kinds();
                        egui::Popup::menu(&response).show(|ui| {
                            action = crate::ui::folders::library_menu(ui, library, kinds);
                        });
                    }
                    None => {
                        if response.on_hover_text("Choose a folder of assets, or drop one on the window").clicked() {
                            action = Some(FolderAction::AddRoot);
                        }
                    }
                }
                ui.add_space(8.0);

                let totals = self.library.as_ref().map(|_| self.source_totals());
                // Narrow windows lose the counts first, then the labels.
                let room = ui.available_width();
                let (show_counts, show_labels) = (room > 960.0, room > 780.0);
                let notes: Vec<Option<String>> = Mode::ALL
                    .iter()
                    .map(|mode| {
                        totals.filter(|_| show_counts).map(|totals| grouped(mode.kinds().count(totals) as usize))
                    })
                    .collect();
                let tips: Vec<String> = Mode::ALL
                    .iter()
                    .map(|mode| {
                        let (tip, action) = match mode {
                            Mode::All => ("Everything", Action::ModeAll),
                            Mode::Images => ("Pictures and textures", Action::ModeImages),
                            Mode::Sounds => ("Sounds, with a player", Action::ModeSounds),
                            Mode::Models => ("3D models, with a 3D view", Action::ModeModels),
                        };
                        format!("{tip}{}", keymap::hint(ui.ctx(), action))
                    })
                    .collect();
                let segments: Vec<widgets::Segment<'_>> = Mode::ALL
                    .iter()
                    .zip(notes)
                    .zip(&tips)
                    .map(|((mode, note), tip)| widgets::Segment {
                        glyph: mode.glyph(),
                        label: if show_labels { mode.label() } else { "" },
                        note,
                        tooltip: tip,
                    })
                    .collect();
                if let Some(index) =
                    widgets::segmented(ui, ui.id().with("modes"), &segments, self.browsing.mode.index())
                {
                    self.set_mode(Mode::ALL[index]);
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let settings_tip = format!("Settings{}", keymap::hint(ui.ctx(), Action::Settings));
                    if widgets::icon_button(ui, icon::GEAR_SIX, &settings_tip, self.preferences.settings.open).clicked()
                    {
                        self.preferences.settings.open = true;
                    }
                    widgets::divider(ui);
                    let inspector_tip = format!(
                        "{} the preview pane{}",
                        if self.chrome.show_inspector { "Hide" } else { "Show" },
                        keymap::hint(ui.ctx(), Action::TogglePane)
                    );
                    if widgets::icon_button(ui, icon::SIDEBAR_SIMPLE, &inspector_tip, self.chrome.show_inspector)
                        .clicked()
                    {
                        self.chrome.show_inspector = !self.chrome.show_inspector;
                    }
                    widgets::divider(ui);
                    let layout = self.mode_view().layout;
                    let list_tip = format!("List{}", keymap::hint(ui.ctx(), Action::List));
                    if widgets::icon_button(ui, icon::LIST, &list_tip, layout == ViewLayout::List).clicked() {
                        self.mode_view_mut().layout = ViewLayout::List;
                        self.browsing.scroll_to_focus = true;
                    }
                    let grid_tip = format!("Grid{}", keymap::hint(ui.ctx(), Action::Grid));
                    if widgets::icon_button(ui, icon::SQUARES_FOUR, &grid_tip, layout == ViewLayout::Grid).clicked() {
                        self.mode_view_mut().layout = ViewLayout::Grid;
                        self.browsing.scroll_to_focus = true;
                    }
                    widgets::divider(ui);
                    let width = (ui.available_width() - 8.0).clamp(140.0, 340.0);
                    if widgets::search_field(
                        ui,
                        &mut self.browsing.query,
                        width,
                        std::mem::take(&mut self.chrome.focus_search),
                    ) {
                        self.browsing.view_dirty = true;
                    }
                });
            });
        });
        if let Some(action) = action {
            self.run_folder_action(action);
        }
    }
}
