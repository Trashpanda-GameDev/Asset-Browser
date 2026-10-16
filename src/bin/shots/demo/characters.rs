//! A sprite sheet and animated GIFs: a bouncing slime, a flame and a portal.

use std::f32::consts::{PI, TAU};
use std::fs;
use std::io;
use std::path::Path;

use super::paint::{BLACK, CLEAR, Canvas, WHITE, alpha, circle, darker, ellipse, hex, lighter, mix, radial, solid};

pub(super) fn characters(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    // A sprite sheet with Aseprite's data beside it: the app plays it at once, by its tag.
    let mut sheet = Canvas::new(512, 64);
    for i in 0..8 {
        slime(&mut sheet, i as f32 * 64.0, i as f32 / 8.0);
    }
    sheet.save(&dir.join("slime_bounce.png"))?;
    let frames: serde_json::Map<String, serde_json::Value> = (0..8)
        .map(|i| {
            let frame = serde_json::json!({
                "frame": { "x": i * 64, "y": 0, "w": 64, "h": 64 },
                "rotated": false,
                "trimmed": false,
                "spriteSourceSize": { "x": 0, "y": 0, "w": 64, "h": 64 },
                "sourceSize": { "w": 64, "h": 64 },
                "duration": 90
            });
            (format!("slime_bounce {i}.aseprite"), frame)
        })
        .collect();
    let data = serde_json::json!({
        "frames": frames,
        "meta": {
            "app": "https://www.aseprite.org/",
            "image": "slime_bounce.png",
            "format": "RGBA8888",
            "size": { "w": 512, "h": 64 },
            "scale": "1",
            "frameTags": [{ "name": "bounce", "from": 0, "to": 7, "direction": "forward" }]
        }
    });
    fs::write(dir.join("slime_bounce.json"), serde_json::to_string_pretty(&data).map_err(io::Error::other)?)?;

    // GameMaker's naming: eight frames in a strip.
    let mut strip = Canvas::new(384, 48);
    for i in 0..8 {
        let (ox, turn) = (i as f32 * 48.0, (i as f32 / 8.0 * TAU).cos());
        let w = (turn.abs() * 18.0).max(3.0);
        let face = if turn >= 0.0 { hex(0xffd75e) } else { hex(0xe0a530) };
        strip.fill_in([ox, 0.0, ox + 48.0, 48.0], ellipse(ox + 24.0, 24.0, w + 2.5, 21.0), solid(hex(0x6b4a12)));
        strip.fill_in(
            [ox, 0.0, ox + 48.0, 48.0],
            ellipse(ox + 24.0, 24.0, w, 18.5),
            radial(ox + 24.0 - w * 0.4, 16.0, 28.0, lighter(face, 0.5), darker(face, 0.2)),
        );
        if w > 8.0 {
            let inner = ellipse(ox + 24.0, 24.0, w * 0.62, 12.0);
            let hole = ellipse(ox + 24.0, 24.0, w * 0.62 - 2.0, 10.0);
            strip.fill_in([ox, 0.0, ox + 48.0, 48.0], move |x, y| inner(x, y) && !hole(x, y), solid(darker(face, 0.3)));
        }
    }
    strip.save(&dir.join("coin_strip8.png"))?;

    gif(&dir.join("flame.gif"), 64, 96, 8, (0..10).map(|i| flame(i as f32 / 10.0 * TAU)).collect())?;
    gif(&dir.join("portal.gif"), 96, 96, 7, (0..12).map(|i| portal(i as f32 / 12.0 * TAU)).collect())
}

/// One frame of the slime's hop, `t` from 0 to 1, drawn at `ox`.
fn slime(sheet: &mut Canvas, ox: f32, t: f32) {
    let area = [ox, 0.0, ox + 64.0, 64.0];
    let hop = (t * PI).sin() * 14.0;
    let squash = (t * TAU).cos();
    let (rx, ry) = (20.0 * (1.0 + 0.18 * squash), 16.0 * (1.0 - 0.18 * squash));
    let (cx, cy) = (ox + 32.0, 57.0 - ry - hop);
    sheet.fill_in(area, ellipse(cx, 59.0, 18.0 - hop * 0.45, 3.5), solid(alpha(BLACK, 0.25)));
    sheet.fill_in(area, ellipse(cx, cy, rx + 2.5, ry + 2.5), solid(hex(0x1d5a24)));
    sheet.fill_in(area, ellipse(cx, cy, rx, ry), radial(cx - 8.0, cy - 8.0, 32.0, hex(0xa6f294), hex(0x31a246)));
    for side in [-1.0, 1.0] {
        let (ex, ey) = (cx + side * 7.5, cy - 2.0);
        sheet.fill_in(area, ellipse(ex, ey, 4.6, 5.6), solid(WHITE));
        sheet.fill_in(area, circle(ex + 1.0, ey + 1.0, 2.4), solid(hex(0x14231a)));
    }
    sheet.fill_in(area, ellipse(cx - 10.0, cy - 8.0, 4.0, 2.8), solid(alpha(WHITE, 0.75)));
}

fn flame(phase: f32) -> Canvas {
    let mut c = Canvas::new(64, 96);
    for (scale, color) in [(1.0, hex(0xe8401c)), (0.74, hex(0xff9a1f)), (0.46, hex(0xffe58a))] {
        c.fill(
            |x, y| {
                let h = (90.0 - y) / (80.0 * scale);
                if !(0.0..=1.0).contains(&h) {
                    return false;
                }
                let sway = (3.5 * (y * 0.09 + phase).sin() + 2.0 * (y * 0.21 - phase * 2.0).sin()) * h;
                let w = 23.0 * scale * (h * PI).sin().powf(0.7) * (1.0 - h * 0.35);
                (x - 32.0 - sway).abs() < w
            },
            solid(color),
        );
    }
    c
}

fn portal(phase: f32) -> Canvas {
    Canvas::paint(96, 96, |x, y| {
        let (dx, dy) = (x - 48.0, y - 48.0);
        let r = dx.hypot(dy);
        if r > 42.0 {
            return CLEAR;
        }
        let swirl = (dy.atan2(dx) * 3.0 + r * 0.18 - phase).sin() * 0.5 + 0.5;
        let color = mix(mix(hex(0x5b2bd6), hex(0x37e0ff), swirl), WHITE, 1.0 - r / 16.0);
        mix(color, hex(0x2a0f6b), (r - 37.0) / 5.0)
    })
}

/// An animated GIF from `frames`, `delay` hundredths of a second each, looping.
fn gif(path: &Path, w: u16, h: u16, delay: u16, frames: Vec<Canvas>) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    let mut encoder = gif::Encoder::new(&mut file, w, h, &[]).map_err(io::Error::other)?;
    encoder.set_repeat(gif::Repeat::Infinite).map_err(io::Error::other)?;
    for canvas in frames {
        // GIFs have no partial transparency: each pixel is there or not.
        let mut rgba: Vec<u8> = canvas
            .rgba8()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[r, g, b, a]| if a >= 128 { [r, g, b, 255] } else { [0; 4] })
            .collect();
        let mut frame = gif::Frame::from_rgba_speed(w, h, &mut rgba, 10);
        frame.delay = delay;
        frame.dispose = gif::DisposalMethod::Background;
        encoder.write_frame(&frame).map_err(io::Error::other)?;
    }
    Ok(())
}
