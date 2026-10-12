//! Makes previews on a pool of worker threads, on-screen items first: thumbnails for images and
//! models, waveforms for sounds.
//!
//! The UI hands over the full list of previews it wants each frame (`set_wanted`), visible ones
//! first. That list replaces the queue, so items scrolled past are dropped instead of being made
//! before the ones now on screen. Each preview is made once and then served from the SQLite
//! cache, whose writes go through one writer thread in batches.

use std::cell::Cell;
use std::collections::{HashSet, VecDeque};
use std::io::Cursor;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use fast_image_resize as fr;
use image::{ExtendedColorType, ImageEncoder, ImageFormat};

use crate::asset::AssetKind;
use crate::audio::{self, Waveform};
use crate::db::{
    self, CacheWrite, FORMAT_FAILED, FORMAT_JPEG, FORMAT_QOI, FORMAT_RAW_RGBA, StoredThumb, StoredWaveform,
};
use crate::model::{self, ModelInfo};
use crate::{diag, psd_composite};

/// Longest side of a stored thumbnail, in pixels. Images no bigger than this are stored as they
/// are, so pixel art keeps its exact pixels.
pub const THUMB_MAX: u32 = 192;

/// Bump when the way image thumbnails or waveforms are made changes, so old cache entries stop
/// matching.
const THUMB_VERSION: u64 = 1;

/// The same for model previews, kept apart so they can be made again without throwing away the
/// image and sound caches. Starts at 2: version 1 is what model keys were before models had
/// previews.
pub const MODEL_PREVIEW_VERSION: u64 = 2;

pub struct ThumbJob {
    pub key: i64,
    pub path: PathBuf,
    pub kind: AssetKind,
}

pub enum Preview {
    Image(ThumbPixels),
    Sound(Arc<Waveform>),
    /// A model's thumbnail and what's in the file. The picture is a render, `THUMB_MAX` square
    /// with a transparent background (`src_w` and `src_h` 0), or for a .blend the preview
    /// Blender saved (`src_w` × `src_h` its size).
    Model(ThumbPixels, Arc<ModelInfo>),
}

pub struct ThumbPixels {
    pub w: u32,
    pub h: u32,
    pub src_w: u32,
    pub src_h: u32,
    pub rgba: Vec<u8>,
}

impl ThumbPixels {
    /// Stored unscaled: draw it with sharp pixels rather than smoothed.
    pub fn is_unscaled(&self) -> bool {
        self.w == self.src_w && self.h == self.src_h
    }
}

pub struct Thumb {
    pub key: i64,
    pub result: Result<Preview, String>,
    pub from_cache: bool,
    pub micros: u64,
}

#[derive(Default)]
pub struct ThumbStats {
    pub made: AtomicU64,
    pub from_cache: AtomicU64,
    pub failed: AtomicU64,
    pub make_micros: AtomicU64,
}

struct Shared {
    state: Mutex<State>,
    wake_workers: Condvar,
}

#[derive(Default)]
struct State {
    queue: VecDeque<ThumbJob>,
    in_flight: HashSet<i64>,
    shutdown: bool,
}

pub struct ThumbService {
    shared: Arc<Shared>,
    results: Receiver<Thumb>,
    workers: Vec<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    pub stats: Arc<ThumbStats>,
}

/// One worker per core, leaving one for the UI and the OS.
pub fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get()).saturating_sub(1).max(1)
}

impl ThumbService {
    /// `on_result` runs on a worker thread after each thumbnail, e.g. to wake the UI.
    pub fn start(db_path: PathBuf, on_result: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::start_with_threads(db_path, default_threads(), on_result)
    }

    /// Every worker reads its own file, so many reads are in flight at once. Measured on a
    /// 5,400 rpm laptop disk (`bench`, 1,000 fresh images each run, 19 workers): 1 read at a
    /// time made 43 thumbnails/s, 2 made 43/s, 4 made 72/s, all 19 made 86-95/s, and 32 made
    /// 100/s. A spinning disk serves queued requests in the order that needs the least seeking,
    /// so it does best with many queued, and it tops out near 100 random files/s however
    /// they're sent. Capping reads made it slower, so there is no cap.
    pub fn start_with_threads(db_path: PathBuf, threads: usize, on_result: Arc<dyn Fn() + Send + Sync>) -> Self {
        let threads = threads.max(1);
        let shared = Arc::new(Shared { state: Mutex::default(), wake_workers: Condvar::new() });
        let stats = Arc::new(ThumbStats::default());
        let (result_tx, results) = crossbeam_channel::unbounded();
        let (store_tx, store_rx) = crossbeam_channel::unbounded();

        let writer = {
            let db_path = db_path.clone();
            std::thread::Builder::new()
                .name("thumb-writer".into())
                .spawn(move || writer_loop(&db_path, &store_rx))
                .expect("spawn thumbnail writer")
        };
        let workers = (0..threads)
            .map(|index| {
                let worker = Worker {
                    shared: shared.clone(),
                    results: result_tx.clone(),
                    store: store_tx.clone(),
                    on_result: on_result.clone(),
                    stats: stats.clone(),
                };
                let db_path = db_path.clone();
                std::thread::Builder::new()
                    .name(format!("thumb-{index}"))
                    .spawn(move || worker.run(&db_path))
                    .expect("spawn thumbnail worker")
            })
            .collect();

        Self { shared, results, workers, writer: Some(writer), stats }
    }

    pub fn thread_count(&self) -> usize {
        self.workers.len()
    }

    /// Replaces the queue. Jobs already being worked on are skipped.
    pub fn set_wanted(&self, jobs: Vec<ThumbJob>) {
        let mut state = self.shared.state.lock().unwrap();
        state.queue.clear();
        for job in jobs {
            if !state.in_flight.contains(&job.key) {
                state.queue.push_back(job);
            }
        }
        let any = !state.queue.is_empty();
        drop(state);
        if any {
            self.shared.wake_workers.notify_all();
        }
    }

    /// (queued, being made)
    pub fn pending(&self) -> (usize, usize) {
        let state = self.shared.state.lock().unwrap();
        (state.queue.len(), state.in_flight.len())
    }

    pub fn is_in_flight(&self, key: i64) -> bool {
        self.shared.state.lock().unwrap().in_flight.contains(&key)
    }

    pub fn try_recv(&self) -> Option<Thumb> {
        self.results.try_recv().ok()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<Thumb> {
        self.results.recv_timeout(timeout).ok()
    }
}

impl Drop for ThumbService {
    fn drop(&mut self) {
        {
            let mut state = self.shared.state.lock().unwrap();
            state.shutdown = true;
            state.queue.clear();
        }
        self.shared.wake_workers.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        // The writer stops once every worker's sender is gone, after committing what's left.
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

struct Worker {
    shared: Arc<Shared>,
    results: Sender<Thumb>,
    store: Sender<CacheWrite>,
    on_result: Arc<dyn Fn() + Send + Sync>,
    stats: Arc<ThumbStats>,
}

impl Worker {
    fn run(self, db_path: &Path) {
        IN_BUDGET.set(true);
        // Without a cache, thumbnails are still made, just never reused.
        let conn = db::open(db_path).map_err(|err| diag::report("Thumbnail worker cache", err)).ok();
        let mut resizer = fr::Resizer::new();
        loop {
            let job = {
                let mut state = self.shared.state.lock().unwrap();
                loop {
                    if state.shutdown {
                        return;
                    }
                    if let Some(job) = state.queue.pop_front() {
                        state.in_flight.insert(job.key);
                        break job;
                    }
                    state = self.shared.wake_workers.wait(state).unwrap();
                }
            };
            let thumb = self.process(&job, conn.as_ref(), &mut resizer);
            trim_resizer(&mut resizer);
            // Send before leaving `in_flight`, so the UI never re-requests a finished thumbnail.
            let sent = self.results.send(thumb).is_ok();
            self.shared.state.lock().unwrap().in_flight.remove(&job.key);
            if !sent {
                return;
            }
            (self.on_result)();
        }
    }

    fn process(&self, job: &ThumbJob, conn: Option<&rusqlite::Connection>, resizer: &mut fr::Resizer) -> Thumb {
        let start = Instant::now();
        if let Some(result) = conn.and_then(|conn| cached_preview(conn, job)) {
            self.stats.from_cache.fetch_add(1, Ordering::Relaxed);
            return Thumb { key: job.key, result, from_cache: true, micros: start.elapsed().as_micros() as u64 };
        }

        // Some decoders panic on malformed files. Contain it here: the worker carries on and the
        // file counts as failed, instead of staying "in progress" forever.
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| match job.kind {
            AssetKind::Image => make_thumbnail(&job.path, resizer).map(Preview::Image),
            AssetKind::Audio => audio::waveform(&job.path).map(|waveform| Preview::Sound(Arc::new(waveform))),
            AssetKind::Model => {
                model::thumbnail(&job.path, resizer).map(|(pixels, info)| Preview::Model(pixels, Arc::new(info)))
            }
        }))
        .unwrap_or_else(|_| Err("the decoder crashed on this file".into()));
        let micros = start.elapsed().as_micros() as u64;
        if result.is_err() {
            self.stats.failed.fetch_add(1, Ordering::Relaxed);
        }
        // Failures are remembered too, so a broken file isn't decoded again on every visit.
        let _ = self.store.send(to_cache(job, &result));
        self.stats.made.fetch_add(1, Ordering::Relaxed);
        self.stats.make_micros.fetch_add(micros, Ordering::Relaxed);
        Thumb { key: job.key, result, from_cache: false, micros }
    }
}

fn cached_preview(conn: &rusqlite::Connection, job: &ThumbJob) -> Option<Result<Preview, String>> {
    let report = |err: rusqlite::Error| diag::report("Reading the preview cache", err);
    match job.kind {
        AssetKind::Image => db::get_thumb(conn, job.key)
            .map_err(report)
            .ok()
            .flatten()
            .map(|stored| decode_stored(&stored).map(Preview::Image)),
        AssetKind::Audio => db::get_waveform(conn, job.key).map_err(report).ok().flatten().map(decode_waveform),
        AssetKind::Model => {
            let stored = db::get_thumb(conn, job.key).map_err(report).ok().flatten()?;
            if stored.format == FORMAT_FAILED {
                return Some(Err(String::from_utf8_lossy(&stored.data).into_owned()));
            }
            // A picture without its info is made again.
            let info = db::get_model(conn, job.key).map_err(report).ok().flatten()?;
            Some(decode_stored(&stored).map(|pixels| Preview::Model(pixels, Arc::new(info))))
        }
    }
}

fn to_cache(job: &ThumbJob, result: &Result<Preview, String>) -> CacheWrite {
    match (job.kind, result) {
        (_, Ok(Preview::Image(pixels))) => CacheWrite::Thumb(job.key, encode_stored(pixels)),
        (_, Ok(Preview::Sound(waveform))) => CacheWrite::Waveform(job.key, encode_waveform(waveform)),
        (_, Ok(Preview::Model(pixels, info))) => CacheWrite::Model(job.key, encode_stored(pixels), (**info).clone()),
        (AssetKind::Audio, Err(message)) => CacheWrite::Waveform(
            job.key,
            StoredWaveform {
                duration_ms: 0,
                sample_rate: 0,
                channels: 0,
                peaks: Vec::new(),
                error: Some(message.clone()),
            },
        ),
        (_, Err(message)) => CacheWrite::Thumb(
            job.key,
            StoredThumb { w: 0, h: 0, src_w: 0, src_h: 0, format: FORMAT_FAILED, data: message.clone().into_bytes() },
        ),
    }
}

fn encode_waveform(waveform: &Waveform) -> StoredWaveform {
    StoredWaveform {
        duration_ms: (waveform.duration * 1000.0).round() as u32,
        sample_rate: waveform.sample_rate,
        channels: waveform.channels,
        peaks: waveform.peaks.iter().flat_map(|[low, high]| [*low as u8, *high as u8]).collect(),
        error: None,
    }
}

fn decode_waveform(stored: StoredWaveform) -> Result<Preview, String> {
    if let Some(error) = stored.error {
        return Err(error);
    }
    Ok(Preview::Sound(Arc::new(Waveform {
        peaks: stored.peaks.as_chunks::<2>().0.iter().map(|pair| [pair[0] as i8, pair[1] as i8]).collect(),
        duration: stored.duration_ms as f32 / 1000.0,
        sample_rate: stored.sample_rate,
        channels: stored.channels,
    })))
}

fn writer_loop(db_path: &Path, jobs: &Receiver<CacheWrite>) {
    let mut conn = match db::open(db_path) {
        Ok(conn) => conn,
        Err(err) => {
            diag::report("Thumbnail cache writer", err);
            while jobs.recv().is_ok() {}
            return;
        }
    };
    while let Ok(first) = jobs.recv() {
        let mut batch = vec![first];
        let deadline = Instant::now() + Duration::from_millis(100);
        while batch.len() < 512 {
            match jobs.recv_deadline(deadline) {
                Ok(item) => batch.push(item),
                Err(_) => break,
            }
        }
        if let Err(err) = db::put_cache(&mut conn, &batch) {
            diag::report("Saving previews", err);
        }
    }
}

/// How much memory the thumbnail workers may hold at once for decoding. A picture is decoded at
/// full size before it's shrunk, so 19 workers meeting a folder of 4K textures held 3.7 GB at
/// once (measured). Big pictures take turns; small ones never wait.
static DECODING: DecodeBudget = DecodeBudget::new(512 << 20);
/// What decoding a picture holds per pixel: the decoded picture (4 bytes) and, while it's shrunk,
/// the resizer's premultiplied copy when it has transparency (4 more).
const DECODE_BYTES_PER_PIXEL: u64 = 8;

/// What a resizer may keep between pictures. It keeps its working buffers to reuse (for a
/// picture with transparency, a premultiplied copy of the whole picture), so after a big picture
/// every worker kept that much for good: 2.1 GB across 19 workers after a folder of 4K textures
/// (measured). Past this, they're let go after each picture (`trim_resizer`).
const RESIZER_KEEP: usize = 8 << 20;

thread_local! {
    /// Set on the thumbnail workers: what they decode shares `DECODING`. (The preview pane's
    /// loader decodes one picture at a time and never waits.)
    static IN_BUDGET: Cell<bool> = const { Cell::new(false) };
}

/// On a thumbnail worker, waits for room in the decode budget for a `w` × `h` picture and holds
/// it until dropped; elsewhere, there's nothing to wait for.
pub(crate) fn reserve_decode(w: u32, h: u32) -> Option<Reserved<'static>> {
    IN_BUDGET.get().then(|| DECODING.reserve(u64::from(w) * u64::from(h) * DECODE_BYTES_PER_PIXEL))
}

/// Reads an image file and shrinks the picture to fit `THUMB_MAX`.
pub fn make_thumbnail(path: &Path, resizer: &mut fr::Resizer) -> Result<ThumbPixels, String> {
    make_image(path, resizer, THUMB_MAX)
}

/// Reads an image file and shrinks the picture to fit `max` pixels on its longer side. Pictures
/// no bigger are kept as they are.
pub fn make_image(path: &Path, resizer: &mut fr::Resizer, max: u32) -> Result<ThumbPixels, String> {
    if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("psd")) {
        // Reads only the part of the file it needs, so it opens the file itself.
        let sampled = psd_composite::read(path, max)?;
        return shrink(sampled.w, sampled.h, sampled.rgba, sampled.src_w, sampled.src_h, max, resizer);
    }
    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    // Trust the content over the extension (a PNG named .jpg is common); TGA has no signature,
    // so fall back to the extension.
    let reader = || -> Result<image::ImageReader<Cursor<&[u8]>>, String> {
        let mut reader =
            image::ImageReader::new(Cursor::new(&bytes[..])).with_guessed_format().map_err(|err| err.to_string())?;
        if reader.format().is_none()
            && let Ok(format) = ImageFormat::from_path(path)
        {
            reader.set_format(format);
        }
        Ok(reader)
    };
    // The header says how big the picture is before it's decoded; the budget is held meanwhile.
    let _decoding = if IN_BUDGET.get() {
        let (w, h) = reader()?.into_dimensions().map_err(|err| err.to_string())?;
        reserve_decode(w, h)
    } else {
        None
    };
    let image = reader()?.decode().map_err(|err| err.to_string())?;
    let (w, h) = (image.width(), image.height());
    if image.color().has_alpha() {
        shrink(w, h, image.into_rgba8().into_raw(), w, h, max, resizer)
    } else {
        shrink_rgb(w, h, image.into_rgb8().into_raw(), max, resizer)
    }
}

fn resize_options(alpha: bool) -> fr::ResizeOptions {
    fr::ResizeOptions::new().resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::CatmullRom)).use_alpha(alpha)
}

/// Fits `w` × `h` pixels (already reduced from `src_w` × `src_h`, or not) into `max`.
pub(crate) fn shrink(
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    src_w: u32,
    src_h: u32,
    max: u32,
    resizer: &mut fr::Resizer,
) -> Result<ThumbPixels, String> {
    if w == 0 || h == 0 {
        return Err("image has no pixels".into());
    }
    let (thumb_w, thumb_h) = fit_size(src_w, src_h, max);
    if (thumb_w, thumb_h) == (w, h) {
        return Ok(ThumbPixels { w, h, src_w, src_h, rgba });
    }
    // A fully opaque picture (most textures) needs no premultiplied copy of itself.
    let opaque = rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 255);
    let src = fr::images::Image::from_vec_u8(w, h, rgba, fr::PixelType::U8x4).map_err(|err| err.to_string())?;
    let mut dst = fr::images::Image::new(thumb_w, thumb_h, fr::PixelType::U8x4);
    resizer.resize(&src, &mut dst, &resize_options(!opaque)).map_err(|err| err.to_string())?;
    Ok(ThumbPixels { w: thumb_w, h: thumb_h, src_w, src_h, rgba: dst.into_vec() })
}

/// `shrink` for a picture without alpha (JPEGs, many textures): resized as it was decoded, so
/// it's never copied at full size with an alpha channel added.
fn shrink_rgb(w: u32, h: u32, rgb: Vec<u8>, max: u32, resizer: &mut fr::Resizer) -> Result<ThumbPixels, String> {
    if w == 0 || h == 0 {
        return Err("image has no pixels".into());
    }
    let (thumb_w, thumb_h) = fit_size(w, h, max);
    let rgb = if (thumb_w, thumb_h) == (w, h) {
        rgb
    } else {
        let src = fr::images::Image::from_vec_u8(w, h, rgb, fr::PixelType::U8x3).map_err(|err| err.to_string())?;
        let mut dst = fr::images::Image::new(thumb_w, thumb_h, fr::PixelType::U8x3);
        resizer.resize(&src, &mut dst, &resize_options(false)).map_err(|err| err.to_string())?;
        dst.into_vec()
    };
    let rgba = rgb.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 255]).collect();
    Ok(ThumbPixels { w: thumb_w, h: thumb_h, src_w: w, src_h: h, rgba })
}

/// Lets a resizer's working buffers go when they've grown past `RESIZER_KEEP`. Call after each
/// picture on a thread that keeps its resizer.
pub fn trim_resizer(resizer: &mut fr::Resizer) {
    if resizer.size_of_internal_buffers() > RESIZER_KEEP {
        resizer.reset_internal_buffers();
    }
}

/// Memory for decoding, shared by the threads that decode: each waits, in turn, until what it
/// needs fits. A picture alone always goes ahead, however big.
struct DecodeBudget {
    state: Mutex<BudgetState>,
    changed: Condvar,
    limit: u64,
}

struct BudgetState {
    used: u64,
    /// Tickets, so a big picture isn't overtaken forever by small ones.
    next: u64,
    serving: u64,
}

impl DecodeBudget {
    const fn new(limit: u64) -> Self {
        Self { state: Mutex::new(BudgetState { used: 0, next: 0, serving: 0 }), changed: Condvar::new(), limit }
    }

    fn reserve(&self, bytes: u64) -> Reserved<'_> {
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let ticket = state.next;
        state.next += 1;
        while state.serving != ticket || (state.used > 0 && state.used + bytes > self.limit) {
            state = self.changed.wait(state).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        state.serving += 1;
        state.used += bytes;
        // The next in line may fit too.
        self.changed.notify_all();
        Reserved { budget: self, bytes }
    }
}

/// Holds its share of the budget until dropped (also when a decoder panics).
pub(crate) struct Reserved<'a> {
    budget: &'a DecodeBudget,
    bytes: u64,
}

impl Drop for Reserved<'_> {
    fn drop(&mut self) {
        let mut state = self.budget.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.used -= self.bytes;
        self.budget.changed.notify_all();
    }
}

/// The size `w` × `h` scaled down, keeping its aspect, to fit `max` on its longer side.
pub(crate) fn fit_size(w: u32, h: u32, max: u32) -> (u32, u32) {
    if w <= max && h <= max {
        return (w, h);
    }
    let scale = max as f64 / w.max(h) as f64;
    (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1))
}

/// JPEG for opaque thumbnails (small), QOI when there's transparency (lossless and fast).
fn encode_stored(pixels: &ThumbPixels) -> StoredThumb {
    let ThumbPixels { w, h, src_w, src_h, .. } = *pixels;
    let opaque = pixels.rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 255);
    let mut data = Vec::new();
    let encoded = if opaque {
        let rgb: Vec<u8> = pixels.rgba.as_chunks::<4>().0.iter().flat_map(|px| [px[0], px[1], px[2]]).collect();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut data, 85)
            .encode(&rgb, w, h, ExtendedColorType::Rgb8)
            .map(|()| FORMAT_JPEG)
    } else {
        image::codecs::qoi::QoiEncoder::new(&mut data)
            .write_image(&pixels.rgba, w, h, ExtendedColorType::Rgba8)
            .map(|()| FORMAT_QOI)
    };
    match encoded {
        Ok(format) => StoredThumb { w, h, src_w, src_h, format, data },
        Err(_) => StoredThumb { w, h, src_w, src_h, format: FORMAT_RAW_RGBA, data: pixels.rgba.clone() },
    }
}

fn decode_stored(stored: &StoredThumb) -> Result<ThumbPixels, String> {
    let rgba = match stored.format {
        FORMAT_FAILED => return Err(String::from_utf8_lossy(&stored.data).into_owned()),
        FORMAT_RAW_RGBA => stored.data.clone(),
        format => {
            let format = if format == FORMAT_QOI { ImageFormat::Qoi } else { ImageFormat::Jpeg };
            image::load_from_memory_with_format(&stored.data, format)
                .map_err(|err| err.to_string())?
                .into_rgba8()
                .into_raw()
        }
    };
    Ok(ThumbPixels { w: stored.w, h: stored.h, src_w: stored.src_w, src_h: stored.src_h, rgba })
}

/// Identifies one version of one file's preview: a different path, size or modification time is
/// a different key, and so is a new way of making previews of its kind. FNV-1a, because its
/// output never changes between Rust versions.
///
/// The root's separators are normalised so a file gets the same key whichever of its parent
/// folders was opened.
pub fn cache_key(root: &str, rel: &str, size: u64, mtime: i64, kind: AssetKind) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for &byte in bytes {
            hash ^= u64::from(if byte == b'\\' { b'/' } else { byte });
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    feed(root.trim_end_matches(['/', '\\']).as_bytes());
    feed(b"/");
    feed(rel.as_bytes());
    feed(&size.to_le_bytes());
    feed(&mtime.to_le_bytes());
    let version = match kind {
        AssetKind::Image | AssetKind::Audio => THUMB_VERSION,
        AssetKind::Model => MODEL_PREVIEW_VERSION,
    };
    feed(&version.to_le_bytes());
    hash as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelInfo;

    #[test]
    fn big_decodes_take_turns() {
        let budget: &'static DecodeBudget = Box::leak(Box::new(DecodeBudget::new(100)));
        let first = budget.reserve(60);
        let small = budget.reserve(30);
        assert_eq!(budget.state.lock().unwrap().used, 90, "both fit");
        let (tx, rx) = std::sync::mpsc::channel();
        let waiting = std::thread::spawn(move || {
            let held = budget.reserve(60);
            tx.send(()).unwrap();
            drop(held);
        });
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err(), "60 more doesn't fit beside 90");
        drop(first);
        drop(small);
        rx.recv_timeout(Duration::from_secs(5)).expect("goes ahead once there's room");
        waiting.join().unwrap();
        // One picture alone always goes ahead, however big.
        drop(budget.reserve(1_000));
        assert_eq!(budget.state.lock().unwrap().used, 0);
    }

    #[test]
    fn resizers_let_big_buffers_go() {
        let mut resizer = fr::Resizer::new();
        // Half transparent, so the resizer needs its premultiplied copy: 2048 x 2048 x 4 bytes.
        let rgba: Vec<u8> = (0..2048 * 2048).flat_map(|i| [200, 100, 50, (i % 2 * 255) as u8]).collect();
        let small = shrink(2048, 2048, rgba, 2048, 2048, 64, &mut resizer).unwrap();
        assert_eq!((small.w, small.h), (64, 64));
        assert!(resizer.size_of_internal_buffers() > RESIZER_KEEP, "it keeps the copy for next time");
        trim_resizer(&mut resizer);
        assert_eq!(resizer.size_of_internal_buffers(), 0);
    }

    #[test]
    fn opaque_pictures_shrink_without_alpha() {
        let mut resizer = fr::Resizer::new();
        let rgb: Vec<u8> = std::iter::repeat_n([10u8, 20, 30], 400 * 200).flatten().collect();
        let small = shrink_rgb(400, 200, rgb, 100, &mut resizer).unwrap();
        assert_eq!((small.w, small.h, small.src_w, small.src_h), (100, 50, 400, 200));
        assert!(small.rgba.as_chunks::<4>().0.iter().all(|px| *px == [10, 20, 30, 255]));
        assert!(resizer.size_of_internal_buffers() < RESIZER_KEEP, "no full-size copy");
        // Small enough already: kept as it is, with alpha added.
        let tiny = shrink_rgb(2, 1, vec![1, 2, 3, 4, 5, 6], 100, &mut resizer).unwrap();
        assert_eq!(tiny.rgba, [1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn image_and_sound_keys_are_unchanged() {
        // Computed by the version before model previews: users have these thumbnails cached.
        let rock = cache_key("F:\\Resources", "3D/Textures/rock.png", 123456, 1700000000, AssetKind::Image);
        assert_eq!(rock, -9117893502861981840);
        let hit = cache_key("F:\\Resources\\", "Sounds/hit.wav", 98765, -5, AssetKind::Audio);
        assert_eq!(hit, -7692076259545324008);
        let bomb = "3D/Platformer Pack by Quaternius/FBX/Bomb.fbx";
        assert_eq!(cache_key("F:\\Resources", bomb, 4242, 1534000000, AssetKind::Model), -7762848195514578235);
    }

    #[test]
    fn model_preview_round_trips_through_the_cache() {
        let path = std::env::temp_dir().join(format!("asset_browser_model_cache_{}.sqlite", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        let mut conn = db::open(&path).unwrap();
        let info = ModelInfo {
            format: "FBX 7.4 binary".into(),
            vertices: 1234,
            triangles: 2000,
            meshes: 3,
            materials: 2,
            textures: 1,
            missing_textures: 1,
            size: [1.5, 2.0, 0.25],
            animations: vec![("Idle\tloop".into(), 2.5), ("Run".into(), 0.75)],
            geometry: true,
        };
        let mut rgba = vec![0u8; 4 * 4 * 4];
        rgba[0..4].copy_from_slice(&[10, 20, 30, 255]);
        let pixels = ThumbPixels { w: 4, h: 4, src_w: 0, src_h: 0, rgba };
        let job = ThumbJob { key: 7, path: PathBuf::from("a.fbx"), kind: AssetKind::Model };
        let failed = ThumbJob { key: 8, path: PathBuf::from("b.fbx"), kind: AssetKind::Model };
        let writes = [
            to_cache(&job, &Ok(Preview::Model(pixels, Arc::new(info.clone())))),
            to_cache(&failed, &Err("there are no meshes in this file".into())),
        ];
        db::put_cache(&mut conn, &writes).unwrap();

        let Some(Ok(Preview::Model(back, back_info))) = cached_preview(&conn, &job) else { panic!("cached model") };
        assert_eq!((back.w, back.h, back.src_w, back.src_h), (4, 4, 0, 0));
        assert_eq!(&back.rgba[0..4], &[10, 20, 30, 255], "QOI keeps pixels exact");
        // Tabs can't survive the text column; they become spaces.
        let mut expected = info;
        expected.animations[0].0 = "Idle loop".into();
        assert_eq!(*back_info, expected);
        let Some(Err(message)) = cached_preview(&conn, &failed) else { panic!("cached failure") };
        assert_eq!(message, "there are no meshes in this file");
        let unknown = ThumbJob { key: 9, path: PathBuf::from("c.fbx"), kind: AssetKind::Model };
        assert!(cached_preview(&conn, &unknown).is_none());
        drop(conn);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
}
