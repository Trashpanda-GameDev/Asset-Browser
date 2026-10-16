//! Landscape backgrounds: a forest by day, a desert and a night sky.

use std::fs;
use std::io;
use std::path::Path;

use super::paint::{Canvas, WHITE, alpha, capsule, circle, darker, hash, hex, mix, polygon, radial, rect, solid};

pub(super) fn backgrounds(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    forest().save(&dir.join("forest_day.png"))?;
    desert().save(&dir.join("desert_dusk.png"))?;
    night().save(&dir.join("night_sky.png"))
}

/// The height of a hill line at `x`.
fn ridge(x: f32, base: f32, amp: f32, freq: f32, phase: f32) -> f32 {
    base - amp
        * ((x * freq + phase).sin() * 0.6 + (x * freq * 2.3 + phase * 1.3).sin() * 0.3 + (x * freq * 5.1).sin() * 0.1)
}

fn forest() -> Canvas {
    let layers = [
        (300.0, 40.0, 0.006, 0.0, hex(0x9ed1a6)),
        (365.0, 46.0, 0.009, 1.7, hex(0x5fae6a)),
        (440.0, 36.0, 0.013, 3.1, hex(0x2d7a3c)),
    ];
    let mut c = Canvas::paint(960, 540, |x, y| {
        let mut color = mix(hex(0x63b0ec), hex(0xe2f4ff), y / 380.0);
        let d = (x - 760.0).hypot(y - 110.0);
        color = mix(color, hex(0xfff6c9), (1.0 - d / 150.0).clamp(0.0, 1.0).powi(2));
        if d < 46.0 {
            color = hex(0xfff2b0);
        }
        for (base, amp, freq, phase, tint) in layers {
            let top = ridge(x, base, amp, freq, phase);
            if y > top {
                color = mix(tint, darker(tint, 0.3), (y - top) / 300.0);
            }
        }
        color
    });
    for (cx, cy, size) in [(180.0, 90.0, 1.0), (420.0, 60.0, 0.7), (600.0, 150.0, 0.8)] {
        let puffs = [(-40.0, 8.0, 30.0), (-10.0, -6.0, 38.0), (28.0, 4.0, 32.0), (55.0, 12.0, 22.0)];
        c.fill_in(
            [cx - 120.0, cy - 60.0, cx + 120.0, cy + 60.0],
            |x, y| puffs.iter().any(|&(dx, dy, r)| circle(cx + dx * size, cy + dy * size, r * size)(x, y)),
            solid(alpha(WHITE, 0.92)),
        );
    }
    let (base, amp, freq, phase, _) = layers[2];
    for i in 0..15 {
        let x = 24.0 + i as f32 * 66.0 + hash(i, 0, 21) * 24.0;
        let foot = ridge(x, base, amp, freq, phase) + 12.0;
        let height = 70.0 + hash(i, 1, 22) * 50.0;
        c.fill_in(
            [x - 6.0, foot - 20.0, x + 6.0, foot + 6.0],
            rect(x - 5.0, foot - 18.0, x + 5.0, foot + 4.0),
            solid(hex(0x4a2f1c)),
        );
        for (step, shade) in [(0.0, 0x1d5a2c), (0.28, 0x23683a), (0.52, 0x2a7744)] {
            let (bottom, half) = (foot - 10.0 - height * step, 26.0 - step * 26.0);
            let tip = foot - height;
            c.fill_in(
                [x - 30.0, tip - 2.0, x + 30.0, bottom + 2.0],
                polygon(vec![(x, tip), (x - half - 4.0, bottom), (x + half + 4.0, bottom)]),
                solid(hex(shade)),
            );
        }
    }
    c
}

fn desert() -> Canvas {
    let dunes = [
        (380.0, 30.0, 0.005, 0.5, hex(0xd9874a)),
        (430.0, 34.0, 0.008, 2.0, hex(0xb8653a)),
        (490.0, 26.0, 0.011, 4.0, hex(0x8a4428)),
    ];
    let mut c = Canvas::paint(960, 540, |x, y| {
        let sky = if y < 260.0 {
            mix(hex(0x2b1b4f), hex(0xe0653c), y / 260.0)
        } else {
            mix(hex(0xe0653c), hex(0xffc86b), (y - 260.0) / 110.0)
        };
        let d = (x - 480.0).hypot(y - 360.0);
        let mut color = mix(sky, hex(0xffd88a), (1.0 - d / 260.0).clamp(0.0, 1.0).powi(2) * 0.8);
        if d < 92.0 {
            color = mix(hex(0xfff0b5), hex(0xffc35a), d / 92.0);
        }
        for (base, amp, freq, phase, tint) in dunes {
            let top = ridge(x, base, amp, freq, phase);
            if y > top {
                color = mix(tint, darker(tint, 0.25), (y - top) / 200.0);
            }
        }
        color
    });
    let (base, amp, freq, phase, _) = dunes[1];
    for (i, x) in [140.0, 300.0, 700.0, 860.0].into_iter().enumerate() {
        let foot = ridge(x, base, amp, freq, phase) + 6.0;
        let h = 70.0 + i as f32 * 12.0;
        let parts = [
            capsule(x, foot, x, foot - h, 9.0),
            capsule(x, foot - h * 0.45, x - 22.0, foot - h * 0.45, 6.0),
            capsule(x - 22.0, foot - h * 0.45, x - 22.0, foot - h * 0.75, 6.0),
            capsule(x, foot - h * 0.6, x + 20.0, foot - h * 0.6, 6.0),
            capsule(x + 20.0, foot - h * 0.6, x + 20.0, foot - h * 0.9, 6.0),
        ];
        c.fill_in(
            [x - 32.0, foot - h - 12.0, x + 32.0, foot + 4.0],
            |px, py| parts.iter().any(|part| part(px, py)),
            solid(hex(0x3d1f1a)),
        );
    }
    c
}

fn night() -> Canvas {
    let mut c = Canvas::paint(960, 540, |x, y| {
        let mut color = mix(hex(0x060a22), hex(0x23306a), y / 420.0);
        let twinkle = hash(x as i32, y as i32, 31);
        if twinkle > 0.9975 && y < 360.0 {
            color = mix(color, hex(0xf3f6ff), 0.4 + (twinkle - 0.9975) * 240.0);
        }
        let d = (x - 210.0).hypot(y - 120.0);
        color = mix(color, hex(0x8fa0d8), (1.0 - d / 170.0).clamp(0.0, 1.0).powi(3) * 0.6);
        let peaks = 330.0 - 70.0 * ((x * 0.011 + 0.4).sin().abs() * 0.7 + (x * 0.027).sin() * 0.3);
        if y > peaks {
            color = mix(hex(0x1a2350), hex(0x0e1430), (y - peaks) / 150.0);
            if y < peaks + 10.0 && peaks < 285.0 {
                color = hex(0xc9d3f0);
            }
        }
        let near = ridge(x, 450.0, 30.0, 0.008, 1.0);
        if y > near {
            color = hex(0x070a1c);
        }
        color
    });
    for (x, y, r) in
        [(330.0, 70.0, 2.6), (520.0, 40.0, 2.0), (700.0, 110.0, 2.4), (860.0, 60.0, 2.8), (620.0, 200.0, 1.8)]
    {
        c.fill_in([x - 4.0, y - 4.0, x + 4.0, y + 4.0], circle(x, y, r), solid(hex(0xffffff)));
    }
    c.fill_in(
        [150.0, 60.0, 270.0, 180.0],
        circle(210.0, 120.0, 50.0),
        radial(195.0, 105.0, 70.0, hex(0xfffbe6), hex(0xd8d2b4)),
    );
    for (x, y, r) in [(195.0, 105.0, 9.0), (228.0, 130.0, 12.0), (200.0, 145.0, 6.0)] {
        c.fill_in([x - r, y - r, x + r, y + r], circle(x, y, r), solid(alpha(hex(0xb9b294), 0.6)));
    }
    c
}
