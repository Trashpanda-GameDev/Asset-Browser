//! What can be done to the selected assets, from the context menu, the preview pane's buttons and
//! the keys: open, show in folder, copy, delete.

use eframe::egui::Ui;
use egui_phosphor::regular as icon;

use super::keymap::{self, Action};
use super::widgets;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetAction {
    Open,
    Reveal,
    /// The files themselves on the clipboard, to paste into a folder or another app.
    CopyFiles,
    CopyPaths,
    /// Ask "Delete?" for the selection (to the Recycle Bin).
    Delete,
}

/// The context menu's first items, for `count` assets: open, show, copy. What was clicked.
pub fn menu_items(ui: &mut Ui, count: usize) -> Option<AssetAction> {
    let mut clicked = None;
    if ui.button(format!("{}  Open", icon::ARROW_SQUARE_OUT)).clicked() {
        clicked = Some(AssetAction::Open);
    }
    if ui.button(format!("{}  Show in folder", icon::FOLDER_OPEN)).clicked() {
        clicked = Some(AssetAction::Reveal);
    }
    if ui.button(format!("{}  {}", icon::FILES, files_label(count))).on_hover_text(copy_files_tip(ui)).clicked() {
        clicked = Some(AssetAction::CopyFiles);
    }
    if ui.button(format!("{}  {}", icon::COPY, paths_label(count))).clicked() {
        clicked = Some(AssetAction::CopyPaths);
    }
    if clicked.is_some() {
        ui.close();
    }
    clicked
}

/// The context menu's last item: delete `count` assets.
pub fn delete_menu_item(ui: &mut Ui, count: usize) -> Option<AssetAction> {
    let label =
        if count > 1 { format!("{}  Delete {count}…", icon::TRASH) } else { format!("{}  Delete…", icon::TRASH) };
    let tip = keymap::tooltip(ui.ctx(), &format!("To the {}", crate::platform::trash_name()), Action::Delete);
    let clicked = ui.button(label).on_hover_text(tip).clicked();
    if clicked {
        ui.close();
    }
    clicked.then_some(AssetAction::Delete)
}

/// The preview pane's buttons, for `count` assets: open, show, copy. What was clicked.
pub fn buttons(ui: &mut Ui, count: usize) -> Option<AssetAction> {
    let mut clicked = None;
    let open_tip = keymap::tooltip(ui.ctx(), "Open in its default app", Action::Open);
    if widgets::button(ui, icon::ARROW_SQUARE_OUT, "Open").on_hover_text(open_tip).clicked() {
        clicked = Some(AssetAction::Open);
    }
    if widgets::button(ui, icon::FOLDER_OPEN, "Show in folder").clicked() {
        clicked = Some(AssetAction::Reveal);
    }
    let tip = copy_files_tip(ui);
    if widgets::button(ui, icon::FILES, &files_label(count)).on_hover_text(tip).clicked() {
        clicked = Some(AssetAction::CopyFiles);
    }
    if widgets::button(ui, icon::COPY, &paths_label(count)).clicked() {
        clicked = Some(AssetAction::CopyPaths);
    }
    clicked
}

fn files_label(count: usize) -> String {
    if count > 1 { format!("Copy {count} files") } else { "Copy file".to_owned() }
}

fn paths_label(count: usize) -> String {
    if count > 1 { format!("Copy {count} paths") } else { "Copy path".to_owned() }
}

fn copy_files_tip(ui: &Ui) -> String {
    keymap::tooltip(ui.ctx(), "To paste into a folder or another app", Action::CopyFiles)
}
