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

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::diag;

/// Alpha below this counts as transparent when looking for gaps between frames.
const CLEAR_ALPHA: u8 = 8;
/// Frames smaller than this along a side aren't guessed.
const MIN_FRAME: u32 = 4;
const MAX_FRAMES_PER_LINE: u32 = 128;

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

/// The picture the guesses look at: as decoded (`w` × `h`, straight RGBA), and the file's own size.
pub struct Pixels<'a> {
    pub rgba: &'a [u8],
    pub w: u32,
    pub h: u32,
    pub src_w: u32,
    pub src_h: u32,
}

impl Pixels<'_> {
    fn alpha(&self, x: u32, y: u32) -> u8 {
        self.rgba.get(((y * self.w + x) * 4 + 3) as usize).copied().unwrap_or(0)
    }

    /// Whether a rectangle in file pixels has nothing visible in it.
    fn is_empty(&self, rect: [u32; 4]) -> bool {
        let (sx, sy) = (self.w as f32 / self.src_w.max(1) as f32, self.h as f32 / self.src_h.max(1) as f32);
        let x0 = (rect[0] as f32 * sx) as u32;
        let y0 = (rect[1] as f32 * sy) as u32;
        let x1 = (((rect[0] + rect[2]) as f32 * sx).ceil() as u32).min(self.w);
        let y1 = (((rect[1] + rect[3]) as f32 * sy).ceil() as u32).min(self.h);
        !(y0..y1).any(|y| (x0..x1).any(|x| self.alpha(x, y) >= CLEAR_ALPHA))
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
        if pixels.is_empty(rect) {
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

/// GameMaker names strips `something_strip8`.
fn strip_count(name: &str) -> Option<u32> {
    let at = name.rfind("_strip")?;
    let digits: String = name[at + 6..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn name_says_sheet(name: &str) -> bool {
    ["sheet", "strip", "anim", "sprite", "frames", "walk", "run", "idle", "attack", "jump", "death"]
        .iter()
        .any(|word| name.contains(word))
}

/// As many square frames side by side (or stacked) as fit exactly.
fn square_strip(w: u32, h: u32) -> Option<Grid> {
    if h >= MIN_FRAME && w > h && w.is_multiple_of(h) && w / h <= MAX_FRAMES_PER_LINE {
        return Some(Grid::strip(w / h));
    }
    if w >= MIN_FRAME && h > w && h.is_multiple_of(w) && h / w <= MAX_FRAMES_PER_LINE {
        return Some(Grid { columns: 1, rows: h / w, margin: 0, spacing: 0, split: Split::All });
    }
    None
}

/// Frames separated by fully transparent lines: the finest even division along each side whose
/// every cut is clear and every cell has something in it.
fn gaps(pixels: &Pixels) -> Option<Grid> {
    // Gaps are looked for in file pixels when the picture is its own size; a shrunk copy's lines
    // are matched to the file's by scale, which is close enough for a starting point.
    let clear_columns: Vec<bool> =
        (0..pixels.w).map(|x| (0..pixels.h).all(|y| pixels.alpha(x, y) < CLEAR_ALPHA)).collect();
    let clear_rows: Vec<bool> =
        (0..pixels.h).map(|y| (0..pixels.w).all(|x| pixels.alpha(x, y) < CLEAR_ALPHA)).collect();
    let columns = divide(&clear_columns);
    let rows = divide(&clear_rows);
    if columns.is_none() && rows.is_none() {
        return None;
    }
    let (columns, column_margin, column_spacing) = columns.unwrap_or((1, 0, 0));
    let (rows, row_margin, row_spacing) = rows.unwrap_or((1, 0, 0));
    let scale = pixels.src_w as f32 / pixels.w.max(1) as f32;
    let margin = (column_margin.max(row_margin) as f32 * scale).round() as u32;
    let spacing = (column_spacing.max(row_spacing) as f32 * scale).round() as u32;
    Some(Grid { columns, rows, margin, spacing, split: if rows > 1 { Split::Rows } else { Split::All } })
}

/// (count, margin, spacing) of the finest division of a line of `clear` flags into equal cells.
fn divide(clear: &[bool]) -> Option<(u32, u32, u32)> {
    let n = clear.len() as u32;
    let mut best: Option<(u32, u32, u32)> = None;
    for margin in 0..=2u32 {
        if (0..margin).any(|i| !clear.get(i as usize).copied().unwrap_or(false))
            || (n.saturating_sub(margin)..n).any(|i| !clear[i as usize])
        {
            continue;
        }
        for spacing in 0..=2u32 {
            for count in 2..=MAX_FRAMES_PER_LINE {
                let Some(inner) = n.checked_sub(2 * margin + (count - 1) * spacing) else { break };
                let size = inner / count;
                if size < MIN_FRAME {
                    break;
                }
                if !inner.is_multiple_of(count) {
                    continue;
                }
                let start = |k: u32| margin + k * (size + spacing);
                let cuts_clear = (1..count).all(|k| {
                    let cut = start(k);
                    if spacing > 0 {
                        (cut - spacing..cut).all(|i| clear[i as usize])
                    } else {
                        clear[cut as usize - 1] || clear[cut as usize]
                    }
                });
                let cells_full = (0..count).all(|k| (start(k)..start(k) + size).any(|i| !clear[i as usize]));
                if cuts_clear && cells_full && best.is_none_or(|(c, _, _)| count > c) {
                    best = Some((count, margin, spacing));
                }
            }
        }
    }
    best
}

/// Reads `name.json` (or `name.png.json`) beside the picture, if it's sprite sheet data.
fn from_data_file(path: &Path) -> Option<Sheet> {
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    let file_name = path.file_name()?.to_string_lossy().into_owned();
    let dir = path.parent()?;
    let candidates = [dir.join(format!("{stem}.json")), dir.join(format!("{file_name}.json"))];
    let (data_path, text) = candidates
        .iter()
        .find_map(|candidate| std::fs::read_to_string(candidate).ok().map(|text| (candidate, text)))?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let data_name = data_path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    parse_data(&json, &data_name)
}

/// Aseprite / TexturePacker JSON: `frames` as an array or an object of named frames, and
/// optional `meta.frameTags`.
fn parse_data(json: &serde_json::Value, data_name: &str) -> Option<Sheet> {
    let entries: Vec<(String, &serde_json::Value)> = match json.get("frames")? {
        serde_json::Value::Array(list) => list
            .iter()
            .map(|frame| (frame.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_owned(), frame))
            .collect(),
        serde_json::Value::Object(map) => map.iter().map(|(name, frame)| (name.clone(), frame)).collect(),
        _ => return None,
    };
    let number = |value: Option<&serde_json::Value>, key: &str| -> Option<u32> {
        value?.get(key)?.as_f64().map(|n| n.max(0.0).round() as u32)
    };
    let mut frames = Vec::new();
    let mut names = Vec::new();
    for (name, frame) in entries {
        let rect = frame.get("frame");
        let [x, y, w, h] = ["x", "y", "w", "h"].map(|key| number(rect, key));
        let (Some(x), Some(y), Some(w), Some(h)) = (x, y, w, h) else { continue };
        if w == 0 || h == 0 {
            continue;
        }
        let trimmed = frame.get("spriteSourceSize");
        let source = frame.get("sourceSize");
        let offset = [number(trimmed, "x").unwrap_or(0), number(trimmed, "y").unwrap_or(0)];
        let size = [number(source, "w").unwrap_or(w).max(w), number(source, "h").unwrap_or(h).max(h)];
        let rotated = frame.get("rotated").and_then(|v| v.as_bool()).unwrap_or(false);
        let duration_ms = frame.get("duration").and_then(|v| v.as_f64()).map(|d| d.max(1.0).round() as u32);
        frames.push(Frame { rect: [x, y, w, h], rotated, offset, size, duration_ms });
        names.push(name);
    }
    if frames.is_empty() {
        return None;
    }

    let mut animations = Vec::new();
    if let Some(tags) = json.get("meta").and_then(|meta| meta.get("frameTags")).and_then(|tags| tags.as_array()) {
        for tag in tags {
            let name = tag.get("name").and_then(|v| v.as_str()).unwrap_or("").to_owned();
            let (Some(from), Some(to)) = (number(Some(tag), "from"), number(Some(tag), "to")) else { continue };
            let last = frames.len() as u32 - 1;
            let (from, to) = (from.min(last) as usize, to.min(last) as usize);
            let forward: Vec<usize> = (from.min(to)..=from.max(to)).collect();
            let order = match tag.get("direction").and_then(|v| v.as_str()).unwrap_or("forward") {
                "reverse" => forward.into_iter().rev().collect(),
                "pingpong" if forward.len() > 2 => {
                    let back: Vec<usize> = forward[1..forward.len() - 1].iter().rev().copied().collect();
                    forward.into_iter().chain(back).collect()
                }
                _ => forward,
            };
            animations.push(Animation { name, frames: order });
        }
    }
    if animations.is_empty() {
        animations = animations_by_name(&names);
    }
    if animations.is_empty() {
        animations = vec![Animation { name: "All frames".to_owned(), frames: (0..frames.len()).collect() }];
    }
    Some(Sheet { frames, animations, source: Source::Data(data_name.to_owned()) })
}

/// Atlas frames named `run-1`, `run-2`, `bat/fly_01`…: grouped by the name without its
/// number, in number order. Only worth it when some name has more than one frame.
fn animations_by_name(names: &[String]) -> Vec<Animation> {
    let mut groups: BTreeMap<String, Vec<(u64, usize)>> = BTreeMap::new();
    for (index, name) in names.iter().enumerate() {
        let base = name.rsplit_once('.').filter(|(_, ext)| ext.len() <= 4).map_or(name.as_str(), |(base, _)| base);
        let digits = base.chars().rev().take_while(char::is_ascii_digit).count();
        let (stem, number) = base.split_at(base.len() - digits);
        let stem = stem.trim_end_matches(['-', '_', ' ', '.']);
        groups.entry(stem.to_owned()).or_default().push((number.parse().unwrap_or(0), index));
    }
    if !groups.values().any(|frames| frames.len() > 1) {
        return Vec::new();
    }
    groups
        .into_iter()
        .map(|(stem, mut frames)| {
            frames.sort_unstable();
            let name = stem.rsplit('/').next().unwrap_or(&stem).to_owned();
            let name = if name.is_empty() { "Frames".to_owned() } else { name };
            Animation { name, frames: frames.into_iter().map(|(_, index)| index).collect() }
        })
        .collect()
}

/// What the user chose for one picture: to see it as an animation, their grid, and the speed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Saved {
    pub animate: bool,
    pub grid: Option<Grid>,
    /// The speed the user picked; `None` plays at the default speed, whatever that is now.
    pub fps: Option<f32>,
}

/// Per-picture sheet choices, in the user data file beside the favourites and collections.
pub struct SheetStore {
    conn: Option<Connection>,
}

impl SheetStore {
    pub fn open(path: &Path) -> Self {
        let opened = (|| -> rusqlite::Result<Connection> {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let conn = Connection::open(path)?;
            conn.busy_timeout(std::time::Duration::from_secs(5))?;
            conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))?;
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS sheets(
                     key TEXT PRIMARY KEY,
                     animate INTEGER NOT NULL,
                     columns INTEGER,
                     rows INTEGER,
                     margin INTEGER,
                     spacing INTEGER,
                     split INTEGER,
                     fps REAL NOT NULL) WITHOUT ROWID;",
            )?;
            // Version 1 of the user data file stores the default speed as 0, so changing the
            // default reaches every picture. Before, the default (12 fps) was saved as a number:
            // those become the default.
            let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
            if version < 1 {
                let tx = conn.unchecked_transaction()?;
                tx.execute("UPDATE sheets SET fps = 0 WHERE fps = 12", [])?;
                tx.pragma_update(None, "user_version", 1)?;
                tx.commit()?;
            }
            Ok(conn)
        })();
        match opened {
            Ok(conn) => Self { conn: Some(conn) },
            Err(err) => {
                diag::report("Sprite sheet settings unavailable", err);
                Self { conn: None }
            }
        }
    }

    pub fn get(&self, key: &str) -> Option<Saved> {
        let conn = self.conn.as_ref()?;
        conn.query_row(
            "SELECT animate, columns, rows, margin, spacing, split, fps FROM sheets WHERE key = ?1",
            [key],
            |row| {
                let columns: Option<u32> = row.get(1)?;
                let grid = match columns {
                    Some(columns) => Some(Grid {
                        columns,
                        rows: row.get::<_, Option<u32>>(2)?.unwrap_or(1),
                        margin: row.get::<_, Option<u32>>(3)?.unwrap_or(0),
                        spacing: row.get::<_, Option<u32>>(4)?.unwrap_or(0),
                        split: Split::from_db(row.get::<_, Option<i64>>(5)?.unwrap_or(0)),
                    }),
                    None => None,
                };
                let fps: f64 = row.get(6)?;
                Ok(Saved { animate: row.get::<_, i64>(0)? != 0, grid, fps: (fps > 0.0).then_some(fps as f32) })
            },
        )
        .optional()
        .map_err(|err| diag::report("Reading sprite sheet settings", err))
        .ok()
        .flatten()
    }

    /// Forgets every picture's choices (Settings → Storage).
    pub fn clear(&self) {
        if let Some(conn) = &self.conn
            && let Err(err) = conn.execute("DELETE FROM sheets", [])
        {
            diag::report("Clearing sprite sheet choices", err);
        }
    }

    pub fn set(&self, key: &str, saved: &Saved) {
        let Some(conn) = &self.conn else { return };
        let grid = saved.grid;
        let result = conn.execute(
            "INSERT OR REPLACE INTO sheets(key, animate, columns, rows, margin, spacing, split, fps)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                key,
                saved.animate,
                grid.map(|g| g.columns),
                grid.map(|g| g.rows),
                grid.map(|g| g.margin),
                grid.map(|g| g.spacing),
                grid.map(|g| g.split.to_db()),
                saved.fps.unwrap_or(0.0),
            ],
        );
        if let Err(err) = result {
            diag::report("Saving sprite sheet settings", err);
        }
    }
}

#[cfg(test)]
mod tests {
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

    fn pixels(rgba: &[u8], w: u32, h: u32) -> Pixels<'_> {
        Pixels { rgba, w, h, src_w: w, src_h: h }
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
