//! The tile grid: a picture of each asset with its name, its type and its categories.

use eframe::egui::{self, Align2, FontId, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::library::Entry;
use crate::ui::previews::Previews;
use crate::ui::{theme, widgets};

use super::preview::{PreviewOf, draw_preview};
use super::{Browse, BrowseOutput, Look, item_response, scroll_into_view, star_button, want_rows};

const LABEL_HEIGHT: f32 = 22.0;

const GAP: f32 = 10.0;

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
        let preview = PreviewOf { entry, previews: self.previews, playing, large: true };
        draw_preview(painter, inner, &preview, self.look);

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
                painter.circle_filled(center, 5.0, theme::shade::DOT);
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
