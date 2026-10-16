//! The groups an asset is in, and putting it in more: the asset menu's items and the preview pane's
//! chips.

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Rect, Response, RichText, Sense, Ui, pos2, vec2};
use egui_phosphor::regular as icon;

use crate::groups::{FAVORITES, Group, GroupKind, Groups, Membership};
use crate::ui::{theme, widgets};

use super::{GroupCommand, Targets, all_in, any_in, favorite_action};

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
    let on = GroupTargets { groups, membership, targets };
    for kind in [GroupKind::Collection, GroupKind::Category] {
        let glyph = if kind == GroupKind::Collection { icon::STACK } else { icon::TAG };
        ui.menu_button(format!("{glyph}  {}", kind.title()), |ui| {
            if let Some(asked) = group_items(ui, &on, kind, true) {
                command = Some(asked);
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
/// The assets something is done to (entry indices), and the groups they can go in.
struct GroupTargets<'a> {
    groups: &'a Groups,
    membership: &'a Membership,
    targets: &'a [u32],
}

/// The groups of `kind` to put the targets in, and "New …", for a menu or a popup. With
/// `ticked`, the groups they're all in already are listed too, ticked: clicking one takes them
/// out.
fn group_items(ui: &mut Ui, on: &GroupTargets<'_>, kind: GroupKind, ticked: bool) -> Option<GroupCommand> {
    ui.set_min_width(180.0);
    let mut command = None;
    let mut any = false;
    for group in on.groups.of_kind(kind) {
        let all = all_in(on.membership, group.id, on.targets);
        if all && !ticked {
            continue;
        }
        any = true;
        if menu_toggle(ui, all, group_color(group), &group.name).clicked() {
            command = Some(if all {
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
    command
}

/// A category's colour; other groups have none.
fn group_color(group: &Group) -> Option<Color32> {
    (group.kind == GroupKind::Category).then(|| theme::category_color(group.color))
}

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
            FontId::proportional(theme::size::BODY),
            theme::ACCENT,
        );
    }
    let mut x = rect.left() + 22.0;
    if let Some(color) = color {
        painter.circle_filled(pos2(x + 4.0, y), 4.5, color);
        x += 14.0;
    }
    painter.text(pos2(x, y), Align2::LEFT_CENTER, name, FontId::proportional(theme::size::BODY), theme::TEXT);
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
            let color = group_color(group).unwrap_or(theme::TEXT_WEAK);
            match chip(ui, &group.name, color, partly) {
                ChipClick::Body => command = Some(GroupCommand::Show(group.id)),
                ChipClick::Remove => command = Some(GroupCommand::Remove(group.id, Targets::Selection)),
                ChipClick::None => {}
            }
        }
        let tip = if kind == GroupKind::Collection { "Add to a collection" } else { "Add a category" };
        let add = widgets::icon_button_sized(ui, icon::PLUS, tip, false, 22.0);
        let on = GroupTargets { groups, membership, targets };
        egui::Popup::menu(&add).show(|ui| {
            if let Some(asked) = group_items(ui, &on, kind, false) {
                command = Some(asked);
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
    let font = FontId::proportional(theme::size::CAPTION);
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
    painter.text(
        remove_rect.center(),
        Align2::CENTER_CENTER,
        icon::X,
        FontId::proportional(theme::size::FINE),
        x_color,
    );
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
