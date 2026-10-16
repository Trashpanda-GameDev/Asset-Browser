//! A thumbnail worker: takes the next wanted preview, serves it from the cache or makes it, and
//! hands it to the window and to the cache writer.

use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crossbeam_channel::Sender;
use fast_image_resize as fr;

use super::cache::{cached_preview, to_cache};
use super::writer::Store;
use super::{Preview, Shared, THUMB_MAX, THUMB_TEXTURE, Thumb, ThumbJob, ThumbStats};
use crate::asset::AssetKind;
use crate::imaging::{self, Pixels};
use crate::model::{self, LoadOptions, ModelInfo, blend, raster};
use crate::work::{self, Wake};
use crate::{audio, db, diag};

pub(super) struct Worker {
    pub(super) shared: Arc<Shared>,
    pub(super) results: Sender<Thumb>,
    pub(super) store: Sender<Store>,
    pub(super) on_result: Wake,
    pub(super) stats: Arc<ThumbStats>,
}

impl Worker {
    pub(super) fn run(self, db_path: &Path) {
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

/// Reads an image file and shrinks the picture to fit `THUMB_MAX`.
fn make_thumbnail(path: &Path, resizer: &mut fr::Resizer) -> Result<Pixels, String> {
    imaging::make_image(path, resizer, THUMB_MAX)
}

/// The grid thumbnail of a model file, `THUMB_MAX` square with a transparent background, and
/// what's in the file. For .blend files it's the picture Blender saved, cropped to what's drawn
/// (`blend::crop_to_content`) and scaled to fit.
///
/// Renders have `src_w` and `src_h` set to 0: they weren't scaled from a picture of known size.
fn model_thumbnail(path: &Path, resizer: &mut fr::Resizer) -> Result<(Pixels, ModelInfo), String> {
    if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("blend")) {
        let preview = blend::read_preview(path)?;
        let (w, h, rgba) = blend::crop_to_content(&preview);
        let mut pixels = fit_picture(w, h, rgba, THUMB_MAX, resizer)?;
        (pixels.src_w, pixels.src_h) = (preview.width, preview.height);
        let info = ModelInfo { format: preview.format, ..ModelInfo::default() };
        return Ok((pixels, info));
    }
    let model = model::load(path, &LoadOptions { max_texture: THUMB_TEXTURE }).map_err(|err| err.to_string())?;
    let rgba = raster::render_thumbnail(&model, THUMB_MAX);
    Ok((Pixels { w: THUMB_MAX, h: THUMB_MAX, src_w: 0, src_h: 0, rgba }, model.info))
}

/// Scales a picture up or down so its longer side is `max`.
fn fit_picture(w: u32, h: u32, rgba: Vec<u8>, max: u32, resizer: &mut fr::Resizer) -> Result<Pixels, String> {
    if w == 0 || h == 0 {
        return Err("the saved preview has no pixels".into());
    }
    let scale = max as f64 / w.max(h) as f64;
    let (fit_w, fit_h) = (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1));
    if (fit_w, fit_h) == (w, h) {
        return Ok(Pixels { w, h, src_w: w, src_h: h, rgba });
    }
    let src = fr::images::Image::from_vec_u8(w, h, rgba, fr::PixelType::U8x4).map_err(|err| err.to_string())?;
    let mut dst = fr::images::Image::new(fit_w, fit_h, fr::PixelType::U8x4);
    let options = fr::ResizeOptions::new().resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::CatmullRom));
    resizer.resize(&src, &mut dst, &options).map_err(|err| err.to_string())?;
    Ok(Pixels { w: fit_w, h: fit_h, src_w: w, src_h: h, rgba: dst.into_vec() })
}
