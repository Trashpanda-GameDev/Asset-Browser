//! The keyboard (as the user's shortcuts have it), and what's done to the selected assets: open,
//! show in folder, copy, delete.

use eframe::egui;

use crate::asset::AssetKind;
use crate::keymap::Action;
use crate::library::folder_name;
use crate::ui::browse::MenuAction;
use crate::ui::mode::{Layout as ViewLayout, Mode};
use crate::ui::organize::{GroupCommand, Targets, plural};
use crate::{diag, platform};

use super::BrowserApp;

/// More files than this opened at once is surely a slip: only the focused one opens.
const OPEN_MANY_MAX: usize = 12;

impl BrowserApp {
    pub(super) fn run_action(&mut self, action: MenuAction) {
        if action == MenuAction::Delete {
            self.ask_to_delete_selection();
            return;
        }
        let Some(library) = &self.library else { return };
        let items = self.selected_in_order();
        match action {
            MenuAction::Delete => {}
            MenuAction::Open => self.open_entries(&items),
            MenuAction::Reveal => {
                if let Some(&first) = items.first() {
                    let _ = platform::reveal(&library.path_of(&library.entries[first as usize]));
                }
            }
            MenuAction::CopyFiles => {
                let paths = self.paths_of(&items);
                let copied = match paths.as_slice() {
                    [] => return,
                    [path] => format!("Copied \u{201c}{}\u{201d}", folder_name(path)),
                    _ => format!("Copied {}", plural(paths.len(), "file")),
                };
                match platform::copy_files(&paths) {
                    Ok(()) => self.notify(copied),
                    Err(err) => diag::report("Copying the files", err),
                }
            }
            MenuAction::CopyPaths => {
                let paths: Vec<String> = items
                    .iter()
                    .map(|&index| platform::native(&library.path_of(&library.entries[index as usize])))
                    .collect();
                if !paths.is_empty() {
                    self.ctx.copy_text(paths.join("\n"));
                }
            }
        }
    }

    pub(super) fn open_entries(&self, items: &[u32]) {
        let Some(library) = &self.library else { return };
        let items = if items.len() > OPEN_MANY_MAX { &items[..1] } else { items };
        for &index in items {
            let _ = platform::open(&library.path_of(&library.entries[index as usize]));
        }
    }

    /// Runs before any widget is drawn and consumes the keys it handles, so a focused button
    /// can't also react to Space or Enter.
    pub(super) fn handle_keys(&mut self, ctx: &egui::Context) {
        // A "Delete?" dialog takes the keys itself (Enter, Esc), and so do the settings and "What's
        // new" (Esc, a shortcut being changed).
        if self.confirm_files.is_some()
            || self.confirm_delete.is_some()
            || self.settings.wants_keys()
            || self.whats_new.is_some()
        {
            return;
        }
        // The shortcuts are the user's (Settings → Keyboard), so a shortcut's press is matched
        // exactly: Ctrl+Shift+F isn't Ctrl+F.
        let keys = self.keymap.clone();
        let typing = ctx.egui_wants_keyboard_input();
        // The window's own shortcuts work while typing in the search box when they have Ctrl or
        // Alt; a plain key is typed.
        let window = |action: Action| (!typing || keys.has_modifier(action)) && keys.pressed(ctx, action).is_some();
        if window(Action::Search) {
            self.focus_search = true;
        }
        for (action, mode) in
            [Action::ModeAll, Action::ModeImages, Action::ModeSounds, Action::ModeModels].into_iter().zip(Mode::ALL)
        {
            if window(action) {
                self.set_mode(mode);
            }
        }
        if window(Action::Settings) {
            self.settings.open = true;
        }
        if window(Action::TogglePane) {
            self.show_inspector = !self.show_inspector;
        }
        for (action, layout) in [(Action::Grid, ViewLayout::Grid), (Action::List, ViewLayout::List)] {
            if window(action) {
                self.mode_view_mut().layout = layout;
                self.scroll_to_focus = true;
            }
        }
        if typing {
            return;
        }
        let pressed = |action| keys.pressed(ctx, action);
        if pressed(Action::Delete).is_some() {
            self.ask_to_delete_selection();
        }
        if pressed(Action::SelectAll).is_some() {
            let view = std::mem::take(&mut self.view);
            self.mode_view_mut().selection.all(&view);
            self.view = view;
        }
        if pressed(Action::CopyFiles).is_some() {
            self.run_action(MenuAction::CopyFiles);
        }
        if pressed(Action::CopyPaths).is_some() {
            self.run_action(MenuAction::CopyPaths);
        }
        if pressed(Action::ShowInFolder).is_some() {
            self.run_action(MenuAction::Reveal);
        }
        if pressed(Action::Favorite).is_some() {
            self.run_group_command(GroupCommand::ToggleFavorite(Targets::Selection));
        }

        let columns = self.columns.max(1) as isize;
        let page = columns * self.page_rows.max(1) as isize;
        let len = self.view.len() as isize;
        let mut step = None;
        // Holding Shift on a move selects as it goes.
        let mut shift = false;
        for (action, delta) in [
            (Action::Left, -1),
            (Action::Right, 1),
            (Action::Up, -columns),
            (Action::Down, columns),
            (Action::PageUp, -page),
            (Action::PageDown, page),
            (Action::First, -len),
            (Action::Last, len),
        ] {
            if let Some(held) = pressed(action) {
                step = Some(delta);
                shift = held;
            }
        }
        let enter = pressed(Action::Open).is_some();
        let space = pressed(Action::PlayPause).is_some();
        let escape = pressed(Action::StopOrClear).is_some();

        if let Some(step) = step
            && !self.view.is_empty()
        {
            let focus = self.mode_view().selection.focus;
            let current = focus.and_then(|focus| self.view.iter().position(|&index| index == focus));
            let next = match current {
                Some(position) => (position as isize + step).clamp(0, len - 1) as usize,
                None => 0,
            };
            let index = self.view[next];
            if current != Some(next) {
                let view = std::mem::take(&mut self.view);
                let selection = &mut self.mode_view_mut().selection;
                if shift {
                    selection.extend_to(&view, index);
                } else {
                    selection.only(index);
                }
                self.view = view;
                self.focused(index);
            }
            self.scroll_to_focus = true;
        }
        let focus = self.mode_view().selection.focus;
        if enter && let Some(focus) = focus {
            self.open_entries(&[focus]);
        }
        if space && let Some(library) = &self.library {
            // The selected sound, or else whatever is playing.
            let selected_sound = focus.filter(|&focus| library.entries[focus as usize].kind == AssetKind::Audio);
            let playing = self.sound.playing.filter(|(_, key)| self.sound.now_for(*key).is_some());
            if let Some(index) = selected_sound.or(playing.map(|(index, _)| index)) {
                self.sound.toggle(library, index);
            }
        }
        if escape {
            if self.sound.now().is_some_and(|now| !now.finished) {
                self.sound.stop();
            } else {
                self.mode_view_mut().selection.clear();
            }
        }
    }
}
