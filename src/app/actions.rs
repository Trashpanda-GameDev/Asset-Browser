//! The keyboard (as the user's shortcuts have it), and what's done to the selected assets: open,
//! show in folder, copy, delete.

use eframe::egui;

use crate::asset::AssetKind;
use crate::library::folder_name;
use crate::ui::asset_actions::AssetAction;
use crate::ui::keymap::{Action, Keymap};
use crate::ui::mode::{Layout as ViewLayout, Mode};
use crate::ui::organize::{GroupCommand, Targets, plural};
use crate::ui::sound::SoundCommand;
use crate::{diag, platform};

use super::BrowserApp;

/// More files than this opened at once is surely a slip: only the focused one opens.
const OPEN_MANY_MAX: usize = 12;

impl BrowserApp {
    pub(super) fn run_action(&mut self, action: AssetAction) {
        if action == AssetAction::Delete {
            self.ask_to_delete_selection();
            return;
        }
        let Some(library) = &self.library else { return };
        let items = self.browsing.selected_in_order();
        match action {
            AssetAction::Delete => {}
            AssetAction::Open => self.open_entries(&items),
            AssetAction::Reveal => {
                if let Some(&first) = items.first()
                    && let Err(err) = platform::reveal(&library.path_of(library.entry(first)))
                {
                    diag::report("Showing the file", err);
                }
            }
            AssetAction::CopyFiles => {
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
            AssetAction::CopyPaths => {
                let paths: Vec<String> =
                    items.iter().map(|&index| platform::native(&library.path_of(library.entry(index)))).collect();
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
            if let Err(err) = platform::open(&library.path_of(library.entry(index))) {
                diag::report("Opening the file", err);
            }
        }
    }

    /// Runs before any widget is drawn and consumes the keys it handles, so a focused button
    /// can't also react to Space or Enter.
    pub(super) fn handle_keys(&mut self, ctx: &egui::Context) {
        // A "Delete?" dialog takes the keys itself (Enter, Esc), and so do the settings and "What's
        // new" (Esc, a shortcut being changed).
        if self.deletion.confirm_files.is_some()
            || self.organizing.confirm_delete.is_some()
            || self.preferences.settings.wants_keys()
            || self.preferences.whats_new.is_some()
        {
            return;
        }
        // The shortcuts are the user's (Settings → Keyboard), so a shortcut's press is matched
        // exactly: Ctrl+Shift+F isn't Ctrl+F.
        let keys = self.preferences.keymap.clone();
        let typing = ctx.egui_wants_keyboard_input();
        self.window_keys(ctx, &keys, typing);
        if typing {
            return;
        }
        self.asset_keys(ctx, &keys);
        let step = self.step_pressed(ctx, &keys);
        let enter = keys.pressed(ctx, Action::Open).is_some();
        let space = keys.pressed(ctx, Action::PlayPause).is_some();
        let escape = keys.pressed(ctx, Action::StopOrClear).is_some();
        if let Some((step, extend)) = step {
            self.step_focus(step, extend);
        }
        let focus = self.browsing.mode_view().selection.focus;
        if enter && let Some(focus) = focus {
            self.open_entries(&[focus]);
        }
        if space {
            self.play_or_pause(focus);
        }
        if escape {
            self.stop_or_clear();
        }
    }

    /// The window's own shortcuts. They work while typing in the search box when they have Ctrl
    /// or Alt; a plain key is typed.
    fn window_keys(&mut self, ctx: &egui::Context, keys: &Keymap, typing: bool) {
        let pressed = |action: Action| (!typing || keys.has_modifier(action)) && keys.pressed(ctx, action).is_some();
        if pressed(Action::Search) {
            self.chrome.focus_search = true;
        }
        for mode in Mode::ALL {
            if pressed(mode.shortcut()) {
                self.set_mode(mode);
            }
        }
        if pressed(Action::Settings) {
            self.preferences.settings.open = true;
        }
        if pressed(Action::TogglePane) {
            self.chrome.show_inspector = !self.chrome.show_inspector;
        }
        for (action, layout) in [(Action::Grid, ViewLayout::Grid), (Action::List, ViewLayout::List)] {
            if pressed(action) {
                self.browsing.set_layout(layout);
            }
        }
    }

    /// What's done to the selected assets.
    fn asset_keys(&mut self, ctx: &egui::Context, keys: &Keymap) {
        let pressed = |action| keys.pressed(ctx, action).is_some();
        if pressed(Action::Delete) {
            self.ask_to_delete_selection();
        }
        if pressed(Action::SelectAll) {
            let view = std::mem::take(&mut self.browsing.view);
            self.browsing.mode_view_mut().selection.all(&view);
            self.browsing.view = view;
        }
        for (action, menu_action) in [
            (Action::CopyFiles, AssetAction::CopyFiles),
            (Action::CopyPaths, AssetAction::CopyPaths),
            (Action::ShowInFolder, AssetAction::Reveal),
        ] {
            if pressed(action) {
                self.run_action(menu_action);
            }
        }
        if pressed(Action::Favorite) {
            self.run_group_command(GroupCommand::ToggleFavorite(Targets::Selection));
        }
    }

    /// The move pressed, in places along the view, and whether it selects as it goes (Shift held).
    fn step_pressed(&self, ctx: &egui::Context, keys: &Keymap) -> Option<(isize, bool)> {
        let columns = self.browsing.columns.max(1) as isize;
        let page = columns * self.browsing.page_rows.max(1) as isize;
        let len = self.browsing.view.len() as isize;
        let mut step = None;
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
            if let Some(shift) = keys.pressed(ctx, action) {
                step = Some((delta, shift));
            }
        }
        step
    }

    /// Moves the focus `step` places along the view; with `extend`, the selection grows to it.
    fn step_focus(&mut self, step: isize, extend: bool) {
        let view = &self.browsing.view;
        if view.is_empty() {
            return;
        }
        let focus = self.browsing.mode_view().selection.focus;
        let current = focus.and_then(|focus| view.iter().position(|&index| index == focus));
        let next = match current {
            Some(position) => (position as isize + step).clamp(0, view.len() as isize - 1) as usize,
            None => 0,
        };
        let index = view[next];
        if current != Some(next) {
            let view = std::mem::take(&mut self.browsing.view);
            let selection = &mut self.browsing.mode_view_mut().selection;
            if extend {
                selection.extend_to(&view, index);
            } else {
                selection.only(index);
            }
            self.browsing.view = view;
            self.focused(index);
        }
        self.browsing.scroll_to_focus = true;
    }

    /// Does what was asked of the player, from the pane, the player bar or the status bar.
    pub(super) fn run_sound_command(&mut self, command: SoundCommand) {
        let Some(library) = &self.library else { return };
        match command {
            SoundCommand::Toggle(index) => self.playback.toggle(library, index),
            SoundCommand::Stop => self.playback.stop(),
            SoundCommand::PlayFrom(index, seconds) => self.playback.play_from(library, index, seconds),
            SoundCommand::Loop(on) => self.playback.set_looping(on),
            SoundCommand::Volume(volume) => self.playback.set_volume(volume),
        }
    }

    /// Plays or pauses the selected sound, or else whatever is playing.
    fn play_or_pause(&mut self, focus: Option<u32>) {
        let Some(library) = &self.library else { return };
        let selected_sound = focus.filter(|&focus| library.entry(focus).kind == AssetKind::Audio);
        let playing = self.playback.playing.filter(|(_, key)| self.playback.now_for(*key).is_some());
        if let Some(index) = selected_sound.or(playing.map(|(index, _)| index)) {
            self.playback.toggle(library, index);
        }
    }

    /// Stops the sound playing, or else clears the selection.
    fn stop_or_clear(&mut self) {
        if self.playback.now().is_some_and(|now| !now.finished) {
            self.playback.stop();
        } else {
            self.browsing.mode_view_mut().selection.clear();
        }
    }
}
