//! Short clips of the window as animated GIFs: frames taken on a timer while the plan runs, cut to
//! an area and made smaller. Each frame after the first holds only the box that changed since the
//! one before (a GIF frame can cover part of the picture), so a mostly still window stays small.

use std::io;
use std::path::Path;
use std::thread::JoinHandle;
use std::time::Instant;

use eframe::egui;
use image::{RgbImage, imageops};

use crate::looping_gif::{LoopingGif, SPEED};

/// What to record.
#[derive(Clone, Copy)]
pub struct Clip {
    pub name: &'static str,
    /// How long the GIF lasts before it starts again.
    pub seconds: f32,
    /// Left, top, right and bottom of the window's part to keep, in points.
    pub area: [f32; 4],
    /// How wide the GIF is, in pixels.
    pub width: u32,
    /// Seconds between frames.
    pub every: f32,
}

/// A clip being recorded.
pub struct Recording {
    clip: Clip,
    started: Instant,
    asked: Option<Instant>,
    /// The frames taken, cut to the area, with when after the first; one that's no different from
    /// the frame before isn't kept.
    frames: Vec<(f32, RgbImage)>,
    first: Option<Instant>,
}

impl Recording {
    pub fn start(clip: Clip) -> Self {
        Self { clip, started: Instant::now(), asked: None, frames: Vec::new(), first: None }
    }

    /// Whether to take a frame now.
    pub fn due(&mut self) -> bool {
        let now = Instant::now();
        let due = self.asked.is_none_or(|asked| now.duration_since(asked).as_secs_f32() >= self.clip.every);
        if due && !self.done() {
            self.asked = Some(now);
        }
        due && !self.done()
    }

    /// Whether it has frames enough: the clip's length after the first, and a little more.
    pub fn done(&self) -> bool {
        self.started.elapsed().as_secs_f32() >= self.clip.seconds + 0.5
    }

    /// Keeps a frame taken of the window.
    pub fn take(&mut self, image: &egui::ColorImage, pixels_per_point: f32) {
        let first = *self.first.get_or_insert_with(Instant::now);
        let at = first.elapsed().as_secs_f32();
        if at >= self.clip.seconds {
            return;
        }
        let [w, h] = image.size;
        let [x0, y0, x1, y1] = self.clip.area.map(|v| (v * pixels_per_point).round().max(0.0) as usize);
        let (x1, y1) = (x1.min(w), y1.min(h));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let cut = RgbImage::from_fn((x1 - x0) as u32, (y1 - y0) as u32, |x, y| {
            let color = image.pixels[(y0 + y as usize) * w + x0 + x as usize];
            image::Rgb([color.r(), color.g(), color.b()])
        });
        if self.frames.last().is_none_or(|(_, last)| *last != cut) {
            self.frames.push((at, cut));
        }
    }

    /// Writes the GIF into `dir`, on a thread of its own.
    pub fn finish(self, dir: &Path) -> JoinHandle<()> {
        let path = dir.join(format!("{}.gif", self.clip.name));
        std::thread::spawn(move || match write_gif(&path, self.clip, self.frames) {
            Ok(()) => println!("saved {}", path.display()),
            Err(err) => eprintln!("couldn't save {}: {err}", path.display()),
        })
    }
}

fn write_gif(path: &Path, clip: Clip, frames: Vec<(f32, RgbImage)>) -> io::Result<()> {
    let Some((_, first)) = frames.first() else {
        return Err(io::Error::other("no frames were taken"));
    };
    let scale = clip.width as f32 / first.width() as f32;
    let (w, h) = (clip.width, (first.height() as f32 * scale).round() as u32);
    // Made smaller first, so frames that only differed in what doesn't show any more are one.
    let mut small: Vec<(f32, RgbImage)> = Vec::new();
    for (at, frame) in frames {
        let frame = if scale == 1.0 { frame } else { imageops::resize(&frame, w, h, imageops::FilterType::Lanczos3) };
        if small.last().is_none_or(|(_, last)| *last != frame) {
            small.push((at, frame));
        }
    }
    let mut out = LoopingGif::create(path, w as u16, h as u16)?;
    // In hundredths of a second, rounded from the start, so the whole loop keeps its length.
    let hundredths = |seconds: f32| (seconds * 100.0).round() as u32;
    for (i, (at, frame)) in small.iter().enumerate() {
        let end = small.get(i + 1).map_or(clip.seconds, |(next, _)| *next);
        let [x0, y0, x1, y1] = match i {
            0 => [0, 0, w, h],
            _ => changed(&small[i - 1].1, frame),
        };
        let part = imageops::crop_imm(frame, x0, y0, x1 - x0, y1 - y0).to_image();
        let mut gif_frame = gif::Frame::from_rgb_speed((x1 - x0) as u16, (y1 - y0) as u16, part.as_raw(), SPEED);
        gif_frame.left = x0 as u16;
        gif_frame.top = y0 as u16;
        gif_frame.delay = (hundredths(end).saturating_sub(hundredths(*at))).max(2) as u16;
        gif_frame.dispose = gif::DisposalMethod::Keep;
        out.write(&gif_frame)?;
    }
    Ok(())
}

/// The box (left, top, right, bottom) around the pixels that differ.
fn changed(before: &RgbImage, after: &RgbImage) -> [u32; 4] {
    let (mut x0, mut y0, mut x1, mut y1) = (after.width(), after.height(), 0, 0);
    for (x, y, pixel) in after.enumerate_pixels() {
        if before.get_pixel(x, y) != pixel {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
        }
    }
    if x0 >= x1 { [0, 0, 1, 1] } else { [x0, y0, x1, y1] }
}
