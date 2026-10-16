//! Contact sheets: the thumbnails made, laid out to check by eye.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use asset_browser::imaging::Pixels;
use asset_browser::library::Entry;
use asset_browser::thumbs::THUMB_MAX;

use super::{COLUMNS, PER_PAGE};

/// Every model's thumbnail in sample order, a page at a time (`<sheet>-models-<page>.png`),
/// failures as red cells, with a list of which cell is which file (`<sheet>-models.txt`).
pub(super) fn write_model_sheets(path: &Path, sample: &[&Entry], cells: &[Option<Pixels>], failed: &[(i64, String)]) {
    let stem = path.with_extension("");
    let failures: HashMap<i64, &str> = failed.iter().map(|(key, message)| (*key, message.as_str())).collect();
    let mut legend = String::new();
    for (page, start) in (0..cells.len()).step_by(PER_PAGE).enumerate() {
        let end = (start + PER_PAGE).min(cells.len());
        let side = THUMB_MAX;
        let failed_cell = Pixels {
            w: side,
            h: side,
            src_w: side,
            src_h: side,
            rgba: [200, 40, 40, 160].repeat((side * side) as usize),
        };
        let page_cells: Vec<Pixels> = cells[start..end]
            .iter()
            .map(|cell| match cell {
                Some(cell) => Pixels { rgba: cell.rgba.clone(), ..*cell },
                None => Pixels { rgba: failed_cell.rgba.clone(), ..failed_cell },
            })
            .collect();
        let page_path = PathBuf::from(format!("{}-models-{:02}.png", stem.display(), page + 1));
        write_sheet(&page_path, &page_cells);
        for (i, entry) in sample[start..end].iter().enumerate() {
            let failure = failures.get(&entry.key).map(|message| format!("  FAILED: {message}")).unwrap_or_default();
            let (row, column) = (i / COLUMNS + 1, i % COLUMNS + 1);
            legend.push_str(&format!("{:02} r{row} c{column}  {}{failure}\n", page + 1, entry.rel));
        }
    }
    let legend_path = PathBuf::from(format!("{}-models.txt", stem.display()));
    std::fs::write(&legend_path, legend).expect("write contact sheet legend");
    println!("    contact sheets: {}-models-NN.png, legend {}", stem.display(), legend_path.display());
}

/// Lays thumbnails out on a checkerboard, `COLUMNS` to a row, to check them by eye.
pub(super) fn write_sheet(path: &std::path::Path, cells: &[Pixels]) {
    let cell_side = THUMB_MAX + 8;
    let columns = COLUMNS as u32;
    let rows = (cells.len() as u32).div_ceil(columns);
    let mut sheet = image::RgbaImage::from_fn(columns * cell_side, rows * cell_side, |x, y| {
        let light = ((x / 8) + (y / 8)) % 2 == 0;
        let v = if light { 200 } else { 160 };
        image::Rgba([v, v, v, 255])
    });
    for (i, cell) in cells.iter().enumerate() {
        let (ox, oy) = ((i as u32 % columns) * cell_side + 4, (i as u32 / columns) * cell_side + 4);
        for y in 0..cell.h {
            for x in 0..cell.w {
                let s = &cell.rgba[((y * cell.w + x) * 4) as usize..][..4];
                let d = sheet.get_pixel_mut(ox + x, oy + y);
                let a = s[3] as u32;
                for (dst, &src) in d.0.iter_mut().zip(s).take(3) {
                    *dst = ((src as u32 * a + *dst as u32 * (255 - a)) / 255) as u8;
                }
            }
        }
    }
    sheet.save(path).expect("write contact sheet");
}
