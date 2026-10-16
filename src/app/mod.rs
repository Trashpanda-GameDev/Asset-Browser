//! The window: a header with the modes and search, the folder tree, the browsed assets as a grid or
//! a list, the inspector on the right, and a status bar.
//!
//! Nothing here touches the disk or decodes a file on the UI thread, apart from loading the cached
//! index of the library's folders at the start and when one is added. Scanning, previews, large
//! previews and sound arrive from background threads, and the views only draw (and only ask
//! previews for) the rows on screen.
//!
//! `BrowserApp` holds the window's state; what it does is split by area across this module's files:
//! the library's folders and scans (`library`), what's browsed and selected (`view`), keys and the
//! asset actions (`actions`), favorites, collections and categories (`organizing`), deleting
//! (`deleting`), the settings (`settings`), the window's parts (`header`, `toolbar`, `sidebar`,
//! `panels`, `toast`), and what the screenshots tool drives (`screenshots`).

mod actions;
mod deleting;
mod header;
mod library;
mod organizing;
mod panels;
mod screenshots;
mod settings;
mod sidebar;
mod toast;
mod toolbar;
mod view;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use eframe::egui_wgpu::RenderState;
use rusqlite::Connection;

use crate::groups::{Groups, Membership};
use crate::keymap::{self, Keymap};
use crate::library::Library;
use crate::memory;
use crate::thumbs::ThumbService;
use crate::ui::folders::FolderTree;
use crate::ui::inspector::Inspector;
use crate::ui::mode::{Mode, ModeView};
use crate::ui::organize::{self, GroupEdit};
use crate::ui::previews::Previews;
use crate::ui::settings::Settings;
use crate::ui::sound::Sound;
use crate::ui::theme;
use crate::ui::whats_new::WhatsNew;
use crate::updates::Updates;
use crate::watch::Watch;
use crate::{db, diag};
use deleting::{DeleteRequest, Deleting};
use library::{LastScan, ScanDone};
use settings::Clearing;

pub use crate::ui::format::{format_size, format_time, grouped};
pub use screenshots::ScreenshotView;

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
