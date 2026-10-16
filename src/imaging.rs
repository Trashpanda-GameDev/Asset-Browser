//! Pictures: reading image files (a PSD from its flattened picture), shrinking them to a size, and
//! the memory decoding takes, shared by the threads that decode.

use std::cell::Cell;
use std::io::Cursor;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};

use fast_image_resize as fr;
use image::ImageFormat;

use crate::psd_composite;

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

/// How much memory the thumbnail workers may hold at once for decoding. A picture is decoded at
/// full size before it's shrunk, so 19 workers meeting a folder of 4K textures held 3.7 GB at
/// once (measured). Big pictures take turns; small ones never wait. The memory setting changes it
/// (`set_decode_limit`).
static DECODING: DecodeBudget = DecodeBudget::new(512 << 20);

/// How much the thumbnail workers may decode at once, from now on.
pub fn set_decode_limit(bytes: u64) {
    DECODING.set_limit(bytes);
}
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

/// From now on, what this thread decodes shares the decode budget. The thumbnail workers call it
/// when they start.
pub fn join_decode_budget() {
    IN_BUDGET.set(true);
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
#[expect(clippy::too_many_arguments, reason = "refactor: take fewer, grouped arguments")]
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
    limit: AtomicU64,
}

struct BudgetState {
    used: u64,
    /// Tickets, so a big picture isn't overtaken forever by small ones.
    next: u64,
    serving: u64,
}

impl DecodeBudget {
    const fn new(limit: u64) -> Self {
        Self {
            state: Mutex::new(BudgetState { used: 0, next: 0, serving: 0 }),
            changed: Condvar::new(),
            limit: AtomicU64::new(limit),
        }
    }

    fn set_limit(&self, bytes: u64) {
        // Under the lock, so a waiter can't miss the change between checking and waiting.
        let _state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.limit.store(bytes, Ordering::Relaxed);
        self.changed.notify_all();
    }

    fn reserve(&self, bytes: u64) -> Reserved<'_> {
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let ticket = state.next;
        state.next += 1;
        while state.serving != ticket || (state.used > 0 && state.used + bytes > self.limit.load(Ordering::Relaxed)) {
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

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
}
