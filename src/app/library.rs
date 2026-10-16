//! The library's folders: opening them from the cached index, adding and taking them out, and
//! scanning them in the background (when they're opened, and when the OS says they changed).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::library::{self, Library, folder_name, path_key, root_string};
use crate::scan::ScannedFile;
use crate::{db, diag, platform, scan};

use super::BrowserApp;

/// The one folder that was open, from before the library could have several: it becomes the
/// library's first folder.
const SETTING_LAST_ROOT: &str = "last_root";

/// A scan of some of the library's folders, and the library built with what it found.
pub(super) struct ScanDone {
    /// The folders the library was built for.
    roots: Vec<PathBuf>,
    /// The library built with what was found and the files the other folders had when the scan
    /// started. When a folder couldn't be read, its files stay as they were, which only the app
    /// knows: then it's just what was found, for the app to build the library with.
    built: Result<Library, Vec<(PathBuf, Vec<ScannedFile>)>>,
    /// The folders scanned, and those that couldn't be read (unplugged, moved or renamed).
    scanned: Vec<PathBuf>,
    unreadable: Vec<PathBuf>,
    /// `BrowserApp::library_version` when the scan started. If the library has changed since,
    /// `built` is out of date for the folders that weren't scanned.
    version: u64,
    elapsed: Duration,
    assets: usize,
    dirs_visited: usize,
    skipped: usize,
}

pub(super) struct LastScan {
    pub(super) elapsed: Duration,
    pub(super) assets: usize,
    pub(super) dirs_visited: usize,
    pub(super) skipped: usize,
}

impl BrowserApp {
    /// Shows the library's folders from the cached index at once, then rescans them all in the
    /// background. `start` (from the command line) is added to them, or shown if it's in already.
    pub(super) fn open_library(&mut self, start: Option<PathBuf>) {
        self.roots = self.saved_roots();
        let mut show = None;
        if let Some(path) = start {
            let path = std::path::absolute(&path).unwrap_or(path);
            if path.is_dir() {
                if library::add_root(&mut self.roots, path.clone()).is_ok() {
                    self.save_roots();
                }
                show = Some(path_key(&path));
            } else {
                self.notify(format!("{} isn't a folder", platform::native(&path)));
            }
        }
        let start = Instant::now();
        let files: Vec<(PathBuf, Vec<ScannedFile>)> =
            self.roots.iter().map(|root| (root.clone(), self.cached_index(root))).collect();
        if !files.is_empty() {
            self.set_library(Some(Library::build(files)));
            self.index_note = Some(format!("Index from cache in {} ms", start.elapsed().as_millis()));
        }
        if let Some(dir) = show.and_then(|key| self.library.as_ref()?.find_dir(&key)) {
            self.select_dir(dir);
        }
        self.update_title();
        self.scan(self.roots.clone());
    }

    /// The library's folders as saved; the first time, the one folder that was open before the
    /// library could have several.
    fn saved_roots(&self) -> Vec<PathBuf> {
        let Some(user) = &self.user_conn else {
            return self.legacy_root().into_iter().collect();
        };
        let roots = db::load_roots(user).unwrap_or_else(|err| {
            diag::report("Loading the library's folders", err);
            Vec::new()
        });
        if !roots.is_empty() {
            return roots;
        }
        let Some(root) = self.legacy_root() else { return roots };
        let roots = vec![root];
        match db::save_roots(user, &roots) {
            // Saved in its new place, it mustn't come back after being taken out.
            Ok(()) => {
                if let Some(conn) = &self.conn {
                    let _ = db::remove_setting(conn, SETTING_LAST_ROOT);
                }
            }
            Err(err) => diag::report("Saving the library's folders", err),
        }
        roots
    }

    fn legacy_root(&self) -> Option<PathBuf> {
        self.conn.as_ref().and_then(|conn| db::get_setting(conn, SETTING_LAST_ROOT)).map(PathBuf::from)
    }

    pub(super) fn save_roots(&self) {
        if let Some(user) = &self.user_conn
            && let Err(err) = db::save_roots(user, &self.roots)
        {
            diag::report("Saving the library's folders", err);
        }
    }

    /// The files the last scan of `root` found, or none if it was never scanned.
    fn cached_index(&self, root: &Path) -> Vec<ScannedFile> {
        let Some(conn) = &self.conn else { return Vec::new() };
        match db::load_index(conn, &root_string(root)) {
            Ok(files) => files.unwrap_or_default(),
            Err(err) => {
                diag::report("Loading the cached index", err);
                Vec::new()
            }
        }
    }

    pub(super) fn update_title(&self) {
        let title = match self.roots.as_slice() {
            [] => "Asset Browser".to_owned(),
            [root] => format!("Asset Browser - {}", platform::native(root)),
            roots => {
                let names: Vec<String> = roots.iter().map(|root| folder_name(root)).collect();
                format!("Asset Browser - {}", names.join(", "))
            }
        };
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    /// Adds a folder to the library and scans it, or shows it if it's in the library already.
    pub(super) fn add_root(&mut self, path: &Path) {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if !path.is_dir() {
            self.notify(format!("{} isn't a folder", platform::native(&path)));
            return;
        }
        let name = folder_name(&path);
        match library::add_root(&mut self.roots, path.clone()) {
            Err(index) => {
                if let Some(dir) = self.library.as_ref().and_then(|library| library.find_dir(&path_key(&path))) {
                    self.select_dir(dir);
                }
                let root = folder_name(&self.roots[index]);
                self.notify(if path_key(&path) == path_key(&self.roots[index]) {
                    format!("{root} is in the library already")
                } else {
                    format!("{name} is in {root}, which is in the library already")
                });
            }
            Ok(replaced) => {
                self.save_roots();
                // What the cache has of it shows at once; the scan brings it up to date.
                let cached = self.cached_index(&path);
                self.rebuild_library(vec![(path.clone(), cached)]);
                self.scan(vec![path.clone()]);
                if let Some(index) = self.library.as_ref().and_then(|library| library.root_index(&path)) {
                    self.select_dir(Library::root_dir(index));
                }
                self.update_title();
                let replaced: Vec<String> = replaced.iter().map(|root| folder_name(root)).collect();
                self.notify(match replaced.as_slice() {
                    [] => format!("Added {name} to the library"),
                    _ => format!("Added {name} in place of {}, which it holds", replaced.join(", ")),
                });
            }
        }
    }

    /// Takes a folder out of the library. Its files, and the favorites, collections and
    /// categories they're in, are untouched: adding it back brings them all back.
    pub(super) fn remove_root(&mut self, index: usize) {
        if index >= self.roots.len() {
            return;
        }
        let removed = self.roots.remove(index);
        self.save_roots();
        self.rebuild_library(Vec::new());
        self.update_title();
        self.notify(format!("Took {} out of the library; its files are untouched", folder_name(&removed)));
    }

    /// The files each folder of the library has now, with `fresh` (newly scanned or loaded)
    /// taking the place of what a folder had.
    fn current_files(&self, mut fresh: Vec<(PathBuf, Vec<ScannedFile>)>) -> Vec<(PathBuf, Vec<ScannedFile>)> {
        self.roots
            .iter()
            .map(|root| {
                let files = match fresh.iter().position(|(path, _)| path == root) {
                    Some(at) => fresh.swap_remove(at).1,
                    None => self.files_now(root),
                };
                (root.clone(), files)
            })
            .collect()
    }

    /// The files the library has of `root` (none if it isn't one of its folders yet).
    fn files_now(&self, root: &Path) -> Vec<ScannedFile> {
        self.library.as_ref().and_then(|library| Some(library.files_of(library.root_index(root)?))).unwrap_or_default()
    }

    /// Builds the library again after its folders changed (on this thread: it's a user's action).
    pub(super) fn rebuild_library(&mut self, fresh: Vec<(PathBuf, Vec<ScannedFile>)>) {
        let library = (!self.roots.is_empty()).then(|| Library::build(self.current_files(fresh)));
        self.set_library(library);
    }

    /// Puts in a new library, keeping the selections, the folder and the playing sound: entry
    /// indices change, keys don't.
    pub(super) fn set_library(&mut self, library: Option<Library>) {
        self.library_version += 1;
        self.follow_focus = false;
        match (&self.library, &library) {
            (Some(old), Some(new)) => {
                let new_index = new.index_by_key();
                let old_keys: Vec<i64> = old.entries.iter().map(|entry| entry.key).collect();
                for view in &mut self.modes {
                    view.selection.remap(&old_keys, &new_index);
                }
                let dir = old.dirs.get(self.selected_dir as usize).and_then(|dir| new.find_dir(&dir.key));
                // The top only shows with several folders.
                self.selected_dir = dir.unwrap_or(new.home()).max(new.home());
                self.sound.remap(&new_index);
            }
            (None, Some(new)) => self.selected_dir = new.home(),
            (_, None) => {
                for view in &mut self.modes {
                    view.selection.clear();
                }
                self.selected_dir = 0;
                self.sound.reset();
            }
        }
        self.library = library;
        self.previews.forget_wanted();
        self.view_dirty = true;
        self.membership_dirty = true;
    }

    /// Scans `targets` (some of the library's folders) in the background, then builds the
    /// library with what was found and the files the other folders have now.
    #[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
    pub(super) fn scan(&mut self, targets: Vec<PathBuf>) {
        if targets.is_empty() {
            return;
        }
        let roots = self.roots.clone();
        let others: Vec<(PathBuf, Vec<ScannedFile>)> = self
            .roots
            .iter()
            .filter(|root| !targets.contains(root))
            .map(|root| (root.clone(), self.files_now(root)))
            .collect();
        let version = self.library_version;
        let tx = self.scan_tx.clone();
        let db_path = self.db_path.clone();
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new().name("scan".into()).spawn(move || {
            let conn = db::open_cache(&db_path).map_err(|err| diag::report("Saving the index", err)).ok();
            let (mut elapsed, mut assets, mut dirs_visited, mut skipped) = (Duration::ZERO, 0, 0, 0);
            let mut fresh = Vec::new();
            let mut unreadable = Vec::new();
            for root in &targets {
                // A folder that can't be read keeps what was found there last time, in the index
                // too: an unplugged drive mustn't empty its favorites and collections.
                if let Err(err) = std::fs::read_dir(root) {
                    let context =
                        format!("Can't read {} (showing what was found there last time)", platform::native(root));
                    diag::report(&context, err);
                    unreadable.push(root.clone());
                    continue;
                }
                let result = scan::scan(root);
                if result.dirs_visited == 0 {
                    unreadable.push(root.clone());
                    continue;
                }
                if let Some(conn) = &conn
                    && let Err(err) = db::save_index(conn, &root_string(root), &result.files)
                {
                    diag::report("Saving the index", err);
                }
                elapsed += result.elapsed;
                assets += result.files.len();
                dirs_visited += result.dirs_visited;
                skipped += result.skipped_files;
                fresh.push((root.clone(), result.files));
            }
            let scanned: Vec<PathBuf> = fresh.iter().map(|(root, _)| root.clone()).collect();
            let built = if unreadable.is_empty() {
                let mut others = others;
                let files = roots
                    .iter()
                    .map(|root| {
                        let from = if targets.contains(root) { &mut fresh } else { &mut others };
                        let files = from.iter().position(|(path, _)| path == root).map(|at| from.swap_remove(at).1);
                        (root.clone(), files.unwrap_or_default())
                    })
                    .collect();
                Ok(Library::build(files))
            } else {
                Err(fresh)
            };
            let done = ScanDone { roots, built, scanned, unreadable, version, elapsed, assets, dirs_visited, skipped };
            // Fails only when the window has closed.
            let _ = tx.send(done);
            ctx.request_repaint();
        });
        match spawned {
            Ok(_) => {
                if self.scans == 0 {
                    self.scan_started = Instant::now();
                }
                self.scans += 1;
            }
            Err(err) => diag::report("Starting the scan", err),
        }
    }

    /// Rescans the folders whose files changed while the app was open, once the changes settle.
    pub(super) fn poll_watch(&mut self, ctx: &egui::Context) {
        self.watch.sync(&self.roots);
        let changed = self.watch.poll();
        self.scan(changed);
        if let Some(due) = self.watch.next_due() {
            ctx.request_repaint_after(due);
        }
    }

    pub(super) fn poll_scans(&mut self) {
        while let Ok(done) = self.scan_rx.try_recv() {
            self.scans = self.scans.saturating_sub(1);
            self.last_scan = Some(LastScan {
                elapsed: done.elapsed,
                assets: done.assets,
                dirs_visited: done.dirs_visited,
                skipped: done.skipped,
            });
            for root in &done.scanned {
                self.folders.unreadable.remove(&path_key(root));
            }
            for root in &done.unreadable {
                self.folders.unreadable.insert(path_key(root));
            }
            let current = done.version == self.library_version && done.roots == self.roots;
            let library = match done.built {
                Ok(library) if current => Some(library),
                built => {
                    // The library changed while scanning (a folder added or taken out, or another
                    // scan finished), or a folder couldn't be read: use what was found, and the
                    // files the rest have now.
                    let found = match built {
                        Ok(library) => done
                            .scanned
                            .iter()
                            .filter_map(|root| Some((root.clone(), library.files_of(library.root_index(root)?))))
                            .collect(),
                        Err(found) => found,
                    };
                    (!self.roots.is_empty()).then(|| Library::build(self.current_files(found)))
                }
            };
            let unchanged = match (&self.library, &library) {
                (Some(old), Some(new)) => old.same_files_as(new),
                (None, None) => true,
                _ => false,
            };
            if !unchanged {
                self.set_library(library);
            }
        }
    }
}
