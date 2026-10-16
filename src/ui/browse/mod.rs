//! The browsed assets, as a grid of tiles or a list with columns. Both only draw the rows on
//! screen, and ask for the previews of those rows (plus a few around them) every frame.

use std::ops::Range;

use eframe::egui::{self, Align2, FontId, Modifiers, Pos2, Rect, Sense, Ui, vec2};
use egui_phosphor::regular as icon;

use crate::audio::NowPlaying;
use crate::groups::{GroupKind, Groups, Membership};
use crate::library::{Library, Order, SortBy};

use super::asset_actions::{self, AssetAction};
use super::display::Backdrop;
use super::keymap::{self, Action};
use super::mode::{Mode, Selection};
use super::organize::{self, DraggedAssets, GroupCommand, Targets};
use super::previews::Previews;
use super::theme;

mod grid;
mod list;
mod preview;

pub use grid::grid;
pub use list::list;
pub use preview::{PreviewOf, draw_preview, fit_rect};

/// Rows beyond the visible ones whose previews are made ahead of scrolling.
const PREFETCH_ROWS_BELOW: usize = 3;

const PREFETCH_ROWS_ABOVE: usize = 1;

pub struct Browse<'a> {
    pub library: &'a Library,
    pub view: &'a [u32],
    pub selection: &'a Selection,
    pub now: Option<&'a NowPlaying>,
    pub mode: Mode,
    pub order: Order,
    /// Behind transparent pictures.
    pub backdrop: Backdrop,
    /// Bring this position in `view` into sight (after moving with the keyboard).
    pub scroll_to: Option<usize>,
    /// The view's scroll offset and height last frame.
    pub scroll: (f32, f32),
    pub groups: &'a Groups,
    pub membership: &'a Membership,
    /// The group being browsed, if it's not a folder.
    pub viewing: Option<i64>,
}

impl Browse<'_> {
    /// What a click, drag or menu on `index` acts on: the whole selection if it's part of it.
    fn targets(&self, index: u32) -> Vec<u32> {
        if self.selection.contains(index) {
            self.view.iter().copied().filter(|&i| self.selection.contains(i)).collect()
        } else {
            vec![index]
        }
    }

    /// Lets the asset be dragged onto a collection or category (with the rest of the selection).
    fn drag_source(&self, index: u32, response: &egui::Response) {
        if response.drag_started() {
            let paths = self.targets(index).iter().map(|&i| self.library.path_of(self.library.entry(i))).collect();
            response.dnd_set_drag_payload(DraggedAssets(paths));
        }
    }

    /// The categories the entry is in, for its dots and chips.
    fn categories(&self, index: u32) -> impl Iterator<Item = &crate::groups::Group> {
        self.membership
            .groups_of(index)
            .iter()
            .filter_map(|&id| self.groups.get(id))
            .filter(|group| group.kind == GroupKind::Category)
    }
}

/// How previews are drawn: the screen's pixel density and what goes behind transparent pictures.
#[derive(Clone, Copy)]
pub struct Look {
    pub pixels_per_point: f32,
    pub backdrop: Backdrop,
}

impl Look {
    pub fn new(ui: &Ui, backdrop: Backdrop) -> Self {
        Self { pixels_per_point: ui.ctx().pixels_per_point(), backdrop }
    }
}

/// What happened in the view this frame.
#[derive(Default)]
pub struct BrowseOutput {
    pub clicked: Option<(u32, Modifiers)>,
    pub double_clicked: Option<u32>,
    /// Right-clicked: selected first if it isn't already, so the menu acts on it.
    pub secondary_clicked: Option<u32>,
    pub action: Option<AssetAction>,
    pub sort_clicked: Option<SortBy>,
    /// Favorites, collections and categories asked for.
    pub commands: Vec<GroupCommand>,
    /// Assets per row and rows per screen, for the arrow and page keys.
    pub columns: usize,
    pub page_rows: usize,
    /// The view's scroll offset and height.
    pub scroll: (f32, f32),
}

/// The scroll offset that brings `top..top + height` into a viewport at `(offset, height)`, if
/// it's out of sight.
fn scroll_into_view(top: f32, height: f32, (offset, viewport): (f32, f32)) -> Option<f32> {
    if top < offset {
        Some(top)
    } else if top + height > offset + viewport {
        Some(top + height - viewport)
    } else {
        None
    }
}

/// Asks for the visible rows' previews, then the rows just around them.
fn want_rows(browse: &Browse<'_>, previews: &mut Previews, visible: Range<usize>, columns: usize, rows: usize) {
    let below = visible.end..(visible.end + PREFETCH_ROWS_BELOW).min(rows);
    let above = visible.start.saturating_sub(PREFETCH_ROWS_ABOVE)..visible.start;
    for row in visible.chain(below).chain(above.rev()) {
        for column in 0..columns {
            if let Some(&index) = browse.view.get(row * columns + column) {
                previews.want(index);
            }
        }
    }
}

/// Clicks, double-clicks, the context menu and the tooltip, shared by tiles and rows.
fn item_response(browse: &Browse<'_>, index: u32, response: &egui::Response, output: &mut BrowseOutput) {
    if response.clicked() {
        output.clicked = Some((index, response.ctx.input(|input| input.modifiers)));
    }
    if response.double_clicked() {
        output.double_clicked = Some(index);
    }
    if response.secondary_clicked() {
        output.secondary_clicked = Some(index);
    }
    let count = if browse.selection.contains(index) { browse.selection.len() } else { 1 };
    response.context_menu(|ui| {
        ui.set_min_width(190.0);
        output.action = output.action.or(asset_actions::menu_items(ui, count));
        ui.separator();
        let targets = browse.targets(index);
        if let Some(command) = organize::menu_items(ui, browse.groups, browse.membership, &targets, browse.viewing) {
            output.commands.push(command);
        }
        ui.separator();
        output.action = output.action.or(asset_actions::delete_menu_item(ui, count));
    });
}

/// The favourite star in a tile's or row's corner: filled when it's a favourite, an outline to
/// click while the pointer is over the asset. Clicking it stars that one asset.
fn star_button(ui: &Ui, browse: &Browse<'_>, index: u32, center: Pos2, show_outline: bool) -> Option<GroupCommand> {
    let favorite = browse.membership.is_favorite(index);
    let rect = Rect::from_center_size(center, vec2(22.0, 22.0));
    let response = ui.interact(rect, egui::Id::new(("star", index)), Sense::click());
    if !favorite && !show_outline && !response.hovered() {
        return None;
    }
    let painter = ui.painter();
    let (family, color) = if favorite {
        (theme::filled_icons(), theme::FAVORITE)
    } else if response.hovered() {
        (egui::FontFamily::Proportional, theme::FAVORITE)
    } else {
        (egui::FontFamily::Proportional, theme::TEXT.gamma_multiply(0.7))
    };
    // A dark halo keeps the star readable over light pictures.
    painter.circle_filled(rect.center(), 10.0, if favorite { theme::shade::FAVORITE } else { theme::shade::STAR });
    painter.text(rect.center(), Align2::CENTER_CENTER, icon::STAR, FontId::new(theme::size::MEDIUM, family), color);
    let tip = keymap::tooltip(&response.ctx, organize::favorite_action(favorite), Action::Favorite);
    let clicked = response.on_hover_text(tip).clicked();
    let path = browse.library.path_of(browse.library.entry(index));
    clicked.then(|| GroupCommand::ToggleFavorite(Targets::Paths(vec![path])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolls_only_when_out_of_sight() {
        assert_eq!(scroll_into_view(100.0, 30.0, (0.0, 500.0)), None);
        assert_eq!(scroll_into_view(10.0, 30.0, (50.0, 500.0)), Some(10.0));
        assert_eq!(scroll_into_view(600.0, 30.0, (0.0, 500.0)), Some(130.0));
    }
}
