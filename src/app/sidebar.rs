//! The window's left side, favorites, collections and categories over the folder tree, and what's
//! done to folders: from the tree, the header's list, the + button and folders dropped on the window.

use std::path::PathBuf;

use eframe::egui::{self, Align, Frame, Layout, Margin, RichText};
use egui_phosphor::regular as icon;

use crate::ui::folders::FolderAction;
use crate::ui::organize::Sidebar;
use crate::ui::{theme, widgets};
use crate::{diag, platform};

use super::BrowserApp;

impl BrowserApp {
    /// The left side: Favorites, collections and categories, then the folders.
    pub(super) fn folder_panel(&mut self, ui: &mut egui::Ui) {
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin { left: 6, right: 6, top: 10, bottom: 6 });
        let mut action = None;
        let mut commands = Vec::new();
        egui::Panel::left("folders").resizable(true).default_size(250.0).min_size(160.0).frame(frame).show(ui, |ui| {
            // However many groups there are, the folders keep at least half the height.
            let max_height = (ui.available_height() * 0.5).max(120.0);
            egui::ScrollArea::vertical().id_salt("groups").max_height(max_height).auto_shrink([false, true]).show(
                ui,
                |ui| {
                    let sidebar = Sidebar {
                        groups: &self.organizing.groups,
                        membership: &self.organizing.membership,
                        kinds: self.browsing.mode.kinds(),
                        viewing: self.browsing.group,
                        edit: &mut self.organizing.edit,
                    };
                    commands = sidebar.show(ui);
                },
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                widgets::section_label(ui, "Folders");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(4.0);
                    let add = widgets::icon_button_sized(ui, icon::PLUS, "Add a folder to the library", false, 20.0);
                    if add.clicked() {
                        action = Some(FolderAction::AddRoot);
                    }
                });
            });
            ui.add_space(2.0);
            let Some(library) = &self.library else {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    let hint = "Folders of assets, browsed together. Press + or drop one on the window.";
                    ui.add(egui::Label::new(RichText::new(hint).size(11.5).color(theme::TEXT_FAINT)).wrap());
                });
                return;
            };
            // No folder is highlighted while a group is browsed.
            let selected = if self.browsing.group.is_some() { u32::MAX } else { self.browsing.selected_dir };
            if let Some(asked) = self.chrome.folder_tree.show(ui, library, self.browsing.mode.kinds(), selected) {
                action = Some(asked);
            }
        });
        if let Some(action) = action {
            self.run_folder_action(action);
        }
        for command in commands {
            self.run_group_command(command);
        }
    }

    pub(super) fn run_folder_action(&mut self, action: FolderAction) {
        let dir_path = |app: &Self, dir: u32| app.library.as_ref().and_then(|library| library.dir_path(dir));
        match action {
            FolderAction::Select(dir) => self.select_dir(dir),
            FolderAction::Open(dir) => {
                if let Some(path) = dir_path(self, dir)
                    && let Err(err) = platform::open(&path)
                {
                    diag::report("Opening the folder", err);
                }
            }
            FolderAction::CopyPath(dir) => {
                if let Some(path) = dir_path(self, dir) {
                    self.ctx.copy_text(platform::native(&path));
                    self.notify("Copied the folder's path".to_owned());
                }
            }
            FolderAction::AddRoot => {
                let picked = rfd::FileDialog::new().set_title("Add a folder of assets to the library").pick_folder();
                if let Some(path) = picked {
                    self.add_root(&path);
                }
            }
            FolderAction::RemoveRoot(path) => {
                if let Some(index) = self.folders.roots.iter().position(|root| *root == path) {
                    self.remove_root(index);
                }
            }
            FolderAction::Delete(dir) => self.ask_to_delete_folder(dir),
        }
    }

    /// Folders dropped on the window go into the library.
    pub(super) fn take_dropped_folders(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> =
            ctx.input(|input| input.raw.dropped_files.iter().map(|file| file.path().to_path_buf()).collect());
        if dropped.is_empty() {
            return;
        }
        let folders: Vec<PathBuf> = dropped.into_iter().filter(|path| path.is_dir()).collect();
        if folders.is_empty() {
            self.notify("Drop a folder to add it to the library".to_owned());
        }
        for folder in folders {
            self.add_root(&folder);
        }
    }
}

/// While something is dragged over the window: what dropping it does.
pub(super) fn draw_drop_hint(ctx: &egui::Context) {
    if ctx.input(|input| input.raw.hovered_files.is_empty()) {
        return;
    }
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop-hint")));
    painter.rect_filled(screen, 0.0, theme::BG.gamma_multiply(0.8));
    let area = screen.shrink(24.0);
    painter.rect_stroke(area, 14.0, egui::Stroke::new(2.0, theme::ACCENT), egui::StrokeKind::Inside);
    painter.text(
        area.center(),
        egui::Align2::CENTER_CENTER,
        format!("{}  Drop a folder to add it to the library", icon::FOLDER_PLUS),
        egui::FontId::new(18.0, theme::semibold()),
        theme::TEXT,
    );
}
