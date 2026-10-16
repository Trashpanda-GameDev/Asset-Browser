//! The list: a row per asset, with each mode's columns, which narrow windows drop in turn.

use std::ops::Range;

use eframe::egui::{self, Align2, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use egui_phosphor::regular as icon;

use crate::asset::AssetKind;
use crate::audio::NowPlaying;
use crate::library::{Entry, SortBy};
use crate::ui::format::{channels, dimensions, format_age, format_size, format_time, grouped, sample_rate};
use crate::ui::mode::Mode;
use crate::ui::organize::GroupCommand;
use crate::ui::previews::Previews;
use crate::ui::sound::draw_waveform;
use crate::ui::{theme, widgets};

use super::preview::{PreviewOf, draw_preview};
use super::{Browse, BrowseOutput, Look, item_response, scroll_into_view, star_button, want_rows};

const ROW_HEIGHT: f32 = 34.0;

const HEADER_HEIGHT: f32 = 28.0;

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
        let sorted = sort.is_some_and(|sort| sort == browse.order.by);
        let color = if sorted || (sort.is_some() && response.hovered()) { theme::TEXT_WEAK } else { theme::TEXT_FAINT };
        let mut label = column.label().to_uppercase();
        if sorted {
            label.push(' ');
            label.push_str(if browse.order.descending { icon::CARET_DOWN } else { icon::CARET_UP });
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
    fn show(
        self,
        ui: &Ui,
        rect: Rect,
        response: &egui::Response,
        layout: &[(Column, Range<f32>)],
    ) -> Option<GroupCommand> {
        let painter = ui.painter();
        self.background(painter, rect, response);
        let mut command = None;
        for (column, span) in layout {
            let cell = Rect::from_x_y_ranges((rect.left() + span.start)..=(rect.left() + span.end), rect.y_range());
            let (anchor, align) = text_anchor(*column, &cell);
            match column {
                Column::Star => {
                    command = star_button(ui, self.browse, self.index, cell.center(), response.contains_pointer());
                }
                Column::Categories => self.category_pills(painter, cell),
                Column::Name => self.name_cell(painter, cell),
                Column::Type => {
                    let kind = self.entry.kind;
                    let extension = self.entry.extension().to_uppercase();
                    widgets::badge(painter, anchor, Align2::LEFT_CENTER, &extension, theme::kind_color(kind));
                }
                Column::Waveform => self.waveform_cell(painter, cell),
                Column::Folder => self.folder_cell(painter, cell),
                text => {
                    let font = FontId::proportional(theme::size::LABEL);
                    painter.text(anchor, align, self.text(*text), font, theme::TEXT_WEAK);
                }
            }
        }
        response.clone().on_hover_text_at_pointer(&self.entry.rel);
        command
    }

    fn selected(&self) -> bool {
        self.browse.selection.contains(self.index)
    }

    /// What's playing, if it's this asset.
    fn playing(&self) -> Option<&NowPlaying> {
        self.browse.now.filter(|now| now.key == self.entry.key && !now.finished)
    }

    /// Selected rows are tinted and the focused one outlined; others light up under the pointer.
    fn background(&self, painter: &egui::Painter, rect: Rect, response: &egui::Response) {
        let row = rect.shrink2(vec2(4.0, 1.0));
        if self.selected() {
            painter.rect_filled(row, theme::RADIUS, theme::ACCENT_SOFT);
            if self.browse.selection.focus == Some(self.index) {
                let outline = Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.7));
                painter.rect_stroke(row, theme::RADIUS, outline, StrokeKind::Inside);
            }
        } else if response.hovered() {
            painter.rect_filled(row, theme::RADIUS, theme::CARD);
        }
    }

    /// The asset's categories as pills, as many as fit, then "…".
    fn category_pills(&self, painter: &egui::Painter, cell: Rect) {
        let mut x = cell.left() + 4.0;
        let font = FontId::proportional(theme::size::SMALL);
        for group in self.browse.categories(self.index) {
            let galley = painter.layout_no_wrap(group.name.clone(), font.clone(), theme::TEXT_WEAK);
            let width = galley.size().x + 20.0;
            if x + width > cell.right() - 4.0 {
                painter.text(pos2(x, cell.center().y), Align2::LEFT_CENTER, "…", font.clone(), theme::TEXT_FAINT);
                break;
            }
            let pill = Rect::from_min_size(pos2(x, cell.center().y - 9.0), vec2(width, 18.0));
            painter.rect_filled(pill, CornerRadius::same(9), theme::CARD_HOVER);
            painter.circle_filled(pos2(pill.left() + 8.0, pill.center().y), 3.5, theme::category_color(group.color));
            painter.galley(pos2(pill.left() + 15.0, pill.center().y - galley.size().y / 2.0), galley, theme::TEXT_WEAK);
            x += width + 4.0;
        }
    }

    /// A small preview (for a sound, a speaker while it plays or else a waveform glyph), and the
    /// name.
    fn name_cell(&self, painter: &egui::Painter, cell: Rect) {
        let entry = self.entry;
        let thumb = Rect::from_min_size(pos2(cell.left() + 2.0, cell.top() + 4.0), vec2(26.0, 26.0));
        painter.rect_filled(thumb, CornerRadius::same(4), theme::CARD);
        if entry.kind == AssetKind::Audio {
            let glyph = if self.playing().is_some() { icon::SPEAKER_HIGH } else { icon::WAVEFORM };
            let font = FontId::proportional(theme::size::TITLE);
            painter.text(thumb.center(), Align2::CENTER_CENTER, glyph, font, theme::kind_color(entry.kind));
        } else {
            let preview = PreviewOf { entry, previews: self.previews, playing: None, large: false };
            draw_preview(painter, thumb.shrink(1.0), &preview, self.look);
        }
        let color = if self.selected() { theme::TEXT } else { theme::TEXT.gamma_multiply(0.9) };
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

    /// A sound's waveform, how far it's played shown while it plays.
    fn waveform_cell(&self, painter: &egui::Painter, cell: Rect) {
        if let Some(waveform) = self.previews.waveforms.get(&self.entry.key) {
            let progress = self.playing().map(|now| now.progress(waveform.duration as f64));
            let color = theme::kind_color(self.entry.kind);
            let wave = cell.shrink2(vec2(4.0, 7.0));
            draw_waveform(painter, wave, waveform, [color.gamma_multiply(0.5), color], progress);
        }
    }

    /// The folder the asset is in, cut short to fit.
    fn folder_cell(&self, painter: &egui::Painter, cell: Rect) {
        let galley = widgets::elided(
            painter,
            self.browse.library.folder_label(self.entry),
            FontId::proportional(theme::size::LABEL),
            theme::TEXT_FAINT,
            (cell.width() - 12.0).max(10.0),
        );
        painter.galley(pos2(cell.left() + 4.0, cell.center().y - galley.size().y / 2.0), galley, theme::TEXT_FAINT);
    }

    /// A text column's value, empty while it isn't known.
    fn text(&self, column: Column) -> String {
        let entry = self.entry;
        let waveform = || self.previews.waveforms.get(&entry.key);
        match column {
            Column::Details => self.details(),
            Column::Dimensions => self.previews.dimensions(entry.key).map_or_else(String::new, dimensions),
            Column::Length => waveform().map_or_else(String::new, |w| format_time(w.duration as f64)),
            Column::Format => waveform()
                .map_or_else(String::new, |w| format!("{} · {}", sample_rate(w.sample_rate), channels(w.channels))),
            Column::Triangles => self
                .previews
                .models
                .get(&entry.key)
                .filter(|info| info.geometry)
                .map_or_else(String::new, |info| grouped(info.triangles as usize)),
            Column::Size => format_size(entry.size),
            Column::Modified => format_age(entry.mtime),
            Column::Star | Column::Categories | Column::Name | Column::Type | Column::Waveform | Column::Folder => {
                String::new()
            }
        }
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
}
