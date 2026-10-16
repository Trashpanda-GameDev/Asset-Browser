//! The settings window: updates, keyboard shortcuts, storage and memory, a page each. It draws and
//! collects what was asked for (`Command`); the app does it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Layout, Rect, RichText, Sense, Ui, UiBuilder, Vec2, pos2, vec2,
};
use egui_phosphor::regular as icon;

use crate::storage::{Part, Usage};
use crate::ui::keymap::{self, Action, Keymap, Shortcut};
use crate::updates::Updates;

use super::updates::UpdateAction;
use super::{theme, widgets};

mod keyboard_page;
mod memory_page;
mod storage_page;
mod updates_page;

use memory_page::memory_page;
use updates_page::updates_page;

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
