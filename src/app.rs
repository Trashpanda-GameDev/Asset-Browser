//! The window: a header with the modes and search, the folder tree, the browsed assets as a grid or
//! a list, the inspector on the right, and a status bar.
//!
//! Nothing here touches the disk or decodes a file on the UI thread, apart from loading the cached
//! index when a folder is opened. Scanning, previews, large previews and sound arrive from
//! background threads, and the views only draw (and only ask previews for) the rows on screen.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use eframe::egui::{self, Align, Frame, Key, Layout, Margin, Modifiers, RichText, Sense, vec2};
use eframe::egui_wgpu::RenderState;
use egui_phosphor::regular as icon;
use rusqlite::Connection;

use crate::asset::AssetKind;
use crate::library::{Library, SortBy, root_string};
use crate::thumbs::ThumbService;
use crate::ui::browse::{self, Browse, BrowseOutput, MenuAction};
use crate::ui::folders::{FolderTree, root_name};
use crate::ui::image_view::Backdrop;
use crate::ui::inspector::{Inspector, InspectorArgs};
use crate::ui::mode::{Layout as ViewLayout, Mode, ModeView};
use crate::ui::previews::Previews;
use crate::ui::sound::{self, Sound};
use crate::ui::{theme, widgets};
use crate::{db, diag, platform, scan};

pub use crate::ui::format::{format_size, format_time, grouped};

const SETTING_LAST_ROOT: &str = "last_root";
/// More files than this opened at once is surely a slip: only the focused one opens.
const OPEN_MANY_MAX: usize = 12;

struct ScanDone {
    library: Library,
    elapsed: Duration,
    dirs_visited: usize,
    skipped: usize,
}

struct LastScan {
    elapsed: Duration,
    assets: usize,
    dirs_visited: usize,
    skipped: usize,
}

pub struct BrowserApp {
    ctx: egui::Context,
    db_path: PathBuf,
    conn: Option<Connection>,
    thumbs: ThumbService,
    render_state: Option<RenderState>,
    library: Option<Library>,
    index_note: Option<String>,
    scan_rx: Option<Receiver<ScanDone>>,
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
        let db_path = db::default_path();
        let conn = db::open(&db_path)
            .map_err(|err| diag::report(&format!("Cache unavailable ({})", db_path.display()), err))
            .ok();
        let ctx = cc.egui_ctx.clone();
        let thumbs = ThumbService::start(db_path.clone(), std::sync::Arc::new(move || ctx.request_repaint()));
        let root = start_root
            .or_else(|| conn.as_ref().and_then(|conn| db::get_setting(conn, SETTING_LAST_ROOT)).map(PathBuf::from));

        let mut app = Self {
            ctx: cc.egui_ctx.clone(),
            db_path,
            conn,
            thumbs,
            render_state: cc.wgpu_render_state.clone(),
            library: None,
            index_note: None,
            scan_rx: None,
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
            inspector: Inspector::new(&cc.egui_ctx),
            sound: Sound::start(&cc.egui_ctx),
            columns: 1,
            page_rows: 1,
            scroll: (0.0, 0.0),
            scroll_to_focus: false,
        };
        app.load_settings();
        if let Some(root) = root {
            app.open_root(&root, &cc.egui_ctx);
        }
        app
    }

    fn mode_view(&self) -> &ModeView {
        &self.modes[self.mode.index()]
    }

    fn mode_view_mut(&mut self) -> &mut ModeView {
        &mut self.modes[self.mode.index()]
    }

    /// Shows the cached index at once, then rescans in the background.
    fn open_root(&mut self, path: &Path, ctx: &egui::Context) {
        let root = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let root_key = root_string(&root);
        self.library = None;
        self.index_note = None;
        self.view.clear();
        for view in &mut self.modes {
            view.selection.clear();
        }
        self.selected_dir = 0;
        self.folders = FolderTree::default();
        self.previews.clear(&self.thumbs);
        self.sound.reset();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("Asset Browser - {}", platform::native(&root))));

        if let Some(conn) = &self.conn {
            if let Err(err) = db::set_setting(conn, SETTING_LAST_ROOT, &root_key) {
                diag::report("Saving the last folder", err);
            }
            let start = Instant::now();
            match db::load_index(conn, &root_key) {
                Ok(Some(files)) => {
                    self.library = Some(Library::build(&root, files));
                    self.index_note = Some(format!("Index from cache in {} ms", start.elapsed().as_millis()));
                    self.view_dirty = true;
                }
                Ok(None) => {}
                Err(err) => diag::report("Loading the cached index", err),
            }
        }

        let (tx, rx) = crossbeam_channel::bounded(1);
        let db_path = self.db_path.clone();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("scan".into()).spawn(move || {
            let result = scan::scan(&root);
            let saved = db::open(&db_path).and_then(|mut conn| db::save_index(&mut conn, &root_key, &result.files));
            if let Err(err) = saved {
                diag::report("Saving the index", err);
            }
            let library = Library::build(&root, result.files);
            // Fails only if another folder was opened meanwhile; that result is no longer wanted.
            let _ = tx.send(ScanDone {
                library,
                elapsed: result.elapsed,
                dirs_visited: result.dirs_visited,
                skipped: result.skipped_files,
            });
            ctx.request_repaint();
        });
        if let Err(err) = spawned {
            diag::report("Starting the scan", err);
        }
        self.scan_rx = Some(rx);
        self.scan_started = Instant::now();
    }

    fn poll_scan(&mut self) {
        let Some(done) = self.scan_rx.as_ref().and_then(|rx| rx.try_recv().ok()) else { return };
        self.scan_rx = None;
        self.last_scan = Some(LastScan {
            elapsed: done.elapsed,
            assets: done.library.entries.len(),
            dirs_visited: done.dirs_visited,
            skipped: done.skipped,
        });
        if self.library.as_ref().is_some_and(|old| old.same_files_as(&done.library)) {
            return;
        }
        // Keep the selections, folder and playing sound across the swap: entry indices change,
        // keys don't.
        let library = done.library;
        let new_index = library.index_by_key();
        if let Some(old) = &self.library {
            let old_keys: Vec<i64> = old.entries.iter().map(|entry| entry.key).collect();
            for view in &mut self.modes {
                view.selection.remap(&old_keys, &new_index);
            }
            let dir_lower = old.dirs.get(self.selected_dir as usize).map(|dir| dir.lower.clone());
            self.selected_dir = dir_lower.and_then(|lower| library.find_dir(&lower)).unwrap_or(0);
        }
        self.sound.remap(&new_index);
        self.library = Some(library);
        self.view_dirty = true;
    }

    fn rebuild_view(&mut self) {
        self.view_dirty = false;
        let Some(library) = &self.library else {
            self.view.clear();
            return;
        };
        if self.selected_dir as usize >= library.dirs.len() {
            self.selected_dir = 0;
        }
        let mode_view = &mut self.modes[self.mode.index()];
        let mut view = library.filter(self.selected_dir, self.mode.kinds(), &self.query.to_lowercase());
        library.sort(&mut view, mode_view.sort, mode_view.descending);
        if !mode_view.selection.is_empty() {
            let shown: HashSet<u32> = view.iter().copied().collect();
            mode_view.selection.retain(&shown);
        }
        self.view = view;
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
        self.view_dirty = true;
        if let Some(library) = &self.library {
            self.folders.reveal(library, dir);
        }
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
        if let Some(sort) = output.sort_clicked {
            let view = self.mode_view_mut();
            if view.sort == sort {
                view.descending = !view.descending;
            } else {
                view.sort = sort;
                // Biggest and newest first is what people look for.
                view.descending = matches!(sort, SortBy::Size | SortBy::Modified);
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
        let Some(library) = &self.library else { return };
        let items = self.selected_in_order();
        match action {
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
        let command = |key| ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, key));
        if command(Key::F) {
            self.focus_search = true;
        }
        for (key, mode) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4].into_iter().zip(Mode::ALL) {
            if command(key) {
                self.set_mode(mode);
            }
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if command(Key::A) {
            let view = std::mem::take(&mut self.view);
            self.mode_view_mut().selection.all(&view);
            self.view = view;
        }
        if command(Key::C) {
            self.run_action(MenuAction::CopyPaths);
        }

        let pressed = |modifiers, key| ctx.input_mut(|input| input.consume_key(modifiers, key));
        let shift = ctx.input(|input| input.modifiers.shift);
        let modifiers = if shift { Modifiers::SHIFT } else { Modifiers::NONE };
        let columns = self.columns.max(1) as isize;
        let page = columns * self.page_rows.max(1) as isize;
        let len = self.view.len() as isize;
        let mut step = None;
        for (key, delta) in [
            (Key::ArrowLeft, -1),
            (Key::ArrowRight, 1),
            (Key::ArrowUp, -columns),
            (Key::ArrowDown, columns),
            (Key::PageUp, -page),
            (Key::PageDown, page),
            (Key::Home, -len),
            (Key::End, len),
        ] {
            if pressed(modifiers, key) {
                step = Some(delta);
            }
        }
        let enter = pressed(Modifiers::NONE, Key::Enter);
        let space = pressed(Modifiers::NONE, Key::Space);
        let escape = pressed(Modifiers::NONE, Key::Escape);

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

    fn header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(12, 8));
        let mut open = None;
        egui::Panel::top("header").frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                // The folder being browsed; click to open another.
                let name = self.library.as_ref().map_or_else(|| "Open a folder".to_owned(), root_name);
                let label = format!("{}  {name}  {}", icon::FOLDER_SIMPLE, icon::CARET_DOWN);
                let tooltip = match &self.library {
                    Some(library) => format!("{}\nClick to open another folder", platform::native(&library.root)),
                    None => "Choose a folder of assets".to_owned(),
                };
                let button = egui::Button::new(RichText::new(label).font(egui::FontId::new(14.0, theme::semibold())))
                    .frame_when_inactive(false)
                    .min_size(vec2(0.0, 30.0));
                if ui.add(button).on_hover_text(tooltip).clicked() {
                    open = rfd::FileDialog::new().set_title("Choose an asset folder").pick_folder();
                }
                ui.add_space(8.0);

                let totals = self
                    .library
                    .as_ref()
                    .and_then(|library| library.dirs.get(self.selected_dir as usize))
                    .map(|dir| dir.totals);
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
                let segments: Vec<widgets::Segment<'_>> = Mode::ALL
                    .iter()
                    .zip(notes)
                    .map(|(mode, note)| widgets::Segment {
                        glyph: mode.glyph(),
                        label: if show_labels { mode.label() } else { "" },
                        note,
                        tooltip: match mode {
                            Mode::All => "Everything (Ctrl+1)",
                            Mode::Images => "Pictures and textures (Ctrl+2)",
                            Mode::Sounds => "Sounds, with a player (Ctrl+3)",
                            Mode::Models => "3D models, with a 3D view (Ctrl+4)",
                        },
                    })
                    .collect();
                if let Some(index) = widgets::segmented(ui, ui.id().with("modes"), &segments, self.mode.index()) {
                    self.set_mode(Mode::ALL[index]);
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let inspector_tip =
                        if self.show_inspector { "Hide the preview pane" } else { "Show the preview pane" };
                    if widgets::icon_button(ui, icon::SIDEBAR_SIMPLE, inspector_tip, self.show_inspector).clicked() {
                        self.show_inspector = !self.show_inspector;
                    }
                    widgets::divider(ui);
                    let layout = self.mode_view().layout;
                    if widgets::icon_button(ui, icon::LIST, "List", layout == ViewLayout::List).clicked() {
                        self.mode_view_mut().layout = ViewLayout::List;
                        self.scroll_to_focus = true;
                    }
                    if widgets::icon_button(ui, icon::SQUARES_FOUR, "Grid", layout == ViewLayout::Grid).clicked() {
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
        if let Some(dir) = open {
            self.open_root(&dir, ctx);
        }
    }

    /// Above the assets: where you are, how many there are, and how they're shown.
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let Some(library) = &self.library else { return };
        let crumbs: Vec<(u32, String)> = library
            .dir_chain(self.selected_dir)
            .into_iter()
            .map(|dir| (dir, if dir == 0 { root_name(library) } else { library.dirs[dir as usize].name.clone() }))
            .collect();
        let mode = self.mode;
        let shown = self.view.len();
        let mut go_to = None;
        ui.horizontal(|ui| {
            ui.set_min_height(32.0);
            ui.spacing_mut().item_spacing.x = 2.0;
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
            ui.label(RichText::new(count).size(12.0).color(theme::TEXT_FAINT));

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if view.layout == ViewLayout::Grid {
                    ui.add_sized(vec2(96.0, 20.0), egui::Slider::new(&mut view.tile, 64.0..=256.0).show_value(false))
                        .on_hover_text("Tile size");
                    ui.label(RichText::new(icon::SQUARES_FOUR).size(14.0).color(theme::TEXT_FAINT));
                }
                let arrow = if view.descending { icon::SORT_DESCENDING } else { icon::SORT_ASCENDING };
                let sort_menu = ui.menu_button(format!("{arrow}  {}", view.sort.label()), |ui| {
                    ui.set_min_width(150.0);
                    for option in SortBy::ALL {
                        if ui.selectable_label(option == view.sort, option.label()).clicked() {
                            view.sort = option;
                            view.descending = matches!(option, SortBy::Size | SortBy::Modified);
                            self.view_dirty = true;
                            ui.close();
                        }
                    }
                    ui.separator();
                    for (descending, glyph, label) in
                        [(false, icon::SORT_ASCENDING, "Ascending"), (true, icon::SORT_DESCENDING, "Descending")]
                    {
                        if ui.selectable_label(view.descending == descending, format!("{glyph}  {label}")).clicked() {
                            view.descending = descending;
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
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(12, 4));
        let mut show = None;
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
                if self.scan_rx.is_some() {
                    ui.add(egui::Spinner::new().size(12.0));
                    ui.label(small(format!("Scanning… {:.1} s", self.scan_started.elapsed().as_secs_f32())));
                    ui.ctx().request_repaint_after(Duration::from_millis(100));
                } else if let Some(scan) = &self.last_scan {
                    ui.label(small(format!(
                        "Scanned {} folders in {:.2} s",
                        grouped(scan.dirs_visited),
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

    fn folder_panel(&mut self, ui: &mut egui::Ui) {
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin { left: 6, right: 6, top: 10, bottom: 6 });
        let mut clicked = None;
        egui::Panel::left("folders").resizable(true).default_size(250.0).min_size(160.0).frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                widgets::section_label(ui, "Folders");
            });
            ui.add_space(2.0);
            let Some(library) = &self.library else {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(if self.scan_rx.is_some() { "Scanning…" } else { "No folder open" })
                            .color(theme::TEXT_FAINT),
                    );
                });
                return;
            };
            clicked = self.folders.show(ui, library, self.mode.kinds(), self.selected_dir);
        });
        if let Some(dir) = clicked {
            self.select_dir(dir);
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
                let args = InspectorArgs {
                    library,
                    selection: &self.modes[self.mode.index()].selection,
                    previews: &mut self.previews,
                    sound: &mut self.sound,
                    render_state: self.render_state.as_ref(),
                    dir: self.selected_dir,
                    kinds: self.mode.kinds(),
                };
                output = Some(self.inspector.show(ui, args));
            });
        let Some(output) = output else { return };
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
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            if self.library.is_none() {
                ui.centered_and_justified(|ui| {
                    let text =
                        if self.scan_rx.is_some() { "Scanning…" } else { "Open a folder to browse its assets." };
                    ui.label(RichText::new(text).size(15.0).color(theme::TEXT_FAINT));
                });
                return;
            }
            self.toolbar(ui);
            let Some(library) = self.library.as_ref() else { return };
            if self.view.is_empty() {
                ui.centered_and_justified(|ui| {
                    let what = self.mode.kind().map_or("assets", |kind| match kind {
                        AssetKind::Image => "images",
                        AssetKind::Audio => "sounds",
                        AssetKind::Model => "models",
                    });
                    let text = if self.query.trim().is_empty() {
                        format!("No {what} in this folder.")
                    } else {
                        format!("No {what} here match \u{201c}{}\u{201d}.", self.query.trim())
                    };
                    ui.label(RichText::new(text).size(14.0).color(theme::TEXT_FAINT));
                });
                return;
            }
            let mode_view = &self.modes[self.mode.index()];
            let scroll_to = std::mem::take(&mut self.scroll_to_focus)
                .then_some(mode_view.selection.focus)
                .flatten()
                .and_then(|focus| self.view.iter().position(|&index| index == focus));
            let now = self.sound.now();
            let browse = Browse {
                library,
                view: &self.view,
                selection: &mode_view.selection,
                now: now.as_ref(),
                mode: self.mode,
                sort: mode_view.sort,
                descending: mode_view.descending,
                backdrop: self.inspector.image.backdrop,
                scroll_to,
            };
            output = Some(match mode_view.layout {
                ViewLayout::Grid => browse::grid(ui, &browse, &mut self.previews, mode_view.tile, &mut self.scroll),
                ViewLayout::List => browse::list(ui, &browse, &mut self.previews, &mut self.scroll),
            });
        });
        if let Some(output) = output {
            self.apply_browse(output);
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
        }
        let settings: Vec<(&str, &str)> = settings.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect();
        if let Err(err) = db::set_settings(conn, &settings) {
            diag::report("Saving settings", err);
        }
    }
}

impl eframe::App for BrowserApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.previews.begin_frame();
        self.poll_scan();
        self.previews.drain(&self.thumbs, &ctx);
        self.inspector.poll(&ctx);
        if self.view_dirty {
            self.rebuild_view();
        }
        self.handle_keys(&ctx);
        self.header(ui, &ctx);
        self.status_bar(ui);
        if !self.show_inspector {
            self.player_bar(ui);
        }
        self.folder_panel(ui);
        self.inspector_panel(ui);
        if self.view_dirty {
            self.rebuild_view();
        }
        self.central(ui);
        if let Some(library) = &self.library {
            self.previews.submit(&self.thumbs, library);
        }
        self.previews.evict();
        // Keep the playhead moving while something is audible.
        if self.sound.is_audible() {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }

    fn on_exit(&mut self) {
        self.save_settings();
    }
}
