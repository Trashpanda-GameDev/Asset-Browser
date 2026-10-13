//! The window: a header with the modes and search, the folder tree, the browsed assets as a grid or
//! a list, the inspector on the right, and a status bar.
//!
//! Nothing here touches the disk or decodes a file on the UI thread, apart from loading the cached
//! index of the library's folders at the start and when one is added. Scanning, previews, large
//! previews and sound arrive from background threads, and the views only draw (and only ask
//! previews for) the rows on screen.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{self, Align, Frame, Key, Layout, Margin, Modifiers, RichText, Sense, vec2};
use eframe::egui_wgpu::RenderState;
use egui_phosphor::regular as icon;
use rusqlite::Connection;

use crate::asset::AssetKind;
use crate::changelog;
use crate::deletion::{self, Deleted};
use crate::groups::{self, FAVORITES, GroupKind, Groups, Membership, UNCATEGORIZED};
use crate::keymap::{self, Action, Keymap};
use crate::library::{self, Library, SortBy, folder_name, path_key, root_string};
use crate::memory;
use crate::scan::ScannedFile;
use crate::storage::Part;
use crate::thumbs::ThumbService;
use crate::ui::browse::{self, Browse, BrowseOutput, MenuAction};
use crate::ui::folders::{FolderAction, FolderTree};
use crate::ui::image_view::Backdrop;
use crate::ui::image_view::ImageView;
use crate::ui::inspector::{Inspector, InspectorArgs};
use crate::ui::mode::{Layout as ViewLayout, Mode, ModeView};
use crate::ui::organize::{self, GroupCommand, GroupEdit, Sidebar, Targets, plural};
use crate::ui::previews::Previews;
use crate::ui::settings::{Command, Settings, SettingsArgs};
use crate::ui::sound::{self, Sound};
use crate::ui::updates::UpdateAction;
use crate::ui::whats_new::WhatsNew;
use crate::ui::{theme, widgets};
use crate::updates::Updates;
use crate::watch::Watch;
use crate::{db, diag, platform, scan};

pub use crate::ui::format::{format_size, format_time, grouped};

/// The one folder that was open, from before the library could have several: it becomes the
/// library's first folder.
const SETTING_LAST_ROOT: &str = "last_root";
/// More files than this opened at once is surely a slip: only the focused one opens.
const OPEN_MANY_MAX: usize = 12;
/// How long a confirmation ("Added 3 to Forest props") stays up.
const TOAST_TIME: Duration = Duration::from_millis(2600);
/// The last version whose "What's new" was shown (or would have been, on a first install).
const SETTING_WHATS_NEW: &str = "app.whats_new_seen";
const SETTING_MEMORY: &str = "memory.limit_mb";

/// A scan of some of the library's folders, and the library built with what it found.
struct ScanDone {
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

struct LastScan {
    elapsed: Duration,
    assets: usize,
    dirs_visited: usize,
    skipped: usize,
}

/// Files or a folder to delete, waiting for "Delete?" to be answered.
struct DeleteRequest {
    paths: Vec<PathBuf>,
    /// For the messages: "3 files", "\u{201c}hero.png\u{201d}", "the folder \u{201c}ui\u{201d}".
    what: String,
    /// The dialog's question, and what it lists.
    title: String,
    details: String,
    /// On a drive with no Recycle Bin, so it can't be undone.
    permanent: bool,
    /// The group being browsed: the files could just be taken out of it instead.
    group: Option<i64>,
    after: AfterDelete,
}

/// Where to be once the files are gone, as in a file manager.
#[derive(Clone, Default)]
struct AfterDelete {
    /// The asset after them in the view, to select (by cache key).
    next: Option<i64>,
    /// A deleted folder and its parent (`DirNode::key`s): the parent is browsed instead if the
    /// folder, or one inside it, was.
    folder: Option<String>,
    parent: Option<String>,
}

/// Parts of the cache being cleared on the cache writer's thread.
struct Clearing {
    parts: Vec<Part>,
    done: Receiver<Result<(), String>>,
}

/// A deletion under way.
struct Deleting {
    rx: Receiver<Deleted>,
    what: String,
    permanent: bool,
    after: AfterDelete,
}

/// What `BrowserApp::show_view` shows, for the screenshots tool (`src/bin/shots`).
#[derive(Clone, Copy)]
pub struct ScreenshotView {
    pub mode: Mode,
    pub layout: ViewLayout,
    /// The order, or else the mode's own.
    pub sort: Option<SortBy>,
    /// The tiles' size, or else the mode's own.
    pub tile: Option<f32>,
    /// A collection or category by name, or else the whole library.
    pub group: Option<&'static str>,
    /// The asset whose path ends with this, selected.
    pub file: Option<&'static str>,
}

impl ScreenshotView {
    pub const fn new(mode: Mode, layout: ViewLayout) -> Self {
        Self { mode, layout, sort: None, tile: None, group: None, file: None }
    }
}

pub struct BrowserApp {
    ctx: egui::Context,
    db_path: PathBuf,
    conn: Option<Connection>,
    /// The user data file, for the library's folders.
    user_conn: Option<Connection>,
    thumbs: ThumbService,
    render_state: Option<RenderState>,
    /// The folders the library is made of, in the order they were added.
    roots: Vec<PathBuf>,
    /// `None` when there are no folders.
    library: Option<Library>,
    /// Counts the libraries put in, so a scan can tell whether the one it started from is current.
    library_version: u64,
    index_note: Option<String>,
    scan_tx: Sender<ScanDone>,
    scan_rx: Receiver<ScanDone>,
    /// Scans running.
    scans: usize,
    /// Told by the OS when files change in the library's folders, to rescan them.
    watch: Watch,
    scan_started: Instant,
    last_scan: Option<LastScan>,

    mode: Mode,
    modes: [ModeView; 4],
    /// Indices into `library.entries` shown for the mode, folder and search, in display order.
    view: Vec<u32>,
    view_dirty: bool,
    selected_dir: u32,
    query: String,
    focus_search: bool,
    show_inspector: bool,
    folders: FolderTree,

    previews: Previews,
    inspector: Inspector,
    sound: Sound,
    updates: Updates,
    /// The settings window, and what it sets.
    settings: Settings,
    keymap: Keymap,
    memory_mb: u64,
    whats_new: Option<WhatsNew>,
    clearing: Option<Clearing>,

    /// Favorites, collections and categories, and where their members are in the library.
    groups: Groups,
    membership: Membership,
    membership_dirty: bool,
    /// Browsing one of them instead of the folder.
    group: Option<i64>,
    /// A group being named in the sidebar.
    edit: Option<GroupEdit>,
    /// A group waiting for "Delete?" to be confirmed.
    confirm_delete: Option<i64>,
    /// Files or a folder waiting for "Delete?" to be confirmed, and a deletion under way.
    confirm_files: Option<DeleteRequest>,
    deleting: Option<Deleting>,
    toast: Option<(String, Instant)>,
    /// The membership changed by the user's hand: an asset that leaves the group being browsed
    /// hands the focus on to the next one.
    follow_focus: bool,

    /// Assets per row and rows per screen in the current view, for the keyboard.
    columns: usize,
    page_rows: usize,
    /// The current view's scroll offset and height, last frame.
    scroll: (f32, f32),
    scroll_to_focus: bool,
}

impl BrowserApp {
    pub fn new(cc: &eframe::CreationContext<'_>, start_root: Option<PathBuf>) -> Self {
        theme::apply(&cc.egui_ctx);
        theme::color_title_bar(&cc.egui_ctx, cc);
        let db_path = db::default_path();
        let conn = db::open(&db_path)
            .map_err(|err| diag::report(&format!("Cache unavailable ({})", db_path.display()), err))
            .ok();
        let ctx = cc.egui_ctx.clone();
        let thumbs = ThumbService::start(db_path.clone(), std::sync::Arc::new(move || ctx.request_repaint()));
        let user_path = db::user_data_path();
        let user_conn = db::open_user(&user_path)
            .map_err(|err| {
                diag::report(&format!("The library's folders can't be saved ({})", user_path.display()), err)
            })
            .ok();
        let (scan_tx, scan_rx) = crossbeam_channel::unbounded();
        let updates = {
            let ctx = cc.egui_ctx.clone();
            Updates::start(std::sync::Arc::new(move || ctx.request_repaint()))
        };
        let watch = {
            let ctx = cc.egui_ctx.clone();
            Watch::start(move |after| ctx.request_repaint_after(after))
        };

        let mut app = Self {
            ctx: cc.egui_ctx.clone(),
            db_path,
            conn,
            user_conn,
            thumbs,
            render_state: cc.wgpu_render_state.clone(),
            roots: Vec::new(),
            library: None,
            library_version: 0,
            index_note: None,
            scan_tx,
            scan_rx,
            scans: 0,
            watch,
            scan_started: Instant::now(),
            last_scan: None,
            mode: Mode::All,
            modes: Mode::ALL.map(ModeView::new),
            view: Vec::new(),
            view_dirty: false,
            selected_dir: 0,
            query: String::new(),
            focus_search: false,
            show_inspector: true,
            folders: FolderTree::default(),
            previews: Previews::new(&cc.egui_ctx),
            inspector: Inspector::new(&cc.egui_ctx, &db::user_data_path()),
            sound: Sound::start(&cc.egui_ctx),
            updates,
            settings: Settings::default(),
            keymap: Keymap::default(),
            memory_mb: memory::DEFAULT_MB,
            whats_new: None,
            clearing: None,
            groups: Groups::open(&db::user_data_path()),
            membership: Membership::default(),
            membership_dirty: true,
            group: None,
            edit: None,
            confirm_delete: None,
            confirm_files: None,
            deleting: None,
            toast: None,
            follow_focus: false,
            columns: 1,
            page_rows: 1,
            scroll: (0.0, 0.0),
            scroll_to_focus: false,
        };
        app.load_settings();
        app.apply_memory();
        app.whats_new_at_start();
        app.open_library(start_root);
        app
    }

    /// Opens the settings at `page`; `None` closes them. (Also for `src/bin/shots.rs`.)
    pub fn show_settings(&mut self, page: Option<crate::ui::settings::Page>) {
        match page {
            Some(page) => self.settings.open_at(page),
            None => self.settings.open = false,
        }
    }

    /// Shows "What's new": every version's notes, or as after an update, the running version's.
    /// (Also for `src/bin/shots.rs`.)
    pub fn show_whats_new(&mut self, every_version: bool) {
        let Ok(current) = semver::Version::parse(&self.updates.version()) else { return };
        self.whats_new = Some(if every_version {
            WhatsNew::every_version(current)
        } else {
            WhatsNew::since(semver::Version::new(0, 0, 0), current)
        });
    }

    pub fn hide_whats_new(&mut self) {
        self.whats_new = None;
    }

    /// Shows what `view` says, with its asset selected but not played. For `src/bin/shots`.
    pub fn show_view(&mut self, view: ScreenshotView) {
        self.set_mode(view.mode);
        self.mode_view_mut().layout = view.layout;
        if let Some(tile) = view.tile {
            self.mode_view_mut().tile = tile.clamp(64.0, 256.0);
        }
        let group = view.group.and_then(|name| match name {
            groups::UNCATEGORIZED_NAME => Some(UNCATEGORIZED),
            name => self.groups.all().iter().find(|group| group.name == name).map(|group| group.id),
        });
        match group {
            Some(id) => self.show_group(Some(id)),
            None => self.select_dir(self.library.as_ref().map_or(0, Library::home)),
        }
        if let Some(sort) = view.sort {
            let in_group = self.in_group_order();
            self.mode_view_mut().set_order(in_group, sort, false);
        }
        let found = (self.library.as_ref().zip(view.file))
            .and_then(|(library, file)| library.entries.iter().position(|entry| entry.rel.ends_with(file)));
        let selection = &mut self.mode_view_mut().selection;
        match found {
            Some(index) => selection.only(index as u32),
            None => selection.clear(),
        }
        self.view_dirty = true;
        self.scroll_to_focus = true;
    }

    /// Shares the memory limit out among what the app keeps (`memory`).
    fn apply_memory(&mut self) {
        let shares = memory::apply(self.memory_mb);
        self.previews.set_max_textures(shares.thumbnails);
    }

    /// After an update, "What's new" for the versions since the one whose notes were last shown.
    fn whats_new_at_start(&mut self) {
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

    /// Shows the library's folders from the cached index at once, then rescans them all in the
    /// background. `start` (from the command line) is added to them, or shown if it's in already.
    fn open_library(&mut self, start: Option<PathBuf>) {
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

    fn save_roots(&self) {
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

    fn update_title(&self) {
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
    fn add_root(&mut self, path: &Path) {
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
    fn remove_root(&mut self, index: usize) {
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
    fn rebuild_library(&mut self, fresh: Vec<(PathBuf, Vec<ScannedFile>)>) {
        let library = (!self.roots.is_empty()).then(|| Library::build(self.current_files(fresh)));
        self.set_library(library);
    }

    /// Puts in a new library, keeping the selections, the folder and the playing sound: entry
    /// indices change, keys don't.
    fn set_library(&mut self, library: Option<Library>) {
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
    fn scan(&mut self, targets: Vec<PathBuf>) {
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
            let mut conn = db::open(&db_path).map_err(|err| diag::report("Saving the index", err)).ok();
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
                if let Some(conn) = &mut conn
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
    fn poll_watch(&mut self, ctx: &egui::Context) {
        self.watch.sync(&self.roots);
        let changed = self.watch.poll();
        self.scan(changed);
        if let Some(due) = self.watch.next_due() {
            ctx.request_repaint_after(due);
        }
    }

    fn poll_scans(&mut self) {
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

    fn mode_view(&self) -> &ModeView {
        &self.modes[self.mode.index()]
    }

    fn mode_view_mut(&mut self) -> &mut ModeView {
        &mut self.modes[self.mode.index()]
    }

    /// Re-finds the groups' members in the library after either changed.
    fn refresh_membership(&mut self) {
        if !self.membership_dirty {
            return;
        }
        self.membership_dirty = false;
        self.membership = match &self.library {
            Some(library) => Membership::build(library, &self.groups),
            None => Membership::default(),
        };
        if self.group.is_some_and(|id| id != UNCATEGORIZED && self.groups.get(id).is_none()) {
            self.group = None;
        }
        // Group views and searches (which match category names) both depend on it.
        self.view_dirty = true;
    }

    /// Whether the browsed assets are in a group's own order (the order they were added) or
    /// ordered like a folder's. Uncategorized was never added to, so it's ordered like a folder.
    fn in_group_order(&self) -> bool {
        self.group.is_some_and(|id| id != UNCATEGORIZED)
    }

    fn rebuild_view(&mut self) {
        self.view_dirty = false;
        let follow_focus = std::mem::take(&mut self.follow_focus);
        let Some(library) = &self.library else {
            self.view.clear();
            return;
        };
        if self.selected_dir as usize >= library.dirs.len() {
            self.selected_dir = library.home();
        }
        let in_group_order = self.in_group_order();
        let query = self.query.to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        let (groups, membership) = (&self.groups, &self.membership);
        let by_category = |index: u32, word: &str| membership.category_matches(groups, index, word);
        let kinds = self.mode.kinds();
        let mode_view = &mut self.modes[self.mode.index()];
        let (sort, descending) = mode_view.order(in_group_order);
        let mut view = match self.group {
            Some(id) => {
                let members = membership.entries.get(&id).map_or(&[][..], Vec::as_slice);
                let mut view: Vec<u32> = members
                    .iter()
                    .copied()
                    .filter(|&index| library.matches(index, kinds, &words, &by_category))
                    .collect();
                // The group's own order is the order things were added.
                if in_group_order && sort == SortBy::Added && descending {
                    view.reverse();
                }
                view
            }
            None => library.filter_with(self.selected_dir, kinds, &query, by_category),
        };
        if !(in_group_order && sort == SortBy::Added) {
            library.sort(&mut view, sort, descending);
        }
        let mut focus_moved = None;
        if !mode_view.selection.is_empty() {
            let shown: HashSet<u32> = view.iter().copied().collect();
            // Put in a category from Uncategorized, say: the next asset (or else the one before)
            // takes the focus, so working through a group keeps its place.
            let next =
                mode_view.selection.focus.filter(|focus| follow_focus && !shown.contains(focus)).and_then(|focus| {
                    let at = self.view.iter().position(|&index| index == focus)?;
                    self.view[at + 1..].iter().chain(self.view[..at].iter().rev()).copied().find(|i| shown.contains(i))
                });
            mode_view.selection.retain(&shown);
            if let Some(next) = next
                && mode_view.selection.is_empty()
            {
                mode_view.selection.only(next);
                focus_moved = Some(next);
            }
        }
        self.view = view;
        if let Some(index) = focus_moved {
            self.focused(index);
            self.scroll_to_focus = true;
        }
    }

    /// Browse a group instead of a folder, or go back to the folder.
    fn show_group(&mut self, group: Option<i64>) {
        if self.group != group {
            self.group = group;
            self.view_dirty = true;
            self.scroll_to_focus = true;
        }
    }

    fn set_mode(&mut self, mode: Mode) {
        if mode == self.mode {
            return;
        }
        self.mode = mode;
        self.view_dirty = true;
        self.scroll_to_focus = true;
        self.save_settings();
    }

    fn select_dir(&mut self, dir: u32) {
        self.selected_dir = dir;
        self.group = None;
        self.view_dirty = true;
        if let Some(library) = &self.library {
            self.folders.reveal(library, dir);
        }
    }

    /// Counts of each kind in what's browsed: the folder or the group.
    fn source_totals(&self) -> [u32; 3] {
        match (self.group, &self.library) {
            (Some(id), _) => self.membership.totals(id),
            (None, Some(library)) => library.dirs.get(self.selected_dir as usize).map_or([0; 3], |dir| dir.totals),
            (None, None) => [0; 3],
        }
    }

    fn paths_of(&self, indices: &[u32]) -> Vec<PathBuf> {
        let Some(library) = &self.library else { return Vec::new() };
        indices.iter().map(|&index| library.path_of(&library.entries[index as usize])).collect()
    }

    fn resolve(&self, targets: Targets) -> Vec<PathBuf> {
        match targets {
            Targets::Selection => self.paths_of(&self.selected_in_order()),
            Targets::Paths(paths) => paths,
        }
    }

    fn notify(&mut self, text: String) {
        self.toast = Some((text, Instant::now()));
    }

    /// Applies what the user asked for from the sidebar, a menu, a star, a drop or the pane.
    fn run_group_command(&mut self, command: GroupCommand) {
        let name_of = |groups: &Groups, id: i64| groups.get(id).map_or_else(String::new, |group| group.name.clone());
        // These can take assets out of the group being browsed (Uncategorized included).
        if matches!(
            command,
            GroupCommand::Add(..)
                | GroupCommand::Remove(..)
                | GroupCommand::ToggleFavorite(_)
                | GroupCommand::FinishEdit(_)
        ) {
            self.follow_focus = true;
        }
        match command {
            GroupCommand::Show(id) => self.show_group(Some(id)),
            GroupCommand::Add(id, targets) => {
                let paths = self.resolve(targets);
                let added = self.groups.add(id, &paths);
                self.membership_dirty = true;
                let name = name_of(&self.groups, id);
                self.notify(match added {
                    0 if paths.is_empty() => return,
                    0 => format!("Already in {name}"),
                    n => format!("Added {} to {name}", plural(n, "asset")),
                });
            }
            GroupCommand::Remove(id, targets) => {
                let keys: Vec<String> = self.resolve(targets).iter().map(|path| groups::path_key(path)).collect();
                let removed = self.groups.remove(id, &keys);
                self.membership_dirty = true;
                if removed > 0 {
                    let name = name_of(&self.groups, id);
                    self.notify(format!(
                        "Took {} out of {name}; the files stay where they are",
                        plural(removed, "asset")
                    ));
                }
            }
            GroupCommand::ToggleFavorite(targets) => {
                let paths = self.resolve(targets);
                if paths.is_empty() {
                    return;
                }
                let keys: Vec<String> = paths.iter().map(|path| groups::path_key(path)).collect();
                if keys.iter().all(|key| self.groups.contains(FAVORITES, key)) {
                    let removed = self.groups.remove(FAVORITES, &keys);
                    self.notify(format!("Took {} out of Favorites", plural(removed, "asset")));
                } else {
                    let added = self.groups.add(FAVORITES, &paths);
                    self.notify(format!("Added {} to Favorites", plural(added, "asset")));
                }
                self.membership_dirty = true;
            }
            GroupCommand::New(kind, targets) => {
                let paths = self.resolve(targets);
                self.edit = Some(GroupEdit::new(kind, self.groups.unused_name(kind), paths));
            }
            GroupCommand::StartRename(id) => {
                self.edit = self.groups.get(id).map(GroupEdit::rename);
            }
            GroupCommand::FinishEdit(edit) => match edit.id {
                Some(id) => {
                    self.groups.rename(id, &edit.name);
                }
                None => {
                    if let Some(id) = self.groups.create(edit.kind, &edit.name) {
                        let added = self.groups.add(id, &edit.add);
                        self.membership_dirty = true;
                        if added > 0 {
                            self.notify(format!("Made {} with {}", edit.name.trim(), plural(added, "asset")));
                        }
                    }
                }
            },
            GroupCommand::Recolor(id, color) => self.groups.set_color(id, color),
            GroupCommand::AskDelete(id) => self.confirm_delete = Some(id),
            GroupCommand::RemoveMissing(id) => {
                let keys = self.membership.missing.get(&id).cloned().unwrap_or_default();
                let removed = self.groups.remove(id, &keys);
                self.membership_dirty = true;
                self.notify(format!("Took out {} that weren't found", plural(removed, "missing asset")));
            }
        }
    }

    /// "Delete this collection?" The files are never touched.
    fn delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(id) = self.confirm_delete else { return };
        let Some(group) = self.groups.get(id) else {
            self.confirm_delete = None;
            return;
        };
        let (name, noun, count) = (group.name.clone(), group.kind.noun(), group.members.len());
        let mut decision = None;
        let modal = egui::Modal::new(egui::Id::new("delete-group")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.label(
                RichText::new(format!("Delete the {noun} \u{201c}{name}\u{201d}?"))
                    .font(egui::FontId::new(15.0, theme::semibold())),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "{} will stay where {} now.",
                    plural(count, "file"),
                    if count == 1 { "it is" } else { "they are" }
                ))
                .color(theme::TEXT_WEAK),
            );
            ui.add_space(12.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::button(ui, icon::TRASH, "Delete").clicked() {
                    decision = Some(true);
                }
                if widgets::button(ui, "", "Cancel").clicked() {
                    decision = Some(false);
                }
            });
        });
        if modal.should_close() && decision.is_none() {
            decision = Some(false);
        }
        match decision {
            Some(true) => {
                if self.groups.delete(id) {
                    self.membership_dirty = true;
                    if self.group == Some(id) {
                        self.show_group(None);
                    }
                    self.notify(format!("Deleted the {noun} \u{201c}{name}\u{201d}"));
                }
                self.confirm_delete = None;
            }
            Some(false) => self.confirm_delete = None,
            None => {}
        }
    }

    /// "Delete these?" for the selection: the Delete key, or the asset menu.
    fn ask_to_delete_selection(&mut self) {
        if self.deleting.is_some() {
            self.notify("Still deleting the last ones…".to_owned());
            return;
        }
        let items = self.selected_in_order();
        let Some(library) = &self.library else { return };
        if items.is_empty() {
            return;
        }
        let entry = |index: u32| &library.entries[index as usize];
        let paths: Vec<PathBuf> = items.iter().map(|&index| library.path_of(entry(index))).collect();
        let names: Vec<&str> = items.iter().take(3).map(|&index| entry(index).name()).collect();
        // One file is named in the question, so this says where it is; several are listed.
        let details = match items.as_slice() {
            &[index] => {
                let root = &library.roots[entry(index).root as usize].name;
                let folder = entry(index).folder();
                if folder.is_empty() {
                    format!("In {root}")
                } else {
                    format!("In {root} › {}", folder.replace('/', " › "))
                }
            }
            _ if items.len() > names.len() => {
                format!("{} and {} more", names.join(", "), grouped(items.len() - names.len()))
            }
            _ => names.join(", "),
        };
        // The asset after them takes the selection afterwards, or else the one before.
        let chosen: HashSet<u32> = items.iter().copied().collect();
        let focus = self.mode_view().selection.focus;
        let at = focus.and_then(|focus| self.view.iter().position(|&index| index == focus)).unwrap_or(0);
        let next = self.view[at..]
            .iter()
            .chain(self.view[..at].iter().rev())
            .find(|index| !chosen.contains(index))
            .map(|&index| entry(index).key);
        let what = match names.as_slice() {
            [name] if items.len() == 1 => format!("\u{201c}{name}\u{201d}"),
            _ => plural(items.len(), "file"),
        };
        self.confirm_files = Some(DeleteRequest {
            title: format!("Delete {what}?"),
            details,
            permanent: !paths.iter().all(|path| platform::has_trash(path)),
            group: self.group.filter(|&id| id != UNCATEGORIZED),
            after: AfterDelete { next, ..AfterDelete::default() },
            paths,
            what,
        });
    }

    /// "Delete this folder?" from its menu in the tree. The library's own folders are only ever
    /// taken out of the library (`remove_root`), never deleted from here.
    fn ask_to_delete_folder(&mut self, dir: u32) {
        if self.deleting.is_some() {
            self.notify("Still deleting the last ones…".to_owned());
            return;
        }
        let Some(library) = &self.library else { return };
        let Some(node) = library.dirs.get(dir as usize).filter(|node| node.root.is_some() && !node.lower.is_empty())
        else {
            return;
        };
        let Some(path) = library.dir_path(dir) else { return };
        let assets: u32 = node.totals.iter().sum();
        let parent = node.parent.and_then(|parent| library.dirs.get(parent as usize)).map(|parent| parent.key.clone());
        let what = format!("the folder \u{201c}{}\u{201d}", node.name);
        self.confirm_files = Some(DeleteRequest {
            title: format!("Delete {what}?"),
            details: format!(
                "Everything in it goes with it: {} and any other files.",
                plural(assets as usize, "asset")
            ),
            permanent: !platform::has_trash(&path),
            group: None,
            after: AfterDelete { next: None, folder: Some(node.key.clone()), parent },
            paths: vec![path],
            what,
        });
    }

    /// "Delete?", saying where things go. Enter deletes when it can be undone; Esc cancels.
    fn delete_files_dialog(&mut self, ctx: &egui::Context) {
        enum Choice {
            Delete,
            /// Take the files out of the group being browsed instead.
            TakeOut(i64),
            Cancel,
        }
        let Some(request) = &self.confirm_files else { return };
        let bin = platform::trash_name();
        let group = request.group.and_then(|id| self.groups.get(id)).map(|group| (group.id, group.name.clone()));
        let mut choice = None;
        let modal = egui::Modal::new(egui::Id::new("delete-files")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new(&request.title).font(egui::FontId::new(15.0, theme::semibold())));
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(&request.details).color(theme::TEXT_WEAK)).wrap());
            ui.add_space(6.0);
            let note = if request.permanent {
                RichText::new(format!("{}  This drive has no {bin}, so this can't be undone.", icon::WARNING))
                    .color(theme::ERROR)
            } else {
                RichText::new(format!("Deleted files and folders go to the {bin}, so they can be put back."))
                    .color(theme::TEXT_FAINT)
            };
            ui.add(egui::Label::new(note.size(12.5)).wrap());
            ui.add_space(12.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if request.permanent { "Delete for good" } else { "Delete" };
                if widgets::button(ui, icon::TRASH, label).clicked() {
                    choice = Some(Choice::Delete);
                }
                if widgets::button(ui, "", "Cancel").clicked() {
                    choice = Some(Choice::Cancel);
                }
                if let Some((id, name)) = &group {
                    let take_out = widgets::button(ui, icon::MINUS_CIRCLE, &format!("Only take out of {name}"));
                    if take_out.on_hover_text("The files stay where they are").clicked() {
                        choice = Some(Choice::TakeOut(*id));
                    }
                }
            });
        });
        let enter = !request.permanent && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter));
        if choice.is_none() && enter {
            choice = Some(Choice::Delete);
        }
        if choice.is_none() && modal.should_close() {
            choice = Some(Choice::Cancel);
        }
        let Some(choice) = choice else { return };
        let Some(request) = self.confirm_files.take() else { return };
        match choice {
            Choice::Delete => self.start_deleting(request),
            Choice::TakeOut(id) => self.run_group_command(GroupCommand::Remove(id, Targets::Paths(request.paths))),
            Choice::Cancel => {}
        }
    }

    fn start_deleting(&mut self, request: DeleteRequest) {
        // A sound that's being deleted stops first.
        if let (Some(library), Some((index, _))) = (&self.library, self.sound.playing)
            && let Some(entry) = library.entries.get(index as usize)
        {
            let playing = library.path_of(entry);
            if request.paths.iter().any(|path| library::is_within(&playing, path)) {
                self.sound.reset();
            }
        }
        let ctx = self.ctx.clone();
        let rx = deletion::start(request.paths, move || ctx.request_repaint());
        self.deleting = Some(Deleting { rx, what: request.what, permanent: request.permanent, after: request.after });
    }

    /// Takes in a finished deletion: drops what went from the library, moves to what's next, and
    /// says how it went.
    fn poll_deleting(&mut self) {
        let Some(job) = self.deleting.take() else { return };
        let deleted = match job.rx.try_recv() {
            Ok(deleted) => deleted,
            Err(crossbeam_channel::TryRecvError::Empty) => {
                self.deleting = Some(job);
                return;
            }
            // The thread couldn't start (that's reported) or died.
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.notify(format!("Couldn't delete {}", job.what));
                return;
            }
        };
        if !deleted.gone.is_empty() {
            // Browsing the deleted folder, or one inside it: its parent is shown instead.
            let browsing_it = job.after.folder.as_ref().is_some_and(|folder| {
                let shown = self.library.as_ref().and_then(|library| library.dirs.get(self.selected_dir as usize));
                self.group.is_none()
                    && shown.is_some_and(|dir| dir.key == *folder || dir.key.starts_with(&format!("{folder}/")))
            });
            self.forget_paths(&deleted.gone);
            if browsing_it
                && let Some(dir) = job.after.parent.as_ref().and_then(|parent| self.library.as_ref()?.find_dir(parent))
            {
                self.select_dir(dir);
            }
            // The asset after the deleted ones takes the selection, as in a file manager. The view
            // is rebuilt first: it held indices into the old library.
            self.refresh_membership();
            if self.view_dirty {
                self.rebuild_view();
            }
            let next = job.after.next.and_then(|key| {
                let library = self.library.as_ref()?;
                library.entries.iter().position(|entry| entry.key == key).map(|index| index as u32)
            });
            if self.mode_view().selection.is_empty()
                && let Some(index) = next.filter(|index| self.view.contains(index))
            {
                self.mode_view_mut().selection.only(index);
                self.scroll_to_focus = true;
                self.focused(index);
            }
        }
        match &deleted.error {
            None if job.permanent => self.notify(format!("Deleted {}", job.what)),
            None => self.notify(format!("Moved {} to the {}", job.what, platform::trash_name())),
            Some(error) => {
                let mut kept: Vec<String> = deleted.kept.iter().take(3).map(|path| platform::native(path)).collect();
                if deleted.kept.len() > kept.len() {
                    kept.push(format!("and {} more", grouped(deleted.kept.len() - kept.len())));
                }
                diag::report(&format!("Couldn't delete {}", kept.join(", ")), error);
                self.notify(if deleted.gone.is_empty() {
                    format!("Couldn't delete {}: see the status bar", job.what)
                } else {
                    format!("Deleted {}, but not {}", plural(deleted.gone.len(), "file"), grouped(deleted.kept.len()))
                });
            }
        }
    }

    /// Drops deleted files and folders from the library at once; a rescan of their library
    /// folders follows, to make sure and to save the index.
    fn forget_paths(&mut self, gone: &[PathBuf]) {
        let Some(library) = &self.library else { return };
        let mut by_root: Vec<(u16, Vec<String>)> = Vec::new();
        for path in gone {
            let Some((root, rel)) = library.place_of(path) else { continue };
            match by_root.iter_mut().find(|(other, _)| *other == root) {
                Some((_, rels)) => rels.push(rel),
                None => by_root.push((root, vec![rel])),
            }
        }
        let fresh: Vec<(PathBuf, Vec<ScannedFile>)> = by_root
            .iter()
            .map(|(root, rels)| (library.roots[*root as usize].path.clone(), library.files_without(*root, rels)))
            .collect();
        if fresh.is_empty() {
            return;
        }
        let rescan: Vec<PathBuf> = fresh.iter().map(|(path, _)| path.clone()).collect();
        self.rebuild_library(fresh);
        self.scan(rescan);
    }

    /// The brief confirmation at the bottom of the window.
    fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some((text, at)) = &self.toast else { return };
        let age = at.elapsed();
        if age > TOAST_TIME {
            self.toast = None;
            return;
        }
        let fade = 1.0 - ((age.as_secs_f32() - (TOAST_TIME.as_secs_f32() - 0.4)) / 0.4).clamp(0.0, 1.0);
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::new("toast"))
            .order(egui::Order::Foreground)
            .interactable(false)
            .pivot(egui::Align2::CENTER_BOTTOM)
            .fixed_pos(screen.center_bottom() + vec2(0.0, -48.0))
            .show(ctx, |ui| {
                ui.set_opacity(fade);
                Frame::new()
                    .fill(egui::Color32::from_rgb(40, 43, 51))
                    .stroke(egui::Stroke::new(1.0, theme::BORDER))
                    .corner_radius(10)
                    .inner_margin(Margin::symmetric(14, 8))
                    .shadow(ui.visuals().popup_shadow)
                    .show(ui, |ui| {
                        let text = RichText::new(format!("{}  {text}", icon::CHECK_CIRCLE)).color(theme::TEXT);
                        ui.add(egui::Label::new(text).extend());
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(50));
    }

    /// The focus moved to `index` (by click or key): a sound starts when auto-play is on, and the
    /// sounds after it are read into memory so moving on to them starts without a disk wait.
    fn focused(&mut self, index: u32) {
        let Some(library) = &self.library else { return };
        if library.entries[index as usize].kind == AssetKind::Audio && self.sound.autoplay {
            self.sound.play(library, index);
        }
        self.sound.read_ahead_around(library, &self.view, index);
    }

    fn apply_browse(&mut self, output: BrowseOutput) {
        if output.columns > 0 {
            self.columns = output.columns;
            self.page_rows = output.page_rows;
        }
        let view = &self.view;
        let selection = &mut self.modes[self.mode.index()].selection;
        if let Some((index, modifiers)) = output.clicked {
            if modifiers.command {
                selection.toggle(index);
            } else if modifiers.shift {
                selection.extend_to(view, index);
            } else {
                selection.only(index);
            }
            if selection.focus == Some(index) && !modifiers.shift {
                self.focused(index);
            }
        }
        let selection = &mut self.modes[self.mode.index()].selection;
        if let Some(index) = output.secondary_clicked
            && !selection.contains(index)
        {
            selection.only(index);
        }
        if let Some(index) = output.double_clicked {
            self.open_entries(&[index]);
        }
        if let Some(action) = output.menu {
            self.run_action(action);
        }
        for command in output.commands {
            self.run_group_command(command);
        }
        if let Some(sort) = output.sort_clicked {
            let in_group = self.in_group_order();
            let view = self.mode_view_mut();
            let (current, descending) = view.order(in_group);
            if current == sort {
                view.set_order(in_group, sort, !descending);
            } else {
                view.set_order(in_group, sort, sort.starts_descending());
            }
            self.view_dirty = true;
        }
    }

    /// The selection in display order, focus first.
    fn selected_in_order(&self) -> Vec<u32> {
        let selection = &self.mode_view().selection;
        let mut items: Vec<u32> = self.view.iter().copied().filter(|&index| selection.contains(index)).collect();
        if let Some(focus) = selection.focus {
            items.retain(|&index| index != focus);
            items.insert(0, focus);
        }
        items
    }

    fn run_action(&mut self, action: MenuAction) {
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

    fn open_entries(&self, items: &[u32]) {
        let Some(library) = &self.library else { return };
        let items = if items.len() > OPEN_MANY_MAX { &items[..1] } else { items };
        for &index in items {
            let _ = platform::open(&library.path_of(&library.entries[index as usize]));
        }
    }

    /// Runs before any widget is drawn and consumes the keys it handles, so a focused button
    /// can't also react to Space or Enter.
    fn handle_keys(&mut self, ctx: &egui::Context) {
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

    fn header(&mut self, ui: &mut egui::Ui) {
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(12, 8));
        let mut action = None;
        egui::Panel::top("header").frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                // The library's folders: click for the list, to go to one, add one or take one out.
                let (glyph, name) = match self.roots.as_slice() {
                    [] => (icon::FOLDER_PLUS, "Add a folder".to_owned()),
                    [root] => (icon::FOLDER_SIMPLE, folder_name(root)),
                    roots => (icon::FOLDERS, format!("{} folders", roots.len())),
                };
                let label = format!("{glyph}  {name}  {}", icon::CARET_DOWN);
                let button = egui::Button::new(RichText::new(label).font(egui::FontId::new(14.0, theme::semibold())))
                    .frame_when_inactive(false)
                    .min_size(vec2(0.0, 30.0));
                let response = ui.add(button);
                match &self.library {
                    Some(library) => {
                        let paths: Vec<String> = self.roots.iter().map(|root| platform::native(root)).collect();
                        let response = response.on_hover_text(format!(
                            "{}\nClick to go to a folder, add one or take one out",
                            paths.join("\n")
                        ));
                        let kinds = self.mode.kinds();
                        egui::Popup::menu(&response).show(|ui| {
                            action = crate::ui::folders::library_menu(ui, library, kinds);
                        });
                    }
                    None => {
                        if response.on_hover_text("Choose a folder of assets, or drop one on the window").clicked() {
                            action = Some(FolderAction::AddRoot);
                        }
                    }
                }
                ui.add_space(8.0);

                let totals = self.library.as_ref().map(|_| self.source_totals());
                // Narrow windows lose the counts first, then the labels.
                let room = ui.available_width();
                let (show_counts, show_labels) = (room > 960.0, room > 780.0);
                let notes: Vec<Option<String>> = Mode::ALL
                    .iter()
                    .map(|mode| {
                        totals.filter(|_| show_counts).map(|totals| {
                            grouped(
                                mode.kinds().iter().zip(totals).filter(|(on, _)| **on).map(|(_, n)| n as usize).sum(),
                            )
                        })
                    })
                    .collect();
                let tips: Vec<String> = Mode::ALL
                    .iter()
                    .map(|mode| {
                        let (tip, action) = match mode {
                            Mode::All => ("Everything", Action::ModeAll),
                            Mode::Images => ("Pictures and textures", Action::ModeImages),
                            Mode::Sounds => ("Sounds, with a player", Action::ModeSounds),
                            Mode::Models => ("3D models, with a 3D view", Action::ModeModels),
                        };
                        format!("{tip}{}", keymap::hint(ui.ctx(), action))
                    })
                    .collect();
                let segments: Vec<widgets::Segment<'_>> = Mode::ALL
                    .iter()
                    .zip(notes)
                    .zip(&tips)
                    .map(|((mode, note), tip)| widgets::Segment {
                        glyph: mode.glyph(),
                        label: if show_labels { mode.label() } else { "" },
                        note,
                        tooltip: tip,
                    })
                    .collect();
                if let Some(index) = widgets::segmented(ui, ui.id().with("modes"), &segments, self.mode.index()) {
                    self.set_mode(Mode::ALL[index]);
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let settings_tip = format!("Settings{}", keymap::hint(ui.ctx(), Action::Settings));
                    if widgets::icon_button(ui, icon::GEAR_SIX, &settings_tip, self.settings.open).clicked() {
                        self.settings.open = true;
                    }
                    widgets::divider(ui);
                    let inspector_tip = format!(
                        "{} the preview pane{}",
                        if self.show_inspector { "Hide" } else { "Show" },
                        keymap::hint(ui.ctx(), Action::TogglePane)
                    );
                    if widgets::icon_button(ui, icon::SIDEBAR_SIMPLE, &inspector_tip, self.show_inspector).clicked() {
                        self.show_inspector = !self.show_inspector;
                    }
                    widgets::divider(ui);
                    let layout = self.mode_view().layout;
                    let list_tip = format!("List{}", keymap::hint(ui.ctx(), Action::List));
                    if widgets::icon_button(ui, icon::LIST, &list_tip, layout == ViewLayout::List).clicked() {
                        self.mode_view_mut().layout = ViewLayout::List;
                        self.scroll_to_focus = true;
                    }
                    let grid_tip = format!("Grid{}", keymap::hint(ui.ctx(), Action::Grid));
                    if widgets::icon_button(ui, icon::SQUARES_FOUR, &grid_tip, layout == ViewLayout::Grid).clicked() {
                        self.mode_view_mut().layout = ViewLayout::Grid;
                        self.scroll_to_focus = true;
                    }
                    widgets::divider(ui);
                    let width = (ui.available_width() - 8.0).clamp(140.0, 340.0);
                    if widgets::search_field(ui, &mut self.query, width, std::mem::take(&mut self.focus_search)) {
                        self.view_dirty = true;
                    }
                });
            });
        });
        if let Some(action) = action {
            self.run_folder_action(action);
        }
    }

    fn run_folder_action(&mut self, action: FolderAction) {
        let dir_path = |app: &Self, dir: u32| app.library.as_ref().and_then(|library| library.dir_path(dir));
        match action {
            FolderAction::Select(dir) => self.select_dir(dir),
            FolderAction::Open(dir) => {
                if let Some(path) = dir_path(self, dir)
                    && let Err(err) = platform::open(&path)
                {
                    diag::report("Opening the folder", err);
                }
            }
            FolderAction::CopyPath(dir) => {
                if let Some(path) = dir_path(self, dir) {
                    self.ctx.copy_text(platform::native(&path));
                    self.notify("Copied the folder's path".to_owned());
                }
            }
            FolderAction::AddRoot => {
                let picked = rfd::FileDialog::new().set_title("Add a folder of assets to the library").pick_folder();
                if let Some(path) = picked {
                    self.add_root(&path);
                }
            }
            FolderAction::RemoveRoot(path) => {
                if let Some(index) = self.roots.iter().position(|root| *root == path) {
                    self.remove_root(index);
                }
            }
            FolderAction::Delete(dir) => self.ask_to_delete_folder(dir),
        }
    }

    /// Folders dropped on the window go into the library.
    fn take_dropped_folders(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> =
            ctx.input(|input| input.raw.dropped_files.iter().map(|file| file.path().to_path_buf()).collect());
        if dropped.is_empty() {
            return;
        }
        let folders: Vec<PathBuf> = dropped.into_iter().filter(|path| path.is_dir()).collect();
        if folders.is_empty() {
            self.notify("Drop a folder to add it to the library".to_owned());
        }
        for folder in folders {
            self.add_root(&folder);
        }
    }

    /// While something is dragged over the window: what dropping it does.
    fn draw_drop_hint(&self, ctx: &egui::Context) {
        if ctx.input(|input| input.raw.hovered_files.is_empty()) {
            return;
        }
        let screen = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop-hint")));
        painter.rect_filled(screen, 0.0, theme::BG.gamma_multiply(0.8));
        let area = screen.shrink(24.0);
        painter.rect_stroke(area, 14.0, egui::Stroke::new(2.0, theme::ACCENT), egui::StrokeKind::Inside);
        painter.text(
            area.center(),
            egui::Align2::CENTER_CENTER,
            format!("{}  Drop a folder to add it to the library", icon::FOLDER_PLUS),
            egui::FontId::new(18.0, theme::semibold()),
            theme::TEXT,
        );
    }

    /// Above the assets: where you are, how many there are, and how they're shown.
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let Some(library) = &self.library else { return };
        // A folder's path, or the group's name with what's not here.
        let crumbs: Vec<(u32, String)> = match self.group {
            Some(_) => Vec::new(),
            None => library
                .dir_chain(self.selected_dir)
                .into_iter()
                // "All folders" only with several.
                .filter(|&dir| dir >= library.home())
                .map(|dir| (dir, library.dirs[dir as usize].name.clone()))
                .collect(),
        };
        // `None` for the kind is Uncategorized.
        let group_title = match self.group {
            Some(UNCATEGORIZED) => Some((None, groups::UNCATEGORIZED_NAME.to_owned(), 0)),
            Some(id) => self.groups.get(id).map(|group| (Some(group.kind), group.name.clone(), group.color)),
            None => None,
        };
        let missing = self.group.and_then(|id| self.membership.missing.get(&id)).map_or(0, Vec::len);
        let elsewhere = self.group.and_then(|id| self.membership.elsewhere.get(&id)).copied().unwrap_or(0);
        let outside = match library.roots.as_slice() {
            [root] => root.name.clone(),
            _ => "the library's folders".to_owned(),
        };
        let mode = self.mode;
        let in_group = self.in_group_order();
        let shown = self.view.len();
        let mut go_to = None;
        let mut command = None;
        ui.horizontal(|ui| {
            ui.set_min_height(32.0);
            ui.spacing_mut().item_spacing.x = 2.0;
            if let Some((kind, name, color)) = &group_title {
                let title = RichText::new(name).font(egui::FontId::new(13.5, theme::semibold())).color(theme::TEXT);
                match kind {
                    Some(GroupKind::Favorites) => {
                        let star = RichText::new(icon::STAR).family(theme::filled_icons()).color(theme::FAVORITE);
                        ui.label(star);
                    }
                    Some(GroupKind::Collection) => {
                        ui.label(RichText::new(icon::STACK).size(14.0).color(theme::TEXT_WEAK));
                    }
                    Some(GroupKind::Category) => {
                        let (dot, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                        ui.painter().circle_filled(dot.center(), 5.0, theme::category_color(*color));
                    }
                    None => {
                        let (dot, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                        ui.painter().circle_stroke(dot.center(), 4.5, egui::Stroke::new(1.5, theme::TEXT_WEAK));
                    }
                }
                ui.add_space(4.0);
                ui.label(title);
            }
            for (i, (dir, name)) in crumbs.iter().enumerate() {
                if i > 0 {
                    ui.label(RichText::new(icon::CARET_RIGHT).size(11.0).color(theme::TEXT_FAINT));
                }
                let last = i + 1 == crumbs.len();
                let text = if last {
                    RichText::new(name).font(egui::FontId::new(13.5, theme::semibold())).color(theme::TEXT)
                } else {
                    RichText::new(name).size(13.0).color(theme::TEXT_WEAK)
                };
                if ui.add(egui::Label::new(text).sense(Sense::click())).clicked() && !last {
                    go_to = Some(*dir);
                }
            }
            ui.add_space(10.0);
            let view = &mut self.modes[mode.index()];
            let mut count = format!("{} {}", grouped(shown), if shown == 1 { "item" } else { "items" });
            if view.selection.len() > 1 {
                count.push_str(&format!(" · {} selected", grouped(view.selection.len())));
            }
            if elsewhere > 0 {
                count.push_str(&format!(" · {} outside {outside}", grouped(elsewhere)));
            }
            ui.label(RichText::new(count).size(12.0).color(theme::TEXT_FAINT));
            if missing > 0
                && let Some(id) = self.group
            {
                ui.add_space(6.0);
                let text = RichText::new(format!("{}  {} missing", icon::WARNING, grouped(missing)))
                    .size(12.0)
                    .color(theme::ERROR);
                let response = ui.add(egui::Label::new(text).sense(Sense::click()));
                if response
                    .on_hover_text("Moved, renamed or deleted since they were added. Click to take them out.")
                    .clicked()
                {
                    command = Some(GroupCommand::RemoveMissing(id));
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if view.layout == ViewLayout::Grid {
                    ui.add_sized(vec2(96.0, 20.0), egui::Slider::new(&mut view.tile, 64.0..=256.0).show_value(false))
                        .on_hover_text("Tile size");
                    ui.label(RichText::new(icon::SQUARES_FOUR).size(14.0).color(theme::TEXT_FAINT));
                }
                let (sort, descending) = view.order(in_group);
                let arrow = if descending { icon::SORT_DESCENDING } else { icon::SORT_ASCENDING };
                let options: &[SortBy] = if in_group { &SortBy::IN_GROUPS } else { &SortBy::ALL };
                let sort_menu = ui.menu_button(format!("{arrow}  {}", sort.label()), |ui| {
                    ui.set_min_width(150.0);
                    for &option in options {
                        if ui.selectable_label(option == sort, option.label()).clicked() {
                            view.set_order(in_group, option, option.starts_descending());
                            self.view_dirty = true;
                            ui.close();
                        }
                    }
                    ui.separator();
                    for (down, glyph, label) in
                        [(false, icon::SORT_ASCENDING, "Ascending"), (true, icon::SORT_DESCENDING, "Descending")]
                    {
                        if ui.selectable_label(descending == down, format!("{glyph}  {label}")).clicked() {
                            view.set_order(in_group, sort, down);
                            self.view_dirty = true;
                            ui.close();
                        }
                    }
                });
                sort_menu.response.on_hover_text("Order");
                // Each mode's own view options.
                match mode {
                    Mode::All | Mode::Sounds => {
                        widgets::toggle_chip(
                            ui,
                            icon::PLAY_CIRCLE,
                            "Auto-play",
                            &mut self.sound.autoplay,
                            "Play sounds as soon as they're selected, by click or arrow key",
                        );
                    }
                    Mode::Images => crate::ui::image_view::backdrop_buttons(ui, &mut self.inspector.image.backdrop),
                    Mode::Models => {
                        widgets::toggle_chip(
                            ui,
                            icon::ARROWS_CLOCKWISE,
                            "Turntable",
                            &mut self.inspector.viewport.auto_rotate,
                            "Turn the model in the preview pane slowly",
                        );
                    }
                }
            });
        });
        if let Some(dir) = go_to {
            self.select_dir(dir);
        }
        if let Some(command) = command {
            self.run_group_command(command);
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(12, 4));
        let mut show = None;
        let mut update = None;
        egui::Panel::bottom("status").frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                let small = |text: String| RichText::new(text).size(11.5).color(theme::TEXT_FAINT);
                if let Some(error) = diag::last() {
                    ui.label(
                        RichText::new(format!("{}  {error}", icon::WARNING_CIRCLE)).size(11.5).color(theme::ERROR),
                    )
                    .on_hover_text(format!("Logged to {}", diag::log_path().display()));
                    widgets::divider(ui);
                }
                if let Some(library) = &self.library {
                    ui.label(small(format!("{} assets", grouped(library.entries.len()))));
                    widgets::divider(ui);
                }
                if self.deleting.is_some() {
                    ui.add(egui::Spinner::new().size(12.0));
                    ui.label(small("Deleting…".to_owned()));
                    widgets::divider(ui);
                }
                if self.scans > 0 {
                    ui.add(egui::Spinner::new().size(12.0));
                    ui.label(small(format!("Scanning… {:.1} s", self.scan_started.elapsed().as_secs_f32())));
                    ui.ctx().request_repaint_after(Duration::from_millis(100));
                } else if let Some(scan) = &self.last_scan {
                    ui.label(small(format!(
                        "Scanned {} in {:.2} s",
                        plural(scan.dirs_visited, "folder"),
                        scan.elapsed.as_secs_f32()
                    )))
                    .on_hover_text(format!(
                        "{} assets; {} other files skipped. {}",
                        grouped(scan.assets),
                        grouped(scan.skipped),
                        self.index_note.as_deref().unwrap_or("")
                    ));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    update = crate::ui::updates::status(ui, &self.updates);
                    widgets::divider(ui);
                    let stats = &self.thumbs.stats;
                    let made = stats.made.load(std::sync::atomic::Ordering::Relaxed);
                    let micros = stats.make_micros.load(std::sync::atomic::Ordering::Relaxed);
                    let from_cache = stats.from_cache.load(std::sync::atomic::Ordering::Relaxed);
                    let failed = stats.failed.load(std::sync::atomic::Ordering::Relaxed);
                    let (queued, busy) = self.thumbs.pending();
                    let average = if made > 0 { micros as f64 / made as f64 / 1000.0 } else { 0.0 };
                    let details = format!(
                        "{} made ({average:.1} ms average on {} threads), {} from the cache, {} couldn't be read",
                        grouped(made as usize),
                        self.thumbs.thread_count(),
                        grouped(from_cache as usize),
                        grouped(failed as usize),
                    );
                    if queued + busy > 0 {
                        ui.label(small(format!("Making previews… {}", grouped(queued + busy)))).on_hover_text(details);
                        ui.add(egui::Spinner::new().size(12.0));
                    } else {
                        ui.label(small(format!("{} previews", grouped((made + from_cache) as usize))))
                            .on_hover_text(details);
                    }
                    // The playing sound, when it isn't the one shown.
                    let focus = self.modes[self.mode.index()].selection.focus;
                    let playing_elsewhere = self.sound.playing.is_some_and(|(index, _)| Some(index) != focus);
                    if playing_elsewhere && let Some(library) = &self.library {
                        widgets::divider(ui);
                        show = sound::now_playing_chip(ui, &mut self.sound, library);
                    }
                });
            });
        });
        if let Some(index) = show {
            self.reveal(index);
        }
        match update {
            Some(UpdateAction::Check) => self.updates.check_now(),
            Some(UpdateAction::Restart) => self.restart_to_update(),
            None => {}
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
    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings.open {
            return;
        }
        let (user_path, log_path) = (db::user_data_path(), diag::log_path());
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

    fn poll_clearing(&mut self) {
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

    /// Shows and selects `index`, switching mode, folder and search as needed.
    fn reveal(&mut self, index: u32) {
        let Some(library) = &self.library else { return };
        let entry = &library.entries[index as usize];
        let (kind, dir) = (entry.kind, entry.dir);
        let in_folder = library.dir_chain(dir).contains(&self.selected_dir);
        let query = self.query.to_lowercase();
        let matches = query.split_whitespace().all(|word| entry.lower.contains(word));
        if !self.mode.kinds()[kind.index()] {
            self.set_mode(Mode::ALL.into_iter().find(|mode| mode.kind() == Some(kind)).unwrap_or(Mode::All));
        }
        if !matches {
            self.query.clear();
        }
        if !in_folder {
            self.select_dir(dir);
        }
        self.view_dirty = true;
        self.mode_view_mut().selection.only(index);
        self.scroll_to_focus = true;
    }

    /// The left side: Favorites, collections and categories, then the folders.
    fn folder_panel(&mut self, ui: &mut egui::Ui) {
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin { left: 6, right: 6, top: 10, bottom: 6 });
        let mut action = None;
        let mut commands = Vec::new();
        egui::Panel::left("folders").resizable(true).default_size(250.0).min_size(160.0).frame(frame).show(ui, |ui| {
            // However many groups there are, the folders keep at least half the height.
            let max_height = (ui.available_height() * 0.5).max(120.0);
            egui::ScrollArea::vertical().id_salt("groups").max_height(max_height).auto_shrink([false, true]).show(
                ui,
                |ui| {
                    let sidebar = Sidebar {
                        groups: &self.groups,
                        membership: &self.membership,
                        kinds: self.mode.kinds(),
                        viewing: self.group,
                        edit: &mut self.edit,
                    };
                    commands = sidebar.show(ui);
                },
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                widgets::section_label(ui, "Folders");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(4.0);
                    let add = widgets::icon_button_sized(ui, icon::PLUS, "Add a folder to the library", false, 20.0);
                    if add.clicked() {
                        action = Some(FolderAction::AddRoot);
                    }
                });
            });
            ui.add_space(2.0);
            let Some(library) = &self.library else {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    let hint = "Folders of assets, browsed together. Press + or drop one on the window.";
                    ui.add(egui::Label::new(RichText::new(hint).size(11.5).color(theme::TEXT_FAINT)).wrap());
                });
                return;
            };
            // No folder is highlighted while a group is browsed.
            let selected = if self.group.is_some() { u32::MAX } else { self.selected_dir };
            if let Some(asked) = self.folders.show(ui, library, self.mode.kinds(), selected) {
                action = Some(asked);
            }
        });
        if let Some(action) = action {
            self.run_folder_action(action);
        }
        for command in commands {
            self.run_group_command(command);
        }
    }

    fn inspector_panel(&mut self, ui: &mut egui::Ui) {
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin::same(12));
        let mut output = None;
        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(360.0)
            .size_range(260.0..=720.0)
            .frame(frame)
            .show_collapsible(ui, &mut self.show_inspector, |ui| {
                let Some(library) = &self.library else {
                    ui.label(RichText::new("Open a folder to see its assets here.").color(theme::TEXT_FAINT));
                    return;
                };
                let totals = match self.group {
                    Some(id) => self.membership.totals(id),
                    None => library.dirs.get(self.selected_dir as usize).map_or([0; 3], |dir| dir.totals),
                };
                let args = InspectorArgs {
                    library,
                    selection: &self.modes[self.mode.index()].selection,
                    previews: &mut self.previews,
                    sound: &mut self.sound,
                    render_state: self.render_state.as_ref(),
                    totals,
                    kinds: self.mode.kinds(),
                    groups: &self.groups,
                    membership: &self.membership,
                };
                output = Some(self.inspector.show(ui, args));
            });
        let Some(output) = output else { return };
        if let Some(command) = output.command {
            self.run_group_command(command);
        }
        if let Some(index) = output.focus {
            let selection = &mut self.mode_view_mut().selection;
            selection.focus = Some(index);
            selection.anchor = Some(index);
            self.focused(index);
            self.scroll_to_focus = true;
        }
        if let Some(action) = output.action {
            self.run_action(action);
        }
    }

    /// The sound controls at the bottom, for when the preview pane is hidden.
    fn player_bar(&mut self, ui: &mut egui::Ui) {
        let Some(library) = &self.library else { return };
        let focus =
            self.mode_view().selection.focus.filter(|&index| library.entries[index as usize].kind == AssetKind::Audio);
        let Some(index) = focus.or(self.sound.playing.map(|(index, _)| index)) else { return };
        if index as usize >= library.entries.len() {
            return;
        }
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(12, 8));
        egui::Panel::bottom("player").frame(frame).show(ui, |ui| {
            let entry = &library.entries[index as usize];
            let waveform = self.previews.waveforms.get(&entry.key).map(|waveform| waveform.as_ref());
            ui.label(RichText::new(entry.name()).font(egui::FontId::new(13.0, theme::semibold())));
            sound::scrubber(
                ui,
                56.0,
                &mut self.sound,
                library,
                index,
                waveform,
                self.previews.failed.contains_key(&entry.key),
            );
            let duration = waveform.map(|waveform| waveform.duration as f64);
            sound::transport(ui, &mut self.sound, library, index, duration);
        });
    }

    fn central(&mut self, ui: &mut egui::Ui) {
        let frame = Frame::new().fill(theme::BG).inner_margin(Margin { left: 12, right: 6, top: 6, bottom: 0 });
        let mut output = None;
        let mut add_folder = false;
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            if self.library.is_none() {
                // No folders yet.
                ui.vertical_centered(|ui| {
                    ui.add_space((ui.available_height() / 2.0 - 80.0).max(0.0));
                    ui.label(RichText::new(icon::FOLDERS).size(44.0).color(theme::TEXT_FAINT));
                    ui.add_space(10.0);
                    ui.label(RichText::new("Add a folder to browse its assets").size(16.0).color(theme::TEXT_WEAK));
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Or drop one on the window. Add as many as you like: they're browsed together.")
                            .size(12.5)
                            .color(theme::TEXT_FAINT),
                    );
                    ui.add_space(14.0);
                    add_folder = widgets::button(ui, icon::FOLDER_PLUS, "Add a folder…").clicked();
                });
                return;
            }
            self.toolbar(ui);
            let Some(library) = self.library.as_ref() else { return };
            if self.view.is_empty() {
                // A folder that's empty only because its first scan hasn't finished.
                let scanning = self.scans > 0
                    && self.group.is_none()
                    && library.dirs.get(self.selected_dir as usize).is_some_and(|dir| dir.totals == [0; 3]);
                ui.centered_and_justified(|ui| {
                    let what = self.mode.kind().map_or("assets", |kind| match kind {
                        AssetKind::Image => "images",
                        AssetKind::Audio => "sounds",
                        AssetKind::Model => "models",
                    });
                    let place = match self.group {
                        Some(UNCATEGORIZED) => "without a category".to_owned(),
                        Some(id) => match self.groups.get(id) {
                            Some(group) if group.kind == GroupKind::Favorites => {
                                "in Favorites yet. Star assets with F or the star on their tile".to_owned()
                            }
                            Some(group) => {
                                format!("in \u{201c}{}\u{201d} yet. Drag assets onto it in the sidebar", group.name)
                            }
                            None => "here".to_owned(),
                        },
                        None => "in this folder".to_owned(),
                    };
                    let text = if scanning {
                        "Scanning…".to_owned()
                    } else if self.query.trim().is_empty() {
                        format!("No {what} {place}.")
                    } else {
                        format!("No {what} here match \u{201c}{}\u{201d}.", self.query.trim())
                    };
                    ui.add(egui::Label::new(RichText::new(text).size(14.0).color(theme::TEXT_FAINT)).wrap());
                });
                return;
            }
            let mode_view = &self.modes[self.mode.index()];
            let scroll_to = std::mem::take(&mut self.scroll_to_focus)
                .then_some(mode_view.selection.focus)
                .flatten()
                .and_then(|focus| self.view.iter().position(|&index| index == focus));
            let now = self.sound.now();
            let (sort, descending) = mode_view.order(self.in_group_order());
            let browse = Browse {
                library,
                view: &self.view,
                selection: &mode_view.selection,
                now: now.as_ref(),
                mode: self.mode,
                sort,
                descending,
                backdrop: self.inspector.image.backdrop,
                scroll_to,
                groups: &self.groups,
                membership: &self.membership,
                viewing: self.group,
            };
            output = Some(match mode_view.layout {
                ViewLayout::Grid => browse::grid(ui, &browse, &mut self.previews, mode_view.tile, &mut self.scroll),
                ViewLayout::List => browse::list(ui, &browse, &mut self.previews, &mut self.scroll),
            });
        });
        if let Some(output) = output {
            self.apply_browse(output);
        }
        if add_folder {
            self.run_folder_action(FolderAction::AddRoot);
        }
    }

    fn load_settings(&mut self) {
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

    fn save_settings(&self) {
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

impl eframe::App for BrowserApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // For the tooltips that name a shortcut.
        keymap::publish(&ctx, &self.keymap);
        self.previews.begin_frame();
        self.poll_scans();
        self.poll_deleting();
        self.poll_clearing();
        self.updates.poll();
        self.poll_watch(&ctx);
        self.take_dropped_folders(&ctx);
        self.previews.drain(&self.thumbs, &ctx);
        self.inspector.poll(&ctx);
        self.refresh_membership();
        if self.view_dirty {
            self.rebuild_view();
        }
        self.handle_keys(&ctx);
        self.header(ui);
        self.status_bar(ui);
        if !self.show_inspector {
            self.player_bar(ui);
        }
        self.folder_panel(ui);
        self.inspector_panel(ui);
        self.refresh_membership();
        if self.view_dirty {
            self.rebuild_view();
        }
        self.central(ui);
        if let Some(library) = &self.library {
            self.previews.submit(&self.thumbs, library);
        }
        self.previews.evict();
        self.delete_dialog(&ctx);
        self.delete_files_dialog(&ctx);
        self.settings_window(&ctx);
        if let Some(whats_new) = &mut self.whats_new
            && !whats_new.show(&ctx)
        {
            self.whats_new = None;
        }
        self.draw_toast(&ctx);
        organize::drag_indicator(&ctx);
        self.draw_drop_hint(&ctx);
        // Keep the playhead moving while something is audible.
        if self.sound.is_audible() {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }

    fn on_exit(&mut self) {
        // Settings first: once the updater starts it may stop the app.
        self.save_settings();
        self.updates.on_exit();
    }
}
