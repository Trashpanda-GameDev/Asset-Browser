//! The settings window: updates, keyboard shortcuts, storage and memory, a page each. It draws and
//! collects what was asked for (`Command`); the app does it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Frame, Layout, Margin, Rect, RichText, Sense, Ui, UiBuilder, Vec2,
    pos2, vec2,
};
use egui_phosphor::regular as icon;

use crate::memory::{self, Shares};
use crate::storage::{Part, Usage};
use crate::ui::keymap::{self, Action, Keymap, Shortcut};
use crate::updates::{RELEASES, Status, Updates};

use super::format::{format_size, grouped, plural};
use super::updates::{self, UpdateAction};
use super::{theme, widgets};

/// The window's size, and the list of pages' width.
const SIZE: Vec2 = vec2(780.0, 540.0);
const SIDEBAR_WIDTH: f32 = 180.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Updates,
    Keyboard,
    Storage,
    Memory,
}

impl Page {
    const ALL: [Page; 4] = [Page::Updates, Page::Keyboard, Page::Storage, Page::Memory];

    fn label(self) -> &'static str {
        match self {
            Page::Updates => "Updates",
            Page::Keyboard => "Keyboard",
            Page::Storage => "Storage",
            Page::Memory => "Memory",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Page::Updates => icon::ARROW_CIRCLE_UP,
            Page::Keyboard => icon::KEYBOARD,
            Page::Storage => icon::DATABASE,
            Page::Memory => icon::MEMORY,
        }
    }
}

/// What the user asked for, for the app to do.
pub enum Command {
    Update(UpdateAction),
    WhatsNew,
    SetShortcut(Action, Option<Shortcut>),
    ResetShortcuts,
    /// Clear these parts of the cache and the user data.
    Clear(Vec<Part>),
    SetMemory(u64),
    /// Show a file in the file manager.
    Reveal(PathBuf),
    /// Open a web page in the browser.
    OpenLink(String),
    /// Measure the files for the Storage page.
    Measure,
}

/// What the window shows, from the app.
pub struct SettingsArgs<'a> {
    pub updates: &'a Updates,
    pub keymap: &'a Keymap,
    pub memory_mb: u64,
    /// Clearing under way.
    pub clearing: bool,
    pub cache_path: &'a Path,
    pub user_path: &'a Path,
    pub log_path: &'a Path,
    /// The last measure of the files (`Command::Measure` asks for one), and whether one is under way.
    pub usage: Option<&'a Result<Usage, String>>,
    pub measuring: bool,
}

pub struct Settings {
    open: bool,
    page: Page,
    /// The shortcut being changed: the next key pressed becomes it.
    capturing: Option<Action>,
    /// Said under the shortcuts after a change took a shortcut from another action.
    keys_note: Option<String>,
    /// Parts ticked to clear, and whether "Clear?" is being asked.
    chosen: HashSet<Part>,
    confirming: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            open: false,
            page: Page::Updates,
            capturing: None,
            keys_note: None,
            chosen: HashSet::new(),
            confirming: false,
        }
    }
}

impl Settings {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens at the page it was on.
    pub fn open(&mut self) {
        self.open = true;
    }

    pub fn open_at(&mut self, page: Page) {
        self.open = true;
        self.page = page;
    }

    /// While it's open the window has the keys (Esc closes it; a shortcut being changed takes the
    /// next one), so none reach the assets behind it.
    pub fn wants_keys(&self) -> bool {
        self.open
    }

    pub fn show(&mut self, ctx: &egui::Context, args: &SettingsArgs<'_>) -> Vec<Command> {
        let mut commands = Vec::new();
        if !self.open {
            return commands;
        }
        // A shortcut being changed takes the next key, Esc included, before the window closes on it.
        if let Some(action) = self.capturing
            && let Some(shortcut) = keymap::capture(ctx)
        {
            commands.push(self.change_shortcut(ctx, args.keymap, action, Some(shortcut)));
            self.capturing = None;
        }
        let modal = widgets::modal_window("settings", 0);
        let response = modal.show(ctx, |ui| {
            // A fixed size, split into the list of pages and the page, each laid out top to bottom.
            let (rect, _) = ui.allocate_exact_size(SIZE, Sense::hover());
            let sidebar = Rect::from_min_size(rect.min, vec2(SIDEBAR_WIDTH, rect.height()));
            // Rounded on the left, as the window is.
            let corners = CornerRadius { nw: theme::CARD_RADIUS, sw: theme::CARD_RADIUS, ne: 0, se: 0 };
            ui.painter().rect_filled(sidebar, corners, theme::BG);
            let top_down = Layout::top_down(Align::Min);
            let mut side = ui.new_child(UiBuilder::new().max_rect(sidebar.shrink2(vec2(10.0, 20.0))).layout(top_down));
            self.sidebar(&mut side);

            let area = Rect::from_min_max(pos2(sidebar.right(), rect.top()), rect.max).shrink2(vec2(24.0, 20.0));
            let mut ui = ui.new_child(UiBuilder::new().max_rect(area).layout(top_down));
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            ui.horizontal(|ui| {
                let title =
                    RichText::new(self.page.label()).font(FontId::new(theme::size::WINDOW_TITLE, theme::semibold()));
                ui.label(title.color(theme::TEXT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, icon::X, "Close (Esc)", false).clicked() {
                        self.close();
                    }
                });
            });
            ui.add_space(4.0);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut ui, |ui| match self.page {
                Page::Updates => updates_page(ui, args, &mut commands),
                Page::Keyboard => self.keyboard_page(ui, args, &mut commands),
                Page::Storage => self.storage_page(ui, args, &mut commands),
                Page::Memory => memory_page(ui, args, &mut commands),
            });
        });
        if response.should_close() {
            self.close();
        }
        commands
    }

    pub fn close(&mut self) {
        self.open = false;
        self.capturing = None;
        self.confirming = false;
        self.keys_note = None;
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
        ui.label(RichText::new("SETTINGS").size(theme::size::SMALL).color(theme::TEXT_FAINT));
        ui.add_space(8.0);
        for page in Page::ALL {
            let selected = self.page == page;
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
            let fill = if selected {
                theme::ACCENT_SOFT
            } else if response.hovered() {
                theme::CARD_HOVER
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, theme::RADIUS, fill);
            let color = if selected { theme::TEXT } else { theme::TEXT_WEAK };
            let text = format!("{}   {}", page.glyph(), page.label());
            ui.painter().text(
                rect.left_center() + vec2(10.0, 0.0),
                egui::Align2::LEFT_CENTER,
                text,
                FontId::proportional(theme::size::MEDIUM),
                color,
            );
            if response.clicked() {
                self.page = page;
                self.capturing = None;
                self.confirming = false;
            }
        }
    }

    fn keyboard_page(&mut self, ui: &mut Ui, args: &SettingsArgs<'_>, commands: &mut Vec<Command>) {
        note(ui, "Click a shortcut, then press the keys for it. Moves select as they go with Shift held too.");
        let mut group = "";
        for action in keymap::Action::ALL {
            if action.group() != group {
                group = action.group();
                ui.add_space(6.0);
                widgets::section_label(ui, group);
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new(action.label()).color(theme::TEXT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    self.shortcut_controls(ui, args, action, commands);
                });
            });
        }
        ui.add_space(10.0);
        if let Some(note_text) = &self.keys_note {
            ui.label(RichText::new(note_text).color(theme::FAVORITE));
        }
        if widgets::button(ui, icon::ARROW_COUNTER_CLOCKWISE, "Reset all").clicked() {
            commands.push(Command::ResetShortcuts);
            self.capturing = None;
            self.keys_note = None;
        }
    }

    /// Right to left: no shortcut, back to the default, and the shortcut to click and change.
    fn shortcut_controls(&mut self, ui: &mut Ui, args: &SettingsArgs<'_>, action: Action, commands: &mut Vec<Command>) {
        let bound = args.keymap.get(action);
        if icon_slot(ui, bound.is_some(), icon::X, "No shortcut") {
            commands.push(self.change_shortcut(ui.ctx(), args.keymap, action, None));
        }
        let tip = match action.default_shortcut() {
            Some(default) => format!("Back to {}", default.label(ui.ctx())),
            None => "Back to none".to_owned(),
        };
        if icon_slot(ui, !args.keymap.is_default(action), icon::ARROW_COUNTER_CLOCKWISE, &tip) {
            let default = action.default_shortcut();
            commands.push(self.change_shortcut(ui.ctx(), args.keymap, action, default));
        }
        self.shortcut_button(ui, action, bound);
    }

    /// The shortcut, or "Press keys…" while it's being changed: click to change it, or to keep it.
    fn shortcut_button(&mut self, ui: &mut Ui, action: Action, bound: Option<Shortcut>) {
        let capturing = self.capturing == Some(action);
        let text = match (capturing, bound) {
            (true, _) => RichText::new("Press keys…").color(theme::ACCENT),
            (false, Some(shortcut)) => RichText::new(shortcut.label(ui.ctx())).monospace().color(theme::TEXT),
            (false, None) => RichText::new("None").color(theme::TEXT_FAINT),
        };
        let fill = if capturing { theme::ACCENT_SOFT } else { theme::CARD };
        let button = egui::Button::new(text).min_size(vec2(150.0, 26.0)).fill(fill);
        let tip = if capturing { "Press the keys, or click to keep it as it is" } else { "Click to change" };
        if ui.add(button).on_hover_text(tip).clicked() {
            self.capturing = if capturing { None } else { Some(action) };
            self.keys_note = None;
        }
    }

    /// Asks for `action` to have `shortcut` (or none), with a note when that takes it from another
    /// action.
    fn change_shortcut(
        &mut self,
        ctx: &egui::Context,
        keymap: &Keymap,
        action: Action,
        shortcut: Option<Shortcut>,
    ) -> Command {
        self.keys_note = shortcut.zip(keymap.taken_from(action, shortcut)).map(|(shortcut, other)| {
            format!("{} was for \u{201c}{}\u{201d}, which has no shortcut now.", shortcut.label(ctx), other.label())
        });
        Command::SetShortcut(action, shortcut)
    }

    fn storage_page(&mut self, ui: &mut Ui, args: &SettingsArgs<'_>, commands: &mut Vec<Command>) {
        // Measured when first shown, and again after clearing.
        if args.usage.is_none() && !args.measuring && !args.clearing {
            commands.push(Command::Measure);
        }
        let usage = match args.usage {
            Some(Ok(usage)) => Some(usage.clone()),
            Some(Err(error)) => {
                ui.colored_label(theme::ERROR, format!("Couldn't measure the files: {error}"));
                None
            }
            None => None,
        };
        let measuring = args.measuring || args.clearing;
        note(ui, "The cache is made again as it's needed; your data can't be. Tick what to clear.");
        ui.add_space(6.0);
        file_row(ui, "Cache", args.cache_path, usage.as_ref().map(|usage| usage.cache_file), commands);
        for part in Part::CACHE {
            self.part_row(ui, part, usage.as_ref(), measuring);
        }
        ui.add_space(10.0);
        file_row(ui, "Your data", args.user_path, usage.as_ref().map(|usage| usage.user_file), commands);
        for part in Part::USER {
            self.part_row(ui, part, usage.as_ref(), measuring);
        }
        ui.add_space(10.0);
        self.clear_controls(ui, args.clearing, usage.as_ref(), commands);
        ui.add_space(14.0);
        log_rows(ui, usage.as_ref(), args.log_path, commands);
        if measuring {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
    }

    /// "Clear selected…", the question that follows it, or how clearing is going.
    fn clear_controls(&mut self, ui: &mut Ui, clearing: bool, usage: Option<&Usage>, commands: &mut Vec<Command>) {
        if clearing {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(theme::size::MEDIUM));
                ui.label(RichText::new("Clearing…").color(theme::TEXT_WEAK));
            });
        } else if self.confirming {
            self.confirm_clear(ui, usage, commands);
        } else {
            let button = egui::Button::new(format!("{}  Clear selected…", icon::BROOM)).min_size(vec2(0.0, 28.0));
            if ui.add_enabled(!self.chosen.is_empty(), button).clicked() {
                self.confirming = true;
            }
        }
    }

    fn part_row(&mut self, ui: &mut Ui, part: Part, usage: Option<&Usage>, measuring: bool) {
        ui.horizontal(|ui| {
            let mut ticked = self.chosen.contains(&part);
            let label =
                RichText::new(part.label()).color(if part.in_user_data() { theme::TEXT } else { theme::TEXT_WEAK });
            if tick(ui, &mut ticked, label) {
                if ticked {
                    self.chosen.insert(part);
                } else {
                    self.chosen.remove(&part);
                }
                self.confirming = false;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                match usage.and_then(|usage| usage.parts.get(&part).map(|measured| (usage, measured))) {
                    Some((usage, measured)) => {
                        let count = measured.count as usize;
                        let count = match part {
                            Part::Thumbnails | Part::SheetChoices => plural(count, "picture"),
                            Part::Waveforms => plural(count, "sound"),
                            Part::ModelFacts => plural(count, "model"),
                            Part::Index => plural(count, "file"),
                            Part::Settings => format!("{} saved", grouped(count)),
                            Part::Groups => {
                                format!("{} · {}", plural(usage.groups as usize, "group"), plural(count, "asset"))
                            }
                            Part::LibraryFolders => plural(count, "folder"),
                        };
                        // Unknown when the file's statistics couldn't be read (it's in the log).
                        let size = measured.bytes.map_or_else(|| "—".to_owned(), format_size);
                        ui.label(RichText::new(size).monospace().color(theme::TEXT_WEAK));
                        ui.label(RichText::new(count).color(theme::TEXT_FAINT));
                    }
                    None if measuring => {
                        ui.add(egui::Spinner::new().size(theme::size::CAPTION));
                    }
                    None => {}
                }
            });
        })
        .response
        .on_hover_text(part.note());
    }

    fn confirm_clear(&mut self, ui: &mut Ui, usage: Option<&Usage>, commands: &mut Vec<Command>) {
        let chosen: Vec<Part> =
            Part::CACHE.into_iter().chain(Part::USER).filter(|part| self.chosen.contains(part)).collect();
        let bytes: u64 = chosen
            .iter()
            .filter_map(|part| usage.and_then(|usage| usage.parts.get(part)))
            .filter_map(|measured| measured.bytes)
            .sum();
        let yours = chosen.iter().any(|part| part.in_user_data());
        Frame::new().fill(theme::CARD).corner_radius(theme::RADIUS).inner_margin(Margin::same(12)).show(ui, |ui| {
            let names: Vec<&str> = chosen.iter().map(|part| part.label()).collect();
            ui.label(RichText::new(format!("Clear {}?", names.join(", "))).color(theme::TEXT));
            for part in &chosen {
                ui.label(
                    RichText::new(format!("{}: {}", part.label(), part.note()))
                        .size(theme::size::CAPTION)
                        .color(theme::TEXT_WEAK),
                );
            }
            if yours {
                ui.label(RichText::new("Your data can't be brought back once it's cleared.").color(theme::ERROR));
            }
            ui.horizontal(|ui| {
                let label = if bytes > 0 { format!("Clear ({})", format_size(bytes)) } else { "Clear".to_owned() };
                let fill = if yours { theme::ERROR } else { theme::ACCENT };
                let clear = egui::Button::new(RichText::new(label).color(theme::ON_ACCENT))
                    .fill(fill)
                    .min_size(vec2(0.0, 28.0));
                if ui.add(clear).clicked() {
                    commands.push(Command::Clear(chosen.clone()));
                    self.chosen.clear();
                    self.confirming = false;
                }
                if widgets::button(ui, "", "Cancel").clicked() {
                    self.confirming = false;
                }
            });
        });
    }
}

fn updates_page(ui: &mut Ui, args: &SettingsArgs<'_>, commands: &mut Vec<Command>) {
    let updates = args.updates;
    version_heading(ui, updates);
    ui.add_space(6.0);
    update_status(ui, updates);
    ui.add_space(6.0);
    update_buttons(ui, updates, commands);
    ui.add_space(10.0);
    note(
        ui,
        "Installed and portable copies check when they start and every 6 hours, and download an update in the background.",
    );
}

/// This copy's version, and whether it's installed, portable or run from a build.
fn version_heading(ui: &mut Ui, updates: &Updates) {
    let kind = match updates.status() {
        Status::NotInstalled => "run from a build",
        _ if updates.is_portable() => "portable",
        _ => "installed",
    };
    ui.label(
        RichText::new(format!("Asset Browser {}", updates.version()))
            .font(FontId::new(theme::size::LARGE, theme::semibold()))
            .color(theme::TEXT),
    );
    ui.label(RichText::new(kind).color(theme::TEXT_FAINT));
}

/// What the updater is doing, or last found.
fn update_status(ui: &mut Ui, updates: &Updates) {
    let checked = updates::checked_ago(updates).map(|ago| format!(", checked {ago}"));
    let (text, color) = match updates.status() {
        Status::NotInstalled => (
            "This copy doesn't update itself: installed and portable copies do, from the releases on GitHub."
                .to_owned(),
            theme::TEXT_WEAK,
        ),
        Status::Checking => ("Checking for updates…".to_owned(), theme::TEXT_WEAK),
        Status::UpToDate => (format!("Up to date{}.", checked.unwrap_or_default()), theme::TEXT_WEAK),
        Status::Downloading { version: new, percent } => (updates::downloading(new, *percent), theme::TEXT_WEAK),
        Status::Ready { version: new } => (updates::ready(new), theme::TEXT),
        Status::Failed { action, error } => (format!("Couldn't {action}: {error}"), theme::ERROR),
    };
    ui.horizontal(|ui| {
        if matches!(updates.status(), Status::Checking | Status::Downloading { .. }) {
            ui.add(egui::Spinner::new().size(theme::size::MEDIUM));
        }
        ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
    });
}

/// "Restart to update" once an update has downloaded, or else "Check for updates"; What's new,
/// and the releases.
fn update_buttons(ui: &mut Ui, updates: &Updates, commands: &mut Vec<Command>) {
    ui.horizontal(|ui| {
        match updates.status() {
            Status::Ready { .. } => {
                let restart = egui::Button::new(
                    RichText::new(format!("{}  Restart to update", icon::ARROW_CLOCKWISE)).color(theme::ON_ACCENT),
                )
                .fill(theme::ACCENT)
                .min_size(vec2(0.0, 28.0));
                if ui.add(restart).clicked() {
                    commands.push(Command::Update(UpdateAction::Restart));
                }
            }
            status => {
                let can_check = matches!(status, Status::UpToDate | Status::Failed { .. });
                let check = egui::Button::new(format!("{}  Check for updates", icon::ARROW_CLOCKWISE))
                    .min_size(vec2(0.0, 28.0));
                if ui.add_enabled(can_check, check).clicked() {
                    commands.push(Command::Update(UpdateAction::Check));
                }
            }
        }
        if widgets::button(ui, icon::SPARKLE, "What's new").on_hover_text("What changed in each version").clicked() {
            commands.push(Command::WhatsNew);
        }
        if widgets::button(ui, icon::GITHUB_LOGO, "Releases").on_hover_text(format!("{RELEASES}/releases")).clicked() {
            commands.push(Command::OpenLink(format!("{RELEASES}/releases")));
        }
    });
}

/// A small icon button, only where it would do something; its room is kept either way, so the
/// rows line up. Whether it was clicked.
fn icon_slot(ui: &mut Ui, shown: bool, glyph: &str, tip: &str) -> bool {
    if shown {
        widgets::icon_button_sized(ui, glyph, tip, false, 24.0).clicked()
    } else {
        ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
        false
    }
}

/// A file's name and size, a button to show it in its folder, and its path.
fn file_row(ui: &mut Ui, label: &str, path: &Path, size: Option<u64>, commands: &mut Vec<Command>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).font(FontId::new(theme::size::MEDIUM, theme::semibold())).color(theme::TEXT));
        if let Some(size) = size {
            ui.label(RichText::new(format_size(size)).color(theme::TEXT_WEAK));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::button(ui, icon::FOLDER_OPEN, "Show in folder").clicked() {
                commands.push(Command::Reveal(path.to_path_buf()));
            }
        });
    });
    ui.label(RichText::new(crate::platform::native(path)).size(theme::size::SMALL).color(theme::TEXT_FAINT));
}

/// The logs, each with its size and folder; the app's own before it's been measured.
fn log_rows(ui: &mut Ui, usage: Option<&Usage>, log_path: &Path, commands: &mut Vec<Command>) {
    widgets::section_label(ui, "Logs");
    match usage {
        Some(usage) => {
            for log in &usage.logs {
                file_row(ui, log.name, &log.path, log.bytes, commands);
                if log.name == "App log" {
                    note(ui, "Errors the app carried on after, and crashes. Attach it when reporting a problem.");
                }
            }
        }
        None => file_row(ui, "App log", log_path, None, commands),
    }
}

fn memory_page(ui: &mut Ui, args: &SettingsArgs<'_>, commands: &mut Vec<Command>) {
    note(
        ui,
        "How much memory the app may use. It shares this out among what it keeps to be quick; what the window and the \
         graphics card need comes first, and opening one very large file can take more for a moment.",
    );
    ui.add_space(6.0);
    let mut gigabytes = args.memory_mb as f32 / 1024.0;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Memory limit").color(theme::TEXT));
        let range = memory::MIN_MB as f32 / 1024.0..=memory::MAX_MB as f32 / 1024.0;
        let slider = egui::Slider::new(&mut gigabytes, range)
            .step_by(0.25)
            .custom_formatter(|value, _| format!("{value:.2} GB"))
            .custom_parser(|text| text.trim().trim_end_matches("GB").trim().parse::<f64>().ok());
        // A slider's length is a spacing setting, not the room it's given.
        ui.spacing_mut().slider_width = 300.0;
        if ui.add(slider).changed() {
            commands.push(Command::SetMemory((gigabytes * 1024.0).round() as u64));
        }
        if args.memory_mb != memory::DEFAULT_MB
            && widgets::icon_button_sized(
                ui,
                icon::ARROW_COUNTER_CLOCKWISE,
                "Back to the default (1.5 GB)",
                false,
                24.0,
            )
            .clicked()
        {
            commands.push(Command::SetMemory(memory::DEFAULT_MB));
        }
    });
    if let Some(in_use) = crate::platform::memory_in_use() {
        ui.label(RichText::new(format!("In use now: {}", format_size(in_use))).color(theme::TEXT_WEAK));
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    }
    ui.add_space(8.0);
    widgets::section_label(ui, "Within it");
    let shares: Shares = memory::shares(args.memory_mb);
    let thumbnails = shares.thumbnails as u64 * 192 * 192 * 4;
    share_row(ui, "Pictures being made into thumbnails, at once", format_size(shares.decoding));
    share_row(
        ui,
        "Thumbnails kept on the graphics card",
        format!("{} ({})", grouped(shares.thumbnails), format_size(thumbnails)),
    );
    share_row(ui, "Sounds read ahead of the selection", format_size(shares.read_ahead));
    share_row(ui, "Textures of 3D models", format_size(shares.model_textures));
    share_row(ui, "An animated GIF's frames", format!("{:.1} megapixels", shares.animation_pixels as f64 / 1e6));
}

/// A line of explanation, faint and wrapping.
fn note(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(theme::size::LABEL).color(theme::TEXT_FAINT)).wrap());
}

/// A tick box that shows on the dark panel (egui's own is faint there), with its label; either
/// toggles it. True when it changed.
fn tick(ui: &mut Ui, ticked: &mut bool, label: RichText) -> bool {
    let (rect, square) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
    let text = ui.add(egui::Label::new(label).sense(Sense::click()));
    let hovered = square.hovered() || text.hovered();
    let painter = ui.painter();
    if *ticked {
        painter.rect_filled(rect, 4.0, theme::ACCENT);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icon::CHECK,
            FontId::proportional(theme::size::BODY),
            theme::ON_ACCENT,
        );
    } else {
        let color = if hovered { theme::TEXT_WEAK } else { theme::TEXT_FAINT };
        painter.rect_stroke(rect.shrink(1.0), 4.0, egui::Stroke::new(1.5, color), egui::StrokeKind::Inside);
    }
    let clicked = square.clicked() || text.clicked();
    if clicked {
        *ticked = !*ticked;
    }
    clicked
}

/// "Name ........ value" with the value at the right.
fn share_row(ui: &mut Ui, name: &str, value: String) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(name).color(theme::TEXT_WEAK));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(value).monospace().color(theme::TEXT));
        });
    });
}
