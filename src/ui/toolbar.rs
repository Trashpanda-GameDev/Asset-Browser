//! Above the browsed assets: where the user is, how many assets there are, and how they're shown.

use eframe::egui::{self, Align, Layout, RichText, Sense, Ui, vec2};
use egui_phosphor::regular as icon;

use crate::groups::{self, GroupKind, Groups, Membership, UNCATEGORIZED};
use crate::library::{Library, SortBy};

use super::display::{self, Display};
use super::format::{grouped, plural};
use super::mode::{Layout as ViewLayout, Mode, ModeView};
use super::organize::GroupCommand;
use super::{theme, widgets};

pub struct ToolbarArgs<'a> {
    pub library: &'a Library,
    /// The folder browsed, when no group is.
    pub selected_dir: u32,
    /// The group browsed, if one is.
    pub group: Option<i64>,
    pub groups: &'a Groups,
    pub membership: &'a Membership,
    pub mode: Mode,
    pub mode_view: &'a ModeView,
    /// The group's own order applies (`Browsing::in_group_order`).
    pub in_group_order: bool,
    /// How many assets are shown.
    pub shown: usize,
    pub autoplay: bool,
    pub display: Display,
}

/// What the user asked for from the toolbar.
pub enum ToolbarCommand {
    /// A folder in the path was clicked.
    GoTo(u32),
    Group(GroupCommand),
    Order(SortBy, bool),
    Tile(f32),
    Autoplay(bool),
    Display(Display),
}

pub fn show(ui: &mut Ui, args: &ToolbarArgs<'_>) -> Vec<ToolbarCommand> {
    let mut commands = Vec::new();
    ui.horizontal(|ui| {
        ui.set_min_height(32.0);
        ui.spacing_mut().item_spacing.x = 2.0;
        match args.group {
            Some(id) => group_title(ui, id, args.groups),
            None => commands.extend(crumbs(ui, args.library, args.selected_dir).map(ToolbarCommand::GoTo)),
        }
        ui.add_space(10.0);
        commands.extend(counts(ui, args));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| view_options(ui, args, &mut commands));
    });
    commands
}

/// The group's glyph and name.
fn group_title(ui: &mut Ui, id: i64, groups: &Groups) {
    // `None` for the kind is Uncategorized.
    let (kind, name, color) = match id {
        UNCATEGORIZED => (None, groups::UNCATEGORIZED_NAME, 0),
        id => match groups.get(id) {
            Some(group) => (Some(group.kind), group.name.as_str(), group.color),
            None => return,
        },
    };
    match kind {
        Some(GroupKind::Favorites) => {
            ui.label(RichText::new(icon::STAR).family(theme::filled_icons()).color(theme::FAVORITE));
        }
        Some(GroupKind::Collection) => {
            ui.label(RichText::new(icon::STACK).size(14.0).color(theme::TEXT_WEAK));
        }
        Some(GroupKind::Category) => {
            let (dot, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            ui.painter().circle_filled(dot.center(), 5.0, theme::category_color(color));
        }
        None => {
            let (dot, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            ui.painter().circle_stroke(dot.center(), 4.5, egui::Stroke::new(1.5, theme::TEXT_WEAK));
        }
    }
    ui.add_space(4.0);
    ui.label(RichText::new(name).font(egui::FontId::new(13.5, theme::semibold())).color(theme::TEXT));
}

/// The folder's path from the top of its library folder; a folder clicked in it, to go to.
fn crumbs(ui: &mut Ui, library: &Library, dir: u32) -> Option<u32> {
    // "All folders" only with several.
    let chain: Vec<u32> = library.dir_chain(dir).into_iter().filter(|&dir| dir >= library.home()).collect();
    let mut go_to = None;
    for (i, &dir) in chain.iter().enumerate() {
        if i > 0 {
            ui.label(RichText::new(icon::CARET_RIGHT).size(11.0).color(theme::TEXT_FAINT));
        }
        let name = &library.dir(dir).name;
        let last = i + 1 == chain.len();
        let text = if last {
            RichText::new(name).font(egui::FontId::new(13.5, theme::semibold())).color(theme::TEXT)
        } else {
            RichText::new(name).size(13.0).color(theme::TEXT_WEAK)
        };
        if ui.add(egui::Label::new(text).sense(Sense::click())).clicked() && !last {
            go_to = Some(dir);
        }
    }
    go_to
}

/// How many assets are shown and selected, and how many of the group's are elsewhere or missing
/// (click to take the missing ones out).
fn counts(ui: &mut Ui, args: &ToolbarArgs<'_>) -> Option<ToolbarCommand> {
    let missing = args.group.and_then(|id| args.membership.missing.get(&id)).map_or(0, Vec::len);
    let elsewhere = args.group.and_then(|id| args.membership.elsewhere.get(&id)).copied().unwrap_or(0);
    let mut count = plural(args.shown, "item");
    let selected = args.mode_view.selection.len();
    if selected > 1 {
        count.push_str(&format!(" · {} selected", grouped(selected)));
    }
    if elsewhere > 0 {
        let outside = match args.library.roots() {
            [root] => root.name.clone(),
            _ => "the library's folders".to_owned(),
        };
        count.push_str(&format!(" · {} outside {outside}", grouped(elsewhere)));
    }
    ui.label(RichText::new(count).size(12.0).color(theme::TEXT_FAINT));
    let id = args.group.filter(|_| missing > 0)?;
    ui.add_space(6.0);
    let text = RichText::new(format!("{}  {} missing", icon::WARNING, grouped(missing))).size(12.0).color(theme::ERROR);
    let response = ui.add(egui::Label::new(text).sense(Sense::click()));
    let clicked =
        response.on_hover_text("Moved, renamed or deleted since they were added. Click to take them out.").clicked();
    clicked.then_some(ToolbarCommand::Group(GroupCommand::RemoveMissing(id)))
}

/// Right to left: the tile size in a grid, the order, and the mode's own options.
fn view_options(ui: &mut Ui, args: &ToolbarArgs<'_>, commands: &mut Vec<ToolbarCommand>) {
    ui.spacing_mut().item_spacing.x = 6.0;
    let view = args.mode_view;
    if view.layout == ViewLayout::Grid {
        let mut tile = view.tile;
        let slider = egui::Slider::new(&mut tile, 64.0..=256.0).show_value(false);
        if ui.add_sized(vec2(96.0, 20.0), slider).on_hover_text("Tile size").changed() {
            commands.push(ToolbarCommand::Tile(tile));
        }
        ui.label(RichText::new(icon::SQUARES_FOUR).size(14.0).color(theme::TEXT_FAINT));
    }
    if let Some((sort, descending)) = order_menu(ui, view, args.in_group_order) {
        commands.push(ToolbarCommand::Order(sort, descending));
    }
    commands.extend(mode_options(ui, args));
}

/// What the assets are ordered by, and which way; what was picked.
fn order_menu(ui: &mut Ui, view: &ModeView, in_group: bool) -> Option<(SortBy, bool)> {
    let (sort, descending) = view.order(in_group);
    let arrow = if descending { icon::SORT_DESCENDING } else { icon::SORT_ASCENDING };
    let options: &[SortBy] = if in_group { &SortBy::IN_GROUPS } else { &SortBy::ALL };
    let mut picked = None;
    let menu = ui.menu_button(format!("{arrow}  {}", sort.label()), |ui| {
        ui.set_min_width(150.0);
        for &option in options {
            if ui.selectable_label(option == sort, option.label()).clicked() {
                picked = Some((option, option.starts_descending()));
                ui.close();
            }
        }
        ui.separator();
        for (down, glyph, label) in
            [(false, icon::SORT_ASCENDING, "Ascending"), (true, icon::SORT_DESCENDING, "Descending")]
        {
            if ui.selectable_label(descending == down, format!("{glyph}  {label}")).clicked() {
                picked = Some((sort, down));
                ui.close();
            }
        }
    });
    menu.response.on_hover_text("Order");
    picked
}

/// Each mode's own option: auto-play for sounds, the backdrop for pictures, the turntable for
/// models.
fn mode_options(ui: &mut Ui, args: &ToolbarArgs<'_>) -> Option<ToolbarCommand> {
    match args.mode {
        Mode::All | Mode::Sounds => {
            let mut autoplay = args.autoplay;
            let tip = "Play sounds as soon as they're selected, by click or arrow key";
            let chip = widgets::toggle_chip(ui, icon::PLAY_CIRCLE, "Auto-play", &mut autoplay, tip);
            chip.changed().then_some(ToolbarCommand::Autoplay(autoplay))
        }
        Mode::Images => {
            let mut backdrop = args.display.backdrop;
            display::backdrop_buttons(ui, &mut backdrop);
            (backdrop != args.display.backdrop).then_some(ToolbarCommand::Display(Display { backdrop, ..args.display }))
        }
        Mode::Models => {
            let mut turntable = args.display.turntable;
            let tip = "Turn the model in the preview pane slowly";
            let chip = widgets::toggle_chip(ui, icon::ARROWS_CLOCKWISE, "Turntable", &mut turntable, tip);
            chip.changed().then_some(ToolbarCommand::Display(Display { turntable, ..args.display }))
        }
    }
}
