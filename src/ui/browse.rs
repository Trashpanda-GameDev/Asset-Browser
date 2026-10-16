//! The browsed assets, as a grid of tiles or a list with columns. Both only draw the rows on
//! screen, and ask for the previews of those rows (plus a few around them) every frame.

use std::ops::Range;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Modifiers, Pos2, Rect, Sense, Stroke, StrokeKind, TextureId, Ui, pos2,
    vec2,
};
use egui_phosphor::regular as icon;

use crate::asset::AssetKind;
use crate::audio::NowPlaying;
use crate::groups::{GroupKind, Groups, Membership};
use crate::library::{Entry, Library, SortBy};

use super::asset_actions::{self, AssetAction};
use super::display::Backdrop;
use super::format::{channels, dimensions, format_age, format_size, format_time, grouped, sample_rate};
use super::keymap::{self, Action};
use super::mode::{Mode, Selection};
use super::organize::{self, DraggedAssets, GroupCommand, Targets};
use super::previews::{CachedTexture, Previews};
use super::sound::draw_waveform;
use super::{theme, widgets};

const LABEL_HEIGHT: f32 = 22.0;
const GAP: f32 = 10.0;
const ROW_HEIGHT: f32 = 34.0;
const HEADER_HEIGHT: f32 = 28.0;
/// Rows beyond the visible ones whose previews are made ahead of scrolling.
const PREFETCH_ROWS_BELOW: usize = 3;
const PREFETCH_ROWS_ABOVE: usize = 1;

pub struct Browse<'a> {
    pub library: &'a Library,
    pub view: &'a [u32],
    pub selection: &'a Selection,
    pub now: Option<&'a NowPlaying>,
    pub mode: Mode,
    pub sort: SortBy,
    pub descending: bool,
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

/// The tile grid.
pub fn grid(ui: &mut Ui, browse: &Browse<'_>, previews: &mut Previews, tile: f32) -> BrowseOutput {
    let mut output = BrowseOutput::default();
    let width = ui.available_width();
    let columns = (((width - GAP) / (tile + GAP)).floor() as usize).max(1);
    // The spare width is shared out between the gaps, so the grid fills the row evenly. Each row
    // has its gap above it.
    let cell = vec2((width - GAP) / columns as f32, GAP + tile + LABEL_HEIGHT);
    let rows = browse.view.len().div_ceil(columns);
    output.columns = columns;

    let mut area = egui::ScrollArea::vertical().id_salt(("grid", browse.mode.key())).auto_shrink([false; 2]);
    if let Some(position) = browse.scroll_to {
        let top = (position / columns) as f32 * cell.y;
        if let Some(offset) = scroll_into_view(top, cell.y, browse.scroll) {
            area = area.vertical_scroll_offset(offset);
        }
    }
    let look = Look::new(ui, browse.backdrop);
    let mut visible = 0..0;
    // `show_rows` adds the item spacing to every row: set it to nothing before, not inside.
    ui.spacing_mut().item_spacing.y = 0.0;
    let result = area.show_rows(ui, cell.y, rows, |ui, row_range| {
        visible = row_range.clone();
        for row in row_range {
            let (row_rect, _) = ui.allocate_exact_size(vec2(width, cell.y), Sense::hover());
            for column in 0..columns {
                let Some(&index) = browse.view.get(row * columns + column) else { break };
                let entry = browse.library.entry(index);
                let x = row_rect.left() + GAP + column as f32 * cell.x + (cell.x - GAP - tile) / 2.0;
                let rect = Rect::from_min_size(pos2(x, row_rect.top() + GAP), vec2(tile, tile + LABEL_HEIGHT));
                let tile_ui = Tile { browse, entry, index, previews, look };
                tile_ui.show(ui, rect, &mut output);
            }
        }
    });
    output.scroll = (result.state.offset.y, result.inner_rect.height());
    output.page_rows = ((result.inner_rect.height() / cell.y).floor() as usize).max(1);
    want_rows(browse, previews, visible, columns, rows);
    output
}

/// The list, with a header to sort by.
pub fn list(ui: &mut Ui, browse: &Browse<'_>, previews: &mut Previews) -> BrowseOutput {
    let mut output = BrowseOutput { columns: 1, ..Default::default() };
    let width = ui.available_width();
    let layout = column_layout(browse.mode, width);

    // Header: stays put while the rows scroll.
    let (header, _) = ui.allocate_exact_size(vec2(width, HEADER_HEIGHT), Sense::hover());
    ui.painter().hline(header.x_range(), header.bottom() - 0.5, Stroke::new(1.0, theme::BORDER));
    for (column, span) in &layout {
        let rect = Rect::from_x_y_ranges((header.left() + span.start)..=(header.left() + span.end), header.y_range());
        let sort = column.sort();
        let response = ui.interact(
            rect,
            ui.id().with(("header", column.label())),
            if sort.is_some() { Sense::click() } else { Sense::hover() },
        );
        let sorted = sort.is_some_and(|sort| sort == browse.sort);
        let color = if sorted || (sort.is_some() && response.hovered()) { theme::TEXT_WEAK } else { theme::TEXT_FAINT };
        let mut label = column.label().to_uppercase();
        if sorted {
            label.push(' ');
            label.push_str(if browse.descending { icon::CARET_DOWN } else { icon::CARET_UP });
        }
        let (anchor, align) = text_anchor(*column, &rect);
        ui.painter().text(anchor, align, label, FontId::new(theme::size::TINY, theme::semibold()), color);
        if response.clicked() {
            output.sort_clicked = sort;
        }
    }

    let mut area = egui::ScrollArea::vertical().id_salt(("list", browse.mode.key())).auto_shrink([false; 2]);
    if let Some(position) = browse.scroll_to
        && let Some(offset) = scroll_into_view(position as f32 * ROW_HEIGHT, ROW_HEIGHT, browse.scroll)
    {
        area = area.vertical_scroll_offset(offset);
    }
    let look = Look::new(ui, browse.backdrop);
    let mut visible = 0..0;
    // `show_rows` adds the item spacing to every row: set it to nothing before, not inside.
    ui.spacing_mut().item_spacing.y = 0.0;
    let result = area.show_rows(ui, ROW_HEIGHT, browse.view.len(), |ui, row_range| {
        visible = row_range.clone();
        for position in row_range {
            let index = browse.view[position];
            let entry = browse.library.entry(index);
            let (rect, response) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::click_and_drag());
            let row = ListRow { browse, entry, index, previews, look };
            if let Some(command) = row.show(ui, rect, &response, &layout) {
                output.commands.push(command);
            }
            browse.drag_source(index, &response);
            item_response(browse, index, &response, &mut output);
        }
    });
    output.scroll = (result.state.offset.y, result.inner_rect.height());
    output.page_rows = ((result.inner_rect.height() / ROW_HEIGHT).floor() as usize).max(1);
    want_rows(browse, previews, visible, 1, browse.view.len());
    output
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
    painter.circle_filled(rect.center(), 10.0, Color32::from_black_alpha(if favorite { 110 } else { 80 }));
    painter.text(rect.center(), Align2::CENTER_CENTER, icon::STAR, FontId::new(theme::size::MEDIUM, family), color);
    let tip = keymap::tooltip(&response.ctx, organize::favorite_action(favorite), Action::Favorite);
    let clicked = response.on_hover_text(tip).clicked();
    let path = browse.library.path_of(browse.library.entry(index));
    clicked.then(|| GroupCommand::ToggleFavorite(Targets::Paths(vec![path])))
}

struct Tile<'a, 'b> {
    browse: &'a Browse<'b>,
    entry: &'a Entry,
    index: u32,
    previews: &'a Previews,
    look: Look,
}

impl Tile<'_, '_> {
    fn show(self, ui: &mut Ui, rect: Rect, output: &mut BrowseOutput) {
        let entry = self.entry;
        let response = ui.interact(rect, egui::Id::new(("tile", entry.key)), Sense::click_and_drag());
        let selected = self.browse.selection.contains(self.index);
        let focused = self.browse.selection.focus == Some(self.index);
        let painter = ui.painter();
        let card = Rect::from_min_size(rect.min, vec2(rect.width(), rect.width()));
        let fill = if response.hovered() { theme::CARD_HOVER } else { theme::CARD };
        painter.rect_filled(card, theme::CARD_RADIUS, fill);
        let inner = card.shrink(6.0);
        let playing = self.browse.now.filter(|now| now.key == entry.key && !now.finished);
        draw_preview(painter, inner, entry, self.previews, playing, self.look, true);

        if selected {
            painter.rect_filled(card, theme::CARD_RADIUS, theme::ACCENT_SOFT.gamma_multiply(0.6));
            let width = if focused { 2.0 } else { 1.5 };
            painter.rect_stroke(card, theme::CARD_RADIUS, Stroke::new(width, theme::ACCENT), StrokeKind::Inside);
        }
        if response.hovered() || selected {
            widgets::badge(
                painter,
                card.left_top() + vec2(6.0, 6.0),
                Align2::LEFT_TOP,
                &entry.extension().to_uppercase(),
                theme::kind_color(entry.kind),
            );
        }

        let color = if selected { theme::TEXT } else { theme::TEXT.gamma_multiply(0.86) };
        let galley = widgets::elided(
            painter,
            entry.name().to_owned(),
            FontId::proportional(theme::size::LABEL),
            color,
            rect.width(),
        );
        let label_pos = pos2(rect.center().x - galley.size().x / 2.0, card.bottom() + 5.0);
        painter.galley(label_pos, galley, color);

        // Category dots along the bottom of the picture.
        let mut categories = Vec::new();
        for (n, group) in self.browse.categories(self.index).enumerate() {
            if n < 5 {
                let center = pos2(card.left() + 11.0 + n as f32 * 11.0, card.bottom() - 11.0);
                painter.circle_filled(center, 5.0, Color32::from_black_alpha(120));
                painter.circle_filled(center, 3.5, theme::category_color(group.color));
            }
            categories.push(group.name.as_str());
        }

        let pointer_over = response.contains_pointer();
        let star_center = card.right_top() + vec2(-15.0, 15.0);
        if let Some(command) = star_button(ui, self.browse, self.index, star_center, pointer_over) {
            output.commands.push(command);
        }
        self.browse.drag_source(self.index, &response);
        item_response(self.browse, self.index, &response, output);
        if categories.is_empty() {
            response.on_hover_text_at_pointer(&entry.rel);
        } else {
            response.on_hover_text_at_pointer(format!("{}\n{}", entry.rel, categories.join(" · ")));
        }
    }
}

/// Draws an asset's preview into `rect`: its thumbnail, waveform, or a placeholder while it's made.
#[expect(clippy::too_many_arguments, reason = "refactor: take fewer, grouped arguments")]
pub fn draw_preview(
    painter: &egui::Painter,
    rect: Rect,
    entry: &Entry,
    previews: &Previews,
    playing: Option<&NowPlaying>,
    look: Look,
    large: bool,
) {
    if entry.kind == AssetKind::Model {
        // A soft studio backdrop, lighter at the top.
        widgets::vertical_gradient(painter, rect, Color32::from_rgb(44, 47, 56), Color32::from_rgb(29, 31, 37));
    }
    if let Some(texture) = previews.texture(entry.key) {
        let backdrop = (entry.kind == AssetKind::Image).then_some(look.backdrop);
        draw_texture(painter, rect, texture, previews.checker.id(), backdrop, look.pixels_per_point);
    } else if let Some(waveform) = previews.waveforms.get(&entry.key) {
        let color = theme::kind_color(entry.kind);
        let progress = playing.map(|now| now.progress(waveform.duration as f64));
        let wave_rect = if large { Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y - 16.0)) } else { rect };
        draw_waveform(painter, wave_rect, waveform, color.gamma_multiply(0.5), color, progress);
        if large {
            painter.text(
                rect.right_bottom(),
                Align2::RIGHT_BOTTOM,
                format_time(waveform.duration as f64),
                FontId::proportional(theme::size::FINE),
                theme::TEXT_WEAK,
            );
            if playing.is_some() {
                painter.text(
                    rect.left_bottom(),
                    Align2::LEFT_BOTTOM,
                    icon::SPEAKER_HIGH,
                    FontId::proportional(theme::size::BODY),
                    color,
                );
            }
        }
    } else if previews.failed.contains_key(&entry.key) {
        let size = (rect.height() * 0.28).clamp(12.0, 34.0);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            icon::WARNING_CIRCLE,
            FontId::proportional(size),
            theme::ERROR.gamma_multiply(0.8),
        );
    } else {
        let size = (rect.height() * 0.3).clamp(12.0, 40.0);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            theme::kind_icon(entry.kind),
            FontId::proportional(size),
            theme::kind_color(entry.kind).gamma_multiply(0.35),
        );
    }
}

#[expect(clippy::too_many_arguments, reason = "refactor: take fewer, grouped arguments")]
fn draw_texture(
    painter: &egui::Painter,
    area: Rect,
    texture: &CachedTexture,
    checker: TextureId,
    backdrop: Option<Backdrop>,
    pixels_per_point: f32,
) {
    let [w, h] = texture.handle.size();
    let fitted = fit_rect(area, w as f32, h as f32, texture.unscaled, pixels_per_point);
    match backdrop {
        Some(Backdrop::Checker) => {
            let checker_uv = Rect::from_min_max(Pos2::ZERO, pos2(fitted.width() / 12.0, fitted.height() / 12.0));
            painter.image(checker, fitted, checker_uv, Color32::WHITE);
        }
        Some(Backdrop::Light) => {
            painter.rect_filled(fitted, 0.0, Backdrop::LIGHT);
        }
        Some(Backdrop::Dark) | None => {}
    }
    painter.image(texture.handle.id(), fitted, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
}

/// Fits `w` × `h` inside `area`, keeping its shape. Unscaled thumbnails (pixel art) are scaled by
/// whole physical pixels and snapped to the pixel grid, so every pixel stays the same size.
pub fn fit_rect(area: Rect, w: f32, h: f32, pixel_exact: bool, pixels_per_point: f32) -> Rect {
    let mut scale = (area.width() / w).min(area.height() / h);
    let physical = scale * pixels_per_point;
    if pixel_exact && physical >= 1.0 {
        scale = physical.floor() / pixels_per_point;
    }
    let size = vec2(w * scale, h * scale);
    let rect = Rect::from_center_size(area.center(), size);
    if pixel_exact {
        let min = (rect.min.to_vec2() * pixels_per_point).round() / pixels_per_point;
        Rect::from_min_size(min.to_pos2(), size)
    } else {
        rect
    }
}

/// List columns. Which ones a mode shows is its business: sounds get a waveform, models their
/// triangle count, and so on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    /// The favourite star.
    Star,
    Name,
    Type,
    Details,
    Dimensions,
    Waveform,
    Length,
    Format,
    Triangles,
    Categories,
    Folder,
    Size,
    Modified,
}

enum Width {
    Fixed(f32),
    Flex(f32),
}

impl Column {
    fn label(self) -> &'static str {
        match self {
            Self::Star => "",
            Self::Categories => "Categories",
            Self::Name => "Name",
            Self::Type => "Type",
            Self::Details => "Details",
            Self::Dimensions => "Dimensions",
            Self::Waveform => "Waveform",
            Self::Length => "Length",
            Self::Format => "Format",
            Self::Triangles => "Triangles",
            Self::Folder => "Folder",
            Self::Size => "Size",
            Self::Modified => "Modified",
        }
    }

    #[expect(clippy::match_same_arms, reason = "each column's width is its own; equal widths are a coincidence")]
    fn width(self) -> Width {
        match self {
            Self::Star => Width::Fixed(26.0),
            Self::Categories => Width::Fixed(150.0),
            Self::Name => Width::Flex(1.0),
            Self::Folder => Width::Flex(0.8),
            Self::Type => Width::Fixed(64.0),
            Self::Details => Width::Fixed(140.0),
            Self::Dimensions => Width::Fixed(104.0),
            Self::Waveform => Width::Fixed(150.0),
            Self::Length => Width::Fixed(64.0),
            Self::Format => Width::Fixed(112.0),
            Self::Triangles => Width::Fixed(92.0),
            Self::Size => Width::Fixed(78.0),
            Self::Modified => Width::Fixed(104.0),
        }
    }

    /// The order clicking the header sorts by; only what's known for every file can sort.
    fn sort(self) -> Option<SortBy> {
        match self {
            Self::Name => Some(SortBy::Name),
            Self::Type => Some(SortBy::Type),
            Self::Folder => Some(SortBy::Folder),
            Self::Size => Some(SortBy::Size),
            Self::Modified => Some(SortBy::Modified),
            _ => None,
        }
    }

    fn right_aligned(self) -> bool {
        matches!(self, Self::Size | Self::Length | Self::Triangles)
    }
}

pub fn columns(mode: Mode) -> &'static [Column] {
    use Column::*;
    match mode {
        Mode::All => &[Star, Name, Type, Details, Categories, Folder, Size, Modified],
        Mode::Images => &[Star, Name, Type, Dimensions, Categories, Folder, Size, Modified],
        Mode::Sounds => &[Star, Name, Waveform, Length, Format, Categories, Folder, Size, Modified],
        Mode::Models => &[Star, Name, Type, Triangles, Details, Categories, Folder, Size, Modified],
    }
}

/// Each shown column with its horizontal span. Narrow windows drop the folder column first.
fn column_layout(mode: Mode, width: f32) -> Vec<(Column, Range<f32>)> {
    const PAD: f32 = 12.0;
    let mut shown: Vec<Column> = columns(mode).to_vec();
    let fixed = |columns: &[Column]| -> f32 {
        columns.iter().map(|c| if let Width::Fixed(w) = c.width() { w } else { 0.0 }).sum::<f32>()
    };
    while fixed(&shown) + 260.0 > width && shown.len() > 3 {
        let drop = [
            Column::Folder,
            Column::Categories,
            Column::Modified,
            Column::Details,
            Column::Format,
            Column::Type,
            Column::Size,
        ]
        .into_iter()
        .find(|column| shown.contains(column));
        match drop {
            Some(column) => shown.retain(|c| *c != column),
            None => break,
        }
    }
    let flex_total: f32 = shown.iter().map(|c| if let Width::Flex(w) = c.width() { w } else { 0.0 }).sum();
    let spare = (width - fixed(&shown) - PAD * 2.0).max(0.0);
    let mut x = PAD;
    shown
        .into_iter()
        .map(|column| {
            let w = match column.width() {
                Width::Fixed(w) => w,
                Width::Flex(weight) => spare * weight / flex_total.max(f32::EPSILON),
            };
            let span = x..x + w;
            x += w;
            (column, span)
        })
        .collect()
}

fn text_anchor(column: Column, rect: &Rect) -> (Pos2, Align2) {
    if column.right_aligned() {
        (pos2(rect.right() - 10.0, rect.center().y), Align2::RIGHT_CENTER)
    } else {
        (pos2(rect.left() + 4.0, rect.center().y), Align2::LEFT_CENTER)
    }
}

struct ListRow<'a, 'b> {
    browse: &'a Browse<'b>,
    entry: &'a Entry,
    index: u32,
    previews: &'a Previews,
    look: Look,
}

impl ListRow<'_, '_> {
    /// Draws the row; returns what its star asked for.
    #[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
    fn show(
        self,
        ui: &Ui,
        rect: Rect,
        response: &egui::Response,
        layout: &[(Column, Range<f32>)],
    ) -> Option<GroupCommand> {
        let entry = self.entry;
        let mut command = None;
        let painter = ui.painter();
        let selected = self.browse.selection.contains(self.index);
        let row = rect.shrink2(vec2(4.0, 1.0));
        if selected {
            painter.rect_filled(row, theme::RADIUS, theme::ACCENT_SOFT);
            if self.browse.selection.focus == Some(self.index) {
                painter.rect_stroke(
                    row,
                    theme::RADIUS,
                    Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.7)),
                    StrokeKind::Inside,
                );
            }
        } else if response.hovered() {
            painter.rect_filled(row, theme::RADIUS, theme::CARD);
        }
        let playing = self.browse.now.filter(|now| now.key == entry.key && !now.finished);
        let x0 = rect.left();
        for (column, span) in layout {
            let cell = Rect::from_x_y_ranges((x0 + span.start)..=(x0 + span.end), rect.y_range());
            let (anchor, align) = text_anchor(*column, &cell);
            let weak = |text: String| {
                painter.text(anchor, align, text, FontId::proportional(theme::size::LABEL), theme::TEXT_WEAK);
            };
            match column {
                Column::Star => {
                    command = star_button(ui, self.browse, self.index, cell.center(), response.contains_pointer());
                }
                Column::Categories => {
                    let mut x = cell.left() + 4.0;
                    let font = FontId::proportional(theme::size::SMALL);
                    for group in self.browse.categories(self.index) {
                        let galley = painter.layout_no_wrap(group.name.clone(), font.clone(), theme::TEXT_WEAK);
                        let width = galley.size().x + 20.0;
                        if x + width > cell.right() - 4.0 {
                            painter.text(
                                pos2(x, cell.center().y),
                                Align2::LEFT_CENTER,
                                "…",
                                font.clone(),
                                theme::TEXT_FAINT,
                            );
                            break;
                        }
                        let pill = Rect::from_min_size(pos2(x, cell.center().y - 9.0), vec2(width, 18.0));
                        painter.rect_filled(pill, CornerRadius::same(9), theme::CARD_HOVER);
                        painter.circle_filled(
                            pos2(pill.left() + 8.0, pill.center().y),
                            3.5,
                            theme::category_color(group.color),
                        );
                        painter.galley(
                            pos2(pill.left() + 15.0, pill.center().y - galley.size().y / 2.0),
                            galley,
                            theme::TEXT_WEAK,
                        );
                        x += width + 4.0;
                    }
                }
                Column::Name => {
                    let thumb = Rect::from_min_size(pos2(cell.left() + 2.0, cell.top() + 4.0), vec2(26.0, 26.0));
                    painter.rect_filled(thumb, CornerRadius::same(4), theme::CARD);
                    if entry.kind == AssetKind::Audio {
                        let glyph = if playing.is_some() { icon::SPEAKER_HIGH } else { icon::WAVEFORM };
                        painter.text(
                            thumb.center(),
                            Align2::CENTER_CENTER,
                            glyph,
                            FontId::proportional(theme::size::TITLE),
                            theme::kind_color(entry.kind),
                        );
                    } else {
                        draw_preview(painter, thumb.shrink(1.0), entry, self.previews, None, self.look, false);
                    }
                    let color = if selected { theme::TEXT } else { theme::TEXT.gamma_multiply(0.9) };
                    let text_left = thumb.right() + 10.0;
                    let galley = widgets::elided(
                        painter,
                        entry.name().to_owned(),
                        FontId::proportional(theme::size::BODY),
                        color,
                        (cell.right() - text_left - 8.0).max(10.0),
                    );
                    painter.galley(pos2(text_left, cell.center().y - galley.size().y / 2.0), galley, color);
                }
                Column::Type => {
                    widgets::badge(
                        painter,
                        anchor,
                        Align2::LEFT_CENTER,
                        &entry.extension().to_uppercase(),
                        theme::kind_color(entry.kind),
                    );
                }
                Column::Details => weak(self.details()),
                Column::Dimensions => weak(self.previews.dimensions(entry.key).map_or_else(String::new, dimensions)),
                Column::Waveform => {
                    if let Some(waveform) = self.previews.waveforms.get(&entry.key) {
                        let progress = playing.map(|now| now.progress(waveform.duration as f64));
                        let color = theme::kind_color(entry.kind);
                        let wave = cell.shrink2(vec2(4.0, 7.0));
                        draw_waveform(painter, wave, waveform, color.gamma_multiply(0.5), color, progress);
                    }
                }
                Column::Length => weak(
                    self.previews
                        .waveforms
                        .get(&entry.key)
                        .map_or_else(String::new, |w| format_time(w.duration as f64)),
                ),
                Column::Format => weak(self.previews.waveforms.get(&entry.key).map_or_else(String::new, |w| {
                    format!("{} · {}", sample_rate(w.sample_rate), channels(w.channels))
                })),
                Column::Triangles => weak(
                    self.previews
                        .models
                        .get(&entry.key)
                        .filter(|info| info.geometry)
                        .map_or_else(String::new, |info| grouped(info.triangles as usize)),
                ),
                Column::Folder => {
                    let galley = widgets::elided(
                        painter,
                        self.browse.library.folder_label(entry),
                        FontId::proportional(theme::size::LABEL),
                        theme::TEXT_FAINT,
                        (cell.width() - 12.0).max(10.0),
                    );
                    painter.galley(
                        pos2(cell.left() + 4.0, cell.center().y - galley.size().y / 2.0),
                        galley,
                        theme::TEXT_FAINT,
                    );
                }
                Column::Size => weak(format_size(entry.size)),
                Column::Modified => weak(format_age(entry.mtime)),
            }
        }
        response.clone().on_hover_text_at_pointer(&entry.rel);
        command
    }

    /// One line about the asset, whatever its kind.
    fn details(&self) -> String {
        let entry = self.entry;
        match entry.kind {
            AssetKind::Image => self.previews.dimensions(entry.key).map_or_else(String::new, dimensions),
            AssetKind::Audio => {
                self.previews.waveforms.get(&entry.key).map_or_else(String::new, |w| {
                    format!("{} · {}", format_time(w.duration as f64), channels(w.channels))
                })
            }
            AssetKind::Model => self.previews.models.get(&entry.key).map_or_else(String::new, |info| {
                if !info.geometry {
                    return if info.animations.is_empty() { "preview only".to_owned() } else { "animation".to_owned() };
                }
                match info.materials {
                    0 => format!("{} meshes", info.meshes),
                    1 => format!("{} meshes · 1 material", info.meshes),
                    n => format!("{} meshes · {n} materials", info.meshes),
                }
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_lists_drop_the_folder_first() {
        let wide = column_layout(Mode::Sounds, 1400.0);
        assert_eq!(wide.len(), columns(Mode::Sounds).len());
        let narrow = column_layout(Mode::Sounds, 700.0);
        assert!(!narrow.iter().any(|(column, _)| *column == Column::Folder));
        assert!(narrow.iter().any(|(column, _)| *column == Column::Name));
        for layout in [wide, narrow] {
            let spans: Vec<_> = layout.iter().map(|(_, span)| span.clone()).collect();
            assert!(spans.windows(2).all(|pair| pair[0].end <= pair[1].start + 0.01), "columns don't overlap");
        }
    }

    #[test]
    fn scrolls_only_when_out_of_sight() {
        assert_eq!(scroll_into_view(100.0, 30.0, (0.0, 500.0)), None);
        assert_eq!(scroll_into_view(10.0, 30.0, (50.0, 500.0)), Some(10.0));
        assert_eq!(scroll_into_view(600.0, 30.0, (0.0, 500.0)), Some(130.0));
    }
}
