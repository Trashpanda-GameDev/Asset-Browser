//! The window: toolbar, folder tree, preview grid, sound player and status bar.
//!
//! Nothing here touches the disk or decodes a file on the UI thread, apart from loading the cached
//! index when a folder is opened. Scanning, previews and sound arrive from background threads, and
//! the grid only draws (and only asks previews for) the rows on screen.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use eframe::egui::text::{LayoutJob, TextWrapping};
use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Key, Pos2, Rect, RichText, Sense, TextureFilter, TextureHandle,
    TextureId, TextureOptions, pos2, vec2,
};
use rusqlite::Connection;

use crate::asset::AssetKind;
use crate::audio::{NowPlaying, Player, ReadAhead, Source, Waveform};
use crate::library::{Entry, Library, root_string};
use crate::thumbs::{Preview, Thumb, ThumbJob, ThumbService};
use crate::{db, diag, platform, scan};

/// Thumbnails kept on the GPU. The least recently drawn go first; scrolling back reloads them
/// from the cache in about a millisecond each.
const MAX_TEXTURES: usize = 1500;
/// Limits GPU uploads per frame so a burst of finished thumbnails can't stall a frame.
const UPLOADS_PER_FRAME: usize = 48;
/// Rows beyond the visible ones whose previews are made ahead of scrolling.
const PREFETCH_ROWS_BELOW: usize = 3;
const PREFETCH_ROWS_ABOVE: usize = 1;
/// Sounds read into memory ahead of the selection, in the direction of travel.
const READ_AHEAD_SOUNDS: usize = 3;
const LABEL_HEIGHT: f32 = 18.0;
const GAP: f32 = 8.0;
const PLAYER_WAVEFORM_HEIGHT: f32 = 64.0;
const SETTING_LAST_ROOT: &str = "last_root";
const ERROR_COLOR: Color32 = Color32::from_rgb(230, 90, 90);

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

/// What the player bar asked for, applied once the bar is drawn.
enum PlayerAction {
    Play(u32),
    PlayFrom(u32, f64),
    SetPaused(bool),
    Seek(f64),
    Stop,
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
    waveforms: HashMap<i64, Arc<Waveform>>,
    failed: HashMap<i64, String>,
    /// Finished previews waiting to be taken in (thumbnails wait for their GPU upload).
    arrived: VecDeque<Thumb>,
    arrived_keys: HashSet<i64>,
    last_wanted: Vec<i64>,
    checker: TextureHandle,
    frame: u64,

    player: Option<Player>,
    read_ahead: ReadAhead,
    /// The entry last sent to the player: (index, key).
    playing: Option<(u32, i64)>,
    autoplay: bool,
    looping: bool,
    volume: f32,

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
        let ctx = cc.egui_ctx.clone();
        let player = Player::start(Arc::new(move || ctx.request_repaint()))
            .map_err(|err| diag::report("No sound output", err))
            .ok();
        let volume = 0.8;
        if let Some(player) = &player {
            player.set_volume(volume);
        }
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
            waveforms: HashMap::new(),
            failed: HashMap::new(),
            arrived: VecDeque::new(),
            arrived_keys: HashSet::new(),
            last_wanted: Vec::new(),
            checker,
            frame: 0,
            player,
            read_ahead: ReadAhead::start(),
            playing: None,
            autoplay: true,
            looping: false,
            volume,
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
        self.playing = None;
        if let Some(player) = &self.player {
            player.stop();
        }
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
        // Keep the selection, folder and playing sound across the swap; entry indices change, keys
        // don't.
        let (selected_key, dir_lower) = match &self.library {
            Some(old) => (
                self.selected.and_then(|index| old.entries.get(index as usize)).map(|entry| entry.key),
                old.dirs.get(self.selected_dir as usize).map(|dir| dir.lower.clone()),
            ),
            None => (None, None),
        };
        let library = done.library;
        let find = |key: i64| library.entries.iter().position(|entry| entry.key == key).map(|index| index as u32);
        self.selected = selected_key.and_then(find);
        self.playing = self.playing.and_then(|(_, key)| find(key).map(|index| (index, key)));
        self.selected_dir = dir_lower.and_then(|lower| library.find_dir(&lower)).unwrap_or(0);
        self.library = Some(library);
        self.view_dirty = true;
    }

    fn drain_thumbs(&mut self, ctx: &egui::Context) {
        while let Some(thumb) = self.thumbs.try_recv() {
            self.arrived_keys.insert(thumb.key);
            self.arrived.push_back(thumb);
        }
        let mut uploads = 0;
        while uploads < UPLOADS_PER_FRAME {
            let Some(thumb) = self.arrived.pop_front() else { break };
            self.arrived_keys.remove(&thumb.key);
            match thumb.result {
                Ok(Preview::Image(pixels)) => {
                    uploads += 1;
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
                Ok(Preview::Sound(waveform)) => {
                    self.waveforms.insert(thumb.key, waveform);
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

    /// Selects an entry. A sound starts playing straight away when auto-play is on, and the sounds
    /// after it are read into memory so moving on to them starts without a disk wait.
    fn select(&mut self, index: u32) {
        self.selected = Some(index);
        let Some(library) = &self.library else { return };
        if library.entries[index as usize].kind == AssetKind::Audio && self.autoplay {
            self.play(index);
        }
        self.read_ahead_around(index);
    }

    fn play(&mut self, index: u32) {
        let (Some(library), Some(player)) = (&self.library, &self.player) else { return };
        let entry = &library.entries[index as usize];
        let source = match self.read_ahead.get(entry.key) {
            Some(bytes) => Source::Memory(bytes),
            None => Source::File(library.path_of(entry)),
        };
        player.play(entry.key, source, entry.extension());
        self.playing = Some((index, entry.key));
    }

    fn read_ahead_around(&self, index: u32) {
        let Some(library) = &self.library else { return };
        let Some(position) = self.view.iter().position(|&i| i == index) else { return };
        let is_sound = |&&i: &&u32| library.entries[i as usize].kind == AssetKind::Audio;
        let after = self.view[position + 1..].iter().filter(is_sound).take(READ_AHEAD_SOUNDS);
        let before = self.view[..position].iter().rev().filter(is_sound).take(1);
        let files = after
            .chain(before)
            .map(|&i| {
                let entry = &library.entries[i as usize];
                (entry.key, library.path_of(entry), entry.size)
            })
            .collect();
        self.read_ahead.want(files);
    }

    /// Space: pause or resume the selected sound, or start it if it isn't the one playing.
    fn toggle_play(&mut self) {
        let (Some(library), Some(selected)) = (&self.library, self.selected) else { return };
        let entry = &library.entries[selected as usize];
        if entry.kind != AssetKind::Audio {
            return;
        }
        let now = self.player.as_ref().and_then(Player::now);
        match (&self.player, now) {
            (Some(player), Some(now)) if now.key == entry.key && !now.finished => player.set_paused(!now.paused),
            _ => self.play(selected),
        }
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
                ui.toggle_value(&mut self.autoplay, "Auto-play")
                    .on_hover_text("Play sounds as soon as they're selected, by click or arrow key");
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
                    ui.colored_label(ERROR_COLOR, error)
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
                    "Previews: {} made ({average:.1} ms avg on {} threads), {} from cache, {} failed, {} queued",
                    grouped(made as usize),
                    self.thumbs.thread_count(),
                    grouped(stats.from_cache.load(std::sync::atomic::Ordering::Relaxed) as usize),
                    grouped(stats.failed.load(std::sync::atomic::Ordering::Relaxed) as usize),
                    queued + busy,
                ));
            });
        });
    }

    /// The bar for the selected sound (or, when no sound is selected, the one playing): transport,
    /// time, volume and a waveform to click or drag to a position.
    fn player_panel(&mut self, ui: &mut egui::Ui) {
        let Some(library) = &self.library else { return };
        let shown = self
            .selected
            .filter(|&index| library.entries[index as usize].kind == AssetKind::Audio)
            .or(self.playing.map(|(index, _)| index));
        let Some(index) = shown else { return };
        let entry = &library.entries[index as usize];
        let now = self.player.as_ref().and_then(Player::now).filter(|now| now.key == entry.key);
        let waveform = self.waveforms.get(&entry.key);
        let duration = now.as_ref().and_then(|now| now.duration).or(waveform.map(|w| w.duration as f64));
        let mut action = None;

        egui::Panel::bottom("player").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let Some(player) = &self.player else {
                    ui.colored_label(ERROR_COLOR, "No sound output. See the error in the status bar.");
                    return;
                };
                let running = now.as_ref().is_some_and(|now| !now.finished);
                let audible = now.as_ref().is_some_and(|now| !now.finished && !now.paused);
                let play_button = ui.button(if audible { "⏸" } else { "▶" }).on_hover_text("Play or pause (Space)");
                if play_button.clicked() {
                    action = Some(if running { PlayerAction::SetPaused(audible) } else { PlayerAction::Play(index) });
                }
                if ui.button("⏹").on_hover_text("Stop (Esc)").clicked() {
                    action = Some(PlayerAction::Stop);
                }
                if ui.toggle_value(&mut self.looping, "Loop").changed() {
                    player.set_looping(self.looping);
                }
                ui.separator();
                ui.label(RichText::new(entry.name()).strong());
                let position = now.as_ref().map_or(0.0, |now| now.position);
                let total = duration.map_or_else(|| "?".to_owned(), format_time);
                ui.monospace(format!("{} / {total}", format_time(position)));
                if let Some(ready) = now.as_ref().and_then(|now| now.ready_ms) {
                    ui.weak(format!("started in {ready:.0} ms"))
                        .on_hover_text("From the request to the first audio handed to the sound card");
                }
                if let Some(error) = now.as_ref().and_then(|now| now.error.as_deref()) {
                    ui.colored_label(ERROR_COLOR, error);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let slider = egui::Slider::new(&mut self.volume, 0.0..=1.0).show_value(false);
                    if ui.add(slider).changed() {
                        player.set_volume(self.volume);
                    }
                    ui.label("Volume");
                    ui.weak(&player.device_name);
                });
            });

            let (rect, response) =
                ui.allocate_exact_size(vec2(ui.available_width(), PLAYER_WAVEFORM_HEIGHT), Sense::click_and_drag());
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
            let progress = match (&now, duration) {
                (Some(now), Some(total)) if total > 0.0 && !now.finished => Some((now.position / total) as f32),
                _ => None,
            };
            let accent = ui.visuals().selection.bg_fill;
            match waveform {
                Some(waveform) => {
                    let color = entry.kind.color().gamma_multiply(0.55);
                    draw_waveform(
                        &painter,
                        rect.shrink2(vec2(2.0, 4.0)),
                        waveform,
                        color,
                        entry.kind.color(),
                        progress,
                    );
                }
                None => {
                    let text =
                        if self.failed.contains_key(&entry.key) { "Couldn't read this sound" } else { "Reading…" };
                    painter.text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        text,
                        FontId::proportional(13.0),
                        ui.visuals().weak_text_color(),
                    );
                }
            }
            let fraction_at = |x: f32| ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            if let Some(progress) = progress {
                let x = rect.left() + progress * rect.width();
                painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], (1.5, accent));
            }
            if let Some(pointer) = response.interact_pointer_pos().or(response.hover_pos()) {
                // Where a click would jump to.
                let x = rect.left() + fraction_at(pointer.x) * rect.width();
                painter.line_segment(
                    [pos2(x, rect.top()), pos2(x, rect.bottom())],
                    (1.0, ui.visuals().text_color().gamma_multiply(0.5)),
                );
            }
            if (response.clicked() || response.drag_stopped())
                && let (Some(pointer), Some(total)) = (response.interact_pointer_pos(), duration)
            {
                let seconds = fraction_at(pointer.x) as f64 * total;
                action = Some(match &now {
                    Some(now) if !now.finished => PlayerAction::Seek(seconds),
                    _ => PlayerAction::PlayFrom(index, seconds),
                });
            }
            ui.add_space(4.0);
        });

        let Some(action) = action else { return };
        match action {
            PlayerAction::Play(index) => self.play(index),
            PlayerAction::PlayFrom(index, seconds) => {
                self.play(index);
                if let Some(player) = &self.player {
                    player.seek(seconds);
                }
            }
            PlayerAction::SetPaused(paused) => {
                if let Some(player) = &self.player {
                    player.set_paused(paused);
                }
            }
            PlayerAction::Seek(seconds) => {
                if let Some(player) = &self.player {
                    player.seek(seconds);
                }
            }
            PlayerAction::Stop => {
                if let Some(player) = &self.player {
                    player.stop();
                }
            }
        }
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

    /// Runs before any widget is drawn and consumes the keys it handles, so a focused button
    /// can't also react to Space or Enter.
    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let pressed = |key| ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key));
        let (left, right, up, down) =
            (pressed(Key::ArrowLeft), pressed(Key::ArrowRight), pressed(Key::ArrowUp), pressed(Key::ArrowDown));
        let (enter, space, escape) = (pressed(Key::Enter), pressed(Key::Space), pressed(Key::Escape));

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
        if step != 0 && !self.view.is_empty() {
            let current = self.selected.and_then(|selected| self.view.iter().position(|&index| index == selected));
            let next = match current {
                Some(position) => (position as isize + step).clamp(0, self.view.len() as isize - 1) as usize,
                None => 0,
            };
            if current != Some(next) {
                self.select(self.view[next]);
            }
            self.scroll_to_selected = true;
        }
        if enter && let (Some(library), Some(selected)) = (&self.library, self.selected) {
            let _ = platform::open(&library.path_of(&library.entries[selected as usize]));
        }
        if space {
            self.toggle_play();
        }
        if escape && let Some(player) = &self.player {
            player.stop();
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

            let now = self.player.as_ref().and_then(Player::now);
            let view = &self.view;
            let textures = &mut self.textures;
            let waveforms = &self.waveforms;
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
                            waveform: waveforms.get(&entry.key),
                            playing: now.as_ref().filter(|now| now.key == entry.key && !now.finished),
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
                self.select(index);
            }
            self.request_previews(visible_rows, columns, rows);
        });
    }

    /// Asks for previews of the visible rows first, then the rows just around them.
    fn request_previews(&mut self, visible: std::ops::Range<usize>, columns: usize, rows: usize) {
        let Some(library) = &self.library else { return };
        let below = visible.end..(visible.end + PREFETCH_ROWS_BELOW).min(rows);
        let above = visible.start.saturating_sub(PREFETCH_ROWS_ABOVE)..visible.start;
        let mut wanted = Vec::new();
        for row in visible.chain(below).chain(above.rev()) {
            for column in 0..columns {
                let Some(&index) = self.view.get(row * columns + column) else { break };
                let entry = &library.entries[index as usize];
                let has_preview = match entry.kind {
                    AssetKind::Image => self.textures.contains_key(&entry.key),
                    AssetKind::Audio => self.waveforms.contains_key(&entry.key),
                    AssetKind::Model => true, // none to make yet
                };
                if !has_preview && !self.failed.contains_key(&entry.key) && !self.arrived_keys.contains(&entry.key) {
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
                ThumbJob { key: entry.key, path: library.path_of(entry), kind: entry.kind }
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
        if self.view_dirty {
            self.rebuild_view();
        }
        self.handle_keys(&ctx);
        self.toolbar(ui, &ctx);
        self.status_bar(ui);
        self.player_panel(ui);
        self.folder_panel(ui);
        if self.view_dirty {
            self.rebuild_view();
        }
        self.grid(ui);
        self.evict_textures();
        // Keep the playhead moving while something is audible.
        if self.player.as_ref().and_then(Player::now).is_some_and(|now| !now.finished && !now.paused) {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }
}

struct TileUi<'a> {
    library: &'a Library,
    entry: &'a Entry,
    texture: Option<&'a mut CachedTexture>,
    waveform: Option<&'a Arc<Waveform>>,
    playing: Option<&'a NowPlaying>,
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
        } else if let Some(waveform) = self.waveform {
            let color = entry.kind.color();
            // The decoder can't always tell a file's length up front; the waveform always knows it.
            let progress = self.playing.map(|now| {
                let total = now.duration.unwrap_or(waveform.duration as f64).max(f64::EPSILON);
                (now.position / total) as f32
            });
            let wave_rect = Rect::from_min_max(inner.min, pos2(inner.max.x, inner.max.y - 14.0));
            draw_waveform(painter, wave_rect, waveform, color.gamma_multiply(0.55), color, progress);
            painter.text(
                inner.right_bottom(),
                Align2::RIGHT_BOTTOM,
                format_time(waveform.duration as f64),
                FontId::proportional(11.0),
                visuals.weak_text_color(),
            );
            if self.playing.is_some() {
                painter.text(inner.left_bottom(), Align2::LEFT_BOTTOM, "▶", FontId::proportional(11.0), color);
            }
        } else {
            let color = if self.failed.is_some() { ERROR_COLOR } else { entry.kind.color() };
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
        let (failed, waveform) = (self.failed, self.waveform);
        response.on_hover_ui(|ui| {
            ui.label(&entry.rel);
            let mut details = vec![];
            if let Some([w, h]) = source_size {
                details.push(format!("{w} × {h}"));
            }
            if let Some(waveform) = waveform {
                details.push(format_time(waveform.duration as f64));
                details.push(format!("{:.1} kHz", waveform.sample_rate as f32 / 1000.0));
                details.push(match waveform.channels {
                    1 => "mono".to_owned(),
                    2 => "stereo".to_owned(),
                    n => format!("{n} channels"),
                });
            }
            details.push(format_size(entry.size));
            ui.weak(details.join(" · "));
            if let Some(message) = failed {
                ui.colored_label(ERROR_COLOR, format!("Couldn't make a preview: {message}"));
            }
        });
        clicked
    }
}

/// Draws a waveform as one mesh: a bar per column from the lowest to the highest sample in it.
/// Columns before `progress` (0..1) use `played`.
fn draw_waveform(
    painter: &egui::Painter,
    rect: Rect,
    waveform: &Waveform,
    color: Color32,
    played: Color32,
    progress: Option<f32>,
) {
    let peaks = &waveform.peaks;
    if peaks.is_empty() || rect.width() < 1.0 {
        return;
    }
    let columns = rect.width() as usize;
    let column_width = rect.width() / columns as f32;
    let (middle, half) = (rect.center().y, rect.height() / 2.0);
    let played_until = progress.map(|p| rect.left() + p * rect.width());
    let mut mesh = egui::Mesh::default();
    for column in 0..columns {
        let start = column * peaks.len() / columns;
        let end = ((column + 1) * peaks.len() / columns).clamp(start + 1, peaks.len());
        let (low, high) =
            peaks[start..end].iter().fold((i8::MAX, i8::MIN), |(low, high), [l, h]| (low.min(*l), high.max(*h)));
        let x = rect.left() + column as f32 * column_width;
        // Silence still shows as a thin line.
        let top = (middle - high as f32 / 127.0 * half).min(middle - 0.5);
        let bottom = (middle - low as f32 / 127.0 * half).max(middle + 0.5);
        let fill = if played_until.is_some_and(|until| x < until) { played } else { color };
        mesh.add_colored_rect(Rect::from_min_max(pos2(x, top), pos2(x + column_width, bottom)), fill);
    }
    painter.add(egui::Shape::mesh(mesh));
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

/// 0.35 → "0.35 s", 4.2 → "4.2 s", 75 → "1:15"
pub fn format_time(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    if seconds < 1.0 {
        format!("{seconds:.2} s")
    } else if seconds < 60.0 {
        format!("{seconds:.1} s")
    } else {
        let whole = seconds as u64;
        format!("{}:{:02}", whole / 60, whole % 60)
    }
}
