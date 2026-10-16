//! Makes previews on a pool of worker threads, on-screen items first: thumbnails for images and
//! models, waveforms for sounds.
//!
//! The UI hands over the full list of previews it wants each frame (`set_wanted`), visible ones
//! first. That list replaces the queue, so items scrolled past are dropped instead of being made
//! before the ones now on screen. Each preview is made once and then served from the SQLite
//! cache, whose writes go through one writer thread in batches.

use std::collections::{HashSet, VecDeque};
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
use crate::diag;
use crate::imaging::{self, ThumbPixels};
use crate::model::{self, LoadOptions, ModelInfo, blend, raster};
use crate::storage::{self, Part};
use crate::work::{self, Wake};

/// Longest side of a stored thumbnail, in pixels. Images no bigger than this are stored as they
/// are, so pixel art keeps its exact pixels.
pub const THUMB_MAX: u32 = 192;

/// Longest side of the textures loaded for thumbnails. A model fills at most 192 pixels, so more
/// texture detail would only be averaged away.
pub const THUMB_TEXTURE: u32 = 256;

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

pub struct Thumb {
    pub key: i64,
    pub result: Result<Preview, String>,
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
    /// To the writer, for clearing the cache in turn with what it saves.
    store: Option<Sender<Store>>,
    pub stats: Arc<ThumbStats>,
}

/// What the cache writer is given: a preview to save, or parts of the cache to clear (answered on
/// the channel when done).
enum Store {
    Write(CacheWrite),
    Clear(Vec<Part>, Sender<Result<(), String>>),
}

/// One worker per core, leaving one for the UI and the OS.
pub fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get()).saturating_sub(1).max(1)
}

impl ThumbService {
    /// `on_result` runs on a worker thread after each thumbnail, e.g. to wake the UI.
    pub fn start(db_path: &Path, on_result: &Wake) -> Self {
        Self::start_with_threads(db_path, default_threads(), on_result)
    }

    /// Every worker reads its own file, so many reads are in flight at once. Measured on a
    /// 5,400 rpm laptop disk (`bench`, 1,000 fresh images each run, 19 workers): 1 read at a
    /// time made 43 thumbnails/s, 2 made 43/s, 4 made 72/s, all 19 made 86-95/s, and 32 made
    /// 100/s. A spinning disk serves queued requests in the order that needs the least seeking,
    /// so it does best with many queued, and it tops out near 100 random files/s however
    /// they're sent. Capping reads made it slower, so there is no cap.
    pub fn start_with_threads(db_path: &Path, threads: usize, on_result: &Wake) -> Self {
        let threads = threads.max(1);
        let shared = Arc::new(Shared { state: Mutex::default(), wake_workers: Condvar::new() });
        let stats = Arc::new(ThumbStats::default());
        let (result_tx, results) = crossbeam_channel::unbounded();
        let (store_tx, store_rx) = crossbeam_channel::unbounded();

        let writer = {
            let db_path = db_path.to_path_buf();
            work::spawn("thumb-writer", move || writer_loop(&db_path, &store_rx))
        };
        let workers = (0..threads)
            .filter_map(|index| {
                let worker = Worker {
                    shared: shared.clone(),
                    results: result_tx.clone(),
                    store: store_tx.clone(),
                    on_result: on_result.clone(),
                    stats: stats.clone(),
                };
                let db_path = db_path.to_path_buf();
                work::spawn(&format!("thumb-{index}"), move || worker.run(&db_path))
            })
            .collect();

        Self { shared, results, workers, writer, store: Some(store_tx), stats }
    }

    /// Clears parts of the cache, after what's waiting to be saved and before anything after it,
    /// then gives the space back. Answers on the channel when done (it can take seconds).
    pub fn clear(&self, parts: Vec<Part>) -> Receiver<Result<(), String>> {
        let (done_tx, done) = crossbeam_channel::bounded(1);
        let sent = self.store.as_ref().is_some_and(|store| store.send(Store::Clear(parts, done_tx.clone())).is_ok());
        if !sent {
            let _ = done_tx.send(Err("the cache isn't open".to_owned()));
        }
        done
    }

    pub fn thread_count(&self) -> usize {
        self.workers.len()
    }

    /// Replaces the queue. Jobs already being worked on are skipped.
    pub fn set_wanted(&self, jobs: Vec<ThumbJob>) {
        let mut state = work::lock(&self.shared.state);
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
        let state = work::lock(&self.shared.state);
        (state.queue.len(), state.in_flight.len())
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
            let mut state = work::lock(&self.shared.state);
            state.shutdown = true;
            state.queue.clear();
        }
        self.shared.wake_workers.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        // The writer stops once every sender is gone (the workers' and this one), after committing
        // what's left.
        self.store = None;
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

struct Worker {
    shared: Arc<Shared>,
    results: Sender<Thumb>,
    store: Sender<Store>,
    on_result: Wake,
    stats: Arc<ThumbStats>,
}

impl Worker {
    fn run(self, db_path: &Path) {
        imaging::join_decode_budget();
        // Without a cache, thumbnails are still made, just never reused.
        let conn = db::open_cache(db_path).map_err(|err| diag::report("Thumbnail worker cache", err)).ok();
        let mut resizer = fr::Resizer::new();
        loop {
            let job = {
                let mut state = work::lock(&self.shared.state);
                loop {
                    if state.shutdown {
                        return;
                    }
                    if let Some(job) = state.queue.pop_front() {
                        state.in_flight.insert(job.key);
                        break job;
                    }
                    state = work::wait(&self.shared.wake_workers, state);
                }
            };
            let thumb = self.process(&job, conn.as_ref(), &mut resizer);
            imaging::trim_resizer(&mut resizer);
            // Send before leaving `in_flight`, so the UI never re-requests a finished thumbnail.
            let sent = self.results.send(thumb).is_ok();
            work::lock(&self.shared.state).in_flight.remove(&job.key);
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
            return Thumb { key: job.key, result, micros: start.elapsed().as_micros() as u64 };
        }

        // Some decoders panic on malformed files. Contain it here: the worker carries on and the
        // file counts as failed, instead of staying "in progress" forever.
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| match job.kind {
            AssetKind::Image => make_thumbnail(&job.path, resizer).map(Preview::Image),
            AssetKind::Audio => audio::waveform(&job.path).map(|waveform| Preview::Sound(Arc::new(waveform))),
            AssetKind::Model => {
                model_thumbnail(&job.path, resizer).map(|(pixels, info)| Preview::Model(pixels, Arc::new(info)))
            }
        }))
        .unwrap_or_else(|_| Err("the decoder crashed on this file".into()));
        let micros = start.elapsed().as_micros() as u64;
        if result.is_err() {
            self.stats.failed.fetch_add(1, Ordering::Relaxed);
        }
        // Failures are remembered too, so a broken file isn't decoded again on every visit.
        let _ = self.store.send(Store::Write(to_cache(job, &result)));
        self.stats.made.fetch_add(1, Ordering::Relaxed);
        self.stats.make_micros.fetch_add(micros, Ordering::Relaxed);
        Thumb { key: job.key, result, micros }
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

fn writer_loop(db_path: &Path, jobs: &Receiver<Store>) {
    let conn = match db::open_cache(db_path) {
        Ok(conn) => conn,
        Err(err) => {
            let message = err.to_string();
            diag::report("Thumbnail cache writer", err);
            while let Ok(job) = jobs.recv() {
                if let Store::Clear(_, done) = job {
                    let _ = done.send(Err(message.clone()));
                }
            }
            return;
        }
    };
    while let Ok(first) = jobs.recv() {
        // Saves are batched into one transaction; a clear ends the batch and goes after it.
        let mut batch = Vec::new();
        let mut clear = None;
        match first {
            Store::Write(write) => batch.push(write),
            Store::Clear(parts, done) => clear = Some((parts, done)),
        }
        let deadline = Instant::now() + Duration::from_millis(100);
        while clear.is_none() && batch.len() < 512 {
            match jobs.recv_deadline(deadline) {
                Ok(Store::Write(write)) => batch.push(write),
                Ok(Store::Clear(parts, done)) => clear = Some((parts, done)),
                Err(_) => break,
            }
        }
        if !batch.is_empty()
            && let Err(err) = db::put_cache(&conn, &batch)
        {
            diag::report("Saving previews", err);
        }
        if let Some((parts, done)) = clear {
            let _ = done.send(storage::clear_cache(&conn, &parts).map_err(|err| err.to_string()));
        }
    }
}

/// Reads an image file and shrinks the picture to fit `THUMB_MAX`.
fn make_thumbnail(path: &Path, resizer: &mut fr::Resizer) -> Result<ThumbPixels, String> {
    imaging::make_image(path, resizer, THUMB_MAX)
}

/// The grid thumbnail of a model file, `THUMB_MAX` square with a transparent background, and
/// what's in the file. For .blend files it's the picture Blender saved, cropped to what's drawn
/// (`blend::crop_to_content`) and scaled to fit.
///
/// Renders have `src_w` and `src_h` set to 0: they weren't scaled from a picture of known size.
fn model_thumbnail(path: &Path, resizer: &mut fr::Resizer) -> Result<(ThumbPixels, ModelInfo), String> {
    if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("blend")) {
        let preview = blend::read_preview(path)?;
        let (w, h, rgba) = blend::crop_to_content(&preview);
        let mut pixels = fit_picture(w, h, rgba, THUMB_MAX, resizer)?;
        (pixels.src_w, pixels.src_h) = (preview.width, preview.height);
        let info = ModelInfo { format: preview.format, ..ModelInfo::default() };
        return Ok((pixels, info));
    }
    let model = model::load(path, &LoadOptions { max_texture: THUMB_TEXTURE })?;
    let rgba = raster::render_thumbnail(&model, THUMB_MAX);
    Ok((ThumbPixels { w: THUMB_MAX, h: THUMB_MAX, src_w: 0, src_h: 0, rgba }, model.info))
}

/// Scales a picture up or down so its longer side is `max`.
fn fit_picture(w: u32, h: u32, rgba: Vec<u8>, max: u32, resizer: &mut fr::Resizer) -> Result<ThumbPixels, String> {
    if w == 0 || h == 0 {
        return Err("the saved preview has no pixels".into());
    }
    let scale = max as f64 / w.max(h) as f64;
    let (fit_w, fit_h) = (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1));
    if (fit_w, fit_h) == (w, h) {
        return Ok(ThumbPixels { w, h, src_w: w, src_h: h, rgba });
    }
    let src = fr::images::Image::from_vec_u8(w, h, rgba, fr::PixelType::U8x4).map_err(|err| err.to_string())?;
    let mut dst = fr::images::Image::new(fit_w, fit_h, fr::PixelType::U8x4);
    let options = fr::ResizeOptions::new().resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::CatmullRom));
    resizer.resize(&src, &mut dst, &options).map_err(|err| err.to_string())?;
    Ok(ThumbPixels { w: fit_w, h: fit_h, src_w: w, src_h: h, rgba: dst.into_vec() })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelInfo;
    use crate::test_support::TempDir;

    #[test]
    fn model_preview_round_trips_through_the_cache() {
        let dir = TempDir::new("model_cache");
        let conn = db::open_cache(&dir.path().join("library.sqlite")).unwrap();
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
        db::put_cache(&conn, &writes).unwrap();

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
    }
}
