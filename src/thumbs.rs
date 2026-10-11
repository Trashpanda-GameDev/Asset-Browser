//! Makes previews on a pool of worker threads, on-screen items first: thumbnails for images,
//! waveforms for sounds.
//!
//! The UI hands over the full list of previews it wants each frame (`set_wanted`), visible ones
//! first. That list replaces the queue, so items scrolled past are dropped instead of being made
//! before the ones now on screen. Each preview is made once and then served from the SQLite
//! cache, whose writes go through one writer thread in batches.

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
use crate::{diag, psd_composite};

/// Longest side of a stored thumbnail, in pixels. Images no bigger than this are stored as they
/// are, so pixel art keeps its exact pixels.
pub const THUMB_MAX: u32 = 192;

/// Bump when the way thumbnails are made changes, so old cache entries stop matching.
const THUMB_VERSION: u64 = 1;

pub struct ThumbJob {
    pub key: i64,
    pub path: PathBuf,
    pub kind: AssetKind,
}

pub enum Preview {
    Image(ThumbPixels),
    Sound(Arc<Waveform>),
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
            AssetKind::Model => Err("models have no preview yet".to_owned()),
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
        AssetKind::Model => None,
    }
}

fn to_cache(job: &ThumbJob, result: &Result<Preview, String>) -> CacheWrite {
    match (job.kind, result) {
        (_, Ok(Preview::Image(pixels))) => CacheWrite::Thumb(job.key, encode_stored(pixels)),
        (_, Ok(Preview::Sound(waveform))) => CacheWrite::Waveform(job.key, encode_waveform(waveform)),
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

/// Reads an image file and shrinks the picture to fit `THUMB_MAX`.
pub fn make_thumbnail(path: &Path, resizer: &mut fr::Resizer) -> Result<ThumbPixels, String> {
    if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("psd")) {
        // Reads only the part of the file it needs, so it opens the file itself.
        let sampled = psd_composite::read(path, THUMB_MAX)?;
        return shrink(sampled.w, sampled.h, sampled.rgba, sampled.src_w, sampled.src_h, resizer);
    }
    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    // Trust the content over the extension (a PNG named .jpg is common); TGA has no signature,
    // so fall back to the extension.
    let mut reader =
        image::ImageReader::new(Cursor::new(&bytes)).with_guessed_format().map_err(|err| err.to_string())?;
    if reader.format().is_none()
        && let Ok(format) = ImageFormat::from_path(path)
    {
        reader.set_format(format);
    }
    let image = reader.decode().map_err(|err| err.to_string())?.into_rgba8();
    let (w, h) = image.dimensions();
    shrink(w, h, image.into_raw(), w, h, resizer)
}

/// Fits `w` × `h` pixels (already reduced from `src_w` × `src_h`, or not) into `THUMB_MAX`.
fn shrink(
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    src_w: u32,
    src_h: u32,
    resizer: &mut fr::Resizer,
) -> Result<ThumbPixels, String> {
    if w == 0 || h == 0 {
        return Err("image has no pixels".into());
    }
    let (thumb_w, thumb_h) = fit_size(src_w, src_h, THUMB_MAX);
    if (thumb_w, thumb_h) == (w, h) {
        return Ok(ThumbPixels { w, h, src_w, src_h, rgba });
    }
    let src = fr::images::Image::from_vec_u8(w, h, rgba, fr::PixelType::U8x4).map_err(|err| err.to_string())?;
    let mut dst = fr::images::Image::new(thumb_w, thumb_h, fr::PixelType::U8x4);
    let options = fr::ResizeOptions::new().resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::CatmullRom));
    resizer.resize(&src, &mut dst, &options).map_err(|err| err.to_string())?;
    Ok(ThumbPixels { w: thumb_w, h: thumb_h, src_w, src_h, rgba: dst.into_vec() })
}

fn fit_size(w: u32, h: u32, max: u32) -> (u32, u32) {
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

/// Identifies one version of one file: a different path, size or modification time is a
/// different key. FNV-1a, because its output never changes between Rust versions.
///
/// The root's separators are normalised so a file gets the same key whichever of its parent
/// folders was opened.
pub fn cache_key(root: &str, rel: &str, size: u64, mtime: i64) -> i64 {
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
    feed(&THUMB_VERSION.to_le_bytes());
    hash as i64
}
