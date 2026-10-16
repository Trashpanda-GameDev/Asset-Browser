//! The keyboard shortcuts, which the user can change (Settings → Keyboard). Each action has one
//! shortcut or none; they're saved in the settings, only where they differ from the defaults, so a
//! default changed in a later version still reaches everyone who kept it.

use std::collections::HashMap;

use eframe::egui::{self, Event, Key, KeyboardShortcut, Modifiers};

/// Everything a key can do in the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Search,
    ModeAll,
    ModeImages,
    ModeSounds,
    ModeModels,
    Settings,
    TogglePane,
    Grid,
    List,
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    SelectAll,
    Open,
    ShowInFolder,
    CopyFiles,
    CopyPaths,
    Favorite,
    Delete,
    PlayPause,
    StopOrClear,
}

impl Action {
    /// In the order the settings list them.
    pub const ALL: [Action; 26] = [
        Action::Search,
        Action::ModeAll,
        Action::ModeImages,
        Action::ModeSounds,
        Action::ModeModels,
        Action::Settings,
        Action::TogglePane,
        Action::Grid,
        Action::List,
        Action::Left,
        Action::Right,
        Action::Up,
        Action::Down,
        Action::PageUp,
        Action::PageDown,
        Action::First,
        Action::Last,
        Action::SelectAll,
        Action::Open,
        Action::ShowInFolder,
        Action::CopyFiles,
        Action::CopyPaths,
        Action::Favorite,
        Action::Delete,
        Action::PlayPause,
        Action::StopOrClear,
    ];

    /// Its settings key, `keys.<id>`; never changed once released.
    pub fn id(self) -> &'static str {
        match self {
            Action::Search => "search",
            Action::ModeAll => "mode_all",
            Action::ModeImages => "mode_images",
            Action::ModeSounds => "mode_sounds",
            Action::ModeModels => "mode_models",
            Action::Settings => "settings",
            Action::TogglePane => "toggle_pane",
            Action::Grid => "grid",
            Action::List => "list",
            Action::Left => "left",
            Action::Right => "right",
            Action::Up => "up",
            Action::Down => "down",
            Action::PageUp => "page_up",
            Action::PageDown => "page_down",
            Action::First => "first",
            Action::Last => "last",
            Action::SelectAll => "select_all",
            Action::Open => "open",
            Action::ShowInFolder => "show_in_folder",
            Action::CopyFiles => "copy_files",
            Action::CopyPaths => "copy_paths",
            Action::Favorite => "favorite",
            Action::Delete => "delete",
            Action::PlayPause => "play_pause",
            Action::StopOrClear => "stop",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::Search => "Search",
            Action::ModeAll => "Show everything",
            Action::ModeImages => "Show images",
            Action::ModeSounds => "Show sounds",
            Action::ModeModels => "Show 3D models",
            Action::Settings => "Settings",
            Action::TogglePane => "Show or hide the preview pane",
            Action::Grid => "Grid",
            Action::List => "List",
            Action::Left => "Move left",
            Action::Right => "Move right",
            Action::Up => "Move up",
            Action::Down => "Move down",
            Action::PageUp => "Page up",
            Action::PageDown => "Page down",
            Action::First => "First asset",
            Action::Last => "Last asset",
            Action::SelectAll => "Select all",
            Action::Open => "Open in its app",
            Action::ShowInFolder => "Show in folder",
            Action::CopyFiles => "Copy files",
            Action::CopyPaths => "Copy paths",
            Action::Favorite => "Favorite",
            Action::Delete => "Delete…",
            Action::PlayPause => "Play or pause the sound",
            Action::StopOrClear => "Stop the sound, or clear the selection",
        }
    }

    /// The heading it's listed under.
    pub fn group(self) -> &'static str {
        match self {
            Action::Search
            | Action::ModeAll
            | Action::ModeImages
            | Action::ModeSounds
            | Action::ModeModels
            | Action::Settings
            | Action::TogglePane
            | Action::Grid
            | Action::List => "Window",
            Action::Left
            | Action::Right
            | Action::Up
            | Action::Down
            | Action::PageUp
            | Action::PageDown
            | Action::First
            | Action::Last => "Moving around",
            Action::SelectAll
            | Action::Open
            | Action::ShowInFolder
            | Action::CopyFiles
            | Action::CopyPaths
            | Action::Favorite
            | Action::Delete
            | Action::PlayPause
            | Action::StopOrClear => "Selected assets",
        }
    }

    pub fn default_shortcut(self) -> Option<Shortcut> {
        let plain = |key| Some(Shortcut { key, command: false, alt: false, shift: false });
        let command = |key| Some(Shortcut { key, command: true, alt: false, shift: false });
        match self {
            Action::Search => command(Key::F),
            Action::ModeAll => command(Key::Num1),
            Action::ModeImages => command(Key::Num2),
            Action::ModeSounds => command(Key::Num3),
            Action::ModeModels => command(Key::Num4),
            Action::Settings => command(Key::Comma),
            Action::TogglePane | Action::Grid | Action::List | Action::ShowInFolder => None,
            Action::Left => plain(Key::ArrowLeft),
            Action::Right => plain(Key::ArrowRight),
            Action::Up => plain(Key::ArrowUp),
            Action::Down => plain(Key::ArrowDown),
            Action::PageUp => plain(Key::PageUp),
            Action::PageDown => plain(Key::PageDown),
            Action::First => plain(Key::Home),
            Action::Last => plain(Key::End),
            Action::SelectAll => command(Key::A),
            Action::Open => plain(Key::Enter),
            Action::CopyFiles => Some(Shortcut { key: Key::C, command: true, alt: false, shift: true }),
            Action::CopyPaths => command(Key::C),
            Action::Favorite => plain(Key::F),
            Action::Delete => plain(Key::Delete),
            Action::PlayPause => plain(Key::Space),
            Action::StopOrClear => plain(Key::Escape),
        }
    }

    /// Moves, which select a range when Shift is held too.
    fn extends_with_shift(self) -> bool {
        self.group() == "Moving around"
    }
}

/// A key with the modifiers held for it. `command` is Ctrl, or Cmd on a Mac.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub key: Key,
    pub command: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Shortcut {
    fn modifiers(self) -> Modifiers {
        let mut modifiers = Modifiers::NONE;
        if self.command {
            modifiers |= Modifiers::COMMAND;
        }
        if self.alt {
            modifiers |= Modifiers::ALT;
        }
        if self.shift {
            modifiers |= Modifiers::SHIFT;
        }
        modifiers
    }

    /// As the OS writes it: "Ctrl+Shift+F" (or "⇧⌘F" on a Mac), with punctuation as itself
    /// ("Ctrl+,", where egui says "Ctrl+Comma").
    pub fn label(self, ctx: &egui::Context) -> String {
        let label = ctx.format_shortcut(&KeyboardShortcut::new(self.modifiers(), self.key));
        let symbol = self.key.symbol_or_name();
        let punctuation = symbol.chars().count() == 1 && symbol.chars().all(|c| c.is_ascii_punctuation());
        match label.strip_suffix(self.key.name()) {
            Some(modifiers) if punctuation => format!("{modifiers}{symbol}"),
            _ => label,
        }
    }

    /// As saved: "ctrl+shift+F". The key is egui's own name for it.
    fn to_text(self) -> String {
        let mut text = String::new();
        for (on, name) in [(self.command, "ctrl+"), (self.alt, "alt+"), (self.shift, "shift+")] {
            if on {
                text.push_str(name);
            }
        }
        text + self.key.name()
    }

    pub fn parse(text: &str) -> Option<Self> {
        let mut shortcut = Shortcut { key: Key::A, command: false, alt: false, shift: false };
        let mut rest = text.trim();
        loop {
            let lower = rest.to_ascii_lowercase();
            if lower.starts_with("ctrl+") && rest.len() > 5 {
                shortcut.command = true;
                rest = &rest[5..];
            } else if lower.starts_with("alt+") && rest.len() > 4 {
                shortcut.alt = true;
                rest = &rest[4..];
            } else if lower.starts_with("shift+") && rest.len() > 6 {
                shortcut.shift = true;
                rest = &rest[6..];
            } else {
                break;
            }
        }
        shortcut.key = Key::from_name(rest)?;
        Some(shortcut)
    }

    /// Whether a key press with `held` is this shortcut; with `shift_extends`, also with Shift
    /// held on top of it.
    fn matches(self, key: Key, held: Modifiers, shift_extends: bool) -> bool {
        key == self.key
            && held.command == self.command
            && held.alt == self.alt
            && (held.shift == self.shift || (shift_extends && held.shift && !self.shift))
    }
}

/// Which shortcut each action has.
#[derive(Clone, Debug, PartialEq)]
pub struct Keymap {
    bindings: HashMap<Action, Option<Shortcut>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self { bindings: Action::ALL.into_iter().map(|action| (action, action.default_shortcut())).collect() }
    }
}

impl Keymap {
    /// From the settings: `get` is a setting by key.
    pub fn load(get: impl Fn(&str) -> Option<String>) -> Self {
        let mut keymap = Self::default();
        for action in Action::ALL {
            match get(&setting_key(action)).as_deref() {
                Some("none") => {
                    keymap.bindings.insert(action, None);
                }
                Some(text) => {
                    if let Some(shortcut) = Shortcut::parse(text) {
                        keymap.set(action, Some(shortcut));
                    }
                }
                None => {}
            }
        }
        keymap
    }

    /// What to save, per settings key: the shortcut where it isn't the default, and `None` (remove
    /// the setting) where it is.
    pub fn settings(&self) -> Vec<(String, Option<String>)> {
        Action::ALL
            .into_iter()
            .map(|action| {
                let shortcut = self.get(action);
                let value = (shortcut != action.default_shortcut())
                    .then(|| shortcut.map_or_else(|| "none".to_owned(), Shortcut::to_text));
                (setting_key(action), value)
            })
            .collect()
    }

    pub fn get(&self, action: Action) -> Option<Shortcut> {
        self.bindings.get(&action).copied().flatten()
    }

    /// The action that has `shortcut` now, other than `action`: the one that loses it when
    /// `action` gets it.
    pub fn taken_from(&self, action: Action, shortcut: Option<Shortcut>) -> Option<Action> {
        let shortcut = shortcut?;
        Action::ALL.into_iter().find(|&other| other != action && self.get(other) == Some(shortcut))
    }

    /// Gives `action` the shortcut (or none). A shortcut belongs to one action: if another had
    /// it, that one loses it, and is returned.
    pub fn set(&mut self, action: Action, shortcut: Option<Shortcut>) -> Option<Action> {
        let taken_from = self.taken_from(action, shortcut);
        if let Some(other) = taken_from {
            self.bindings.insert(other, None);
        }
        self.bindings.insert(action, shortcut);
        taken_from
    }

    pub fn is_default(&self, action: Action) -> bool {
        self.get(action) == action.default_shortcut()
    }

    /// Takes a press of the action's shortcut out of this frame's input. `Some(shift)` when it was
    /// pressed, with whether Shift was held (for moves that extend the selection).
    pub fn pressed(&self, ctx: &egui::Context, action: Action) -> Option<bool> {
        let shortcut = self.get(action)?;
        let shift_extends = action.extends_with_shift();
        ctx.input_mut(|input| {
            let mut shift = None;
            input.events.retain(|event| match event {
                Event::Key { key, pressed: true, modifiers, .. }
                    if shortcut.matches(*key, *modifiers, shift_extends) =>
                {
                    shift = Some(shift.unwrap_or(false) || modifiers.shift);
                    false
                }
                _ => true,
            });
            shift
        })
    }

    /// Whether the action's shortcut uses Ctrl or Alt, so it can't be typing.
    pub fn has_modifier(&self, action: Action) -> bool {
        self.get(action).is_some_and(|shortcut| shortcut.command || shortcut.alt)
    }
}

fn setting_key(action: Action) -> String {
    format!("keys.{}", action.id())
}

const PUBLISHED: &str = "keymap";

/// Makes the shortcuts known to every part of the window, for its tooltips (`hint`).
pub fn publish(ctx: &egui::Context, keymap: &Keymap) {
    let keymap = std::sync::Arc::new(keymap.clone());
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(PUBLISHED), keymap));
}

/// The action's shortcut as the user has it ("Ctrl+F"), if it has one.
pub fn shortcut_label(ctx: &egui::Context, action: Action) -> Option<String> {
    let keymap: Option<std::sync::Arc<Keymap>> = ctx.data(|data| data.get_temp(egui::Id::new(PUBLISHED)));
    let shortcut = match keymap {
        Some(keymap) => keymap.get(action),
        None => action.default_shortcut(),
    };
    shortcut.map(|shortcut| shortcut.label(ctx))
}

/// `text` with the action's shortcut after it, for a tooltip: "Settings (Ctrl+,)", or just
/// "Settings" when it has none.
pub fn tooltip(ctx: &egui::Context, text: &str, action: Action) -> String {
    match shortcut_label(ctx, action) {
        Some(label) => format!("{text} ({label})"),
        None => text.to_owned(),
    }
}

/// The next key pressed this frame, with its modifiers, taken out of the input: what a shortcut
/// is being changed to.
pub fn capture(ctx: &egui::Context) -> Option<Shortcut> {
    ctx.input_mut(|input| {
        let index =
            input.events.iter().position(|event| matches!(event, Event::Key { pressed: true, repeat: false, .. }))?;
        let Event::Key { key, modifiers, .. } = input.events.remove(index) else { return None };
        // The character the key types, if any, shouldn't reach a text box.
        input.events.retain(|event| !matches!(event, Event::Text(_)));
        Some(Shortcut { key, command: modifiers.command, alt: modifiers.alt, shift: modifiers.shift })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_round_trip_as_text() {
        let shortcut = Shortcut { key: Key::F, command: true, alt: false, shift: true };
        assert_eq!(shortcut.to_text(), "ctrl+shift+F");
        assert_eq!(Shortcut::parse("ctrl+shift+F"), Some(shortcut));
        assert_eq!(
            Shortcut::parse("Ctrl+Alt+ArrowLeft").map(|s| (s.key, s.command, s.alt)),
            Some((Key::ArrowLeft, true, true))
        );
        assert_eq!(Shortcut::parse("ctrl+"), None);
        assert_eq!(Shortcut::parse("nonsense"), None);
        for action in Action::ALL {
            if let Some(shortcut) = action.default_shortcut() {
                assert_eq!(Shortcut::parse(&shortcut.to_text()), Some(shortcut), "{action:?}");
            }
        }
    }

    #[test]
    fn defaults_are_unique_and_saved_only_when_changed() {
        let keymap = Keymap::default();
        let mut seen = std::collections::HashSet::new();
        for action in Action::ALL {
            if let Some(shortcut) = keymap.get(action) {
                assert!(seen.insert(shortcut), "{action:?} shares its default");
            }
        }
        assert!(keymap.settings().iter().all(|(_, value)| value.is_none()), "nothing to save");
        let ids: std::collections::HashSet<&str> = Action::ALL.iter().map(|action| action.id()).collect();
        assert_eq!(ids.len(), Action::ALL.len(), "ids are unique");
    }

    #[test]
    fn a_shortcut_moves_from_the_action_that_had_it() {
        let mut keymap = Keymap::default();
        let ctrl_f = Action::Search.default_shortcut();
        assert_eq!(keymap.set(Action::Favorite, ctrl_f), Some(Action::Search));
        assert_eq!(keymap.get(Action::Search), None);
        assert_eq!(keymap.get(Action::Favorite), ctrl_f);

        let saved: HashMap<String, Option<String>> = keymap.settings().into_iter().collect();
        assert_eq!(saved["keys.search"].as_deref(), Some("none"));
        assert_eq!(saved["keys.favorite"].as_deref(), Some("ctrl+F"));
        assert_eq!(saved["keys.open"], None);
        let loaded = Keymap::load(|key| saved.get(key).cloned().flatten());
        assert_eq!(loaded, keymap);
    }

    #[test]
    fn presses_match_exactly_except_shift_on_moves() {
        let shortcut = Shortcut { key: Key::F, command: true, alt: false, shift: false };
        assert!(shortcut.matches(Key::F, Modifiers::COMMAND, false));
        assert!(!shortcut.matches(Key::F, Modifiers::COMMAND | Modifiers::SHIFT, false), "Ctrl+Shift+F is another one");
        assert!(!shortcut.matches(Key::F, Modifiers::NONE, false));
        let left = Action::Left.default_shortcut().unwrap();
        assert!(left.matches(Key::ArrowLeft, Modifiers::SHIFT, true), "Shift extends a move");
        assert!(!left.matches(Key::ArrowLeft, Modifiers::COMMAND, true));
    }
}
