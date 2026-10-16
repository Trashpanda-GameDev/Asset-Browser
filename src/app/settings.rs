//! The settings: the window and what's done in it (updates, shortcuts, clearing the cache, the
//! memory limit), "What's new" after an update, and saving and loading what the window remembers.

use std::time::Duration;

use crossbeam_channel::Receiver;
use eframe::egui;

use crate::changelog;
use crate::library::SortBy;
use crate::memory;
use crate::storage::Part;
use crate::ui::image_view::{Backdrop, ImageView};
use crate::ui::keymap::Keymap;
use crate::ui::mode::{Layout as ViewLayout, Mode, ModeView};
use crate::ui::settings::{Command, SettingsArgs};
use crate::ui::whats_new::WhatsNew;
use crate::{db, diag, paths, platform};

use super::BrowserApp;

/// The last version whose "What's new" was shown (or would have been, on a first install).
const SETTING_WHATS_NEW: &str = "app.whats_new_seen";
const SETTING_MEMORY: &str = "memory.limit_mb";

/// Parts of the cache being cleared on the cache writer's thread.
pub(super) struct Clearing {
    parts: Vec<Part>,
    done: Receiver<Result<(), String>>,
}

impl BrowserApp {
    /// Shares the memory limit out among what the app keeps (`memory`).
    pub(super) fn apply_memory(&mut self) {
        let shares = memory::apply(self.memory_mb);
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
            self.whats_new = Some(WhatsNew::since(seen, current.clone()));
        }
        let remembered = changelog::remembered(seen.as_deref(), &current);
        if let Err(err) = db::set_setting(conn, SETTING_WHATS_NEW, &remembered.to_string()) {
            diag::report("Saving settings", err);
        }
    }

    pub(super) fn restart_to_update(&mut self) {
        // Saved before the updater starts: it waits for the app to close, but if it can't watch
        // the process it goes ahead and stops it.
        self.save_settings();
        if self.updates.restart() {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// The settings window, and doing what's asked in it.
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings.open {
            return;
        }
        let (user_path, log_path) = (paths::user_data_file(), paths::log_file());
        let args = SettingsArgs {
            updates: &self.updates,
            keymap: &self.keymap,
            memory_mb: self.memory_mb,
            clearing: self.clearing.is_some(),
            cache_path: &self.db_path,
            user_path: &user_path,
            log_path: &log_path,
        };
        let commands = self.settings.show(ctx, &args);
        for command in commands {
            match command {
                Command::CheckForUpdates => self.updates.check_now(),
                Command::RestartToUpdate => self.restart_to_update(),
                Command::WhatsNew => {
                    if let Ok(current) = semver::Version::parse(&self.updates.version()) {
                        self.whats_new = Some(WhatsNew::every_version(current));
                    }
                }
                Command::SetShortcut(action, shortcut) => {
                    self.settings.keys_note = self.keymap.set(action, shortcut).and_then(|other| {
                        let shortcut = shortcut?.label(ctx);
                        Some(format!("{shortcut} was for “{}”, which has no shortcut now.", other.label()))
                    });
                    self.save_settings();
                }
                Command::ResetShortcuts => {
                    self.keymap = Keymap::default();
                    self.settings.keys_note = None;
                    self.save_settings();
                }
                Command::Clear(parts) => self.clear(parts),
                Command::SetMemory(mb) => {
                    self.memory_mb = mb.clamp(memory::MIN_MB, memory::MAX_MB);
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
            }
        }
    }

    /// Clears what was chosen under Settings → Storage: the user's data here and now, the cache's
    /// parts on the cache writer's thread (`poll_clearing` finishes up).
    fn clear(&mut self, parts: Vec<Part>) {
        for part in &parts {
            match part {
                Part::Groups => {
                    self.groups.clear();
                    self.group = None;
                    self.membership_dirty = true;
                    self.view_dirty = true;
                }
                Part::SheetChoices => self.inspector.sheet.forget_choices(),
                Part::LibraryFolders => {
                    self.roots.clear();
                    self.save_roots();
                    self.set_library(None);
                    self.update_title();
                }
                _ => {}
            }
        }
        let cache: Vec<Part> = parts.into_iter().filter(|part| !part.in_user_data()).collect();
        if cache.is_empty() {
            self.settings.remeasure();
            self.notify("Cleared".to_owned());
            return;
        }
        let done = self.thumbs.clear(cache.clone());
        self.clearing = Some(Clearing { parts: cache, done });
    }

    pub(super) fn poll_clearing(&mut self) {
        let Some(clearing) = &self.clearing else { return };
        let result = match clearing.done.try_recv() {
            Ok(result) => result,
            Err(crossbeam_channel::TryRecvError::Empty) => {
                self.ctx.request_repaint_after(Duration::from_millis(100));
                return;
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => Err("the cache writer stopped".to_owned()),
        };
        let parts = clearing.parts.clone();
        self.clearing = None;
        self.settings.remeasure();
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
            self.scan(self.roots.clone());
        }
        self.notify("Cleared".to_owned());
    }

    pub(super) fn load_settings(&mut self) {
        let Some(conn) = &self.conn else { return };
        let get = |key: &str| db::get_setting(conn, key);
        if let Some(mode) = get("ui.mode").as_deref().and_then(Mode::from_key) {
            self.mode = mode;
        }
        self.show_inspector = get("ui.inspector").is_none_or(|value| value == "1");
        for mode in Mode::ALL {
            let view = &mut self.modes[mode.index()];
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
        self.sound.autoplay = get("sound.autoplay").is_none_or(|value| value == "1");
        self.sound.looping = get("sound.loop").is_some_and(|value| value == "1");
        if let Some(volume) = get("sound.volume").and_then(|value| value.parse::<f32>().ok()) {
            self.sound.volume = volume.clamp(0.0, 1.0);
        }
        self.sound.set_volume(self.sound.volume);
        self.sound.set_looping(self.sound.looping);
        if let Some(backdrop) = get("image.backdrop").as_deref().and_then(Backdrop::from_key) {
            self.inspector.image.backdrop = backdrop;
        }
        self.inspector.viewport.auto_rotate = get("model.turntable").is_some_and(|value| value == "1");
        self.inspector.viewport.wireframe = get("model.wireframe").is_some_and(|value| value == "1");
        self.keymap = Keymap::load(get);
        self.memory_mb = get(SETTING_MEMORY)
            .and_then(|value| value.parse::<u64>().ok())
            .map_or(memory::DEFAULT_MB, |mb| mb.clamp(memory::MIN_MB, memory::MAX_MB));
    }

    /// Everything the settings hold back to how it started, after they were cleared.
    fn reset_settings(&mut self) {
        self.mode = Mode::All;
        self.modes = Mode::ALL.map(ModeView::new);
        self.show_inspector = true;
        self.sound.volume = 0.8;
        self.inspector.image.backdrop = ImageView::default().backdrop;
        // The rest have their defaults where nothing is saved.
        self.load_settings();
        self.apply_memory();
        self.view_dirty = true;
        // Cleared settings aren't a first install: no "What's new" next time.
        if let (Some(conn), Ok(current)) = (&self.conn, semver::Version::parse(&self.updates.version())) {
            let _ = db::set_setting(conn, SETTING_WHATS_NEW, &current.to_string());
        }
    }

    pub(super) fn save_settings(&self) {
        let Some(conn) = &self.conn else { return };
        let flag = |on: bool| if on { "1" } else { "0" };
        let mut settings = vec![
            ("ui.mode".to_owned(), self.mode.key().to_owned()),
            ("ui.inspector".to_owned(), flag(self.show_inspector).to_owned()),
            ("sound.autoplay".to_owned(), flag(self.sound.autoplay).to_owned()),
            ("sound.loop".to_owned(), flag(self.sound.looping).to_owned()),
            ("sound.volume".to_owned(), format!("{:.3}", self.sound.volume)),
            ("image.backdrop".to_owned(), self.inspector.image.backdrop.key().to_owned()),
            ("model.turntable".to_owned(), flag(self.inspector.viewport.auto_rotate).to_owned()),
            ("model.wireframe".to_owned(), flag(self.inspector.viewport.wireframe).to_owned()),
        ];
        for mode in Mode::ALL {
            let view = &self.modes[mode.index()];
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
        settings.push((SETTING_MEMORY.to_owned(), self.memory_mb.to_string()));
        // Shortcuts are saved only where they differ from the defaults.
        let mut defaults = Vec::new();
        for (key, value) in self.keymap.settings() {
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
