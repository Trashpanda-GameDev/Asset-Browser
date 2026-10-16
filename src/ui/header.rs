//! The window's top: the library's folders, the modes with their counts, the buttons for the
//! settings, the preview pane and the layout, and the search.

use std::path::PathBuf;

use eframe::egui::{self, Align, Frame, Layout, Margin, RichText, Ui, vec2};
use egui_phosphor::regular as icon;

use crate::asset::KindSet;
use crate::library::{Library, folder_name};
use crate::platform;

use super::folders::{self, FolderAction};
use super::format::grouped;
use super::keymap::{self, Action};
use super::mode::{Layout as ViewLayout, Mode};
use super::{theme, widgets};

pub struct HeaderArgs<'a> {
    /// The library's folders, in the order they were added.
    pub roots: &'a [PathBuf],
    /// `None` when there are no folders.
    pub library: Option<&'a Library>,
    /// How many assets of each kind there are where the user is, for the modes' counts.
    pub totals: Option<[u32; 3]>,
    pub mode: Mode,
    pub layout: ViewLayout,
    pub settings_open: bool,
    pub pane_shown: bool,
    /// What's searched for, typed in place.
    pub query: &'a mut String,
    /// Puts the cursor in the search box.
    pub focus_search: bool,
}

/// What the user asked for from the header.
pub enum HeaderCommand {
    Folder(FolderAction),
    Mode(Mode),
    OpenSettings,
    TogglePane,
    Layout(ViewLayout),
    /// What's searched for changed.
    Searched,
}

pub fn show(ui: &mut Ui, mut args: HeaderArgs<'_>) -> Vec<HeaderCommand> {
    let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(12, 8));
    let mut commands = Vec::new();
    egui::Panel::top("header").frame(frame).show(ui, |ui| {
        ui.horizontal(|ui| {
            if let Some(action) = library_button(ui, args.roots, args.library, args.mode.kinds()) {
                commands.push(HeaderCommand::Folder(action));
            }
            ui.add_space(8.0);
            if let Some(mode) = mode_buttons(ui, args.totals, args.mode) {
                commands.push(HeaderCommand::Mode(mode));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| view_buttons(ui, &mut args, &mut commands));
        });
    });
    commands
}

/// The library's folders: click for the list, to go to one, add one or take one out.
fn library_button(ui: &mut Ui, roots: &[PathBuf], library: Option<&Library>, kinds: KindSet) -> Option<FolderAction> {
    let (glyph, name) = match roots {
        [] => (icon::FOLDER_PLUS, "Add a folder".to_owned()),
        [root] => (icon::FOLDER_SIMPLE, folder_name(root)),
        roots => (icon::FOLDERS, format!("{} folders", roots.len())),
    };
    let label = format!("{glyph}  {name}  {}", icon::CARET_DOWN);
    let button = egui::Button::new(RichText::new(label).font(egui::FontId::new(14.0, theme::semibold())))
        .frame_when_inactive(false)
        .min_size(vec2(0.0, 30.0));
    let response = ui.add(button);
    let Some(library) = library else {
        let clicked = response.on_hover_text("Choose a folder of assets, or drop one on the window").clicked();
        return clicked.then_some(FolderAction::AddRoot);
    };
    let paths: Vec<String> = roots.iter().map(|root| platform::native(root)).collect();
    let response =
        response.on_hover_text(format!("{}\nClick to go to a folder, add one or take one out", paths.join("\n")));
    let mut action = None;
    egui::Popup::menu(&response).show(|ui| {
        action = folders::library_menu(ui, library, kinds);
    });
    action
}

/// The modes, each with how many assets it has where the user is. Narrow windows lose the counts
/// first, then the labels.
fn mode_buttons(ui: &mut Ui, totals: Option<[u32; 3]>, current: Mode) -> Option<Mode> {
    let room = ui.available_width();
    let (show_counts, show_labels) = (room > 960.0, room > 780.0);
    let tips: Vec<String> =
        Mode::ALL.iter().map(|mode| keymap::tooltip(ui.ctx(), mode.description(), mode.shortcut())).collect();
    let segments: Vec<widgets::Segment<'_>> = Mode::ALL
        .iter()
        .zip(&tips)
        .map(|(mode, tip)| widgets::Segment {
            glyph: mode.glyph(),
            label: if show_labels { mode.label() } else { "" },
            note: totals.filter(|_| show_counts).map(|totals| grouped(mode.kinds().count(totals) as usize)),
            tooltip: tip,
        })
        .collect();
    widgets::segmented(ui, ui.id().with("modes"), &segments, current.index()).map(|index| Mode::ALL[index])
}

/// Right to left: the settings, the preview pane, list or grid, and the search.
fn view_buttons(ui: &mut Ui, args: &mut HeaderArgs<'_>, commands: &mut Vec<HeaderCommand>) {
    let settings_tip = keymap::tooltip(ui.ctx(), "Settings", Action::Settings);
    if widgets::icon_button(ui, icon::GEAR_SIX, &settings_tip, args.settings_open).clicked() {
        commands.push(HeaderCommand::OpenSettings);
    }
    widgets::divider(ui);
    let pane = if args.pane_shown { "Hide the preview pane" } else { "Show the preview pane" };
    let pane_tip = keymap::tooltip(ui.ctx(), pane, Action::TogglePane);
    if widgets::icon_button(ui, icon::SIDEBAR_SIMPLE, &pane_tip, args.pane_shown).clicked() {
        commands.push(HeaderCommand::TogglePane);
    }
    widgets::divider(ui);
    for (layout, glyph, name, action) in [
        (ViewLayout::List, icon::LIST, "List", Action::List),
        (ViewLayout::Grid, icon::SQUARES_FOUR, "Grid", Action::Grid),
    ] {
        let tip = keymap::tooltip(ui.ctx(), name, action);
        if widgets::icon_button(ui, glyph, &tip, args.layout == layout).clicked() {
            commands.push(HeaderCommand::Layout(layout));
        }
    }
    widgets::divider(ui);
    let width = (ui.available_width() - 8.0).clamp(140.0, 340.0);
    if widgets::search_field(ui, args.query, width, args.focus_search) {
        commands.push(HeaderCommand::Searched);
    }
}
