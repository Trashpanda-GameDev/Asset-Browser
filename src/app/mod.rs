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

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use rusqlite::Connection;

use crate::detail::DetailLoader;
use crate::library::Library;
use crate::playback::Playback;
use crate::thumbs::ThumbService;
use crate::ui::folders::FolderTree;
use crate::ui::inspector::Inspector;
use crate::ui::keymap::{self};
use crate::ui::organize::{self};
use crate::ui::previews::Previews;
use crate::ui::theme;
use crate::updates::Updates;
use crate::watch::Watch;
use crate::{db, diag, paths, platform, work};
use deleting::Deletion;
use library::Folders;
use organizing::Organizing;
use settings::Preferences;
use view::Browsing;

pub use crate::ui::format::{format_size, format_time, grouped};
pub use screenshots::ScreenshotView;

pub struct BrowserApp {
    ctx: egui::Context,
    db_path: PathBuf,
    conn: Option<Connection>,
    /// The user data file, for the library's folders.
    user_conn: Option<Connection>,
    render_state: Option<RenderState>,
    /// `None` when there are no folders.
    library: Option<Library>,
    folders: Folders,
    browsing: Browsing,
    organizing: Organizing,
    deletion: Deletion,
    preferences: Preferences,
    chrome: Chrome,

    thumbs: ThumbService,
    previews: Previews,
    inspector: Inspector,
    /// The large preview of the asset in the pane.
    detail: DetailLoader,
    playback: Playback,
    updates: Updates,
}

/// The window's own state: whether the pane is shown, the search box, the toast, the folder tree.
pub(super) struct Chrome {
    show_inspector: bool,
    focus_search: bool,
    toast: Option<(String, Instant)>,
    folder_tree: FolderTree,
}

impl BrowserApp {
    pub fn new(cc: &eframe::CreationContext<'_>, start_root: Option<PathBuf>) -> Self {
        theme::apply(&cc.egui_ctx, platform::system_fonts());
        theme::color_title_bar(&cc.egui_ctx, cc);
        let db_path = paths::cache_file();
        let conn = db::open_cache(&db_path)
            .map_err(|err| diag::report(&format!("Cache unavailable ({})", db_path.display()), err))
            .ok();
        let ctx = cc.egui_ctx.clone();
        let repaint: work::Wake = std::sync::Arc::new(move || ctx.request_repaint());
        let thumbs = ThumbService::start(&db_path, &repaint);
        let user_path = paths::user_data_file();
        let user_conn = db::open_user_data(&user_path)
            .map_err(|err| {
                diag::report(&format!("The library's folders can't be saved ({})", user_path.display()), err)
            })
            .ok();
        let updates = Updates::start(repaint.clone());
        let watch = {
            let ctx = cc.egui_ctx.clone();
            Watch::start(move |after| ctx.request_repaint_after(after))
        };

        let mut app = Self {
            ctx: cc.egui_ctx.clone(),
            db_path,
            conn,
            user_conn,
            render_state: cc.wgpu_render_state.clone(),
            library: None,
            folders: Folders::new(watch),
            browsing: Browsing::default(),
            organizing: Organizing::open(&paths::user_data_file()),
            deletion: Deletion::default(),
            preferences: Preferences::default(),
            chrome: Chrome {
                show_inspector: true,
                focus_search: false,
                toast: None,
                folder_tree: FolderTree::default(),
            },
            thumbs,
            previews: Previews::new(&cc.egui_ctx),
            inspector: Inspector::default(),
            detail: DetailLoader::start(repaint.clone()),
            playback: Playback::start(repaint),
            updates,
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
        keymap::publish(&ctx, &self.preferences.keymap);
        self.previews.begin_frame();
        self.poll_scans();
        self.poll_deleting();
        self.poll_clearing();
        self.updates.poll();
        self.poll_watch(&ctx);
        self.take_dropped_folders(&ctx);
        self.previews.drain(&self.thumbs, &ctx);
        self.poll_detail(&ctx);
        self.refresh_membership();
        if self.browsing.view_dirty {
            self.rebuild_view();
        }
        self.handle_keys(&ctx);
        self.header(ui);
        self.status_bar(ui);
        if !self.chrome.show_inspector {
            self.player_bar(ui);
        }
        self.folder_panel(ui);
        self.inspector_panel(ui);
        self.refresh_membership();
        if self.browsing.view_dirty {
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
        if let Some(whats_new) = &mut self.preferences.whats_new
            && !whats_new.show(&ctx)
        {
            self.preferences.whats_new = None;
        }
        self.draw_toast(&ctx);
        organize::drag_indicator(&ctx);
        sidebar::draw_drop_hint(&ctx);
        // Keep the playhead moving while something is audible.
        if self.playback.is_audible() {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }

    fn on_exit(&mut self) {
        // Settings first: once the updater starts it may stop the app.
        self.save_settings();
        self.updates.on_exit();
    }
}
