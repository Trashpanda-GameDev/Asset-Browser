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
use crate::library::{Entry, Library, SortBy};

use super::format::{format_age, format_size, format_time, grouped};
use super::image_view::Backdrop;
use super::mode::{Mode, Selection};
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
    pub menu: Option<MenuAction>,
    pub sort_clicked: Option<SortBy>,
    /// Assets per row and rows per screen, for the arrow and page keys.
    pub columns: usize,
    pub page_rows: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Open,
    Reveal,
    CopyPaths,
}

/// The tile grid.
pub fn grid(
    ui: &mut Ui,
    browse: &Browse<'_>,
    previews: &mut Previews,
    tile: f32,
    scroll: &mut (f32, f32),
) -> BrowseOutput {
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
        if let Some(offset) = scroll_into_view(top, cell.y, *scroll) {
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
                let entry = &browse.library.entries[index as usize];
                let x = row_rect.left() + GAP + column as f32 * cell.x + (cell.x - GAP - tile) / 2.0;
                let rect = Rect::from_min_size(pos2(x, row_rect.top() + GAP), vec2(tile, tile + LABEL_HEIGHT));
                let tile_ui = Tile { browse, entry, index, previews, look };
                tile_ui.show(ui, rect, &mut output);
            }
        }
    });
    *scroll = (result.state.offset.y, result.inner_rect.height());
    output.page_rows = ((result.inner_rect.height() / cell.y).floor() as usize).max(1);
    want_rows(browse, previews, visible, columns, rows);
    output
}

/// The list, with a header to sort by.
pub fn list(ui: &mut Ui, browse: &Browse<'_>, previews: &mut Previews, scroll: &mut (f32, f32)) -> BrowseOutput {
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
        ui.painter().text(anchor, align, label, FontId::new(10.5, theme::semibold()), color);
        if response.clicked() {
            output.sort_clicked = sort;
        }
    }

    let mut area = egui::ScrollArea::vertical().id_salt(("list", browse.mode.key())).auto_shrink([false; 2]);
    if let Some(position) = browse.scroll_to
        && let Some(offset) = scroll_into_view(position as f32 * ROW_HEIGHT, ROW_HEIGHT, *scroll)
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
            let entry = &browse.library.entries[index as usize];
            let (rect, response) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::click());
            let row = ListRow { browse, entry, index, previews, look };
            row.show(ui, rect, &response, &layout);
            item_response(browse, index, &response, &mut output);
        }
    });
    *scroll = (result.state.offset.y, result.inner_rect.height());
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
        ui.set_min_width(180.0);
        if ui.button(format!("{}  Open", icon::ARROW_SQUARE_OUT)).clicked() {
            output.menu = Some(MenuAction::Open);
            ui.close();
        }
        if ui.button(format!("{}  Show in folder", icon::FOLDER_OPEN)).clicked() {
            output.menu = Some(MenuAction::Reveal);
            ui.close();
        }
        let copy = if count > 1 {
            format!("{}  Copy {count} paths", icon::COPY)
        } else {
            format!("{}  Copy path", icon::COPY)
        };
        if ui.button(copy).clicked() {
            output.menu = Some(MenuAction::CopyPaths);
            ui.close();
        }
    });
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
        let response = ui.interact(rect, egui::Id::new(("tile", entry.key)), Sense::click());
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
                entry.kind.color(),
            );
        }

        let color = if selected { theme::TEXT } else { theme::TEXT.gamma_multiply(0.86) };
        let mut job =
            egui::text::LayoutJob::simple_singleline(entry.name().to_owned(), FontId::proportional(12.5), color);
        job.wrap = egui::text::TextWrapping {
            max_width: rect.width(),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let galley = painter.layout_job(job);
        let label_pos = pos2(rect.center().x - galley.size().x / 2.0, card.bottom() + 5.0);
        painter.galley(label_pos, galley, color);

        item_response(self.browse, self.index, &response, output);
        response.on_hover_text_at_pointer(&entry.rel);
    }
}

/// Draws an asset's preview into `rect`: its thumbnail, waveform, or a placeholder while it's made.
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
        let color = entry.kind.color();
        let progress = playing.map(|now| {
            let total = now.duration.unwrap_or(waveform.duration as f64).max(f64::EPSILON);
            (now.position / total) as f32
        });
        let wave_rect = if large { Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y - 16.0)) } else { rect };
        draw_waveform(painter, wave_rect, waveform, color.gamma_multiply(0.5), color, progress);
        if large {
            painter.text(
                rect.right_bottom(),
                Align2::RIGHT_BOTTOM,
                format_time(waveform.duration as f64),
                FontId::proportional(11.0),
                theme::TEXT_WEAK,
            );
            if playing.is_some() {
                painter.text(
                    rect.left_bottom(),
                    Align2::LEFT_BOTTOM,
                    icon::SPEAKER_HIGH,
                    FontId::proportional(13.0),
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
            entry.kind.icon(),
            FontId::proportional(size),
            entry.kind.color().gamma_multiply(0.35),
        );
    }
}

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
    Name,
    Type,
    Details,
    Dimensions,
    Waveform,
    Length,
    Format,
    Triangles,
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

    fn width(self) -> Width {
        match self {
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
        Mode::All => &[Name, Type, Details, Folder, Size, Modified],
        Mode::Images => &[Name, Type, Dimensions, Folder, Size, Modified],
        Mode::Sounds => &[Name, Waveform, Length, Format, Folder, Size, Modified],
        Mode::Models => &[Name, Type, Triangles, Details, Folder, Size, Modified],
    }
}

/// Each shown column with its horizontal span. Narrow windows drop the folder column first.
fn column_layout(mode: Mode, width: f32) -> Vec<(Column, Range<f32>)> {
    const PAD: f32 = 12.0;
    let mut shown: Vec<Column> = columns(mode).to_vec();
    let fixed = |columns: &[Column]| -> f32 {
        columns.iter().map(|c| if let Width::Fixed(w) = c.width() { w } else { 0.0 }).sum::<f32>()
    };
    while fixed(&shown) + 260.0 > width && shown.len() > 2 {
        let drop = [Column::Folder, Column::Modified, Column::Details, Column::Format, Column::Type, Column::Size]
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
    fn show(self, ui: &Ui, rect: Rect, response: &egui::Response, layout: &[(Column, Range<f32>)]) {
        let entry = self.entry;
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
                painter.text(anchor, align, text, FontId::proportional(12.5), theme::TEXT_WEAK);
            };
            match column {
                Column::Name => {
                    let thumb = Rect::from_min_size(pos2(cell.left() + 2.0, cell.top() + 4.0), vec2(26.0, 26.0));
                    painter.rect_filled(thumb, CornerRadius::same(4), theme::CARD);
                    if entry.kind == AssetKind::Audio {
                        let glyph = if playing.is_some() { icon::SPEAKER_HIGH } else { icon::WAVEFORM };
                        painter.text(
                            thumb.center(),
                            Align2::CENTER_CENTER,
                            glyph,
                            FontId::proportional(15.0),
                            entry.kind.color(),
                        );
                    } else {
                        draw_preview(painter, thumb.shrink(1.0), entry, self.previews, None, self.look, false);
                    }
                    let color = if selected { theme::TEXT } else { theme::TEXT.gamma_multiply(0.9) };
                    let text_left = thumb.right() + 10.0;
                    let mut job = egui::text::LayoutJob::simple_singleline(
                        entry.name().to_owned(),
                        FontId::proportional(13.0),
                        color,
                    );
                    job.wrap = egui::text::TextWrapping {
                        max_width: (cell.right() - text_left - 8.0).max(10.0),
                        max_rows: 1,
                        break_anywhere: true,
                        overflow_character: Some('…'),
                    };
                    let galley = painter.layout_job(job);
                    painter.galley(pos2(text_left, cell.center().y - galley.size().y / 2.0), galley, color);
                }
                Column::Type => {
                    widgets::badge(
                        painter,
                        anchor,
                        Align2::LEFT_CENTER,
                        &entry.extension().to_uppercase(),
                        entry.kind.color(),
                    );
                }
                Column::Details => weak(self.details()),
                Column::Dimensions => {
                    weak(self.previews.dimensions(entry.key).map_or_else(String::new, |[w, h]| format!("{w} × {h}")))
                }
                Column::Waveform => {
                    if let Some(waveform) = self.previews.waveforms.get(&entry.key) {
                        let progress = playing.map(|now| {
                            let total = now.duration.unwrap_or(waveform.duration as f64).max(f64::EPSILON);
                            (now.position / total) as f32
                        });
                        let color = entry.kind.color();
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
                    format!("{:.1} kHz · {}", w.sample_rate as f32 / 1000.0, channels(w.channels))
                })),
                Column::Triangles => weak(
                    self.previews
                        .models
                        .get(&entry.key)
                        .filter(|info| info.geometry)
                        .map_or_else(String::new, |info| grouped(info.triangles as usize)),
                ),
                Column::Folder => {
                    let mut job = egui::text::LayoutJob::simple_singleline(
                        entry.folder().to_owned(),
                        FontId::proportional(12.5),
                        theme::TEXT_FAINT,
                    );
                    job.wrap = egui::text::TextWrapping {
                        max_width: (cell.width() - 12.0).max(10.0),
                        max_rows: 1,
                        break_anywhere: true,
                        overflow_character: Some('…'),
                    };
                    let galley = painter.layout_job(job);
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
    }

    /// One line about the asset, whatever its kind.
    fn details(&self) -> String {
        let entry = self.entry;
        match entry.kind {
            AssetKind::Image => {
                self.previews.dimensions(entry.key).map_or_else(String::new, |[w, h]| format!("{w} × {h}"))
            }
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

pub fn channels(count: u16) -> String {
    match count {
        1 => "mono".to_owned(),
        2 => "stereo".to_owned(),
        n => format!("{n} ch"),
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
