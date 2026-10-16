//! The Keyboard page: every shortcut, to change, clear or put back.

use eframe::egui::{self, Align, Layout, RichText, Sense, Ui, Vec2, vec2};
use egui_phosphor::regular as icon;

use crate::ui::keymap::{self, Action, Keymap, Shortcut};
use crate::ui::{theme, widgets};

use super::{Command, Settings, SettingsArgs, note};

impl Settings {
    pub(super) fn keyboard_page(&mut self, ui: &mut Ui, args: &SettingsArgs<'_>, commands: &mut Vec<Command>) {
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
    pub(super) fn change_shortcut(
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
