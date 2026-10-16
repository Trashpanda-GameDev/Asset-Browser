//! The organizing parts of the window: Favorites, collections and categories in the sidebar, the
//! menu items and chips that put assets in them, and dragging assets onto them. None of it
//! changes anything itself: it reports `GroupCommand`s, which the app applies (`groups::Groups`).

use std::path::PathBuf;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use egui_phosphor::regular as icon;

use crate::asset::KindSet;
use crate::groups::{FAVORITES, Group, GroupKind, Groups, Membership, UNCATEGORIZED, UNCATEGORIZED_NAME};

use super::format::grouped;
use super::{theme, widgets};

const ROW_HEIGHT: f32 = 26.0;

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
    #[expect(clippy::cognitive_complexity, clippy::too_many_lines, reason = "refactor: split into named steps")]
    pub fn show(self, ui: &mut Ui) -> Vec<GroupCommand> {
        let mut commands = Vec::new();
        let dragging = egui::DragAndDrop::payload::<DraggedAssets>(ui.ctx()).map(|payload| payload.0.len());

        if let Some(favorites) = self.groups.get(FAVORITES) {
            let count = self.membership.total(FAVORITES, self.kinds);
            let row =
                Row { glyph: Glyph::Favorite, name: &favorites.name, count, selected: self.viewing == Some(FAVORITES) };
            let response = row.show(ui);
            drop_target(&response, FAVORITES, &mut commands);
            if response.clicked() {
                commands.push(GroupCommand::Show(FAVORITES));
            }
            response.on_hover_text("Starred assets (F stars the selection)");
        }

        for kind in [GroupKind::Collection, GroupKind::Category] {
            ui.add_space(10.0);
            let title = if kind == GroupKind::Collection { "Collections" } else { "Categories" };
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                widgets::section_label(ui, title);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(4.0);
                    let tip = if kind == GroupKind::Collection { "New collection" } else { "New category" };
                    if widgets::icon_button_sized(ui, icon::PLUS, tip, false, 20.0).clicked() {
                        commands.push(GroupCommand::New(kind, Targets::Paths(Vec::new())));
                    }
                });
            });
            if kind == GroupKind::Category {
                // Always there: whatever no category holds, to work through.
                let count = self.membership.total(UNCATEGORIZED, self.kinds);
                let selected = self.viewing == Some(UNCATEGORIZED);
                let row = Row { glyph: Glyph::Ring, name: UNCATEGORIZED_NAME, count, selected };
                let response = row.show(ui);
                if response.clicked() {
                    commands.push(GroupCommand::Show(UNCATEGORIZED));
                }
                response.on_hover_text("Assets in no category");
            }
            let mut any = false;
            for group in self.groups.of_kind(kind) {
                any = true;
                if self.edit.as_ref().is_some_and(|edit| edit.id == Some(group.id)) {
                    edit_row(ui, self.edit, &mut commands);
                    continue;
                }
                let glyph = match kind {
                    GroupKind::Category => Glyph::Dot(theme::category_color(group.color)),
                    _ => Glyph::Icon(icon::STACK),
                };
                let count = self.membership.total(group.id, self.kinds);
                let row = Row { glyph, name: &group.name, count, selected: self.viewing == Some(group.id) };
                let response = row.show(ui);
                drop_target(&response, group.id, &mut commands);
                if response.clicked() {
                    commands.push(GroupCommand::Show(group.id));
                }
                if response.double_clicked() {
                    commands.push(GroupCommand::StartRename(group.id));
                }
                let missing = self.membership.missing.get(&group.id).map_or(0, Vec::len);
                response.context_menu(|ui| {
                    ui.set_min_width(170.0);
                    if ui.button(format!("{}  Rename", icon::PENCIL_SIMPLE)).clicked() {
                        commands.push(GroupCommand::StartRename(group.id));
                        ui.close();
                    }
                    if kind == GroupKind::Category {
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
                    if ui.button(format!("{}  Delete {}…", icon::TRASH, kind.noun())).clicked() {
                        commands.push(GroupCommand::AskDelete(group.id));
                        ui.close();
                    }
                });
            }
            if self.edit.as_ref().is_some_and(|edit| edit.id.is_none() && edit.kind == kind) {
                any = true;
                edit_row(ui, self.edit, &mut commands);
            }
            if let Some(count) = dragging {
                // Dropping here makes a new group from the dragged assets.
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
                    ui.add(egui::Label::new(RichText::new(hint).size(11.5).color(theme::TEXT_FAINT)).wrap());
                });
            }
        }
        commands
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
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
        let painter = ui.painter();
        let row = rect.shrink2(vec2(4.0, 1.0));
        if self.selected {
            painter.rect_filled(row, theme::RADIUS, theme::ACCENT_SOFT);
        } else if response.hovered() {
            painter.rect_filled(row, theme::RADIUS, theme::CARD_HOVER);
        }
        let y = rect.center().y;
        let x = rect.left() + 26.0;
        match self.glyph {
            Glyph::Favorite => {
                painter.text(
                    pos2(x, y),
                    Align2::CENTER_CENTER,
                    icon::STAR,
                    FontId::new(14.0, theme::filled_icons()),
                    theme::FAVORITE,
                );
            }
            Glyph::Icon(glyph) => {
                let color = if self.selected { theme::ACCENT } else { theme::TEXT_WEAK };
                painter.text(pos2(x, y), Align2::CENTER_CENTER, glyph, FontId::proportional(15.0), color);
            }
            Glyph::Dot(color) => {
                painter.circle_filled(pos2(x, y), 5.0, color);
            }
            Glyph::Ring => {
                painter.circle_stroke(pos2(x, y), 4.5, Stroke::new(1.5, theme::TEXT_WEAK));
            }
        }
        let count = grouped(self.count as usize);
        let count = painter.layout_no_wrap(count, FontId::proportional(11.5), theme::TEXT_FAINT);
        let count_x = rect.right() - 12.0 - count.size().x;
        let color = if self.selected { theme::TEXT } else { theme::TEXT.gamma_multiply(0.88) };
        let mut job = egui::text::LayoutJob::simple_singleline(self.name.to_owned(), FontId::proportional(13.0), color);
        job.wrap = egui::text::TextWrapping {
            max_width: (count_x - x - 22.0).max(10.0),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let name = painter.layout_job(job);
        painter.galley(pos2(x + 14.0, y - name.size().y / 2.0), name, color);
        painter.galley(pos2(count_x, y - count.size().y / 2.0), count, theme::TEXT_FAINT);
        response
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
        FontId::proportional(12.0),
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

/// The organizing items of an asset's menu. `targets` are the assets it acts on (entry indices);
/// `viewing` is the group being browsed, if any.
pub fn menu_items(
    ui: &mut Ui,
    groups: &Groups,
    membership: &Membership,
    targets: &[u32],
    viewing: Option<i64>,
) -> Option<GroupCommand> {
    let mut command = None;
    let favorite = all_in(membership, FAVORITES, targets);
    let label = favorite_action(favorite);
    let star =
        RichText::new(icon::STAR).family(if favorite { theme::filled_icons() } else { egui::FontFamily::Proportional });
    let clicked = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add(egui::Button::new((star.color(theme::FAVORITE), format!("  {label}"))).frame_when_inactive(false))
        })
        .inner
        .clicked();
    if clicked {
        command = Some(GroupCommand::ToggleFavorite(Targets::Selection));
        ui.close();
    }
    for kind in [GroupKind::Collection, GroupKind::Category] {
        let (glyph, title) =
            if kind == GroupKind::Collection { (icon::STACK, "Collections") } else { (icon::TAG, "Categories") };
        ui.menu_button(format!("{glyph}  {title}"), |ui| {
            ui.set_min_width(180.0);
            let mut any = false;
            for group in groups.of_kind(kind) {
                any = true;
                let on = all_in(membership, group.id, targets);
                let color = (kind == GroupKind::Category).then(|| theme::category_color(group.color));
                if menu_toggle(ui, on, color, &group.name).clicked() {
                    command = Some(if on {
                        GroupCommand::Remove(group.id, Targets::Selection)
                    } else {
                        GroupCommand::Add(group.id, Targets::Selection)
                    });
                    ui.close();
                }
            }
            if any {
                ui.separator();
            }
            if ui.button(format!("{}  New {}…", icon::PLUS, kind.noun())).clicked() {
                command = Some(GroupCommand::New(kind, Targets::Selection));
                ui.close();
            }
        });
    }
    if let Some(id) = viewing
        && let Some(group) = groups.get(id)
    {
        ui.separator();
        if ui
            .button(format!("{}  Remove from {}", icon::MINUS_CIRCLE, group.name))
            .on_hover_text("The files stay where they are")
            .clicked()
        {
            command = Some(GroupCommand::Remove(id, Targets::Selection));
            ui.close();
        }
    }
    command
}

/// A menu row with a check mark, an optional colour dot and a name.
fn menu_toggle(ui: &mut Ui, on: bool, color: Option<Color32>, name: &str) -> Response {
    let height = 24.0;
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width().max(160.0), height), Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, theme::RADIUS, theme::CARD_HOVER);
    }
    let y = rect.center().y;
    if on {
        painter.text(
            pos2(rect.left() + 10.0, y),
            Align2::CENTER_CENTER,
            icon::CHECK,
            FontId::proportional(13.0),
            theme::ACCENT,
        );
    }
    let mut x = rect.left() + 22.0;
    if let Some(color) = color {
        painter.circle_filled(pos2(x + 4.0, y), 4.5, color);
        x += 14.0;
    }
    painter.text(pos2(x, y), Align2::LEFT_CENTER, name, FontId::proportional(13.0), theme::TEXT);
    response
}

/// The inspector's line of chips for one kind of group: the groups the targets are in (dimmed
/// when only some of them are), each to click through to or take them out of, and a + to add.
pub fn chips(
    ui: &mut Ui,
    groups: &Groups,
    membership: &Membership,
    targets: &[u32],
    kind: GroupKind,
) -> Option<GroupCommand> {
    let mut command = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for group in groups.of_kind(kind) {
            if !any_in(membership, group.id, targets) {
                continue;
            }
            let partly = !all_in(membership, group.id, targets);
            let color = match kind {
                GroupKind::Category => theme::category_color(group.color),
                _ => theme::TEXT_WEAK,
            };
            match chip(ui, &group.name, color, partly) {
                ChipClick::Body => command = Some(GroupCommand::Show(group.id)),
                ChipClick::Remove => command = Some(GroupCommand::Remove(group.id, Targets::Selection)),
                ChipClick::None => {}
            }
        }
        let tip = if kind == GroupKind::Collection { "Add to a collection" } else { "Add a category" };
        let add = widgets::icon_button_sized(ui, icon::PLUS, tip, false, 22.0);
        egui::Popup::menu(&add).show(|ui| {
            ui.set_min_width(180.0);
            let mut any = false;
            for group in groups.of_kind(kind).filter(|group| !all_in(membership, group.id, targets)) {
                any = true;
                let color = (kind == GroupKind::Category).then(|| theme::category_color(group.color));
                if menu_toggle(ui, false, color, &group.name).clicked() {
                    command = Some(GroupCommand::Add(group.id, Targets::Selection));
                    ui.close();
                }
            }
            if any {
                ui.separator();
            }
            if ui.button(format!("{}  New {}…", icon::PLUS, kind.noun())).clicked() {
                command = Some(GroupCommand::New(kind, Targets::Selection));
                ui.close();
            }
        });
    });
    command
}

enum ChipClick {
    None,
    Body,
    Remove,
}

fn chip(ui: &mut Ui, name: &str, color: Color32, partly: bool) -> ChipClick {
    let font = FontId::proportional(12.0);
    let galley = ui.painter().layout_no_wrap(name.to_owned(), font, theme::TEXT);
    let size = vec2(galley.size().x + 44.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let remove_rect = Rect::from_center_size(pos2(rect.right() - 11.0, rect.center().y), vec2(18.0, 18.0));
    let remove = ui.interact(remove_rect, response.id.with("remove"), Sense::click());
    let painter = ui.painter();
    let alpha = if partly { 0.5 } else { 1.0 };
    let fill = if response.hovered() { theme::CARD_HOVER } else { theme::CARD };
    painter.rect_filled(rect, CornerRadius::same(11), fill);
    painter.circle_filled(pos2(rect.left() + 11.0, rect.center().y), 4.0, color.gamma_multiply(alpha));
    painter.galley(
        pos2(rect.left() + 20.0, rect.center().y - galley.size().y / 2.0),
        galley,
        theme::TEXT.gamma_multiply(alpha),
    );
    let x_color = if remove.hovered() { theme::TEXT } else { theme::TEXT_FAINT };
    painter.text(remove_rect.center(), Align2::CENTER_CENTER, icon::X, FontId::proportional(11.0), x_color);
    if remove.clicked() {
        ChipClick::Remove
    } else if response.clicked() {
        ChipClick::Body
    } else {
        if partly {
            response.on_hover_text("Only some of the selected assets are in it");
        }
        ChipClick::None
    }
}

/// The pill that follows the pointer while assets are dragged.
pub fn drag_indicator(ctx: &egui::Context) {
    let Some(payload) = egui::DragAndDrop::payload::<DraggedAssets>(ctx) else { return };
    let Some(pointer) = ctx.pointer_latest_pos() else { return };
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("dragged-assets")));
    let text = format!("{}  {}", icon::STACK, plural(payload.0.len(), "asset"));
    let galley = painter.layout_no_wrap(text, FontId::proportional(12.5), theme::TEXT);
    let rect = Rect::from_min_size(pointer + vec2(14.0, 10.0), galley.size() + vec2(20.0, 10.0));
    painter.rect_filled(rect, CornerRadius::same(13), theme::ACCENT.gamma_multiply(0.9));
    painter.galley(rect.min + vec2(10.0, 5.0), galley, theme::TEXT);
}

pub fn plural(count: usize, noun: &str) -> String {
    if count == 1 { format!("1 {noun}") } else { format!("{} {noun}s", grouped(count)) }
}
