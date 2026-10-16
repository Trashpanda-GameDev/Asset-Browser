//! Seamless ground and wall tiles.

use std::fs;
use std::io;
use std::path::Path;

use super::paint::{Canvas, Rgba, cells, circle, darker, fbm, hash, hex, lighter, mix, noise, smoothstep};

/// A tile's colour at a point, in pixels.
type Paint = fn(f32, f32) -> Rgba;

/// Each tile's file and its colours: 128 pixels square, and seamless.
const TILES: [(&str, Paint); 9] = [
    ("grass.png", grass),
    ("dirt.png", dirt),
    ("stone.png", stone),
    ("water.png", water),
    ("sand.png", sand),
    ("lava.png", lava),
    ("snow.png", snow),
    ("bricks.png", bricks),
    ("planks.png", planks),
];

pub(super) fn tiles(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    for (name, color) in TILES {
        Canvas::paint(128, 128, color).save(&dir.join(name))?;
    }
    Ok(())
}

fn grass(x: f32, y: f32) -> Rgba {
    let n = fbm(x / 16.0, y / 16.0, 8, 1);
    let blades = noise(x / 2.0, y / 8.0, 64, 2);
    mix(hex(0x2c7a33), hex(0x82c653), n * 0.75 + blades * 0.45 - 0.15)
}

fn dirt(x: f32, y: f32) -> Rgba {
    let n = fbm(x / 16.0, y / 16.0, 8, 3);
    let (d1, _, id) = cells(x / 16.0, y / 16.0, 8, 4);
    let ground = mix(hex(0x55361f), hex(0x8d5f37), n);
    if d1 < 0.16 && hash(id, 0, 5) > 0.55 { mix(hex(0x9b8a78), hex(0x5f5246), d1 / 0.16) } else { ground }
}

fn stone(x: f32, y: f32) -> Rgba {
    let (d1, d2, id) = cells(x / 32.0, y / 32.0, 4, 6);
    let tone =
        mix(hex(0x5a606c), hex(0xa3aab6), 0.3 + 0.5 * hash(id, 1, 7) + fbm(x / 8.0, y / 8.0, 16, 8) * 0.3 - 0.15);
    let edge = d2 - d1;
    if edge < 0.06 { hex(0x2b2e35) } else { mix(darker(tone, 0.25), tone, smoothstep(0.06, 0.3, edge)) }
}

fn water(x: f32, y: f32) -> Rgba {
    let n = fbm(x / 32.0, y / 32.0, 4, 9);
    let wave = (x * 0.09 + n * 7.0 + y * 0.05).sin();
    let base = mix(hex(0x1d5aa3), hex(0x3d92dc), n);
    mix(base, hex(0xd2ecff), (wave - 0.86) * 6.0)
}

fn sand(x: f32, y: f32) -> Rgba {
    let n = fbm(x / 24.0, y / 24.0, 8, 10);
    let ripple = (y * 0.22 + n * 7.0).sin() * 0.5 + 0.5;
    mix(hex(0xcfa868), hex(0xf2dca6), n * 0.6 + ripple * 0.35)
}

fn lava(x: f32, y: f32) -> Rgba {
    let (d1, d2, _) = cells(x / 28.0, y / 28.0, 4, 11);
    let glow = smoothstep(0.22, 0.0, d2 - d1);
    let crust = mix(hex(0x2d140d), hex(0x5c2a17), fbm(x / 12.0, y / 12.0, 8, 12) + d1 * 0.3);
    mix(crust, mix(hex(0xff4d12), hex(0xffd64a), glow * glow), glow)
}

fn snow(x: f32, y: f32) -> Rgba {
    let n = fbm(x / 20.0, y / 20.0, 8, 13);
    let sparkle = if hash(x as i32, y as i32, 14) > 0.992 { 0.6 } else { 0.0 };
    lighter(mix(hex(0xc9d8ea), hex(0xffffff), n * 0.9 + 0.2), sparkle)
}

fn bricks(x: f32, y: f32) -> Rgba {
    let row = (y / 16.0).floor();
    let shifted = x + if row as i32 % 2 == 0 { 0.0 } else { 16.0 };
    if y.rem_euclid(16.0) < 2.0 || shifted.rem_euclid(32.0) < 2.0 {
        return hex(0xb9b0a2);
    }
    let brick = hash((shifted / 32.0).floor() as i32, row as i32, 15);
    darker(mix(hex(0x8e3b2e), hex(0xbd5e43), brick), fbm(x / 6.0, y / 6.0, 32, 16) * 0.35)
}

fn planks(x: f32, y: f32) -> Rgba {
    let board = (x / 32.0).floor() as i32;
    if x.rem_euclid(32.0) < 2.0 {
        return hex(0x3a2615);
    }
    if circle(board as f32 * 32.0 + 16.0, 12.0, 2.2)(x, y) || circle(board as f32 * 32.0 + 16.0, 116.0, 2.2)(x, y) {
        return hex(0x2b2b2b);
    }
    let tone = mix(hex(0x7a4f2a), hex(0xae7841), hash(board, 0, 17));
    let grain = (y * 0.12 + noise(x / 4.0, y / 32.0, 32, 18) * 6.0 + board as f32 * 3.0).sin() * 0.5 + 0.5;
    mix(tone, darker(tone, 0.3), grain * 0.5)
}
