//! The middle of the window with nothing to browse: the welcome before any folder is added, and
//! why a view is empty.

use eframe::egui::{self, RichText, Ui};
use egui_phosphor::regular as icon;

use crate::asset::AssetKind;
use crate::groups::{GroupKind, Groups, UNCATEGORIZED};

use super::{theme, widgets};

/// No folders yet: how to add one. Whether "Add a folder…" was clicked.
pub fn welcome(ui: &mut Ui) -> bool {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() / 2.0 - 80.0).max(0.0));
        ui.label(RichText::new(icon::FOLDERS).size(44.0).color(theme::TEXT_FAINT));
        ui.add_space(10.0);
        ui.label(RichText::new("Add a folder to browse its assets").size(16.0).color(theme::TEXT_WEAK));
        ui.add_space(4.0);
        ui.label(
            RichText::new("Or drop one on the window. Add as many as you like: they're browsed together.")
                .size(12.5)
                .color(theme::TEXT_FAINT),
        );
        ui.add_space(14.0);
        widgets::button(ui, icon::FOLDER_PLUS, "Add a folder…").clicked()
    })
    .inner
}

pub struct EmptyArgs<'a> {
    /// The one kind the mode browses (`None` for all of them).
    pub kind: Option<AssetKind>,
    /// The group browsed, if one is.
    pub group: Option<i64>,
    pub groups: &'a Groups,
    pub query: &'a str,
    /// The folder is empty only because its first scan hasn't finished.
    pub scanning: bool,
}

/// Why there's nothing here: the scan isn't done, the folder or group has none of the mode's
/// kind, or nothing matches the search.
pub fn nothing_here(ui: &mut Ui, args: &EmptyArgs<'_>) {
    let what = args.kind.map_or("assets", |kind| match kind {
        AssetKind::Image => "images",
        AssetKind::Audio => "sounds",
        AssetKind::Model => "models",
    });
    let query = args.query.trim();
    let text = if args.scanning {
        "Scanning…".to_owned()
    } else if query.is_empty() {
        format!("No {what} {}.", place(args.group, args.groups))
    } else {
        format!("No {what} here match \u{201c}{query}\u{201d}.")
    };
    ui.centered_and_justified(|ui| {
        ui.add(egui::Label::new(RichText::new(text).size(14.0).color(theme::TEXT_FAINT)).wrap());
    });
}

/// Where there's nothing, and for a group how to put something in it.
fn place(group: Option<i64>, groups: &Groups) -> String {
    match group {
        Some(UNCATEGORIZED) => "without a category".to_owned(),
        Some(id) => match groups.get(id) {
            Some(group) if group.kind == GroupKind::Favorites => {
                "in Favorites yet. Star assets with F or the star on their tile".to_owned()
            }
            Some(group) => format!("in \u{201c}{}\u{201d} yet. Drag assets onto it in the sidebar", group.name),
            None => "here".to_owned(),
        },
        None => "in this folder".to_owned(),
    }
}
