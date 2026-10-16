//! A small library of made-up game assets, drawn and synthesised here, so the README's pictures
//! show what the app does without anyone's own files: item icons, tiles, backgrounds, interface
//! parts, sprite sheets and GIFs, sound effects and music, and 3D models, with a few favorites,
//! collections and categories.

use std::f32::consts::{PI, TAU};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

use asset_browser::groups::{FAVORITES, GroupKind, Groups};
use asset_browser::paths;

/// Left in a folder holding the library, so it can be written there again.
const MARK: &str = ".made-up-assets";

/// Writes the library into `root`: an empty folder, or one written before. Anything else in it
/// would be in the pictures, and nothing is deleted here but what a run before added (`INCOMING`).
pub fn write(root: &Path) -> io::Result<()> {
    let taken = fs::read_dir(root).is_ok_and(|mut entries| entries.next().is_some());
    if taken && !root.join(MARK).exists() {
        let message =
            format!("{} has other things in it, which the pictures would show: give an empty folder", root.display());
        return Err(io::Error::other(message));
    }
    fs::create_dir_all(root)?;
    fs::write(
        root.join(MARK),
        "Made up for the README's pictures by src/bin/shots/demo.rs, which writes them again here.\n",
    )?;
    // What arrived during the last run arrives again.
    for file in INCOMING {
        let _ = fs::remove_file(root.join(file));
    }
    items(&root.join("Items"))?;
    tiles(&root.join("Environment/Tiles"))?;
    backgrounds(&root.join("Environment/Backgrounds"))?;
    interface(&root.join("UI"))?;
    characters(&root.join("Characters"))?;
    sounds(&root.join("Audio"))?;
    models(&root.join("Models"))
}

/// Files that arrive while the app is open, one at a time, for the clip of the library keeping
/// itself current.
pub const INCOMING: [&str; 4] =
    ["Items/gem_amber.png", "Items/gem_amethyst.png", "Items/potion_purple.png", "Items/potion_yellow.png"];

/// Writes the `i`th of `INCOMING` into the library in `root`.
pub fn arrive(root: &Path, i: usize) -> io::Result<()> {
    let canvas = match i {
        0 => gem(hex(0xf29a2e)),
        1 => gem(hex(0x9b5cf0)),
        2 => potion(hex(0xb16cff)),
        _ => potion(hex(0xf5c542)),
    };
    canvas.save(&root.join(INCOMING[i.min(INCOMING.len() - 1)]))
}

/// Favorites, collections and categories over the library in `root`, in the app's user data.
#[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
pub fn organize(root: &Path) {
    let mut groups = Groups::open(&paths::user_data_file());
    let paths = |files: &[&str]| files.iter().map(|file| root.join(file)).collect::<Vec<_>>();
    groups.add(
        FAVORITES,
        &paths(&[
            "Items/gem_ruby.png",
            "Items/potion_red.png",
            "Characters/slime_bounce.png",
            "Audio/Music/menu_theme.wav",
            "Models/crystal.obj",
        ]),
    );
    let collections: [(&str, &[&str]); 2] = [
        (
            "Forest level",
            &[
                "Environment/Tiles/grass.png",
                "Environment/Backgrounds/forest_day.png",
                "Models/tree.obj",
                "Models/mushroom.obj",
                "Items/mushroom.png",
                "Audio/SFX/footstep_grass.wav",
                "Audio/Music/ambience_forest.wav",
            ],
        ),
        (
            "Boss fight",
            &[
                "Audio/Music/battle_loop.wav",
                "Audio/SFX/explosion.wav",
                "Environment/Tiles/lava.png",
                "Items/bomb.png",
                "Items/sword.png",
                "Items/shield.png",
                "Characters/flame.gif",
            ],
        ),
    ];
    for (name, files) in collections {
        if let Some(id) = groups.create(GroupKind::Collection, name) {
            groups.add(id, &paths(files));
        }
    }
    let categories: [(&str, u8, &[&str]); 4] = [
        (
            "Pickups",
            2,
            &[
                "Items/coin.png",
                "Items/gem_ruby.png",
                "Items/gem_emerald.png",
                "Items/gem_sapphire.png",
                "Items/potion_red.png",
                "Items/potion_blue.png",
                "Items/potion_green.png",
                "Items/heart.png",
                "Items/key.png",
                "Items/star.png",
                "Characters/coin_strip8.png",
                "Audio/SFX/coin.wav",
                "Audio/SFX/powerup.wav",
            ],
        ),
        ("Enemies", 0, &["Characters/slime_bounce.png", "Characters/flame.gif", "Audio/SFX/hit.wav"]),
        (
            "Interface",
            4,
            &[
                "UI/button_play.png",
                "UI/button_pause.png",
                "UI/panel.png",
                "UI/health_bar.png",
                "UI/cursor.png",
                "Audio/SFX/click.wav",
            ],
        ),
        (
            "Magic",
            6,
            &[
                "Models/crystal.obj",
                "Audio/SFX/magic_spell.wav",
                "Items/potion_blue.png",
                "Characters/portal.gif",
                "Items/gem_sapphire.png",
            ],
        ),
    ];
    for (name, color, files) in categories {
        if let Some(id) = groups.create(GroupKind::Category, name) {
            groups.set_color(id, color);
            groups.add(id, &paths(files));
        }
    }
}

// Colours ------------------------------------------------------------------------------------

/// Red, green, blue and alpha, from 0 to 1, not premultiplied.
type Rgba = [f32; 4];

const WHITE: Rgba = [1.0; 4];
const BLACK: Rgba = [0.0, 0.0, 0.0, 1.0];
const CLEAR: Rgba = [0.0; 4];

fn hex(rgb: u32) -> Rgba {
    let channel = |shift: u32| ((rgb >> shift) & 255) as f32 / 255.0;
    [channel(16), channel(8), channel(0), 1.0]
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

fn alpha(color: Rgba, alpha: f32) -> Rgba {
    [color[0], color[1], color[2], color[3] * alpha]
}

fn lighter(color: Rgba, t: f32) -> Rgba {
    mix(color, [1.0, 1.0, 1.0, color[3]], t)
}

fn darker(color: Rgba, t: f32) -> Rgba {
    mix(color, [0.0, 0.0, 0.0, color[3]], t)
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn solid(color: Rgba) -> impl Fn(f32, f32) -> Rgba {
    move |_, _| color
}

/// `top` at `y0`, `bottom` at `y1`.
fn vertical(y0: f32, y1: f32, top: Rgba, bottom: Rgba) -> impl Fn(f32, f32) -> Rgba {
    move |_, y| mix(top, bottom, (y - y0) / (y1 - y0))
}

/// Lit from (`x0`, `y0`): `light` there, `dark` `reach` away.
fn radial(x0: f32, y0: f32, reach: f32, light: Rgba, dark: Rgba) -> impl Fn(f32, f32) -> Rgba {
    move |x, y| mix(light, dark, (x - x0).hypot(y - y0) / reach)
}

// Drawing ------------------------------------------------------------------------------------

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<Rgba>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        Self { w, h, px: vec![CLEAR; w * h] }
    }

    /// Every pixel from `color` at its centre: for textures and backgrounds.
    fn paint(w: usize, h: usize, color: impl Fn(f32, f32) -> Rgba) -> Self {
        let mut canvas = Self::new(w, h);
        for y in 0..h {
            for x in 0..w {
                canvas.px[y * w + x] = color(x as f32 + 0.5, y as f32 + 0.5);
            }
        }
        canvas
    }

    /// `color` over where `inside` holds, with smooth edges (16 samples a pixel).
    fn fill(&mut self, inside: impl Fn(f32, f32) -> bool, color: impl Fn(f32, f32) -> Rgba) {
        self.fill_in([0.0, 0.0, self.w as f32, self.h as f32], inside, color);
    }

    /// `fill`, looking only within `[x0, y0, x1, y1]`.
    fn fill_in(&mut self, area: [f32; 4], inside: impl Fn(f32, f32) -> bool, color: impl Fn(f32, f32) -> Rgba) {
        let clamp = |v: f32, max: usize| (v.max(0.0) as usize).min(max);
        let (x0, y0) = (clamp(area[0].floor(), self.w), clamp(area[1].floor(), self.h));
        let (x1, y1) = (clamp(area[2].ceil(), self.w), clamp(area[3].ceil(), self.h));
        for y in y0..y1 {
            for x in x0..x1 {
                let mut hits = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        if inside(x as f32 + (sx as f32 + 0.5) / 4.0, y as f32 + (sy as f32 + 0.5) / 4.0) {
                            hits += 1;
                        }
                    }
                }
                if hits > 0 {
                    let color = color(x as f32 + 0.5, y as f32 + 0.5);
                    self.blend(x, y, color, hits as f32 / 16.0);
                }
            }
        }
    }

    fn blend(&mut self, x: usize, y: usize, color: Rgba, coverage: f32) {
        let a = color[3] * coverage;
        let under = &mut self.px[y * self.w + x];
        let (keep, out) = (under[3] * (1.0 - a), a + under[3] * (1.0 - a));
        if out > 0.0 {
            for (channel, value) in under.iter_mut().zip(color).take(3) {
                *channel = (value * a + *channel * keep) / out;
            }
            under[3] = out;
        }
    }

    fn rgba8(&self) -> Vec<u8> {
        self.px.iter().flat_map(|color| color.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)).collect()
    }

    fn save(&self, path: &Path) -> io::Result<()> {
        image::save_buffer(path, &self.rgba8(), self.w as u32, self.h as u32, image::ExtendedColorType::Rgba8)
            .map_err(io::Error::other)
    }
}

fn circle(cx: f32, cy: f32, r: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| ((x - cx) / rx).powi(2) + ((y - cy) / ry).powi(2) <= 1.0
}

fn ring(cx: f32, cy: f32, outer: f32, inner: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| {
        let d = (x - cx).hypot(y - cy);
        d <= outer && d >= inner
    }
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| x >= x0 && x <= x1 && y >= y0 && y <= y1
}

fn rounded(x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| {
        let dx = (x0 + r - x).max(x - (x1 - r)).max(0.0);
        let dy = (y0 + r - y).max(y - (y1 - r)).max(0.0);
        x >= x0 && x <= x1 && y >= y0 && y <= y1 && dx * dx + dy * dy <= r * r
    }
}

fn capsule(ax: f32, ay: f32, bx: f32, by: f32, r: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| {
        let (px, py, vx, vy) = (x - ax, y - ay, bx - ax, by - ay);
        let t = ((px * vx + py * vy) / (vx * vx + vy * vy)).clamp(0.0, 1.0);
        (px - vx * t).powi(2) + (py - vy * t).powi(2) <= r * r
    }
}

fn polygon(points: Vec<(f32, f32)>) -> impl Fn(f32, f32) -> bool {
    move |x, y| {
        let mut inside = false;
        let mut j = points.len() - 1;
        for (i, &(xi, yi)) in points.iter().enumerate() {
            let (xj, yj) = points[j];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                inside = !inside;
            }
            j = i;
        }
        inside
    }
}

/// A star's corners, `count` points, the first straight up (turned by `turn`).
#[expect(clippy::too_many_arguments, reason = "refactor: take fewer, grouped arguments")]
fn star_points(cx: f32, cy: f32, outer: f32, inner: f32, count: usize, turn: f32) -> Vec<(f32, f32)> {
    (0..count * 2)
        .map(|i| {
            let r = if i % 2 == 0 { outer } else { inner };
            let a = turn + i as f32 * PI / count as f32 - PI / 2.0;
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

/// `shape` turned by `angle` around (`cx`, `cy`).
fn turned(cx: f32, cy: f32, angle: f32, shape: impl Fn(f32, f32) -> bool) -> impl Fn(f32, f32) -> bool {
    let (sin, cos) = angle.sin_cos();
    move |x, y| {
        let (dx, dy) = (x - cx, y - cy);
        shape(cx + dx * cos + dy * sin, cy - dx * sin + dy * cos)
    }
}

// Noise --------------------------------------------------------------------------------------

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h =
        (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

/// Smooth noise from 0 to 1 that repeats every `period` cells, so tiles join up.
fn noise(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - xi as f32, y - yi as f32);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let at = |dx: i32, dy: i32| hash((xi + dx).rem_euclid(period), (yi + dy).rem_euclid(period), seed);
    let top = at(0, 0) + (at(1, 0) - at(0, 0)) * sx;
    let bottom = at(0, 1) + (at(1, 1) - at(0, 1)) * sx;
    top + (bottom - top) * sy
}

/// Layers of `noise`, each finer and fainter.
fn fbm(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut total, mut scale) = (0.0, 1.0, 0.0, 1);
    for octave in 0..4 {
        sum += noise(x * scale as f32, y * scale as f32, period * scale, seed + octave) * amp;
        total += amp;
        amp *= 0.5;
        scale *= 2;
    }
    sum / total
}

/// Distances to the nearest and next nearest of points scattered one a cell (repeating every
/// `period` cells), and the nearest one's cell.
fn cells(x: f32, y: f32, period: i32, seed: u32) -> (f32, f32, i32) {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (mut d1, mut d2, mut id) = (f32::MAX, f32::MAX, 0);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (cx, cy) = (xi + dx, yi + dy);
            let (wx, wy) = (cx.rem_euclid(period), cy.rem_euclid(period));
            let px = cx as f32 + 0.15 + 0.7 * hash(wx, wy, seed);
            let py = cy as f32 + 0.15 + 0.7 * hash(wx, wy, seed + 1);
            let d = (x - px).hypot(y - py);
            if d < d1 {
                (d2, d1, id) = (d1, d, wy * period + wx);
            } else if d < d2 {
                d2 = d;
            }
        }
    }
    (d1, d2, id)
}

// Items ----------------------------------------------------------------------------------------

fn items(dir: &Path) -> io::Result<()> {
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

fn potion(liquid: Rgba) -> Canvas {
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

fn gem(color: Rgba) -> Canvas {
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
    c.fill(polygon(star_points(47.0, 39.0, 10.0, 2.5, 4, 0.0)), solid(alpha(WHITE, 0.9)));
    c
}

fn coin() -> Canvas {
    let mut c = Canvas::new(128, 128);
    c.fill(circle(64.0, 64.0, 50.0), solid(hex(0x6b4a12)));
    c.fill(circle(64.0, 64.0, 46.0), radial(46.0, 44.0, 76.0, hex(0xffe58a), hex(0xd18f16)));
    c.fill(ring(64.0, 64.0, 37.0, 33.0), solid(hex(0xb47c16)));
    c.fill(
        polygon(star_points(64.0, 66.0, 24.0, 10.0, 5, 0.0)),
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
    c.fill(polygon(star_points(110.0, 16.0, 13.0, 5.0, 8, 0.2)), solid(hex(0xffa024)));
    c.fill(polygon(star_points(110.0, 16.0, 7.0, 3.0, 8, 0.0)), solid(hex(0xfff3a0)));
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
    c.fill(polygon(star_points(64.0, 68.0, 58.0, 26.0, 5, 0.0)), solid(hex(0x7a4a00)));
    c.fill(
        polygon(star_points(64.0, 68.0, 52.0, 23.0, 5, 0.0)),
        radial(52.0, 46.0, 72.0, hex(0xfff2a8), hex(0xf0a21c)),
    );
    c.fill(ellipse(52.0, 48.0, 7.0, 5.0), solid(alpha(WHITE, 0.7)));
    c
}

// Tiles ----------------------------------------------------------------------------------------

#[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
fn tiles(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let tile = |name: &str, color: &dyn Fn(f32, f32) -> Rgba| Canvas::paint(128, 128, color).save(&dir.join(name));
    tile("grass.png", &|x, y| {
        let n = fbm(x / 16.0, y / 16.0, 8, 1);
        let blades = noise(x / 2.0, y / 8.0, 64, 2);
        mix(hex(0x2c7a33), hex(0x82c653), n * 0.75 + blades * 0.45 - 0.15)
    })?;
    tile("dirt.png", &|x, y| {
        let n = fbm(x / 16.0, y / 16.0, 8, 3);
        let (d1, _, id) = cells(x / 16.0, y / 16.0, 8, 4);
        let ground = mix(hex(0x55361f), hex(0x8d5f37), n);
        if d1 < 0.16 && hash(id, 0, 5) > 0.55 { mix(hex(0x9b8a78), hex(0x5f5246), d1 / 0.16) } else { ground }
    })?;
    tile("stone.png", &|x, y| {
        let (d1, d2, id) = cells(x / 32.0, y / 32.0, 4, 6);
        let tone =
            mix(hex(0x5a606c), hex(0xa3aab6), 0.3 + 0.5 * hash(id, 1, 7) + fbm(x / 8.0, y / 8.0, 16, 8) * 0.3 - 0.15);
        let edge = d2 - d1;
        if edge < 0.06 { hex(0x2b2e35) } else { mix(darker(tone, 0.25), tone, smoothstep(0.06, 0.3, edge)) }
    })?;
    tile("water.png", &|x, y| {
        let n = fbm(x / 32.0, y / 32.0, 4, 9);
        let wave = (x * 0.09 + n * 7.0 + y * 0.05).sin();
        let base = mix(hex(0x1d5aa3), hex(0x3d92dc), n);
        mix(base, hex(0xd2ecff), (wave - 0.86) * 6.0)
    })?;
    tile("sand.png", &|x, y| {
        let n = fbm(x / 24.0, y / 24.0, 8, 10);
        let ripple = (y * 0.22 + n * 7.0).sin() * 0.5 + 0.5;
        mix(hex(0xcfa868), hex(0xf2dca6), n * 0.6 + ripple * 0.35)
    })?;
    tile("lava.png", &|x, y| {
        let (d1, d2, _) = cells(x / 28.0, y / 28.0, 4, 11);
        let glow = smoothstep(0.22, 0.0, d2 - d1);
        let crust = mix(hex(0x2d140d), hex(0x5c2a17), fbm(x / 12.0, y / 12.0, 8, 12) + d1 * 0.3);
        mix(crust, mix(hex(0xff4d12), hex(0xffd64a), glow * glow), glow)
    })?;
    tile("snow.png", &|x, y| {
        let n = fbm(x / 20.0, y / 20.0, 8, 13);
        let sparkle = if hash(x as i32, y as i32, 14) > 0.992 { 0.6 } else { 0.0 };
        lighter(mix(hex(0xc9d8ea), hex(0xffffff), n * 0.9 + 0.2), sparkle)
    })?;
    tile("bricks.png", &|x, y| {
        let row = (y / 16.0).floor();
        let shifted = x + if row as i32 % 2 == 0 { 0.0 } else { 16.0 };
        if y.rem_euclid(16.0) < 2.0 || shifted.rem_euclid(32.0) < 2.0 {
            return hex(0xb9b0a2);
        }
        let brick = hash((shifted / 32.0).floor() as i32, row as i32, 15);
        darker(mix(hex(0x8e3b2e), hex(0xbd5e43), brick), fbm(x / 6.0, y / 6.0, 32, 16) * 0.35)
    })?;
    tile("planks.png", &|x, y| {
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
    })
}

// Backgrounds ----------------------------------------------------------------------------------

fn backgrounds(dir: &Path) -> io::Result<()> {
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

// Interface ------------------------------------------------------------------------------------

fn interface(dir: &Path) -> io::Result<()> {
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

// Characters -------------------------------------------------------------------------------------

fn characters(dir: &Path) -> io::Result<()> {
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

// Sounds -----------------------------------------------------------------------------------------

const RATE: u32 = 44_100;

#[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
fn sounds(dir: &Path) -> io::Result<()> {
    let (sfx, music) = (dir.join("SFX"), dir.join("Music"));
    fs::create_dir_all(&sfx)?;
    fs::create_dir_all(&music)?;
    let mut noise = Noise(0x9e37_79b9);

    let mut phase = 0.0;
    wav(
        &sfx.join("jump.wav"),
        synth(0.32, |t| {
            phase += (260.0 * (3.0f32).powf(t / 0.32)) / RATE as f32;
            square(phase) * (1.0 - t / 0.32).powi(2)
        }),
    )?;
    wav(
        &sfx.join("coin.wav"),
        synth(0.5, |t| if t < 0.08 { square(t * 988.0) * 0.8 } else { square(t * 1319.0) * (-(t - 0.08) * 7.0).exp() }),
    )?;
    let mut phase = 0.0;
    wav(
        &sfx.join("hit.wav"),
        synth(0.3, |t| {
            phase += (140.0 - 80.0 * t / 0.3) / RATE as f32;
            noise.sample() * (-t * 28.0).exp() + sine(phase) * (-t * 12.0).exp() * 0.8
        }),
    )?;
    let (mut low, mut phase) = (0.0, 0.0);
    wav(
        &sfx.join("explosion.wav"),
        synth(1.7, |t| {
            low += (noise.sample() - low) * (0.3 * (-t * 2.0).exp() + 0.02);
            phase += 45.0 / RATE as f32;
            (low * 2.5 + sine(phase) * 0.5 * (-t * 3.0).exp()) * (t / 0.01).min(1.0) * (-t * 2.2).exp()
        }),
    )?;
    let mut phase = 0.0;
    wav(
        &sfx.join("laser.wav"),
        synth(0.36, |t| {
            phase += (1800.0 * (220.0f32 / 1800.0).powf(t / 0.36) * (1.0 + 0.02 * (t * 90.0).sin())) / RATE as f32;
            sine(phase) * (1.0 - t / 0.36)
        }),
    )?;
    wav(
        &sfx.join("powerup.wav"),
        synth(0.6, |t| {
            let notes = [523.25, 659.25, 783.99, 1046.5, 1318.5, 1568.0];
            let step = ((t / 0.1) as usize).min(notes.len() - 1);
            square(t * notes[step]) * (-(t % 0.1) * 12.0).exp() * (1.0 - t / 0.6 * 0.5)
        }),
    )?;
    let mut high = 0.0;
    wav(
        &sfx.join("footstep_grass.wav"),
        synth(1.7, |t| {
            let n = noise.sample();
            high += (n - high) * 0.35;
            let crunch: f32 = [0.1, 0.5, 0.9, 1.3]
                .iter()
                .map(|&at| {
                    let dt = t - at;
                    if (0.0..0.14).contains(&dt) {
                        (-dt * 32.0).exp() * (0.6 + 0.4 * (dt * 140.0).sin().abs())
                    } else {
                        0.0
                    }
                })
                .sum();
            (n - high) * crunch
        }),
    )?;
    let mut phase = 0.0;
    wav(
        &sfx.join("door_open.wav"),
        synth(1.5, |t| {
            phase += (105.0 + 30.0 * (t * 7.0).sin() + 6.0 * noise.sample()) / RATE as f32;
            let creak = saw(phase) * (t / 0.15).min(1.0) * (1.0 - (t - 0.9).max(0.0) / 0.3).max(0.0) * 0.5;
            let dt = t - 1.22;
            let thud = if dt >= 0.0 { sine(dt * 68.0) * (-dt * 14.0).exp() } else { 0.0 };
            creak + thud
        }),
    )?;
    wav(
        &sfx.join("click.wav"),
        synth(0.06, |t| sine(t * 2100.0) * (-t * 110.0).exp() + noise.sample() * (-t * 400.0).exp() * 0.5),
    )?;
    wav(
        &sfx.join("magic_spell.wav"),
        synth(1.4, |t| {
            let rise = 1.0 + t * 0.45;
            let tones: f32 = [880.0, 1320.0, 1760.0, 2640.0]
                .iter()
                .enumerate()
                .map(|(i, &f)| sine(t * f * rise) * (1.0 + (t * (9.0 + i as f32 * 3.0)).sin()) * 0.25)
                .sum();
            (tones + noise.sample() * 0.15 * (t * 23.0).sin().abs()) * (t / 0.12).min(1.0) * (-(t * 1.8)).exp()
        }),
    )?;

    wav(&music.join("menu_theme.wav"), menu_theme())?;
    wav(&music.join("battle_loop.wav"), battle_loop(&mut noise))?;
    wav(&music.join("ambience_forest.wav"), ambience(&mut noise))
}

fn menu_theme() -> Vec<f32> {
    let chords = [[261.63, 329.63, 392.0], [220.0, 261.63, 329.63], [174.61, 220.0, 261.63], [196.0, 246.94, 293.66]];
    let melody = [
        523.25, 0.0, 659.25, 587.33, 523.25, 0.0, 392.0, 0.0, 440.0, 523.25, 659.25, 0.0, 587.33, 0.0, 523.25, 0.0,
        349.23, 440.0, 523.25, 0.0, 587.33, 523.25, 440.0, 0.0, 392.0, 0.0, 493.88, 587.33, 783.99, 0.0, 587.33, 0.0,
    ];
    synth(16.0, |t| {
        let chord = chords[(t / 2.0) as usize % 4];
        let in_chord = t % 2.0;
        let pad_env = (in_chord / 0.3).min(1.0) * (1.0 - (in_chord - 1.6).max(0.0) / 0.4);
        let pad = chord.iter().map(|&f| tri(t * f)).sum::<f32>() / 3.0 * 0.35 * pad_env;
        let bass = sine(t * chord[0] / 2.0) * (-(t % 0.6) * 4.0).exp() * 0.5;
        let note = melody[(t / 0.25) as usize % melody.len()];
        let lead = if note > 0.0 { square(t * note) * 0.12 * (-(t % 0.25) * 6.0).exp() } else { 0.0 };
        (pad + bass + lead) * (t / 0.5).min(1.0) * ((16.0 - t) / 1.0).min(1.0)
    })
}

fn battle_loop(noise: &mut Noise) -> Vec<f32> {
    let bass_notes = [110.0, 110.0, 130.81, 110.0, 98.0, 110.0, 82.41, 98.0];
    let mut low = 0.0;
    synth(12.0, |t| {
        let beat = t % 0.4;
        let in_bar = t % 1.6;
        let kick = (TAU * (48.0 + 90.0 * (-beat * 25.0).exp()) * beat).sin() * (-beat * 9.0).exp();
        let snare_at = in_bar % 0.8 - 0.4;
        let snare = if snare_at >= 0.0 { noise.sample() * (-snare_at * 18.0).exp() * 0.55 } else { 0.0 };
        let hat = noise.sample() * (-(t % 0.2) * 60.0).exp() * 0.12;
        let raw = saw(t * bass_notes[(t / 0.2) as usize % 8]) * (-(t % 0.2) * 5.0).exp();
        low += (raw - low) * 0.12;
        let stab = [220.0, 261.63, 329.63].iter().map(|&f| square(t * f)).sum::<f32>() * (-in_bar * 5.0).exp() * 0.05;
        kick * 0.9 + snare + hat + low * 0.9 + stab
    })
}

fn ambience(noise: &mut Noise) -> Vec<f32> {
    let chirps = [
        (1.2, 3200.0),
        (1.45, 3600.0),
        (4.0, 2800.0),
        (4.2, 3000.0),
        (7.5, 3900.0),
        (7.7, 4100.0),
        (7.9, 3900.0),
        (11.0, 3000.0),
        (13.3, 3500.0),
        (13.5, 3700.0),
        (16.8, 2900.0),
        (17.0, 3100.0),
        (18.6, 4000.0),
    ];
    let mut low = 0.0;
    synth(20.0, |t| {
        low += (noise.sample() - low) * 0.02;
        let wind = low * (0.55 + 0.45 * (t * 0.35).sin()) * 3.0;
        let birds: f32 = chirps
            .iter()
            .map(|&(at, f)| {
                let dt = t - at;
                if (0.0..0.18).contains(&dt) {
                    sine(f * dt + 4000.0 * dt * dt) * (dt * PI / 0.18).sin() * 0.35
                } else {
                    0.0
                }
            })
            .sum();
        wind + birds
    })
}

struct Noise(u32);

impl Noise {
    /// From -1 to 1.
    fn sample(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32 * 2.0 - 1.0
    }
}

fn sine(phase: f32) -> f32 {
    (phase * TAU).sin()
}

fn square(phase: f32) -> f32 {
    if phase.fract() < 0.5 { 1.0 } else { -1.0 }
}

fn tri(phase: f32) -> f32 {
    4.0 * (phase.fract() - 0.5).abs() - 1.0
}

fn saw(phase: f32) -> f32 {
    2.0 * phase.fract() - 1.0
}

/// `seconds` of sound from `f(t)`, scaled so its loudest point is at 0.9.
fn synth(seconds: f32, mut f: impl FnMut(f32) -> f32) -> Vec<f32> {
    let mut samples: Vec<f32> = (0..(seconds * RATE as f32) as usize).map(|i| f(i as f32 / RATE as f32)).collect();
    let peak = samples.iter().fold(1e-6f32, |peak, v| peak.max(v.abs()));
    for sample in &mut samples {
        *sample *= 0.9 / peak;
    }
    samples
}

/// 16-bit mono WAV.
fn wav(path: &Path, samples: Vec<f32>) -> io::Result<()> {
    let data: Vec<u8> =
        samples.into_iter().flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
    let mut out = Vec::with_capacity(44 + data.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // one channel
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    fs::write(path, out)
}

// Models -----------------------------------------------------------------------------------------

type Vec3 = [f32; 3];

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: Vec3) -> Vec3 {
    let len = dot(v, v).sqrt().max(1e-9);
    [v[0] / len, v[1] / len, v[2] / len]
}

/// Triangles, each with its own corners (so each face is shaded flat), and their materials.
#[derive(Default)]
struct Mesh {
    corners: Vec<Vec3>,
    faces: Vec<usize>,
    materials: Vec<(&'static str, u32)>,
}

impl Mesh {
    fn material(&mut self, name: &'static str, rgb: u32) -> usize {
        self.materials.push((name, rgb));
        self.materials.len() - 1
    }

    /// A triangle, turned to face away from `inside`.
    #[expect(clippy::too_many_arguments, reason = "refactor: take fewer, grouped arguments")]
    fn tri(&mut self, material: usize, a: Vec3, b: Vec3, c: Vec3, inside: Vec3) {
        let centre = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0];
        let (b, c) = if dot(cross(sub(b, a), sub(c, a)), sub(centre, inside)) < 0.0 { (c, b) } else { (b, c) };
        self.corners.extend([a, b, c]);
        self.faces.push(material);
    }

    fn quad(&mut self, material: usize, [a, b, c, d]: [Vec3; 4], inside: Vec3) {
        self.tri(material, a, b, c, inside);
        self.tri(material, a, c, d, inside);
    }

    fn cuboid(&mut self, material: usize, min: Vec3, max: Vec3) {
        let corner = |i: usize| {
            [
                if i & 1 == 0 { min[0] } else { max[0] },
                if i & 2 == 0 { min[1] } else { max[1] },
                if i & 4 == 0 { min[2] } else { max[2] },
            ]
        };
        let inside = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0, (min[2] + max[2]) / 2.0];
        for face in [[0, 1, 3, 2], [4, 5, 7, 6], [0, 1, 5, 4], [2, 3, 7, 6], [0, 2, 6, 4], [1, 3, 7, 5]] {
            self.quad(material, face.map(corner), inside);
        }
    }

    /// A shape turned around an upright axis through `at`: `profile` is (radius, height) from the
    /// bottom up, closed by flat caps where it doesn't end on the axis.
    #[expect(clippy::too_many_arguments, reason = "refactor: take fewer, grouped arguments")]
    fn lathe(&mut self, material: usize, at: Vec3, profile: &[(f32, f32)], segments: usize, caps: Option<usize>) {
        let point = |(r, h): (f32, f32), i: usize| {
            let a = i as f32 / segments as f32 * TAU;
            [at[0] + r * a.cos(), at[1] + h, at[2] + r * a.sin()]
        };
        for pair in profile.windows(2) {
            let inside = [at[0], at[1] + (pair[0].1 + pair[1].1) / 2.0, at[2]];
            for i in 0..segments {
                let (a, b) = (point(pair[0], i), point(pair[0], i + 1));
                let (c, d) = (point(pair[1], i + 1), point(pair[1], i));
                if pair[0].0 == 0.0 {
                    self.tri(material, a, c, d, inside);
                } else if pair[1].0 == 0.0 {
                    self.tri(material, a, b, c, inside);
                } else {
                    self.quad(material, [a, b, c, d], inside);
                }
            }
        }
        let (first, last) = (profile[0], profile[profile.len() - 1]);
        let cap = caps.unwrap_or(material);
        for (end, inward) in [(first, 0.01), (last, -0.01)] {
            if end.0 > 0.0 {
                let centre = [at[0], at[1] + end.1, at[2]];
                let inside = [at[0], at[1] + end.1 + inward, at[2]];
                for i in 0..segments {
                    self.tri(cap, centre, point(end, i), point(end, i + 1), inside);
                }
            }
        }
    }

    /// Moves and turns (around the upright axis, then tips over sideways) what was added since
    /// `from`.
    fn place(&mut self, from: usize, turn: f32, tip: f32, offset: Vec3) {
        let (st, ct) = turn.sin_cos();
        let (sp, cp) = tip.sin_cos();
        for p in &mut self.corners[from * 3..] {
            let (x, y) = (p[0] * cp - p[1] * sp, p[0] * sp + p[1] * cp);
            let (x, z) = (x * ct - p[2] * st, x * st + p[2] * ct);
            *p = [x + offset[0], y + offset[1], z + offset[2]];
        }
    }

    fn write(&self, dir: &Path, name: &str) -> io::Result<()> {
        let mut mtl = String::new();
        for &(material, rgb) in &self.materials {
            let [r, g, b, _] = hex(rgb);
            let _ = writeln!(mtl, "newmtl {material}\nKd {r:.3} {g:.3} {b:.3}\n");
        }
        fs::write(dir.join(format!("{name}.mtl")), mtl)?;
        let mut obj = format!("# Made up for the README's pictures (src/bin/shots/demo.rs)\nmtllib {name}.mtl\n");
        for p in &self.corners {
            let _ = writeln!(obj, "v {:.4} {:.4} {:.4}", p[0], p[1], p[2]);
        }
        for &[a, b, c] in self.corners.as_chunks::<3>().0 {
            let n = normalize(cross(sub(b, a), sub(c, a)));
            let _ = writeln!(obj, "vn {:.4} {:.4} {:.4}", n[0], n[1], n[2]);
        }
        let mut current = usize::MAX;
        for (i, &material) in self.faces.iter().enumerate() {
            if material != current {
                let _ = writeln!(obj, "usemtl {}", self.materials[material].0);
                current = material;
            }
            let _ = writeln!(obj, "f {0}//{n} {1}//{n} {2}//{n}", i * 3 + 1, i * 3 + 2, i * 3 + 3, n = i + 1);
        }
        fs::write(dir.join(format!("{name}.obj")), obj)
    }
}

#[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
fn models(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;

    let mut crate_box = Mesh::default();
    let (wood, frame) = (crate_box.material("wood", 0xc08a4e), crate_box.material("frame", 0x7a4f2a));
    crate_box.cuboid(wood, [-0.5, 0.0, -0.5], [0.5, 1.0, 0.5]);
    // A beam along each of the box's twelve edges, standing out a little.
    let range = |axis: usize| if axis == 1 { (0.0, 1.0) } else { (-0.5, 0.5) };
    for along in 0..3 {
        for (low_p, low_q) in [(true, true), (true, false), (false, true), (false, false)] {
            let (mut min, mut max) = ([0.0; 3], [0.0; 3]);
            (min[along], max[along]) = range(along);
            for (axis, low) in [((along + 1) % 3, low_p), ((along + 2) % 3, low_q)] {
                let (lo, hi) = range(axis);
                (min[axis], max[axis]) = if low { (lo - 0.01, lo + 0.09) } else { (hi - 0.09, hi + 0.01) };
            }
            crate_box.cuboid(frame, min, max);
        }
    }
    crate_box.write(dir, "crate")?;

    let mut barrel = Mesh::default();
    let (staves, hoops, top) =
        (barrel.material("staves", 0xa86b38), barrel.material("hoops", 0x4b4f58), barrel.material("lid", 0x8a5a2f));
    barrel.lathe(staves, [0.0; 3], &[(0.36, 0.0), (0.42, 0.2), (0.45, 0.5), (0.42, 0.8), (0.36, 1.0)], 20, Some(top));
    for h in [0.16, 0.8] {
        barrel.lathe(hoops, [0.0; 3], &[(0.432, h), (0.432, h + 0.06)], 20, None);
    }
    barrel.write(dir, "barrel")?;

    let mut rock = Mesh::default();
    let stone = rock.material("stone", 0x8c8f96);
    icosphere(&mut rock, stone, 2, |p| {
        let bump = 0.16 * (3.1 * p[0] + 1.3).sin() * (2.7 * p[1] + 0.4).sin() * (3.3 * p[2] + 2.1).sin()
            + 0.07 * (7.0 * p[0]).sin() * (6.0 * p[1] + 1.0).sin() * (8.0 * p[2]).sin();
        let r = 0.55 * (1.0 + bump);
        [p[0] * r * 1.25, (p[1] * r * 0.8).max(-0.18) + 0.18, p[2] * r]
    });
    rock.write(dir, "rock")?;

    let mut crystal = Mesh::default();
    let (glow, deep) = (crystal.material("crystal", 0x7fe3ff), crystal.material("crystal_dark", 0x8a5cff));
    crystal.lathe(glow, [0.0; 3], &[(0.0, 0.0), (0.2, 0.22), (0.2, 1.05), (0.0, 1.4)], 6, None);
    for (turn, tip, scale, material) in [(0.4, 0.5, 0.6, deep), (2.6, 0.45, 0.5, glow), (4.4, 0.4, 0.42, deep)] {
        let from = crystal.faces.len();
        crystal.lathe(
            material,
            [0.0; 3],
            &[(0.0, 0.0), (0.2 * scale, 0.22 * scale), (0.2 * scale, 1.05 * scale), (0.0, 1.4 * scale)],
            6,
            None,
        );
        // Out from the big one's foot, the way it leans.
        let (sin, cos) = f32::sin_cos(turn);
        crystal.place(from, turn, tip, [-0.17 * cos, 0.02, -0.17 * sin]);
    }
    crystal.lathe(deep, [0.0; 3], &[(0.42, 0.0), (0.3, 0.08)], 6, Some(deep));
    crystal.write(dir, "crystal")?;

    let mut tree = Mesh::default();
    let (bark, leaves) = (tree.material("bark", 0x6e4a2c), tree.material("leaves", 0x3f9a4a));
    tree.lathe(bark, [0.0; 3], &[(0.11, 0.0), (0.09, 0.55)], 8, None);
    for (r, base, top) in [(0.62, 0.4, 1.15), (0.5, 0.8, 1.45), (0.36, 1.15, 1.75)] {
        tree.lathe(leaves, [0.0; 3], &[(r, base), (0.0, top)], 8, None);
    }
    tree.write(dir, "tree")?;

    let mut mushroom = Mesh::default();
    let (stem, cap, gills) =
        (mushroom.material("stem", 0xf3e6cf), mushroom.material("cap", 0xd9342b), mushroom.material("gills", 0xe9d3b0));
    mushroom.lathe(stem, [0.0; 3], &[(0.17, 0.0), (0.15, 0.4), (0.13, 0.58)], 14, None);
    mushroom.lathe(
        cap,
        [0.0; 3],
        &[(0.56, 0.52), (0.54, 0.62), (0.46, 0.74), (0.32, 0.84), (0.0, 0.9)],
        18,
        Some(gills),
    );
    mushroom.write(dir, "mushroom")?;

    let mut house = Mesh::default();
    let (walls, roof, door, glass) = (
        house.material("walls", 0xeadcc2),
        house.material("roof", 0xb5432f),
        house.material("door", 0x5e3a20),
        house.material("windows", 0x9fd2f2),
    );
    house.cuboid(walls, [-0.6, 0.0, -0.45], [0.6, 0.8, 0.45]);
    house.cuboid(door, [-0.12, 0.0, 0.45], [0.12, 0.46, 0.47]);
    for x in [-0.4, 0.28] {
        house.cuboid(glass, [x, 0.4, 0.45], [x + 0.14, 0.6, 0.47]);
    }
    house.cuboid(roof, [0.25, 1.0, -0.15], [0.4, 1.4, 0.0]);
    let ridge_inside = [0.0, 0.95, 0.0];
    for side in [-1.0f32, 1.0] {
        house.quad(
            roof,
            [[-0.7, 0.75, side * 0.58], [0.7, 0.75, side * 0.58], [0.7, 1.3, 0.0], [-0.7, 1.3, 0.0]],
            ridge_inside,
        );
        house.tri(walls, [side * 0.6, 0.8, -0.45], [side * 0.6, 0.8, 0.45], [side * 0.6, 1.24, 0.0], [0.0, 1.0, 0.0]);
    }
    house.write(dir, "house")?;

    let mut donut = Mesh::default();
    let (dough, icing) = (donut.material("dough", 0xd99a55), donut.material("icing", 0xff8fc1));
    let (big, small, around, across) = (0.42, 0.19, 32, 16);
    let at = |i: usize, j: usize| {
        let (a, b) = (i as f32 / around as f32 * TAU, j as f32 / across as f32 * TAU);
        let r = big + small * b.cos();
        [r * a.cos(), 0.2 + small * b.sin(), r * a.sin()]
    };
    for i in 0..around {
        for j in 0..across {
            let mid = (i as f32 + 0.5) / around as f32 * TAU;
            let inside = [big * mid.cos(), 0.2, big * mid.sin()];
            let b = (j as f32 + 0.5) / across as f32 * TAU;
            let material = if b.sin() > -0.15 { icing } else { dough };
            donut.quad(material, [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)], inside);
        }
    }
    donut.write(dir, "donut")
}

/// A ball of triangles, `subdivisions` times finer than an icosahedron, its points moved by `shape`.
fn icosphere(mesh: &mut Mesh, material: usize, subdivisions: usize, shape: impl Fn(Vec3) -> Vec3) {
    let g = (1.0 + 5.0f32.sqrt()) / 2.0;
    let points = [
        [-1.0, g, 0.0],
        [1.0, g, 0.0],
        [-1.0, -g, 0.0],
        [1.0, -g, 0.0],
        [0.0, -1.0, g],
        [0.0, 1.0, g],
        [0.0, -1.0, -g],
        [0.0, 1.0, -g],
        [g, 0.0, -1.0],
        [g, 0.0, 1.0],
        [-g, 0.0, -1.0],
        [-g, 0.0, 1.0],
    ];
    let mut triangles: Vec<[Vec3; 3]> = [
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ]
    .iter()
    .map(|t| t.map(|i| normalize(points[i])))
    .collect();
    for _ in 0..subdivisions {
        triangles = triangles
            .iter()
            .flat_map(|&[a, b, c]| {
                let mid = |p: Vec3, q: Vec3| normalize([(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0, (p[2] + q[2]) / 2.0]);
                let (ab, bc, ca) = (mid(a, b), mid(b, c), mid(c, a));
                [[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]]
            })
            .collect();
    }
    let centre = shape([0.0, 0.0, 0.0]);
    for [a, b, c] in triangles {
        mesh.tri(material, shape(a), shape(b), shape(c), centre);
    }
}
