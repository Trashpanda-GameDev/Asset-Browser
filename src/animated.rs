//! Animated GIFs, for the sprite sheet player: their frames side by side in one picture (the
//! sheet), with the GIF's own timing. A big one is scaled down evenly to fit a budget, so a long
//! screen recording can't take hundreds of megabytes.

use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};

use fast_image_resize as fr;
use image::AnimationDecoder;

use crate::imaging::{Pixels, fit_size, shrink};
use crate::sheets::{Animation, Frame, Sheet, Source};

/// The frames picture's longest side, and its pixels at most (16 MP: 64 MB, on the GPU too). The
/// memory setting can lower the pixels (`set_max_pixels`).
const SHEET_SIDE: u32 = 4096;
static SHEET_PIXELS: AtomicU64 = AtomicU64::new(16 << 20);

/// The most pixels a GIF's frames may take, from the next one loaded.
pub fn set_max_pixels(pixels: u64) {
    SHEET_PIXELS.store(pixels, Ordering::Relaxed);
}
/// Frames past this many are left out.
const MAX_FRAMES: usize = 1000;
/// Transparent pixels between frames, so drawing one never picks up its neighbour's edge.
const GUTTER: u32 = 2;

/// The GIF's frames as a sheet, or `None` when it has only one: a still picture. `stop` is asked
/// between frames, to give up on a picture that's no longer wanted.
pub fn load_gif(
    bytes: &[u8],
    resizer: &mut fr::Resizer,
    stop: &dyn Fn() -> bool,
) -> Result<Option<(Pixels, Sheet)>, String> {
    let (width, height, total) = count_frames(bytes)?;
    if total < 2 || width == 0 || height == 0 {
        return Ok(None);
    }
    let kept = total.min(MAX_FRAMES);
    let layout = Layout::plan(width, height, kept as u32, SHEET_PIXELS.load(Ordering::Relaxed));
    let mut rgba = vec![0u8; layout.width as usize * layout.height as usize * 4];
    let mut frames = Vec::with_capacity(kept);
    let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).map_err(|err| err.to_string())?;
    // Each frame comes composited onto the whole canvas, as it's meant to be seen.
    for (index, frame) in decoder.into_frames().take(kept).enumerate() {
        if stop() {
            return Err("no longer wanted".to_owned());
        }
        let frame = match frame {
            Ok(frame) => frame,
            // A damaged end still leaves the frames before it.
            Err(_) if frames.len() >= 2 => break,
            Err(err) => return Err(err.to_string()),
        };
        let (numer, denom) = frame.delay().numer_denom_ms();
        let delay = browser_delay(numer / denom.max(1));
        let picture = frame.into_buffer();
        let (w, h) = picture.dimensions();
        let picture = Pixels { w, h, src_w: width, src_h: height, rgba: picture.into_raw() };
        let small = shrink(picture, layout.longest, resizer).map_err(|err| err.to_string())?;
        if (small.w, small.h) != (layout.frame_w, layout.frame_h) {
            return Err("a frame came out a different size".to_owned());
        }
        let [x, y] = layout.cell(index as u32);
        blit(&mut rgba, layout.width, &small, [x, y]);
        let (frame_w, frame_h) = (layout.frame_w, layout.frame_h);
        frames.push(Frame {
            rect: [x, y, frame_w, frame_h],
            rotated: false,
            offset: [0, 0],
            size: [frame_w, frame_h],
            duration_ms: Some(delay),
        });
    }
    if frames.len() < 2 {
        return Ok(None);
    }
    let animations = vec![Animation { name: "All frames".to_owned(), frames: (0..frames.len()).collect() }];
    let sheet = Sheet { frames, animations, source: Source::Animated { width, height, total } };
    let pixels = Pixels { w: layout.width, h: layout.height, src_w: layout.width, src_h: layout.height, rgba };
    Ok(Some((pixels, sheet)))
}

/// The canvas size and how many frames, without decoding any (the `gif` crate only reads the
/// blocks). A damaged end counts the frames before it.
fn count_frames(bytes: &[u8]) -> Result<(u32, u32, usize), String> {
    let mut options = gif::DecodeOptions::new();
    options.skip_frame_decoding(true);
    let mut decoder = options.read_info(Cursor::new(bytes)).map_err(|err| err.to_string())?;
    let (width, height) = (u32::from(decoder.width()), u32::from(decoder.height()));
    let mut total = 0;
    loop {
        match decoder.next_frame_info() {
            Ok(Some(_)) => total += 1,
            Ok(None) => break,
            Err(_) if total > 0 => break,
            Err(err) => return Err(err.to_string()),
        }
    }
    Ok((width, height, total))
}

/// Browsers play GIF frames that ask for under 20 ms at 100 ms, which is what their makers saw.
fn browser_delay(ms: u32) -> u32 {
    if ms < 20 { 100 } else { ms }
}

/// Where the frames go in the sheet: in rows, close to square overall.
#[derive(Debug, PartialEq)]
struct Layout {
    /// The frames' longest side (`fit_size`'s `max`), and their size.
    longest: u32,
    frame_w: u32,
    frame_h: u32,
    columns: u32,
    width: u32,
    height: u32,
}

impl Layout {
    /// Frames at their own size, or scaled down evenly until all of them fit in `max_pixels`.
    fn plan(w: u32, h: u32, frames: u32, max_pixels: u64) -> Self {
        let frames = frames.max(1);
        let mut longest = w.max(h).max(1);
        loop {
            let (frame_w, frame_h) = fit_size(w.max(1), h.max(1), longest);
            let columns =
                ((f64::from(frames) * f64::from(frame_h) / f64::from(frame_w)).sqrt().ceil() as u32).clamp(1, frames);
            let rows = frames.div_ceil(columns);
            let width = columns * (frame_w + GUTTER) - GUTTER;
            let height = rows * (frame_h + GUTTER) - GUTTER;
            let fits =
                width <= SHEET_SIDE && height <= SHEET_SIDE && u64::from(width) * u64::from(height) <= max_pixels;
            if fits || longest <= 16 {
                return Self { longest, frame_w, frame_h, columns, width, height };
            }
            longest = longest * 9 / 10;
        }
    }

    /// The top left of frame `index`.
    fn cell(&self, index: u32) -> [u32; 2] {
        [index % self.columns * (self.frame_w + GUTTER), index / self.columns * (self.frame_h + GUTTER)]
    }
}

/// Copies a frame into the sheet, `sheet_w` pixels wide, with its top left at `x`, `y`.
fn blit(sheet: &mut [u8], sheet_w: u32, frame: &Pixels, [x, y]: [u32; 2]) {
    let row = frame.w as usize * 4;
    for line in 0..frame.h as usize {
        let at = ((y as usize + line) * sheet_w as usize + x as usize) * 4;
        sheet[at..at + row].copy_from_slice(&frame.rgba[line * row..(line + 1) * row]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::gif::GifEncoder;
    use image::{Delay, RgbaImage};

    /// A 4 × 3 GIF with a frame per colour, each lasting its delay.
    fn gif(frames: &[([u8; 4], u32)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            let frames = frames.iter().map(|&(color, ms)| {
                image::Frame::from_parts(
                    RgbaImage::from_pixel(4, 3, image::Rgba(color)),
                    0,
                    0,
                    Delay::from_numer_denom_ms(ms, 1),
                )
            });
            encoder.encode_frames(frames).unwrap();
        }
        bytes
    }

    #[test]
    fn a_gif_becomes_its_frames() {
        let colors = [([255, 0, 0, 255], 50), ([0, 255, 0, 255], 0), ([0, 0, 255, 255], 120)];
        let mut resizer = fr::Resizer::new();
        let (pixels, sheet) = load_gif(&gif(&colors), &mut resizer, &|| false).unwrap().expect("animated");
        assert_eq!(sheet.source, Source::Animated { width: 4, height: 3, total: 3 });
        assert!(sheet.is_certain(), "plays at once");
        let durations: Vec<Option<u32>> = sheet.frames.iter().map(|frame| frame.duration_ms).collect();
        assert_eq!(durations, [Some(50), Some(100), Some(120)], "a delay under 20 ms plays at 100 ms");
        assert_eq!(sheet.animations[0].frames, [0, 1, 2]);
        for (frame, (color, _)) in sheet.frames.iter().zip(colors) {
            let [x, y, w, h] = frame.rect;
            assert_eq!([w, h], [4, 3], "small frames keep their size");
            let at = ((y + 1) * pixels.w + x + 1) as usize * 4;
            assert_eq!(pixels.rgba[at..at + 4], color, "each frame where its rectangle says");
        }
    }

    #[test]
    fn a_still_gif_is_a_picture() {
        let mut resizer = fr::Resizer::new();
        assert!(load_gif(&gif(&[([9, 9, 9, 255], 100)]), &mut resizer, &|| false).unwrap().is_none());
        let stopped = load_gif(&gif(&[([1, 1, 1, 255], 100), ([2, 2, 2, 255], 100)]), &mut resizer, &|| true);
        assert!(stopped.is_err(), "gives up when it's no longer wanted");
    }

    #[test]
    fn big_animations_fit_the_budget() {
        // The library's biggest: 126 frames of 1270 x 698.
        let budget = 16 << 20;
        let layout = Layout::plan(1270, 698, 126, budget);
        assert!(layout.width <= SHEET_SIDE && layout.height <= SHEET_SIDE);
        assert!(u64::from(layout.width) * u64::from(layout.height) <= budget);
        assert!(layout.frame_w > 300, "still bigger than the preview pane shows it: {layout:?}");
        let rows = 126u32.div_ceil(layout.columns);
        assert_eq!(layout.cell(125)[1], (rows - 1) * (layout.frame_h + GUTTER));
        // Small sprites keep every pixel.
        assert_eq!(Layout::plan(48, 48, 8, budget).frame_w, 48);
        // A smaller budget scales them down further.
        let small = Layout::plan(1270, 698, 126, 4 << 20);
        assert!(small.frame_w < layout.frame_w && u64::from(small.width) * u64::from(small.height) <= 4 << 20);
    }
}
