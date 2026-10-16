//! Guessing a sheet's grid when there's no data for it: a GameMaker strip's name, transparent gaps
//! between frames, or square frames when the name says it's a sheet.

use super::{Grid, Split};
use crate::imaging::Pixels;

/// Alpha below this counts as transparent when looking for gaps between frames.
const CLEAR_ALPHA: u8 = 8;

/// Frames smaller than this along a side aren't guessed.
const MIN_FRAME: u32 = 4;

const MAX_FRAMES_PER_LINE: u32 = 128;

/// A pixel's alpha in the picture the guesses look at (as decoded, `w` × `h`).
pub(super) fn alpha(pixels: &Pixels, x: u32, y: u32) -> u8 {
    pixels.rgba.get(((y * pixels.w + x) * 4 + 3) as usize).copied().unwrap_or(0)
}

/// Whether a rectangle in the file's own pixels (`src_w` × `src_h`) has nothing visible in it.
pub(super) fn is_empty(pixels: &Pixels, rect: [u32; 4]) -> bool {
    let (sx, sy) = (pixels.w as f32 / pixels.src_w.max(1) as f32, pixels.h as f32 / pixels.src_h.max(1) as f32);
    let x0 = (rect[0] as f32 * sx) as u32;
    let y0 = (rect[1] as f32 * sy) as u32;
    let x1 = (((rect[0] + rect[2]) as f32 * sx).ceil() as u32).min(pixels.w);
    let y1 = (((rect[1] + rect[3]) as f32 * sy).ceil() as u32).min(pixels.h);
    !(y0..y1).any(|y| (x0..x1).any(|x| alpha(pixels, x, y) >= CLEAR_ALPHA))
}

/// GameMaker names strips `something_strip8`.
pub(super) fn strip_count(name: &str) -> Option<u32> {
    let at = name.rfind("_strip")?;
    let digits: String = name[at + 6..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

pub(super) fn name_says_sheet(name: &str) -> bool {
    ["sheet", "strip", "anim", "sprite", "frames", "walk", "run", "idle", "attack", "jump", "death"]
        .iter()
        .any(|word| name.contains(word))
}

/// As many square frames side by side (or stacked) as fit exactly.
pub(super) fn square_strip(w: u32, h: u32) -> Option<Grid> {
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
pub(super) fn gaps(pixels: &Pixels) -> Option<Grid> {
    // Gaps are looked for in file pixels when the picture is its own size; a shrunk copy's lines
    // are matched to the file's by scale, which is close enough for a starting point.
    let clear_columns: Vec<bool> =
        (0..pixels.w).map(|x| (0..pixels.h).all(|y| alpha(pixels, x, y) < CLEAR_ALPHA)).collect();
    let clear_rows: Vec<bool> =
        (0..pixels.h).map(|y| (0..pixels.w).all(|x| alpha(pixels, x, y) < CLEAR_ALPHA)).collect();
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
