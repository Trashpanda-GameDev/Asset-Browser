//! Small widgets drawn by hand, so every control shares one look: icon buttons, a segmented
//! control, toggle chips, the search field, badges and property rows.

use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Frame, Galley, Id, Key, Pos2, Rect, Response, Sense, Stroke,
    StrokeKind, Ui, Vec2, pos2, vec2,
};
use egui_phosphor::regular as icon;

use super::theme;

const ICON_SIZE: f32 = theme::size::LARGE;
/// The segmented control's height, and the room between its edge and the segments.
const SEGMENT_HEIGHT: f32 = 30.0;
const SEGMENT_PAD: f32 = 3.0;

/// The sidebar's rows, folders and groups alike.
pub const SIDEBAR_ROW_HEIGHT: f32 = 26.0;

/// A square button showing one icon. `active` shows it switched on.
pub fn icon_button(ui: &mut Ui, glyph: &str, tooltip: &str, active: bool) -> Response {
    icon_button_sized(ui, glyph, tooltip, active, 28.0)
}

pub fn icon_button_sized(ui: &mut Ui, glyph: &str, tooltip: &str, active: bool, side: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), Sense::click());
    let hovered = response.hovered();
    let fill = if active {
        theme::ACCENT_SOFT
    } else if response.is_pointer_button_down_on() {
        theme::CARD_ACTIVE
    } else if hovered {
        theme::CARD_HOVER
    } else {
        Color32::TRANSPARENT
    };
    let color = if active {
        theme::ACCENT
    } else if hovered {
        theme::TEXT
    } else {
        theme::TEXT_WEAK
    };
    let painter = ui.painter();
    painter.rect_filled(rect, theme::RADIUS, fill);
    painter.text(rect.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(side * 0.58), color);
    if tooltip.is_empty() { response } else { response.on_hover_text(tooltip) }
}

/// A button with an icon and a label on a card background.
pub fn button(ui: &mut Ui, glyph: &str, label: &str) -> Response {
    let font = FontId::proportional(theme::size::BODY);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font.clone(), theme::TEXT);
    let icon_width = if glyph.is_empty() { 0.0 } else { ICON_SIZE + 6.0 };
    let size = vec2(galley.size().x + icon_width + 22.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let fill = if response.is_pointer_button_down_on() {
        theme::CARD_ACTIVE
    } else if response.hovered() {
        theme::CARD_HOVER
    } else {
        theme::CARD
    };
    let painter = ui.painter();
    painter.rect_filled(rect, theme::RADIUS, fill);
    let mut x = rect.left() + 11.0;
    if !glyph.is_empty() {
        painter.text(
            pos2(x, rect.center().y),
            Align2::LEFT_CENTER,
            glyph,
            FontId::proportional(ICON_SIZE),
            theme::TEXT,
        );
        x += icon_width;
    }
    painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, theme::TEXT);
    response
}

pub struct Segment<'a> {
    pub glyph: &'a str,
    pub label: &'a str,
    /// Shown after the label, dimmer.
    pub note: Option<String>,
    pub tooltip: &'a str,
}

/// A row of mutually exclusive choices in one pill; the highlight slides to the chosen one.
/// Returns the index clicked, if any.
pub fn segmented(ui: &mut Ui, id: Id, segments: &[Segment<'_>], selected: usize) -> Option<usize> {
    let painter = ui.painter().clone();
    let widths = segment_widths(&painter, segments);
    let total = widths.iter().sum::<f32>() + SEGMENT_PAD * 2.0;
    let (outer, _) = ui.allocate_exact_size(vec2(total, SEGMENT_HEIGHT), Sense::hover());
    painter.rect_filled(outer, theme::CARD_RADIUS, theme::CARD);
    let mut x = outer.left() + SEGMENT_PAD;
    // Where the highlight should be: the chosen segment's left and width.
    let mut target = (x, widths.first().copied().unwrap_or(0.0));
    let mut rects = Vec::with_capacity(segments.len());
    for (index, width) in widths.iter().enumerate() {
        if index == selected {
            target = (x, *width);
        }
        rects.push(Rect::from_min_size(
            pos2(x, outer.top() + SEGMENT_PAD),
            vec2(*width, SEGMENT_HEIGHT - SEGMENT_PAD * 2.0),
        ));
        x += width;
    }
    slide_highlight(ui.ctx(), &painter, id, outer, target);
    let mut clicked = None;
    for (index, (segment, rect)) in segments.iter().zip(&rects).enumerate() {
        let response = ui.interact(*rect, id.with(index), Sense::click());
        if response.clicked() {
            clicked = Some(index);
        }
        let on = index == selected;
        let color = if on || response.hovered() { theme::TEXT } else { theme::TEXT_WEAK };
        paint_segment(&painter, segment, *rect, on, color);
        if !segment.tooltip.is_empty() {
            response.on_hover_text(segment.tooltip);
        }
    }
    clicked
}

/// Each segment's width: its glyph, label and note, with room around them.
fn segment_widths(painter: &egui::Painter, segments: &[Segment<'_>]) -> Vec<f32> {
    let font = FontId::proportional(theme::size::BODY);
    let note_font = FontId::proportional(theme::size::SMALL);
    segments
        .iter()
        .map(|segment| {
            let label = painter.layout_no_wrap(segment.label.to_owned(), font.clone(), theme::TEXT).size().x;
            let note = segment.note.as_ref().map_or(0.0, |note| {
                painter.layout_no_wrap(note.clone(), note_font.clone(), theme::TEXT).size().x + 6.0
            });
            let glyph = match (segment.glyph.is_empty(), segment.label.is_empty() && segment.note.is_none()) {
                (true, _) => 0.0,
                (false, true) => ICON_SIZE, // icon only
                (false, false) => ICON_SIZE + 6.0,
            };
            glyph + label + note + 24.0
        })
        .collect()
}

/// The chosen segment's highlight, eased toward `target` (its left and width) from where it was.
fn slide_highlight(ctx: &egui::Context, painter: &egui::Painter, id: Id, outer: Rect, target: (f32, f32)) {
    let left = ctx.animate_value_with_time(id.with("x"), target.0, 0.16);
    let width = ctx.animate_value_with_time(id.with("w"), target.1, 0.16);
    let highlight =
        Rect::from_min_size(pos2(left, outer.top() + SEGMENT_PAD), vec2(width, SEGMENT_HEIGHT - SEGMENT_PAD * 2.0));
    painter.rect_filled(highlight, theme::RADIUS, theme::CARD_ACTIVE);
}

/// A segment's glyph (in the accent when it's the one chosen), its label in `color`, and its
/// note, dimmer.
fn paint_segment(painter: &egui::Painter, segment: &Segment<'_>, rect: Rect, on: bool, color: Color32) {
    let mut x = rect.left() + 12.0;
    let y = rect.center().y;
    if !segment.glyph.is_empty() {
        let glyph_color = if on { theme::ACCENT } else { color };
        painter.text(pos2(x, y), Align2::LEFT_CENTER, segment.glyph, FontId::proportional(ICON_SIZE), glyph_color);
        x += ICON_SIZE + 6.0;
    }
    let label = painter.layout_no_wrap(segment.label.to_owned(), FontId::proportional(theme::size::BODY), color);
    let label_width = label.size().x;
    painter.galley(pos2(x, y - label.size().y / 2.0), label, color);
    if let Some(note) = &segment.note {
        let font = FontId::proportional(theme::size::SMALL);
        painter.text(pos2(x + label_width + 6.0, y + 0.5), Align2::LEFT_CENTER, note, font, theme::TEXT_FAINT);
    }
}

/// An on/off pill with an icon and a label.
pub fn toggle_chip(ui: &mut Ui, glyph: &str, label: &str, on: &mut bool, tooltip: &str) -> Response {
    let font = FontId::proportional(theme::size::LABEL);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, theme::TEXT);
    let size = vec2(galley.size().x + ICON_SIZE + 26.0, 26.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let (fill, color) = if *on {
        (theme::ACCENT_SOFT, theme::ACCENT)
    } else if response.hovered() {
        (theme::CARD_HOVER, theme::TEXT)
    } else {
        (Color32::TRANSPARENT, theme::TEXT_WEAK)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(13), fill);
    if !*on {
        painter.rect_stroke(rect, CornerRadius::same(13), Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
    }
    let y = rect.center().y;
    painter.text(
        pos2(rect.left() + 10.0, y),
        Align2::LEFT_CENTER,
        glyph,
        FontId::proportional(theme::size::MEDIUM),
        color,
    );
    painter.galley(pos2(rect.left() + 16.0 + ICON_SIZE, y - galley.size().y / 2.0), galley, color);
    if tooltip.is_empty() { response } else { response.on_hover_text(tooltip) }
}

/// A rounded search box with a magnifier and a clear button. `focus` puts the cursor in it.
/// Returns true when the text changed.
pub fn search_field(ui: &mut Ui, text: &mut String, width: f32, focus: bool) -> bool {
    let height = 30.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let id = ui.make_persistent_id("search");
    let focused = ui.ctx().memory(|memory| memory.has_focus(id));
    let painter = ui.painter().clone();
    painter.rect_filled(rect, theme::CARD_RADIUS, theme::CARD);
    let border = if focused { theme::ACCENT.gamma_multiply(0.8) } else { theme::BORDER };
    painter.rect_stroke(rect, theme::CARD_RADIUS, Stroke::new(1.0, border), StrokeKind::Inside);
    painter.text(
        pos2(rect.left() + 12.0, rect.center().y),
        Align2::LEFT_CENTER,
        icon::MAGNIFYING_GLASS,
        FontId::proportional(theme::size::TITLE),
        if focused { theme::TEXT } else { theme::TEXT_FAINT },
    );

    let clear_width = if text.is_empty() { 0.0 } else { 28.0 };
    let edit_rect =
        Rect::from_min_max(pos2(rect.left() + 32.0, rect.top()), pos2(rect.right() - 8.0 - clear_width, rect.bottom()));
    let edit = egui::TextEdit::singleline(text)
        .id(id)
        .frame(Frame::NONE)
        .hint_text(egui::RichText::new("Search names and folders").color(theme::TEXT_FAINT))
        .vertical_align(egui::Align::Center)
        .desired_width(edit_rect.width())
        .min_size(vec2(edit_rect.width(), height));
    let response = ui.put(edit_rect, edit);
    if focus {
        response.request_focus();
    }
    let mut changed = response.changed();
    // Esc clears the search (egui also lets go of the focus); Down goes back to the assets.
    let (escape, down) = ui.input(|input| (input.key_pressed(Key::Escape), input.key_pressed(Key::ArrowDown)));
    if escape && (response.has_focus() || response.lost_focus()) && !text.is_empty() {
        text.clear();
        changed = true;
    }
    if down && response.has_focus() {
        response.surrender_focus();
    }
    if !text.is_empty() {
        let clear = Rect::from_center_size(pos2(rect.right() - 18.0, rect.center().y), Vec2::splat(22.0));
        let response = ui.interact(clear, id.with("clear"), Sense::click());
        let color = if response.hovered() { theme::TEXT } else { theme::TEXT_FAINT };
        painter.text(
            clear.center(),
            Align2::CENTER_CENTER,
            icon::X_CIRCLE,
            FontId::proportional(theme::size::TITLE),
            color,
        );
        if response.on_hover_text("Clear the search (Esc)").clicked() {
            text.clear();
            changed = true;
        }
    }
    changed
}

/// A small dim caption above a group of controls.
pub fn section_label(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text.to_uppercase()).size(theme::size::TINY).color(theme::TEXT_FAINT).strong());
}

/// One "name: value" line in the inspector. Returns the value's response.
pub fn property(ui: &mut Ui, name: &str, value: impl Into<egui::WidgetText>) -> Response {
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(96.0, 18.0), Sense::hover());
        ui.painter().text(
            rect.left_center(),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(theme::size::LABEL),
            theme::TEXT_WEAK,
        );
        ui.add(egui::Label::new(value.into()).wrap())
    })
    .inner
}

/// A window over the app that dims what's behind it, such as the settings: `margin` inside its
/// border.
pub fn modal_window(id: &str, margin: i8) -> egui::Modal {
    let frame = Frame::new()
        .fill(theme::PANEL)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(theme::CARD_RADIUS)
        .inner_margin(egui::Margin::same(margin));
    egui::Modal::new(Id::new(id)).frame(frame).backdrop_color(theme::SCRIM)
}

/// Takes this frame's scrolling out of the input, for a view that zooms when scrolled over: the
/// scroll area around it reads what's left at the end of the frame, so it doesn't scroll too.
pub fn take_scroll(ui: &Ui) -> f32 {
    ui.input_mut(|input| std::mem::take(&mut input.smooth_scroll_delta.y))
}

/// A row of the sidebar to click, the width of it: highlighted when `selected`, and while the
/// pointer is over it.
pub fn sidebar_row(ui: &mut Ui, selected: bool) -> (Rect, Response) {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), SIDEBAR_ROW_HEIGHT), Sense::click());
    let row = rect.shrink2(vec2(4.0, 1.0));
    if selected {
        ui.painter().rect_filled(row, theme::RADIUS, theme::ACCENT_SOFT);
    } else if response.hovered() {
        ui.painter().rect_filled(row, theme::RADIUS, theme::CARD_HOVER);
    }
    (rect, response)
}

/// `text` laid out on one line, cut short with "…" where it's wider than `max_width`.
pub fn elided(painter: &egui::Painter, text: String, font: FontId, color: Color32, max_width: f32) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text, font, color);
    job.wrap =
        egui::text::TextWrapping { max_width, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    painter.layout_job(job)
}

/// A small coloured pill with text, e.g. a file type. Returns its rect.
pub fn badge(painter: &egui::Painter, anchor: Pos2, align: Align2, text: &str, color: Color32) -> Rect {
    let galley = painter.layout_no_wrap(text.to_owned(), badge_font(), color);
    let rect = align.anchor_size(anchor, galley.size() + vec2(10.0, 4.0));
    paint_badge(painter, rect, galley, color);
    rect
}

/// A badge laid out like any widget.
pub fn badge_widget(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), badge_font(), color);
    let (rect, response) = ui.allocate_exact_size(galley.size() + vec2(10.0, 4.0), Sense::hover());
    paint_badge(ui.painter(), rect, galley, color);
    response
}

fn badge_font() -> FontId {
    FontId::new(theme::size::TINY, theme::semibold())
}

fn paint_badge(painter: &egui::Painter, rect: Rect, galley: std::sync::Arc<egui::Galley>, color: Color32) {
    painter.rect_filled(rect, CornerRadius::same(5), theme::shade::BADGE);
    painter.rect_filled(rect, CornerRadius::same(5), color.gamma_multiply(0.16));
    painter.galley(rect.min + vec2(5.0, 2.0), galley, color);
}

/// Fills `rect` with a vertical gradient.
pub fn vertical_gradient(painter: &egui::Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(egui::Shape::mesh(mesh));
}

/// A thin vertical divider between groups of toolbar controls.
pub fn divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 20.0), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), Stroke::new(1.0, theme::BORDER));
}
