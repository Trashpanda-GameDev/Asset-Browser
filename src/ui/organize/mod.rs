//! The organizing parts of the window: Favorites, collections and categories in the sidebar, the
//! menu items and chips that put assets in them, and dragging assets onto them. None of it
//! changes anything itself: it reports `GroupCommand`s, which the app applies (`groups::Groups`).

use std::path::PathBuf;

use eframe::egui::{self, CornerRadius, FontId, Rect, vec2};
use egui_phosphor::regular as icon;

use crate::groups::{Group, GroupKind, Membership};

use super::format::plural;
use super::theme;

mod asset_groups;
mod sidebar;

pub use asset_groups::{chips, menu_items};
pub use sidebar::Sidebar;

/// Assets being dragged: their paths, so a rescan mid-drag can't change what's dropped.
pub struct DraggedAssets(pub Vec<PathBuf>);

/// Which assets a command is about.
pub enum Targets {
    /// The selection (a right-clicked asset is selected first).
    Selection,
    Paths(Vec<PathBuf>),
}

/// What the user asked for, wherever they asked.
pub enum GroupCommand {
    /// Browse the group.
    Show(i64),
    Add(i64, Targets),
    Remove(i64, Targets),
    /// Add the assets to Favorites, or take them out if they're all in already.
    ToggleFavorite(Targets),
    /// Start naming a new collection or category, which gets the targets once named.
    New(GroupKind, Targets),
    StartRename(i64),
    /// The name typed for a new or renamed group.
    FinishEdit(GroupEdit),
    Recolor(i64, u8),
    AskDelete(i64),
    /// Take out the members that aren't in their folder any more.
    RemoveMissing(i64),
}

/// A group being named in the sidebar.
pub struct GroupEdit {
    pub kind: GroupKind,
    /// `None` for a new group.
    pub id: Option<i64>,
    pub name: String,
    /// Assets that go into a new group once it's named.
    pub add: Vec<PathBuf>,
    focused: bool,
}

impl GroupEdit {
    pub fn new(kind: GroupKind, name: String, add: Vec<PathBuf>) -> Self {
        Self { kind, id: None, name, add, focused: false }
    }

    pub fn rename(group: &Group) -> Self {
        Self { kind: group.kind, id: Some(group.id), name: group.name.clone(), add: Vec::new(), focused: false }
    }
}

/// What starring does to assets that are all `favorite` already, or not: for menus and tooltips.
pub fn favorite_action(favorite: bool) -> &'static str {
    if favorite { "Remove from Favorites" } else { "Add to Favorites" }
}

/// Whether every one of `targets` (entry indices) is in the group.
pub fn all_in(membership: &Membership, id: i64, targets: &[u32]) -> bool {
    !targets.is_empty() && targets.iter().all(|&index| membership.groups_of(index).contains(&id))
}

fn any_in(membership: &Membership, id: i64, targets: &[u32]) -> bool {
    targets.iter().any(|&index| membership.groups_of(index).contains(&id))
}

/// The pill that follows the pointer while assets are dragged.
pub fn drag_indicator(ctx: &egui::Context) {
    let Some(payload) = egui::DragAndDrop::payload::<DraggedAssets>(ctx) else { return };
    let Some(pointer) = ctx.pointer_latest_pos() else { return };
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("dragged-assets")));
    let text = format!("{}  {}", icon::STACK, plural(payload.0.len(), "asset"));
    let galley = painter.layout_no_wrap(text, FontId::proportional(theme::size::LABEL), theme::TEXT);
    let rect = Rect::from_min_size(pointer + vec2(14.0, 10.0), galley.size() + vec2(20.0, 10.0));
    painter.rect_filled(rect, CornerRadius::same(13), theme::ACCENT.gamma_multiply(0.9));
    painter.galley(rect.min + vec2(10.0, 5.0), galley, theme::TEXT);
}
