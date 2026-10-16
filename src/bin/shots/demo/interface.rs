//! Interface parts: buttons.

use std::fs;
use std::io;
use std::path::Path;

use super::paint::{Canvas, Rgba, WHITE, alpha, circle, hex, polygon, radial, rounded, solid, vertical};

pub(super) fn interface(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    button(hex(0x7fe07a), hex(0x2c9a3c), hex(0x173d1c), true).save(&dir.join("button_play.png"))?;
    button(hex(0x7cc4ff), hex(0x2f6fd0), hex(0x112a52), false).save(&dir.join("button_pause.png"))?;

    let mut panel = Canvas::new(256, 192);
    panel.fill(rounded(2.0, 2.0, 254.0, 190.0, 18.0), vertical(2.0, 190.0, hex(0xf0c46a), hex(0xa8742a)));
    panel.fill(rounded(9.0, 9.0, 247.0, 183.0, 12.0), vertical(9.0, 183.0, hex(0x323650), hex(0x1d2033)));
    for (x, y) in [(18.0, 18.0), (238.0, 18.0), (18.0, 174.0), (238.0, 174.0)] {
        panel.fill(circle(x, y, 5.0), radial(x - 1.5, y - 1.5, 6.0, hex(0xfff0b8), hex(0xb6812e)));
    }
    panel.fill(rounded(24.0, 30.0, 232.0, 36.0, 3.0), solid(alpha(WHITE, 0.12)));
    panel.save(&dir.join("panel.png"))?;

    let mut bar = Canvas::new(320, 48);
    bar.fill(rounded(2.0, 8.0, 318.0, 40.0, 16.0), solid(hex(0x15171f)));
    bar.fill(rounded(7.0, 13.0, 228.0, 35.0, 11.0), vertical(13.0, 35.0, hex(0xff6b6b), hex(0xb3162b)));
    bar.fill(rounded(12.0, 15.0, 222.0, 21.0, 3.0), solid(alpha(WHITE, 0.35)));
    bar.save(&dir.join("health_bar.png"))?;

    let mut cursor = Canvas::new(64, 64);
    let arrow = vec![(10.0, 6.0), (10.0, 50.0), (22.0, 40.0), (30.0, 58.0), (38.0, 54.0), (30.0, 37.0), (46.0, 37.0)];
    let grown: Vec<(f32, f32)> =
        arrow.iter().map(|&(x, y)| (24.0 + (x - 24.0) * 1.16, 34.0 + (y - 34.0) * 1.12)).collect();
    cursor.fill(polygon(grown), solid(hex(0x111111)));
    cursor.fill(polygon(arrow), vertical(6.0, 58.0, WHITE, hex(0xd6dbe6)));
    cursor.save(&dir.join("cursor.png"))
}

fn button(top: Rgba, bottom: Rgba, outline: Rgba, play: bool) -> Canvas {
    let mut c = Canvas::new(256, 96);
    c.fill(rounded(4.0, 4.0, 252.0, 92.0, 26.0), solid(outline));
    c.fill(rounded(8.0, 8.0, 248.0, 86.0, 22.0), vertical(8.0, 86.0, top, bottom));
    c.fill(rounded(16.0, 12.0, 240.0, 44.0, 16.0), solid(alpha(WHITE, 0.22)));
    if play {
        c.fill(polygon(vec![(114.0, 28.0), (114.0, 68.0), (148.0, 48.0)]), solid(WHITE));
    } else {
        c.fill(rounded(110.0, 28.0, 122.0, 68.0, 3.0), solid(WHITE));
        c.fill(rounded(134.0, 28.0, 146.0, 68.0, 3.0), solid(WHITE));
    }
    c
}
