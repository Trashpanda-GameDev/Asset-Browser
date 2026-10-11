//! The window: toolbar, folder tree, thumbnail grid and status bar.
//!
//! Nothing here touches the disk or decodes a file on the UI thread, apart from loading the cached
//! index when a folder is opened. Scanning and thumbnails arrive from background threads, and the
//! grid only draws (and only asks thumbnails for) the rows on screen.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use eframe::egui::text::{LayoutJob, TextWrapping};
use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Key, Pos2, Rect, Sense, TextureFilter, TextureHandle, TextureId,
    TextureOptions, pos2, vec2,
};
use rusqlite::Connection;

use crate::asset::AssetKind;
use crate::library::{Entry, Library, root_string};
use crate::thumbs::{Thumb, ThumbJob, ThumbService};
use crate::{db, diag, platform, scan};

/// Thumbnails kept on the GPU. The least recently drawn go first; scrolling back reloads them
/// from the cache in about a millisecond each.
const MAX_TEXTURES: usize = 1500;
/// Limits GPU uploads per frame so a burst of finished thumbnails can't stall a frame.
const UPLOADS_PER_FRAME: usize = 48;
/// Rows beyond the visible ones whose thumbnails are made ahead of scrolling.
const PREFETCH_ROWS_BELOW: usize = 3;
const PREFETCH_ROWS_ABOVE: usize = 1;
const LABEL_HEIGHT: f32 = 18.0;
const GAP: f32 = 8.0;
const SETTING_LAST_ROOT: &str = "last_root";

struct CachedTexture {
    handle: TextureHandle,
    src_size: [u32; 2],
    /// Stored at its original size (pixel art, icons): drawn with sharp, whole pixels.
    unscaled: bool,
    last_used: u64,
}

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
    db_path: PathBuf,
    conn: Option<Connection>,
    thumbs: ThumbService,
    library: Option<Library>,
    index_note: Option<String>,
    scan_rx: Option<Receiver<ScanDone>>,
    scan_started: Instant,
    last_scan: Option<LastScan>,

    /// Indices into `library.entries` that pass the folder, kind and search filters.
    view: Vec<u32>,
    view_dirty: bool,
    selected_dir: u32,
    query: String,
    kinds: [bool; 3],
    /// Index into `library.entries`.
    selected: Option<u32>,
    tile: f32,

    textures: HashMap<i64, CachedTexture>,
    failed: HashMap<i64, String>,
    /// Finished thumbnails waiting for their GPU upload.
    arrived: VecDeque<Thumb>,
    arrived_keys: HashSet<i64>,
    last_wanted: Vec<i64>,
    checker: TextureHandle,
    frame: u64,

    columns: usize,
    /// (scroll offset, viewport height) of the grid last frame.
    grid_scroll: (f32, f32),
    scroll_to_selected: bool,
}

impl BrowserApp {
    pub fn new(cc: &eframe::CreationContext<'_>, start_root: Option<PathBuf>) -> Self {
        let db_path = db::default_path();
        let conn = db::open(&db_path)
            .map_err(|err| diag::report(&format!("Cache unavailable ({})", db_path.display()), err))
            .ok();
        let ctx = cc.egui_ctx.clone();
        let thumbs = ThumbService::start(db_path.clone(), Arc::new(move || ctx.request_repaint()));
        let (light, dark) = (Color32::from_gray(58), Color32::from_gray(44));
        let checker = cc.egui_ctx.load_texture(
            "checker",
            ColorImage::new([2, 2], vec![light, dark, dark, light]),
            TextureOptions::NEAREST_REPEAT,
        );
        let root = start_root
            .or_else(|| conn.as_ref().and_then(|conn| db::get_setting(conn, SETTING_LAST_ROOT)).map(PathBuf::from));

        let mut app = Self {
            db_path,
            conn,
            thumbs,
            library: None,
            index_note: None,
            scan_rx: None,
            scan_started: Instant::now(),
            last_scan: None,
            view: Vec::new(),
            view_dirty: false,
            selected_dir: 0,
            query: String::new(),
            kinds: [true; 3],
            selected: None,
            tile: 128.0,
            textures: HashMap::new(),
            failed: HashMap::new(),
            arrived: VecDeque::new(),
            arrived_keys: HashSet::new(),
            last_wanted: Vec::new(),
            checker,
            frame: 0,
            columns: 1,
            grid_scroll: (0.0, 0.0),
            scroll_to_selected: false,
        };
        if let Some(root) = root {
            app.open_root(&root, &cc.egui_ctx);
        }
        app
    }

    /// Shows the cached index at once, then rescans in the background.
    fn open_root(&mut self, path: &Path, ctx: &egui::Context) {
        let root = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let root_key = root_string(&root);
        self.library = None;
        self.index_note = None;
        self.view.clear();
        self.selected = None;
        self.selected_dir = 0;
        self.textures.clear();
        self.failed.clear();
        self.last_wanted.clear();
        self.thumbs.set_wanted(Vec::new());
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("Asset Browser - {}", platform::native(&root))));

        if let Some(conn) = &self.conn {
            if let Err(err) = db::set_setting(conn, SETTING_LAST_ROOT, &root_key) {
                diag::report("Saving the last folder", err);
            }
            let start = Instant::now();
            match db::load_index(conn, &root_key) {
                Ok(Some(files)) => {
                    self.library = Some(Library::build(&root, files));
                    self.index_note = Some(format!("Index from cache: {} ms", start.elapsed().as_millis()));
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
        // Keep the selection and folder across the swap; entry indices change, keys don't.
        let (selected_key, dir_lower) = match &self.library {
            Some(old) => (
                self.selected.and_then(|index| old.entries.get(index as usize)).map(|entry| entry.key),
                old.dirs.get(self.selected_dir as usize).map(|dir| dir.lower.clone()),
            ),
            None => (None, None),
        };
        let library = done.library;
        self.selected = selected_key
            .and_then(|key| library.entries.iter().position(|entry| entry.key == key))
            .map(|index| index as u32);
        self.selected_dir = dir_lower.and_then(|lower| library.find_dir(&lower)).unwrap_or(0);
        self.library = Some(library);
        self.view_dirty = true;
    }

    fn drain_thumbs(&mut self, ctx: &egui::Context) {
        while let Some(thumb) = self.thumbs.try_recv() {
            self.arrived_keys.insert(thumb.key);
            self.arrived.push_back(thumb);
        }
        for _ in 0..UPLOADS_PER_FRAME {
            let Some(thumb) = self.arrived.pop_front() else { break };
            self.arrived_keys.remove(&thumb.key);
            match thumb.result {
                Ok(pixels) => {
                    let unscaled = pixels.is_unscaled();
                    let options = if unscaled {
                        let mut options = TextureOptions::NEAREST;
                        options.minification = TextureFilter::Linear;
                        options
                    } else {
                        TextureOptions::LINEAR.with_mipmap_mode(Some(TextureFilter::Linear))
                    };
                    let image =
                        ColorImage::from_rgba_unmultiplied([pixels.w as usize, pixels.h as usize], &pixels.rgba);
                    let handle = ctx.load_texture(format!("thumb-{}", thumb.key), image, options);
                    self.textures.insert(
                        thumb.key,
                        CachedTexture {
                            handle,
                            src_size: [pixels.src_w, pixels.src_h],
                            unscaled,
                            last_used: self.frame,
                        },
                    );
                }
                Err(message) => {
                    self.failed.insert(thumb.key, message);
                }
            }
        }
        if !self.arrived.is_empty() {
            ctx.request_repaint();
        }
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
        self.view = library.filter(self.selected_dir, self.kinds, &self.query.to_lowercase());
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut open = None;
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open folder…").clicked() {
                    open = rfd::FileDialog::new().set_title("Choose an asset folder").pick_folder();
                }
                if let Some(library) = &self.library {
                    ui.label(platform::native(&library.root));
                }
                ui.separator();
                let search = egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Search names and folders")
                    .desired_width(220.0);
                if ui.add(search).changed() {
                    self.view_dirty = true;
                }
                ui.separator();
                for kind in AssetKind::ALL {
                    let total = self.library.as_ref().map_or(0, |library| library.kind_totals[kind.index()]);
                    let label = format!("{} {}", kind.label(), grouped(total));
                    if ui.toggle_value(&mut self.kinds[kind.index()], label).changed() {
                        self.view_dirty = true;
                    }
                }
                ui.separator();
                ui.add(egui::Slider::new(&mut self.tile, 48.0..=256.0).text("Size"));
            });
        });
        if let Some(dir) = open {
            self.open_root(&dir, ctx);
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(error) = diag::last() {
                    ui.colored_label(Color32::from_rgb(230, 90, 90), error)
                        .on_hover_text(format!("Logged to {}", diag::log_path().display()));
                    ui.separator();
                }
                if let Some(library) = &self.library {
                    ui.label(format!("{} of {} assets", grouped(self.view.len()), grouped(library.entries.len())));
                    ui.separator();
                }
                if let Some(note) = &self.index_note {
                    ui.label(note);
                    ui.separator();
                }
                if self.scan_rx.is_some() {
                    ui.spinner();
                    ui.label(format!("Scanning… {:.1} s", self.scan_started.elapsed().as_secs_f32()));
                    ui.ctx().request_repaint_after(Duration::from_millis(100));
                } else if let Some(scan) = &self.last_scan {
                    ui.label(format!(
                        "Scan: {} assets in {} folders, {:.2} s ({} other files skipped)",
                        grouped(scan.assets),
                        grouped(scan.dirs_visited),
                        scan.elapsed.as_secs_f32(),
                        grouped(scan.skipped),
                    ));
                }
                ui.separator();
                let stats = &self.thumbs.stats;
                let made = stats.made.load(std::sync::atomic::Ordering::Relaxed);
                let micros = stats.make_micros.load(std::sync::atomic::Ordering::Relaxed);
                let (queued, busy) = self.thumbs.pending();
                let average = if made > 0 { micros as f64 / made as f64 / 1000.0 } else { 0.0 };
                ui.label(format!(
                    "Thumbnails: {} made ({average:.1} ms avg on {} threads), {} from cache, {} failed, {} queued",
                    grouped(made as usize),
                    self.thumbs.thread_count(),
                    grouped(stats.from_cache.load(std::sync::atomic::Ordering::Relaxed) as usize),
                    grouped(stats.failed.load(std::sync::atomic::Ordering::Relaxed) as usize),
                    queued + busy,
                ));
            });
        });
    }

    fn folder_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("folders").resizable(true).default_size(260.0).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("folders").auto_shrink([false; 2]).show(ui, |ui| {
                let Some(library) = &self.library else {
                    ui.weak(if self.scan_rx.is_some() { "Scanning…" } else { "Open a folder to start." });
                    return;
                };
                let mut clicked = None;
                dir_tree(ui, library, 0, self.selected_dir, &mut clicked);
                if let Some(dir) = clicked {
                    self.selected_dir = dir;
                    self.view_dirty = true;
                }
            });
        });
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() || self.view.is_empty() {
            return;
        }
        let (left, right, up, down, enter) = ctx.input(|input| {
            (
                input.key_pressed(Key::ArrowLeft),
                input.key_pressed(Key::ArrowRight),
                input.key_pressed(Key::ArrowUp),
                input.key_pressed(Key::ArrowDown),
                input.key_pressed(Key::Enter),
            )
        });
        let columns = self.columns.max(1) as isize;
        let step = if left {
            -1
        } else if right {
            1
        } else if up {
            -columns
        } else if down {
            columns
        } else {
            0
        };
        if step != 0 {
            let current = self.selected.and_then(|selected| self.view.iter().position(|&index| index == selected));
            let next = match current {
                Some(position) => (position as isize + step).clamp(0, self.view.len() as isize - 1) as usize,
                None => 0,
            };
            self.selected = Some(self.view[next]);
            self.scroll_to_selected = true;
        }
        if enter && let (Some(library), Some(selected)) = (&self.library, self.selected) {
            let _ = platform::open(&library.path_of(&library.entries[selected as usize]));
        }
    }

    fn grid(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            let Some(library) = self.library.as_ref() else {
                ui.centered_and_justified(|ui| {
                    ui.weak(if self.scan_rx.is_some() { "Scanning…" } else { "Open a folder to browse its assets." });
                });
                return;
            };
            if self.view.is_empty() {
                ui.centered_and_justified(|ui| ui.weak("Nothing here matches."));
                return;
            }

            let tile = self.tile;
            let cell = vec2(tile + GAP, tile + LABEL_HEIGHT + GAP);
            let width = ui.available_width();
            let columns = (((width + GAP) / cell.x).floor() as usize).max(1);
            let rows = self.view.len().div_ceil(columns);
            self.columns = columns;
            ui.spacing_mut().item_spacing.y = 0.0;

            let mut scroll = egui::ScrollArea::vertical().id_salt("grid").auto_shrink([false; 2]);
            if std::mem::take(&mut self.scroll_to_selected) {
                let position = self.selected.and_then(|selected| self.view.iter().position(|&index| index == selected));
                if let Some(position) = position {
                    let top = (position / columns) as f32 * cell.y;
                    let (offset, height) = self.grid_scroll;
                    if top < offset {
                        scroll = scroll.vertical_scroll_offset(top);
                    } else if top + cell.y > offset + height {
                        scroll = scroll.vertical_scroll_offset(top + cell.y - height);
                    }
                }
            }

            let view = &self.view;
            let textures = &mut self.textures;
            let failed = &self.failed;
            let checker = self.checker.id();
            let frame = self.frame;
            let selected = self.selected;
            let mut clicked = None;
            let mut visible_rows = 0..0;
            let output = scroll.show_rows(ui, cell.y, rows, |ui, row_range| {
                visible_rows = row_range.clone();
                let pixels_per_point = ui.ctx().pixels_per_point();
                for row in row_range {
                    let (row_rect, _) = ui.allocate_exact_size(vec2(width, cell.y), Sense::hover());
                    for column in 0..columns {
                        let Some(&index) = view.get(row * columns + column) else { break };
                        let entry = &library.entries[index as usize];
                        let rect = Rect::from_min_size(
                            row_rect.min + vec2(column as f32 * cell.x, 0.0),
                            vec2(tile, tile + LABEL_HEIGHT),
                        );
                        let tile_ui = TileUi {
                            library,
                            entry,
                            texture: textures.get_mut(&entry.key),
                            failed: failed.get(&entry.key),
                            checker,
                            frame,
                            selected: selected == Some(index),
                            pixels_per_point,
                        };
                        if tile_ui.show(ui, rect, tile) {
                            clicked = Some(index);
                        }
                    }
                }
            });
            self.grid_scroll = (output.state.offset.y, output.inner_rect.height());
            if let Some(index) = clicked {
                self.selected = Some(index);
            }
            self.request_thumbnails(visible_rows, columns, rows);
        });
    }

    /// Asks for thumbnails of the visible rows first, then the rows just around them.
    fn request_thumbnails(&mut self, visible: std::ops::Range<usize>, columns: usize, rows: usize) {
        let Some(library) = &self.library else { return };
        let below = visible.end..(visible.end + PREFETCH_ROWS_BELOW).min(rows);
        let above = visible.start.saturating_sub(PREFETCH_ROWS_ABOVE)..visible.start;
        let mut wanted = Vec::new();
        for row in visible.chain(below).chain(above.rev()) {
            for column in 0..columns {
                let Some(&index) = self.view.get(row * columns + column) else { break };
                let entry = &library.entries[index as usize];
                if entry.kind == AssetKind::Image
                    && !self.textures.contains_key(&entry.key)
                    && !self.failed.contains_key(&entry.key)
                    && !self.arrived_keys.contains(&entry.key)
                {
                    wanted.push(index);
                }
            }
        }
        let keys: Vec<i64> = wanted.iter().map(|&index| library.entries[index as usize].key).collect();
        if keys == self.last_wanted {
            return;
        }
        let jobs = wanted
            .iter()
            .map(|&index| {
                let entry = &library.entries[index as usize];
                ThumbJob { key: entry.key, path: library.path_of(entry) }
            })
            .collect();
        self.thumbs.set_wanted(jobs);
        self.last_wanted = keys;
    }

    fn evict_textures(&mut self) {
        if self.textures.len() <= MAX_TEXTURES {
            return;
        }
        let mut by_age: Vec<(u64, i64)> =
            self.textures.iter().map(|(&key, texture)| (texture.last_used, key)).collect();
        by_age.sort_unstable();
        let excess = self.textures.len() - MAX_TEXTURES + MAX_TEXTURES / 10;
        for (last_used, key) in by_age.into_iter().take(excess) {
            if last_used == self.frame {
                break;
            }
            self.textures.remove(&key);
        }
    }
}

impl eframe::App for BrowserApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frame += 1;
        self.poll_scan();
        self.drain_thumbs(&ctx);
        self.toolbar(ui, &ctx);
        self.status_bar(ui);
        self.folder_panel(ui);
        if self.view_dirty {
            self.rebuild_view();
        }
        self.handle_keys(&ctx);
        self.grid(ui);
        self.evict_textures();
    }
}

struct TileUi<'a> {
    library: &'a Library,
    entry: &'a Entry,
    texture: Option<&'a mut CachedTexture>,
    failed: Option<&'a String>,
    checker: TextureId,
    frame: u64,
    selected: bool,
    pixels_per_point: f32,
}

impl TileUi<'_> {
    /// Draws one tile; returns true when it was clicked.
    fn show(self, ui: &mut egui::Ui, rect: Rect, tile: f32) -> bool {
        let entry = self.entry;
        let response = ui.interact(rect, egui::Id::new(("tile", entry.key)), Sense::click());
        let painter = ui.painter();
        let visuals = ui.visuals();
        let image_rect = Rect::from_min_size(rect.min, vec2(tile, tile));
        let background = if self.selected {
            visuals.selection.bg_fill
        } else if response.hovered() {
            visuals.widgets.hovered.weak_bg_fill
        } else {
            visuals.faint_bg_color
        };
        painter.rect_filled(image_rect, 4.0, background);
        let inner = image_rect.shrink(4.0);

        let mut source_size = None;
        if let Some(texture) = self.texture {
            texture.last_used = self.frame;
            source_size = Some(texture.src_size);
            let [w, h] = texture.handle.size();
            let fitted = fit_rect(inner, w as f32, h as f32, texture.unscaled, self.pixels_per_point);
            let checker_uv = Rect::from_min_max(Pos2::ZERO, pos2(fitted.width() / 16.0, fitted.height() / 16.0));
            painter.image(self.checker, fitted, checker_uv, Color32::WHITE);
            painter.image(texture.handle.id(), fitted, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        } else {
            let color = if self.failed.is_some() { Color32::from_rgb(220, 90, 90) } else { entry.kind.color() };
            painter.text(
                inner.center(),
                Align2::CENTER_CENTER,
                entry.extension().to_uppercase(),
                FontId::proportional((tile * 0.16).clamp(10.0, 26.0)),
                color,
            );
        }

        let text_color = visuals.text_color();
        let mut job = LayoutJob::simple_singleline(entry.name().to_owned(), FontId::proportional(12.0), text_color);
        job.wrap = TextWrapping { max_width: tile, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        let galley = painter.layout_job(job);
        let label_pos = pos2(rect.center().x - galley.size().x / 2.0, image_rect.max.y + 3.0);
        painter.galley(label_pos, galley, text_color);

        let clicked = response.clicked();
        let path = self.library.path_of(entry);
        if response.double_clicked() {
            let _ = platform::open(&path);
        }
        response.context_menu(|ui| {
            if ui.button("Open").clicked() {
                let _ = platform::open(&path);
                ui.close();
            }
            if ui.button("Show in folder").clicked() {
                let _ = platform::reveal(&path);
                ui.close();
            }
            if ui.button("Copy path").clicked() {
                ui.ctx().copy_text(platform::native(&path));
                ui.close();
            }
        });
        let failed = self.failed;
        response.on_hover_ui(|ui| {
            ui.label(&entry.rel);
            let mut details = format_size(entry.size);
            if let Some([w, h]) = source_size {
                details = format!("{w} × {h} · {details}");
            }
            ui.weak(details);
            if let Some(message) = failed {
                ui.colored_label(Color32::from_rgb(230, 90, 90), format!("Couldn't make a thumbnail: {message}"));
            }
        });
        clicked
    }
}

/// Fits `w` × `h` inside `area`, keeping its shape. Unscaled thumbnails (pixel art) are scaled by
/// whole physical pixels and snapped to the pixel grid, so every pixel stays the same size.
fn fit_rect(area: Rect, w: f32, h: f32, pixel_exact: bool, pixels_per_point: f32) -> Rect {
    let mut scale = (area.width() / w).min(area.height() / h);
    let physical = scale * pixels_per_point;
    if pixel_exact && physical >= 1.0 {
        scale = physical.floor() / pixels_per_point;
    }
    let size = vec2(w * scale, h * scale);
    let rect = Rect::from_center_size(area.center(), size);
    if pixel_exact {
        let min = (rect.min.to_vec2() * pixels_per_point).round() / pixels_per_point;
        Rect::from_min_size(min.to_pos2(), size)
    } else {
        rect
    }
}

fn dir_tree(ui: &mut egui::Ui, library: &Library, index: u32, selected: u32, clicked: &mut Option<u32>) {
    let node = &library.dirs[index as usize];
    let name = if index == 0 { "All assets" } else { node.name.as_str() };
    let text = format!("{name}  {}", grouped(node.total as usize));
    let is_selected = index == selected;
    if node.children.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(ui.spacing().icon_width + ui.spacing().item_spacing.x);
            if ui.selectable_label(is_selected, text).clicked() {
                *clicked = Some(index);
            }
        });
        return;
    }
    let id = ui.make_persistent_id(("dir", &node.lower));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, index == 0)
        .show_header(ui, |ui| {
            if ui.selectable_label(is_selected, text).clicked() {
                *clicked = Some(index);
            }
        })
        .body(|ui| {
            for &child in &node.children {
                dir_tree(ui, library, child, selected, clicked);
            }
        });
}

/// 68945 → "68,945"
pub fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}
