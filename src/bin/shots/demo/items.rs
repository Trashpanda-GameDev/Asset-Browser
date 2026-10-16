//! Item icons: potions, gems, a coin, a heart, a key, a sword, a shield, a bomb, a mushroom and a star.

use std::f32::consts::PI;
use std::fs;
use std::io;
use std::path::Path;

use super::paint::{
    Canvas, Rgba, WHITE, alpha, capsule, circle, darker, ellipse, hex, lighter, mix, noise, polygon, radial, rect,
    ring, rounded, solid, star_points, turned, vertical,
};

pub(super) fn items(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    potion(hex(0xe5484d)).save(&dir.join("potion_red.png"))?;
    potion(hex(0x3d8bfd)).save(&dir.join("potion_blue.png"))?;
    potion(hex(0x46c35a)).save(&dir.join("potion_green.png"))?;
    gem(hex(0xe0314b)).save(&dir.join("gem_ruby.png"))?;
    gem(hex(0x22b86a)).save(&dir.join("gem_emerald.png"))?;
    gem(hex(0x2f6fe0)).save(&dir.join("gem_sapphire.png"))?;
    coin().save(&dir.join("coin.png"))?;
    heart().save(&dir.join("heart.png"))?;
    key().save(&dir.join("key.png"))?;
    sword().save(&dir.join("sword.png"))?;
    shield().save(&dir.join("shield.png"))?;
    bomb().save(&dir.join("bomb.png"))?;
    mushroom().save(&dir.join("mushroom.png"))?;
    star().save(&dir.join("star.png"))
}

pub(super) fn potion(liquid: Rgba) -> Canvas {
    let mut c = Canvas::new(128, 128);
    let outline = hex(0x1d1b2a);
    c.fill(circle(64.0, 80.0, 41.0), solid(outline));
    c.fill(rounded(47.0, 26.0, 81.0, 60.0, 6.0), solid(outline));
    c.fill(rounded(45.0, 10.0, 83.0, 37.0, 8.0), solid(outline));
    let glass = hex(0xdfeaf6);
    c.fill(circle(64.0, 80.0, 37.0), solid(glass));
    c.fill(rect(51.0, 30.0, 77.0, 58.0), solid(glass));
    let body = circle(64.0, 80.0, 33.0);
    c.fill(
        move |x, y| body(x, y) && y > 68.0 + 3.0 * (x * 0.17).sin(),
        radial(50.0, 70.0, 52.0, lighter(liquid, 0.4), darker(liquid, 0.35)),
    );
    c.fill(rounded(49.0, 14.0, 79.0, 33.0, 5.0), vertical(14.0, 33.0, hex(0xd29a5c), hex(0x8a5a2b)));
    c.fill(ellipse(47.0, 74.0, 6.0, 13.0), solid(alpha(WHITE, 0.75)));
    c.fill(circle(79.0, 98.0, 4.0), solid(alpha(WHITE, 0.4)));
    c
}

pub(super) fn gem(color: Rgba) -> Canvas {
    let mut c = Canvas::new(128, 128);
    c.fill(
        polygon(vec![(37.0, 22.0), (91.0, 22.0), (118.0, 52.0), (64.0, 118.0), (10.0, 52.0)]),
        solid(darker(color, 0.75)),
    );
    c.fill(polygon(vec![(41.0, 28.0), (87.0, 28.0), (109.0, 52.0), (19.0, 52.0)]), solid(color));
    c.fill(polygon(vec![(50.0, 28.0), (78.0, 28.0), (87.0, 52.0), (41.0, 52.0)]), solid(lighter(color, 0.45)));
    c.fill(polygon(vec![(19.0, 52.0), (64.0, 52.0), (64.0, 108.0)]), solid(lighter(color, 0.12)));
    c.fill(polygon(vec![(64.0, 52.0), (109.0, 52.0), (64.0, 108.0)]), solid(darker(color, 0.35)));
    c.fill(polygon(vec![(41.0, 52.0), (87.0, 52.0), (64.0, 108.0)]), solid(color));
    c.fill(polygon(star_points((47.0, 39.0), 10.0, 2.5, 4, 0.0)), solid(alpha(WHITE, 0.9)));
    c
}

fn coin() -> Canvas {
    let mut c = Canvas::new(128, 128);
    c.fill(circle(64.0, 64.0, 50.0), solid(hex(0x6b4a12)));
    c.fill(circle(64.0, 64.0, 46.0), radial(46.0, 44.0, 76.0, hex(0xffe58a), hex(0xd18f16)));
    c.fill(ring(64.0, 64.0, 37.0, 33.0), solid(hex(0xb47c16)));
    c.fill(
        polygon(star_points((64.0, 66.0), 24.0, 10.0, 5, 0.0)),
        radial(56.0, 54.0, 34.0, hex(0xfff3c4), hex(0xe2a52a)),
    );
    let shine = ring(64.0, 64.0, 44.0, 40.0);
    c.fill(move |x, y| shine(x, y) && x < 58.0 && y < 58.0, solid(alpha(WHITE, 0.6)));
    c
}

fn heart() -> Canvas {
    let mut c = Canvas::new(128, 128);
    let shape = |scale: f32| {
        move |x: f32, y: f32| {
            let (u, v) = ((x - 64.0) / scale, -(y - 70.0) / scale);
            let a = u * u + v * v - 1.0;
            a * a * a - u * u * v * v * v <= 0.0
        }
    };
    c.fill(shape(46.0), solid(hex(0x5a0f1c)));
    c.fill(shape(42.0), radial(46.0, 44.0, 76.0, hex(0xff8392), hex(0xc8162e)));
    c.fill(ellipse(45.0, 46.0, 9.0, 6.0), solid(alpha(WHITE, 0.7)));
    c
}

fn key() -> Canvas {
    let mut c = Canvas::new(128, 128);
    let parts = |grow: f32| {
        let bow = ring(34.0, 64.0, 22.0 + grow, 11.0 - grow);
        let shaft = rect(50.0, 58.0 - grow, 110.0 + grow, 70.0 + grow);
        let tooth = rect(88.0 - grow, 66.0, 96.0 + grow, 86.0 + grow);
        let tip = rect(100.0 - grow, 66.0, 108.0 + grow, 80.0 + grow);
        turned(64.0, 64.0, 0.62, move |x, y| bow(x, y) || shaft(x, y) || tooth(x, y) || tip(x, y))
    };
    c.fill(parts(4.0), solid(hex(0x5e3d0c)));
    c.fill(parts(0.0), radial(40.0, 40.0, 110.0, hex(0xffe27a), hex(0xc58a17)));
    c
}

fn sword() -> Canvas {
    let mut c = Canvas::new(128, 128);
    let angle = PI / 4.0;
    let shape = |points: Vec<(f32, f32)>| turned(64.0, 64.0, angle, polygon(points));
    let outline = hex(0x1f2430);
    c.fill(shape(vec![(30.0, 54.0), (101.0, 54.0), (120.0, 64.0), (101.0, 74.0), (30.0, 74.0)]), solid(outline));
    c.fill(
        shape(vec![(32.0, 57.0), (100.0, 57.0), (115.0, 64.0), (100.0, 71.0), (32.0, 71.0)]),
        vertical(40.0, 90.0, hex(0xf2f5fa), hex(0x8a93a6)),
    );
    c.fill(turned(64.0, 64.0, angle, rect(34.0, 62.5, 98.0, 65.5)), solid(hex(0x9aa3b5)));
    c.fill(turned(64.0, 64.0, angle, rounded(20.0, 40.0, 36.0, 88.0, 4.0)), solid(outline));
    c.fill(turned(64.0, 64.0, angle, rounded(23.0, 43.0, 33.0, 85.0, 3.0)), solid(hex(0xe2b13c)));
    c.fill(turned(64.0, 64.0, angle, rect(6.0, 57.0, 23.0, 71.0)), solid(outline));
    c.fill(turned(64.0, 64.0, angle, rect(8.0, 60.0, 23.0, 68.0)), solid(hex(0x7a4a26)));
    c.fill(turned(64.0, 64.0, angle, circle(7.0, 64.0, 8.0)), solid(outline));
    c.fill(turned(64.0, 64.0, angle, circle(7.0, 64.0, 5.0)), solid(hex(0xe2b13c)));
    c
}

fn shield() -> Canvas {
    let mut c = Canvas::new(128, 128);
    let outline = |grow: f32| {
        let (left, right, top, mid, bottom) = (26.0 - grow, 102.0 + grow, 18.0 - grow, 62.0, 114.0 + grow);
        let mut points = vec![(left, top), (right, top)];
        let curve = |i: usize| {
            let a = i as f32 / 16.0 * PI / 2.0;
            (a.cos(), mid + (bottom - mid) * a.sin())
        };
        for i in 0..=16 {
            let (cos, y) = curve(i);
            points.push((64.0 + (right - 64.0) * cos, y));
        }
        for i in (0..=16).rev() {
            let (cos, y) = curve(i);
            points.push((64.0 - (64.0 - left) * cos, y));
        }
        polygon(points)
    };
    c.fill(outline(4.0), solid(hex(0x262a33)));
    c.fill(outline(0.0), vertical(18.0, 114.0, hex(0xd3d8e0), hex(0x737b8a)));
    c.fill(outline(-7.0), |x, y| {
        let board = ((x - 26.0) / 19.0).floor();
        let tone = if board as i32 % 2 == 0 { hex(0xa0662f) } else { hex(0x8a5426) };
        let grain = noise(x / 3.0, y / 18.0, 64, 5);
        if (x - 26.0).rem_euclid(19.0) < 1.5 { hex(0x4a2c14) } else { mix(tone, darker(tone, 0.3), grain * 0.6) }
    });
    c.fill(circle(64.0, 60.0, 15.0), solid(hex(0x262a33)));
    c.fill(circle(64.0, 60.0, 12.0), radial(59.0, 55.0, 18.0, hex(0xf4f6fa), hex(0x8d95a5)));
    c
}

fn bomb() -> Canvas {
    let mut c = Canvas::new(128, 128);
    let outline = hex(0x101218);
    c.fill(circle(58.0, 74.0, 42.0), solid(outline));
    c.fill(circle(58.0, 74.0, 38.0), radial(44.0, 58.0, 72.0, hex(0x5f677b), hex(0x1a1d27)));
    c.fill(turned(84.0, 40.0, -0.8, rounded(73.0, 29.0, 95.0, 51.0, 4.0)), solid(outline));
    c.fill(
        turned(84.0, 40.0, -0.8, rounded(76.0, 32.0, 92.0, 48.0, 3.0)),
        vertical(32.0, 48.0, hex(0x9aa3b5), hex(0x4b5263)),
    );
    c.fill(capsule(90.0, 32.0, 100.0, 21.0, 3.0), solid(hex(0xc8a46b)));
    c.fill(capsule(100.0, 21.0, 108.0, 17.0, 3.0), solid(hex(0xc8a46b)));
    c.fill(polygon(star_points((110.0, 16.0), 13.0, 5.0, 8, 0.2)), solid(hex(0xffa024)));
    c.fill(polygon(star_points((110.0, 16.0), 7.0, 3.0, 8, 0.0)), solid(hex(0xfff3a0)));
    c.fill(ellipse(44.0, 58.0, 10.0, 7.0), solid(alpha(WHITE, 0.45)));
    c
}

fn mushroom() -> Canvas {
    let mut c = Canvas::new(128, 128);
    c.fill(rounded(43.0, 54.0, 85.0, 114.0, 15.0), solid(hex(0x3b2a20)));
    c.fill(rounded(47.0, 58.0, 81.0, 110.0, 12.0), vertical(58.0, 110.0, hex(0xfff4e0), hex(0xd2b993)));
    let cap = |grow: f32| {
        let dome = ellipse(64.0, 62.0, 54.0 + grow, 44.0 + grow);
        move |x: f32, y: f32| dome(x, y) && y <= 66.0 + grow
    };
    c.fill(cap(4.0), solid(hex(0x3b1010)));
    c.fill(cap(0.0), radial(46.0, 30.0, 84.0, hex(0xff6f5c), hex(0xb01e2a)));
    for (x, y, r) in [(44.0, 38.0, 8.0), (70.0, 28.0, 6.0), (90.0, 48.0, 7.0), (62.0, 54.0, 5.0), (26.0, 58.0, 5.0)] {
        c.fill(circle(x, y, r), solid(hex(0xfff7ee)));
    }
    c
}

fn star() -> Canvas {
    let mut c = Canvas::new(128, 128);
    c.fill(polygon(star_points((64.0, 68.0), 58.0, 26.0, 5, 0.0)), solid(hex(0x7a4a00)));
    c.fill(
        polygon(star_points((64.0, 68.0), 52.0, 23.0, 5, 0.0)),
        radial(52.0, 46.0, 72.0, hex(0xfff2a8), hex(0xf0a21c)),
    );
    c.fill(ellipse(52.0, 48.0, 7.0, 5.0), solid(alpha(WHITE, 0.7)));
    c
}
