//! The sidebar's groups: Favorites, the collections and categories with Uncategorized, each to show,
//! rename, recolour or delete, to drop assets on, and a new one to name.

use eframe::egui::{self, Align2, Color32, FontId, Response, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use egui_phosphor::regular as icon;

use crate::asset::KindSet;
use crate::groups::{FAVORITES, Group, GroupKind, Groups, Membership, UNCATEGORIZED, UNCATEGORIZED_NAME};
use crate::ui::format::{grouped, plural};
use crate::ui::keymap::{self, Action};
use crate::ui::{theme, widgets};

use super::{DraggedAssets, GroupCommand, GroupEdit, Targets};

const ROW_HEIGHT: f32 = widgets::SIDEBAR_ROW_HEIGHT;

/// The sidebar's Favorites, Collections and Categories.
pub struct Sidebar<'a> {
    pub groups: &'a Groups,
    pub membership: &'a Membership,
    /// The mode's kinds: the counts only count those.
    pub kinds: KindSet,
    /// The group being browsed.
    pub viewing: Option<i64>,
    pub edit: &'a mut Option<GroupEdit>,
}

impl Sidebar<'_> {
    pub fn show(mut self, ui: &mut Ui) -> Vec<GroupCommand> {
        let mut commands = Vec::new();
        let dragging = egui::DragAndDrop::payload::<DraggedAssets>(ui.ctx()).map(|payload| payload.0.len());
        self.favorites_row(ui, &mut commands);
        for kind in [GroupKind::Collection, GroupKind::Category] {
            ui.add_space(10.0);
            section_heading(ui, kind, &mut commands);
            if kind == GroupKind::Category {
                self.uncategorized_row(ui, &mut commands);
            }
            let any = self.group_rows(ui, kind, &mut commands);
            below_groups(ui, kind, dragging, any, &mut commands);
        }
        commands
    }

    /// Favorites, always first: the starred assets.
    fn favorites_row(&self, ui: &mut Ui, commands: &mut Vec<GroupCommand>) {
        let Some(favorites) = self.groups.get(FAVORITES) else { return };
        let count = self.membership.total(FAVORITES, self.kinds);
        let row =
            Row { glyph: Glyph::Favorite, name: &favorites.name, count, selected: self.viewing == Some(FAVORITES) };
        let response = row.show(ui);
        drop_target(&response, FAVORITES, commands);
        if response.clicked() {
            commands.push(GroupCommand::Show(FAVORITES));
        }
        let tip = match keymap::shortcut_label(ui.ctx(), Action::Favorite) {
            Some(key) => format!("Starred assets ({key} stars the selection)"),
            None => "Starred assets".to_owned(),
        };
        response.on_hover_text(tip);
    }

    /// Uncategorized, always there: whatever no category holds, to work through.
    fn uncategorized_row(&self, ui: &mut Ui, commands: &mut Vec<GroupCommand>) {
        let count = self.membership.total(UNCATEGORIZED, self.kinds);
        let selected = self.viewing == Some(UNCATEGORIZED);
        let response = Row { glyph: Glyph::Ring, name: UNCATEGORIZED_NAME, count, selected }.show(ui);
        if response.clicked() {
            commands.push(GroupCommand::Show(UNCATEGORIZED));
        }
        response.on_hover_text("Assets in no category");
    }

    /// The groups of `kind`, the one being named as a text field; whether there are any.
    fn group_rows(&mut self, ui: &mut Ui, kind: GroupKind, commands: &mut Vec<GroupCommand>) -> bool {
        let groups = self.groups;
        let mut any = false;
        for group in groups.of_kind(kind) {
            any = true;
            if self.edit.as_ref().is_some_and(|edit| edit.id == Some(group.id)) {
                edit_row(ui, self.edit, commands);
            } else {
                self.group_row(ui, group, commands);
            }
        }
        if self.edit.as_ref().is_some_and(|edit| edit.id.is_none() && edit.kind == kind) {
            any = true;
            edit_row(ui, self.edit, commands);
        }
        any
    }

    /// A collection or category: click to show it, double-click to rename it, right-click for
    /// more, drop assets on it to add them.
    fn group_row(&self, ui: &mut Ui, group: &Group, commands: &mut Vec<GroupCommand>) {
        let glyph = match group.kind {
            GroupKind::Category => Glyph::Dot(theme::category_color(group.color)),
            _ => Glyph::Icon(icon::STACK),
        };
        let count = self.membership.total(group.id, self.kinds);
        let response = Row { glyph, name: &group.name, count, selected: self.viewing == Some(group.id) }.show(ui);
        drop_target(&response, group.id, commands);
        if response.clicked() {
            commands.push(GroupCommand::Show(group.id));
        }
        if response.double_clicked() {
            commands.push(GroupCommand::StartRename(group.id));
        }
        let missing = self.membership.missing.get(&group.id).map_or(0, Vec::len);
        response.context_menu(|ui| group_menu(ui, group, missing, commands));
    }
}

/// Adds dropped assets to the group, and lights the row up while they're held over it.
fn drop_target(response: &Response, id: i64, commands: &mut Vec<GroupCommand>) {
    if response.dnd_hover_payload::<DraggedAssets>().is_some() {
        response.ctx.layer_painter(response.layer_id).rect_stroke(
            response.rect.shrink2(vec2(4.0, 1.0)),
            theme::RADIUS,
            Stroke::new(1.5, theme::ACCENT),
            StrokeKind::Inside,
        );
    }
    if let Some(payload) = response.dnd_release_payload::<DraggedAssets>() {
        commands.push(GroupCommand::Add(id, Targets::Paths(payload.0.clone())));
    }
}

enum Glyph {
    Favorite,
    Icon(&'static str),
    Dot(Color32),
    /// An empty dot: no category.
    Ring,
}

struct Row<'a> {
    glyph: Glyph,
    name: &'a str,
    count: u32,
    selected: bool,
}

impl Row<'_> {
    fn show(self, ui: &mut Ui) -> Response {
        let (rect, response) = widgets::sidebar_row(ui, self.selected);
        let painter = ui.painter();
        let y = rect.center().y;
        let x = rect.left() + 26.0;
        match self.glyph {
            Glyph::Favorite => {
                painter.text(
                    pos2(x, y),
                    Align2::CENTER_CENTER,
                    icon::STAR,
                    FontId::new(theme::size::MEDIUM, theme::filled_icons()),
                    theme::FAVORITE,
                );
            }
            Glyph::Icon(glyph) => {
                let color = if self.selected { theme::ACCENT } else { theme::TEXT_WEAK };
                painter.text(pos2(x, y), Align2::CENTER_CENTER, glyph, FontId::proportional(theme::size::TITLE), color);
            }
            Glyph::Dot(color) => {
                painter.circle_filled(pos2(x, y), 5.0, color);
            }
            Glyph::Ring => {
                painter.circle_stroke(pos2(x, y), 4.5, Stroke::new(1.5, theme::TEXT_WEAK));
            }
        }
        let count = grouped(self.count as usize);
        let count = painter.layout_no_wrap(count, FontId::proportional(theme::size::SMALL), theme::TEXT_FAINT);
        let count_x = rect.right() - 12.0 - count.size().x;
        let color = if self.selected { theme::TEXT } else { theme::TEXT.gamma_multiply(0.88) };
        let name = widgets::elided(
            painter,
            self.name.to_owned(),
            FontId::proportional(theme::size::BODY),
            color,
            (count_x - x - 22.0).max(10.0),
        );
        painter.galley(pos2(x + 14.0, y - name.size().y / 2.0), name, color);
        painter.galley(pos2(count_x, y - count.size().y / 2.0), count, theme::TEXT_FAINT);
        response
    }
}

/// "Collections" or "Categories", with a + to make one.
fn section_heading(ui: &mut Ui, kind: GroupKind, commands: &mut Vec<GroupCommand>) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        widgets::section_label(ui, kind.title());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(4.0);
            let tip = format!("New {}", kind.noun());
            if widgets::icon_button_sized(ui, icon::PLUS, &tip, false, 20.0).clicked() {
                commands.push(GroupCommand::New(kind, Targets::Paths(Vec::new())));
            }
        });
    });
}

/// A group's menu: rename it, a category's colour, take out what's missing, delete it.
fn group_menu(ui: &mut Ui, group: &Group, missing: usize, commands: &mut Vec<GroupCommand>) {
    ui.set_min_width(170.0);
    if ui.button(format!("{}  Rename", icon::PENCIL_SIMPLE)).clicked() {
        commands.push(GroupCommand::StartRename(group.id));
        ui.close();
    }
    if group.kind == GroupKind::Category {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (index, color) in theme::CATEGORY_COLORS.iter().enumerate() {
                if swatch(ui, *color, group.color as usize == index).clicked() {
                    commands.push(GroupCommand::Recolor(group.id, index as u8));
                    ui.close();
                }
            }
        });
    }
    if missing > 0 && ui.button(format!("{}  Remove {missing} missing", icon::BROOM)).clicked() {
        commands.push(GroupCommand::RemoveMissing(group.id));
        ui.close();
    }
    ui.separator();
    if ui.button(format!("{}  Delete {}…", icon::TRASH, group.kind.noun())).clicked() {
        commands.push(GroupCommand::AskDelete(group.id));
        ui.close();
    }
}

/// Under a kind's groups: while assets are dragged, a place to drop them to make a group of them;
/// with no groups yet, what the kind is for.
fn below_groups(ui: &mut Ui, kind: GroupKind, dragging: Option<usize>, any: bool, commands: &mut Vec<GroupCommand>) {
    if let Some(count) = dragging {
        let label = format!("New {} from {}", kind.noun(), plural(count, "asset"));
        let response = dashed_row(ui, &label);
        if let Some(payload) = response.dnd_release_payload::<DraggedAssets>() {
            commands.push(GroupCommand::New(kind, Targets::Paths(payload.0.clone())));
        }
    } else if !any {
        let hint = match kind {
            GroupKind::Collection => "Lists of assets for a purpose. Drag assets here or press +.",
            _ => "Labels for assets, shown on them and found by search.",
        };
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.add(egui::Label::new(RichText::new(hint).size(theme::size::SMALL).color(theme::TEXT_FAINT)).wrap());
        });
    }
}

/// A row for naming a group: Enter keeps the name, Esc cancels.
fn edit_row(ui: &mut Ui, edit: &mut Option<GroupEdit>, commands: &mut Vec<GroupCommand>) {
    let Some(current) = edit.as_mut() else { return };
    let mut finished = None;
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        let field = egui::TextEdit::singleline(&mut current.name)
            .desired_width(ui.available_width() - 10.0)
            .hint_text(if current.kind == GroupKind::Category { "Category name" } else { "Collection name" });
        let response = ui.add(field);
        if !current.focused {
            // Focused with the suggested name selected, so typing replaces it.
            response.request_focus();
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), response.id) {
                let end = egui::text::CCursor::new(current.name.chars().count());
                state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), end)));
                state.store(ui.ctx(), response.id);
            }
            current.focused = true;
        }
        if response.lost_focus() {
            let escaped = ui.input(|input| input.key_pressed(egui::Key::Escape));
            finished = Some(!escaped && !current.name.trim().is_empty());
        }
    });
    match finished {
        Some(true) => {
            if let Some(done) = edit.take() {
                commands.push(GroupCommand::FinishEdit(done));
            }
        }
        Some(false) => *edit = None,
        None => {}
    }
}

/// A drop zone drawn as a dashed outline.
fn dashed_row(ui: &mut Ui, label: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    let row = rect.shrink2(vec2(6.0, 2.0));
    let hovered = response.dnd_hover_payload::<DraggedAssets>().is_some();
    let color = if hovered { theme::ACCENT } else { theme::TEXT_FAINT };
    let painter = ui.painter();
    if hovered {
        painter.rect_filled(row, theme::RADIUS, theme::ACCENT_SOFT);
    }
    let dashes = egui::Shape::dashed_line(
        &[row.left_top(), row.right_top(), row.right_bottom(), row.left_bottom(), row.left_top()],
        Stroke::new(1.0, color),
        4.0,
        3.0,
    );
    painter.extend(dashes);
    painter.text(
        pos2(row.left() + 12.0, row.center().y),
        Align2::LEFT_CENTER,
        format!("{}  {label}", icon::PLUS),
        FontId::proportional(theme::size::CAPTION),
        color,
    );
    response
}

fn swatch(ui: &mut Ui, color: Color32, chosen: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
    ui.painter().circle_filled(rect.center(), 7.0, color);
    if chosen || response.hovered() {
        ui.painter().circle_stroke(rect.center(), 8.5, Stroke::new(1.5, theme::TEXT));
    }
    response
}
