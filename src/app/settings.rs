//! The settings: the window and what's done in it (updates, shortcuts, clearing the cache, the
//! memory limit), "What's new" after an update, and saving and loading what the window remembers.

use std::time::Duration;

use crossbeam_channel::Receiver;
use eframe::egui;

use crate::changelog;
use crate::library::SortBy;
use crate::storage::{Measurement, Part};
use crate::ui::display::{Backdrop, Display};
use crate::ui::keymap::Keymap;
use crate::ui::mode::{Layout as ViewLayout, Mode, ModeView};
use crate::ui::settings::{Command, Settings, SettingsArgs};
use crate::ui::updates::UpdateAction;
use crate::ui::whats_new::WhatsNew;
use crate::{db, diag, paths, platform};
use crate::{memory, playback};

use super::BrowserApp;

/// The last version whose "What's new" was shown (or would have been, on a first install).
const SETTING_WHATS_NEW: &str = "app.whats_new_seen";
const SETTING_MEMORY: &str = "memory.limit_mb";

/// Parts of the cache being cleared on the cache writer's thread.
pub(super) struct Clearing {
    parts: Vec<Part>,
    done: Receiver<Result<(), String>>,
}

/// The settings window, what it sets, and what it has under way.
pub(super) struct Preferences {
    pub(super) settings: Settings,
    pub(super) keymap: Keymap,
    pub(super) memory_mb: u64,
    pub(super) whats_new: Option<WhatsNew>,
    pub(super) clearing: Option<Clearing>,
    /// The Storage page's measure of the files.
    pub(super) storage: Measurement,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            keymap: Keymap::default(),
            memory_mb: memory::DEFAULT_MB,
            whats_new: None,
            clearing: None,
            storage: Measurement::default(),
        }
    }
}

impl BrowserApp {
    /// Shares the memory limit out among what the app keeps (`memory`).
    pub(super) fn apply_memory(&mut self) {
        let shares = memory::apply(self.preferences.memory_mb);
        self.previews.set_max_textures(shares.thumbnails);
    }

    /// After an update, "What's new" for the versions since the one whose notes were last shown.
    pub(super) fn whats_new_at_start(&mut self) {
        let Some(conn) = &self.conn else { return };
        let Ok(current) = semver::Version::parse(&self.updates.version()) else { return };
        let seen = db::get_setting(conn, SETTING_WHATS_NEW);
        // Any saved view means it ran before: an update rather than a first install.
        let used_before = db::get_setting(conn, "ui.mode").is_some();
        if let changelog::AtStart::Since(seen) =
            changelog::at_start(changelog::releases(), seen.as_deref(), used_before, &current)
        {
            self.preferences.whats_new = Some(WhatsNew::since(seen, current.clone()));
        }
        let remembered = changelog::remembered(seen.as_deref(), &current);
        if let Err(err) = db::set_setting(conn, SETTING_WHATS_NEW, &remembered.to_string()) {
            diag::report("Saving settings", err);
        }
    }

    /// Checks for an update, or restarts into the one downloaded.
    pub(super) fn run_update_action(&mut self, action: UpdateAction) {
        match action {
            UpdateAction::Check => self.updates.check_now(),
            UpdateAction::Restart => self.restart_to_update(),
        }
    }

    fn restart_to_update(&mut self) {
        // Saved before the updater starts: it waits for the app to close, but if it can't watch
        // the process it goes ahead and stops it.
        self.save_settings();
        if self.updates.restart() {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// The settings window, and doing what's asked in it.
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.preferences.settings.is_open() {
            return;
        }
        self.preferences.storage.poll();
        let (user_path, log_path) = (paths::user_data_file(), paths::log_file());
        let args = SettingsArgs {
            updates: &self.updates,
            keymap: &self.preferences.keymap,
            memory_mb: self.preferences.memory_mb,
            clearing: self.preferences.clearing.is_some(),
            cache_path: &self.db_path,
            user_path: &user_path,
            log_path: &log_path,
            usage: self.preferences.storage.usage(),
            measuring: self.preferences.storage.is_measuring(),
        };
        for command in self.preferences.settings.show(ctx, &args) {
            self.run_settings_command(ctx, command);
        }
    }

    /// Does what the settings window asked for.
    fn run_settings_command(&mut self, ctx: &egui::Context, command: Command) {
        match command {
            Command::Update(action) => self.run_update_action(action),
            Command::WhatsNew => {
                if let Ok(current) = semver::Version::parse(&self.updates.version()) {
                    self.preferences.whats_new = Some(WhatsNew::every_version(current));
                }
            }
            Command::SetShortcut(action, shortcut) => {
                // The settings window says which action lost the shortcut, if one did.
                self.preferences.keymap.set(action, shortcut);
                self.save_settings();
            }
            Command::ResetShortcuts => {
                self.preferences.keymap = Keymap::default();
                self.save_settings();
            }
            Command::Clear(parts) => self.clear(parts),
            Command::SetMemory(mb) => {
                self.preferences.memory_mb = mb.clamp(memory::MIN_MB, memory::MAX_MB);
                self.apply_memory();
                self.save_settings();
            }
            Command::Reveal(path) => {
                if let Err(err) = platform::reveal(&path) {
                    diag::report("Showing the file", err);
                }
            }
            Command::OpenLink(url) => {
                if let Err(err) = platform::open_link(&url) {
                    diag::report("Opening the link", err);
                }
            }
            Command::Measure => {
                let ctx = ctx.clone();
                let wake = std::sync::Arc::new(move || ctx.request_repaint());
                self.preferences.storage.start(self.db_path.clone(), paths::user_data_file(), wake);
            }
        }
    }

    /// Clears what was chosen under Settings → Storage: the user's data here and now, the cache's
    /// parts on the cache writer's thread (`poll_clearing` finishes up).
    fn clear(&mut self, parts: Vec<Part>) {
        for part in &parts {
            match part {
                // A clear the file refused is reported, and nothing changes.
                Part::Groups => {
                    if self.organizing.groups.clear() {
                        self.browsing.group = None;
                        self.organizing.membership_dirty = true;
                        self.browsing.view_dirty = true;
                    }
                }
                Part::SheetChoices => {
                    self.organizing.sheets.clear();
                    self.inspector.sheet.clear();
                }
                Part::LibraryFolders => {
                    self.folders.roots.clear();
                    self.save_roots();
                    self.set_library(None);
                    self.update_title();
                }
                _ => {}
            }
        }
        let cache: Vec<Part> = parts.into_iter().filter(|part| !part.in_user_data()).collect();
        if cache.is_empty() {
            self.preferences.storage.forget();
            self.notify("Cleared".to_owned());
            return;
        }
        let done = self.thumbs.clear(cache.clone());
        self.preferences.clearing = Some(Clearing { parts: cache, done });
    }

    pub(super) fn poll_clearing(&mut self) {
        let Some(clearing) = &self.preferences.clearing else { return };
        let result = match clearing.done.try_recv() {
            Ok(result) => result,
            Err(crossbeam_channel::TryRecvError::Empty) => {
                self.ctx.request_repaint_after(Duration::from_millis(100));
                return;
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => Err("the cache writer stopped".to_owned()),
        };
        let parts = clearing.parts.clone();
        self.preferences.clearing = None;
        self.preferences.storage.forget();
        if let Err(err) = result {
            diag::report("Clearing the cache", err);
            return;
        }
        let has = |part| parts.contains(&part);
        self.previews.forget(has(Part::Thumbnails), has(Part::Waveforms), has(Part::ModelFacts));
        if has(Part::Settings) {
            self.reset_settings();
        }
        if has(Part::Index) {
            self.scan(self.folders.roots.clone());
        }
        self.notify("Cleared".to_owned());
    }

    pub(super) fn load_settings(&mut self) {
        let Some(conn) = &self.conn else { return };
        let get = |key: &str| db::get_setting(conn, key);
        if let Some(mode) = get("ui.mode").as_deref().and_then(Mode::from_key) {
            self.browsing.mode = mode;
        }
        self.chrome.show_inspector = get("ui.inspector").is_none_or(|value| value == "1");
        for mode in Mode::ALL {
            let view = &mut self.browsing.modes[mode.index()];
            let key = |name: &str| format!("ui.{}.{name}", mode.key());
            match get(&key("layout")).as_deref() {
                Some("grid") => view.layout = ViewLayout::Grid,
                Some("list") => view.layout = ViewLayout::List,
                _ => {}
            }
            if let Some(tile) = get(&key("tile")).and_then(|value| value.parse::<f32>().ok()) {
                view.tile = tile.clamp(64.0, 256.0);
            }
            if let Some(sort) = get(&key("sort")).as_deref().and_then(SortBy::from_key) {
                view.sort = sort;
            }
            view.descending = get(&key("descending")).is_some_and(|value| value == "1");
            if let Some(sort) = get(&key("group_sort")).as_deref().and_then(SortBy::from_key) {
                view.group_sort = sort;
            }
            view.group_descending = get(&key("group_descending")).is_none_or(|value| value == "1");
        }
        self.playback.autoplay = get("sound.autoplay").is_none_or(|value| value == "1");
        self.playback.set_looping(get("sound.loop").is_some_and(|value| value == "1"));
        if let Some(volume) = get("sound.volume").and_then(|value| value.parse::<f32>().ok()) {
            self.playback.set_volume(volume.clamp(0.0, 1.0));
        }
        let display = &mut self.browsing.display;
        if let Some(backdrop) = get("image.backdrop").as_deref().and_then(Backdrop::from_key) {
            display.backdrop = backdrop;
        }
        display.turntable = get("model.turntable").is_some_and(|value| value == "1");
        display.wireframe = get("model.wireframe").is_some_and(|value| value == "1");
        self.preferences.keymap = Keymap::load(get);
        self.preferences.memory_mb = get(SETTING_MEMORY)
            .and_then(|value| value.parse::<u64>().ok())
            .map_or(memory::DEFAULT_MB, |mb| mb.clamp(memory::MIN_MB, memory::MAX_MB));
    }

    /// Everything the settings hold back to how it started, after they were cleared.
    fn reset_settings(&mut self) {
        self.browsing.mode = Mode::All;
        self.browsing.modes = Mode::ALL.map(ModeView::new);
        self.chrome.show_inspector = true;
        self.playback.set_volume(playback::DEFAULT_VOLUME);
        self.browsing.display = Display::default();
        // The rest have their defaults where nothing is saved.
        self.load_settings();
        self.apply_memory();
        self.browsing.view_dirty = true;
        // Cleared settings aren't a first install: no "What's new" next time.
        if let (Some(conn), Ok(current)) = (&self.conn, semver::Version::parse(&self.updates.version()))
            && let Err(err) = db::set_setting(conn, SETTING_WHATS_NEW, &current.to_string())
        {
            diag::note("Saving that What's new was seen", err);
        }
    }

    pub(super) fn save_settings(&self) {
        let Some(conn) = &self.conn else { return };
        let flag = |on: bool| if on { "1" } else { "0" };
        let mut settings = vec![
            ("ui.mode".to_owned(), self.browsing.mode.key().to_owned()),
            ("ui.inspector".to_owned(), flag(self.chrome.show_inspector).to_owned()),
            ("sound.autoplay".to_owned(), flag(self.playback.autoplay).to_owned()),
            ("sound.loop".to_owned(), flag(self.playback.looping()).to_owned()),
            ("sound.volume".to_owned(), format!("{:.3}", self.playback.volume())),
            ("image.backdrop".to_owned(), self.browsing.display.backdrop.key().to_owned()),
            ("model.turntable".to_owned(), flag(self.browsing.display.turntable).to_owned()),
            ("model.wireframe".to_owned(), flag(self.browsing.display.wireframe).to_owned()),
        ];
        for mode in Mode::ALL {
            let view = &self.browsing.modes[mode.index()];
            let key = |name: &str| format!("ui.{}.{name}", mode.key());
            let layout = match view.layout {
                ViewLayout::Grid => "grid",
                ViewLayout::List => "list",
            };
            settings.push((key("layout"), layout.to_owned()));
            settings.push((key("tile"), format!("{:.0}", view.tile)));
            settings.push((key("sort"), view.sort.key().to_owned()));
            settings.push((key("descending"), flag(view.descending).to_owned()));
            settings.push((key("group_sort"), view.group_sort.key().to_owned()));
            settings.push((key("group_descending"), flag(view.group_descending).to_owned()));
        }
        settings.push((SETTING_MEMORY.to_owned(), self.preferences.memory_mb.to_string()));
        // Shortcuts are saved only where they differ from the defaults.
        let mut defaults = Vec::new();
        for (key, value) in self.preferences.keymap.settings() {
            match value {
                Some(value) => settings.push((key, value)),
                None => defaults.push(key),
            }
        }
        let settings: Vec<(&str, &str)> = settings.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect();
        let defaults: Vec<&str> = defaults.iter().map(String::as_str).collect();
        if let Err(err) = db::set_settings(conn, &settings).and_then(|()| db::remove_settings(conn, &defaults)) {
            diag::report("Saving settings", err);
        }
    }
}
