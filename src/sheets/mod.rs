//! Sprite sheets: pictures holding the frames of one or more animations. Where the frames are
//! comes from, most trusted first:
//! 1. a JSON file beside the picture, in the format Aseprite and TexturePacker write
//!    (`hero.json` for `hero.png`): exact rectangles and durations, and named animations:
//!    Aseprite's frame tags, or for an atlas, frames named `run-1`, `run-2`… grouped by name;
//! 2. the user's own grid for that picture, saved in the user data file (`SheetStore`);
//! 3. a guess: GameMaker's `name_strip8.png`, frames separated by fully transparent lines, or a
//!    strip of square frames when the file's name says it's a sheet.
//!
//! Rectangles are in the file's pixels; the picture the guesses look at may be a shrunk copy.

mod data;
mod guess;
mod store;

use std::collections::BTreeMap;
use std::path::Path;

use crate::imaging::Pixels;
use data::from_data_file;
use guess::{gaps, is_empty, name_says_sheet, square_strip, strip_count};

pub use store::{Saved, SheetStore};

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// Where the frame's pixels are in the sheet: x, y, w, h (w and h as drawn, before any
    /// rotation in the sheet).
    pub rect: [u32; 4],
    /// Stored turned 90° clockwise in the sheet (TexturePacker does this to pack tighter).
    pub rotated: bool,
    /// Where those pixels go in the whole frame, and the whole frame's size: frames trimmed of
    /// their empty border keep their place, so the animation doesn't jitter.
    pub offset: [u32; 2],
    pub size: [u32; 2],
    pub duration_ms: Option<u32>,
}

impl Frame {
    fn cell(rect: [u32; 4]) -> Self {
        Self { rect, rotated: false, offset: [0, 0], size: [rect[2], rect[3]], duration_ms: None }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Animation {
    pub name: String,
    /// Indices into `Sheet::frames`, in playing order.
    pub frames: Vec<usize>,
}

/// How a grid's frames are grouped into animations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Split {
    /// One animation per row: the most common layout.
    Rows,
    Columns,
    /// The whole sheet, in reading order.
    All,
}

impl Split {
    fn to_db(self) -> i64 {
        match self {
            Self::Rows => 0,
            Self::Columns => 1,
            Self::All => 2,
        }
    }

    fn from_db(value: i64) -> Self {
        match value {
            1 => Self::Columns,
            2 => Self::All,
            _ => Self::Rows,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    pub columns: u32,
    pub rows: u32,
    /// Empty border around the whole grid, and between frames, in file pixels.
    pub margin: u32,
    pub spacing: u32,
    pub split: Split,
}

impl Grid {
    pub fn strip(columns: u32) -> Self {
        Self { columns: columns.max(1), rows: 1, margin: 0, spacing: 0, split: Split::Rows }
    }

    /// Each frame's rectangle, row by row.
    fn cells(&self, w: u32, h: u32) -> Vec<[u32; 4]> {
        let (columns, rows) = (self.columns.max(1), self.rows.max(1));
        let inner_w = w.saturating_sub(2 * self.margin + (columns - 1) * self.spacing);
        let inner_h = h.saturating_sub(2 * self.margin + (rows - 1) * self.spacing);
        let (fw, fh) = (inner_w / columns, inner_h / rows);
        if fw == 0 || fh == 0 {
            return Vec::new();
        }
        let mut cells = Vec::with_capacity((columns * rows) as usize);
        for row in 0..rows {
            for column in 0..columns {
                let x = self.margin + column * (fw + self.spacing);
                let y = self.margin + row * (fh + self.spacing);
                cells.push([x, y, fw, fh]);
            }
        }
        cells
    }
}

/// How a grid was arrived at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Guess {
    /// GameMaker's `_strip8`.
    FileName,
    /// Fully transparent lines between the frames.
    Gaps,
    /// As many square frames as fit.
    SquareStrip,
    /// The user's own.
    User,
    /// A starting point when nothing was detected.
    Default,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// From the data file beside the picture (its file name).
    Data(String),
    Grid(Grid, Guess),
    /// An animated picture's own frames (a GIF, see `animated`): its canvas size, and how many
    /// frames it has, which can be more than were kept.
    Animated {
        width: u32,
        height: u32,
        total: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    pub frames: Vec<Frame>,
    pub animations: Vec<Animation>,
    pub source: Source,
}

impl Sheet {
    /// Whether this is surely meant as an animation, so the preview pane can play it at once:
    /// it came with its data, or its name says how many frames it has.
    pub fn is_certain(&self) -> bool {
        matches!(self.source, Source::Data(_) | Source::Grid(_, Guess::FileName) | Source::Animated { .. })
    }

    pub fn grid(&self) -> Option<Grid> {
        match self.source {
            Source::Grid(grid, _) => Some(grid),
            Source::Data(_) | Source::Animated { .. } => None,
        }
    }
}

/// What's known about a picture as a sprite sheet, or `None` when nothing suggests it is one.
pub fn detect(path: &Path, pixels: &Pixels) -> Option<Sheet> {
    if let Some(sheet) = from_data_file(path) {
        return Some(sheet);
    }
    let name = path.file_stem().map(|stem| stem.to_string_lossy().to_lowercase()).unwrap_or_default();
    if let Some(count) = strip_count(&name).filter(|&count| count >= 2 && pixels.src_w.is_multiple_of(count)) {
        return Some(from_grid(Grid::strip(count), pixels, Guess::FileName));
    }
    if let Some(grid) = gaps(pixels) {
        return Some(from_grid(grid, pixels, Guess::Gaps));
    }
    if name_says_sheet(&name)
        && let Some(grid) = square_strip(pixels.src_w, pixels.src_h)
    {
        return Some(from_grid(grid, pixels, Guess::SquareStrip));
    }
    None
}

/// A first grid for a picture nothing was detected in, when the user asks to animate it anyway.
pub fn default_grid(pixels: &Pixels) -> Grid {
    gaps(pixels).or_else(|| square_strip(pixels.src_w, pixels.src_h)).unwrap_or(Grid::strip(4))
}

/// The frames of `grid`, leaving out cells with nothing in them, grouped by its split.
pub fn from_grid(grid: Grid, pixels: &Pixels, guessed: Guess) -> Sheet {
    let cells = grid.cells(pixels.src_w, pixels.src_h);
    let columns = grid.columns.max(1) as usize;
    let mut frames = Vec::new();
    // (row, column) → frame index, for the cells that aren't empty.
    let mut at: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for (index, rect) in cells.into_iter().enumerate() {
        if is_empty(pixels, rect) {
            continue;
        }
        at.insert((index / columns, index % columns), frames.len());
        frames.push(Frame::cell(rect));
    }
    let mut animations = Vec::new();
    match grid.split {
        Split::Rows => {
            for row in 0..grid.rows.max(1) as usize {
                let frames: Vec<usize> = at.range((row, 0)..(row, usize::MAX)).map(|(_, &i)| i).collect();
                if !frames.is_empty() {
                    animations.push(Animation { name: format!("Row {}", row + 1), frames });
                }
            }
        }
        Split::Columns => {
            for column in 0..columns {
                let frames: Vec<usize> = at.iter().filter(|((_, c), _)| *c == column).map(|(_, &i)| i).collect();
                if !frames.is_empty() {
                    animations.push(Animation { name: format!("Column {}", column + 1), frames });
                }
            }
        }
        Split::All => {}
    }
    if animations.len() <= 1 {
        animations = vec![Animation { name: "All frames".to_owned(), frames: (0..frames.len()).collect() }];
    }
    Sheet { frames, animations, source: Source::Grid(grid, guessed) }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::data::parse_data;
    use super::*;
    use crate::test_support::TempDir;

    /// A `frames` × 1 strip of `size` × `size` frames, each a dot in the middle with clear
    /// columns between, in `rows` rows; `empty` cells (row, column) are left blank.
    fn strip(frames: u32, rows: u32, size: u32, empty: &[(u32, u32)]) -> (Vec<u8>, u32, u32) {
        let (w, h) = (frames * size, rows * size);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for row in 0..rows {
            for frame in 0..frames {
                if empty.contains(&(row, frame)) {
                    continue;
                }
                for y in size / 4..size * 3 / 4 {
                    for x in size / 4..size * 3 / 4 {
                        let (px, py) = (frame * size + x, row * size + y);
                        rgba[((py * w + px) * 4 + 3) as usize] = 255;
                    }
                }
            }
        }
        (rgba, w, h)
    }

    fn pixels(rgba: &[u8], w: u32, h: u32) -> Pixels {
        Pixels { rgba: rgba.to_vec(), w, h, src_w: w, src_h: h }
    }

    #[test]
    fn transparent_gaps_give_the_grid() {
        let (rgba, w, h) = strip(6, 3, 16, &[(2, 4), (2, 5)]);
        let grid = gaps(&pixels(&rgba, w, h)).expect("a grid");
        assert_eq!((grid.columns, grid.rows), (6, 3));
        let sheet = from_grid(grid, &pixels(&rgba, w, h), Guess::Gaps);
        assert_eq!(sheet.frames.len(), 16, "the two empty cells are left out");
        let lengths: Vec<usize> = sheet.animations.iter().map(|a| a.frames.len()).collect();
        assert_eq!(lengths, [6, 6, 4], "one animation per row");
        assert_eq!(sheet.animations[1].name, "Row 2");
    }

    #[test]
    fn a_single_picture_is_not_a_sheet() {
        let (rgba, w, h) = strip(1, 1, 64, &[]);
        assert!(gaps(&pixels(&rgba, w, h)).is_none());
        assert!(detect(Path::new("C:/none/icon.png"), &pixels(&rgba, w, h)).is_none());
    }

    #[test]
    fn names_say_how_many_frames() {
        assert_eq!(strip_count("spr_player_run_strip8"), Some(8));
        assert_eq!(strip_count("player"), None);
        // Frames that touch each other have no gaps; the name decides.
        let rgba = vec![255u8; (96 * 16 * 4) as usize];
        let sheet = detect(Path::new("C:/none/hero_strip6.png"), &pixels(&rgba, 96, 16)).expect("a sheet");
        assert_eq!(sheet.frames.len(), 6);
        assert!(sheet.is_certain());
        let walk = detect(Path::new("C:/none/hero_walk.png"), &pixels(&rgba, 96, 16)).expect("a square strip");
        assert_eq!(walk.source, Source::Grid(Grid::strip(6), Guess::SquareStrip));
        assert!(!walk.is_certain());
    }

    #[test]
    fn aseprite_data_with_tags() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{ "frames": {
                  "bug 0.ase": { "frame": { "x": 0, "y": 0, "w": 70, "h": 41 }, "duration": 50 },
                  "bug 1.ase": { "frame": { "x": 70, "y": 0, "w": 70, "h": 41 }, "duration": 50 },
                  "bug 2.ase": { "frame": { "x": 140, "y": 0, "w": 70, "h": 41 }, "duration": 100 },
                  "bug 10.ase": { "frame": { "x": 210, "y": 0, "w": 70, "h": 41 }, "duration": 50 }
                },
                "meta": { "frameTags": [
                  { "name": "crawl", "from": 0, "to": 2, "direction": "pingpong" },
                  { "name": "hit", "from": 3, "to": 3, "direction": "forward" } ] } }"#,
        )
        .unwrap();
        let sheet = parse_data(&json, "bug.json").expect("a sheet");
        assert_eq!(sheet.frames.len(), 4);
        assert_eq!(sheet.frames[3].rect, [210, 0, 70, 41], "frames keep the file's order");
        assert_eq!(sheet.frames[2].duration_ms, Some(100));
        assert_eq!(sheet.animations[0].name, "crawl");
        assert_eq!(sheet.animations[0].frames, [0, 1, 2, 1]);
        assert_eq!(sheet.animations[1].frames, [3]);
        assert!(sheet.is_certain());
    }

    #[test]
    fn atlas_frames_are_grouped_by_name() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{ "frames": [
                  { "filename": "bat/bat-flying-2", "frame": { "x": 40, "y": 0, "w": 32, "h": 51 },
                    "spriteSourceSize": { "x": 2, "y": 3, "w": 32, "h": 51 }, "sourceSize": { "w": 36, "h": 56 } },
                  { "filename": "bat/bat-flying-1", "frame": { "x": 0, "y": 0, "w": 32, "h": 51 } },
                  { "filename": "door", "frame": { "x": 80, "y": 0, "w": 20, "h": 30 }, "rotated": true } ] }"#,
        )
        .unwrap();
        let sheet = parse_data(&json, "atlas.json").expect("a sheet");
        let bat = sheet.animations.iter().find(|a| a.name == "bat-flying").expect("the bat animation");
        assert_eq!(bat.frames, [1, 0], "in number order");
        assert_eq!(sheet.frames[0].offset, [2, 3]);
        assert_eq!(sheet.frames[0].size, [36, 56]);
        assert!(sheet.frames[2].rotated);
    }

    /// Saved in the user data file with each picture's grid.
    #[test]
    fn stored_splits_never_change() {
        let splits = [Split::Rows, Split::Columns, Split::All];
        assert_eq!(splits.map(Split::to_db), [0, 1, 2]);
        for split in splits {
            assert_eq!(Split::from_db(split.to_db()), split);
        }
    }

    #[test]
    fn settings_round_trip() {
        let dir = TempDir::new("sheets");
        let store = SheetStore::open(&dir.path().join("user.sqlite"));
        let saved = Saved {
            animate: true,
            grid: Some(Grid { columns: 8, rows: 2, margin: 1, spacing: 2, split: Split::Columns }),
            fps: Some(10.0),
        };
        store.set("f:/a/hero.png", &saved);
        assert_eq!(store.get("f:/a/hero.png"), Some(saved));
        let default_speed = Saved { animate: false, grid: None, fps: None };
        store.set("f:/a/idle.png", &default_speed);
        assert_eq!(store.get("f:/a/idle.png"), Some(default_speed));
        assert_eq!(store.get("f:/a/other.png"), None);
    }

    #[test]
    fn the_old_default_speed_becomes_the_default() {
        let dir = TempDir::new("sheets_old");
        let path = dir.path().join("user.sqlite");
        {
            // A file from before: the default speed (12) saved as a number.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE sheets(key TEXT PRIMARY KEY, animate INTEGER NOT NULL, columns INTEGER,
                     rows INTEGER, margin INTEGER, spacing INTEGER, split INTEGER, fps REAL NOT NULL) WITHOUT ROWID;
                 INSERT INTO sheets(key, animate, fps) VALUES('a', 1, 12), ('b', 1, 6);",
            )
            .unwrap();
        }
        let store = SheetStore::open(&path);
        assert_eq!(store.get("a").unwrap().fps, None, "12 was the default");
        assert_eq!(store.get("b").unwrap().fps, Some(6.0), "a speed picked by hand stays");
        // Picked by hand now, 12 stays 12: the change is made once.
        store.set("a", &Saved { animate: true, grid: None, fps: Some(12.0) });
        drop(store);
        assert_eq!(SheetStore::open(&path).get("a").unwrap().fps, Some(12.0));
    }
}
