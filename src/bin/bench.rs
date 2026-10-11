//! Measures the parts that decide how fast the browser feels, on a real folder.
//!
//!     cargo run --release --bin bench -- <folder> [--thumbs N] [--threads N] [--offset K] [--only TEXT] [--sheet OUT.png]
//!
//! Uses a throwaway cache database, so the first thumbnail pass is a true cold run and the second
//! shows what reopening the folder costs. The OS keeps recently read files in memory, so compare
//! settings with a different `--offset` each time: every offset picks a separate set of images.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asset_browser::app::grouped;
use asset_browser::asset::AssetKind;
use asset_browser::library::{Library, root_string};
use asset_browser::thumbs::{ThumbJob, ThumbService};
use asset_browser::{db, scan};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut root = None;
    let mut thumb_count = 2000usize;
    let mut offset = 0usize;
    let mut threads = asset_browser::thumbs::default_threads();
    let mut only = String::new();
    let mut sheet: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--thumbs" => thumb_count = args.next().and_then(|n| n.parse().ok()).expect("--thumbs needs a number"),
            "--threads" => threads = args.next().and_then(|n| n.parse().ok()).expect("--threads needs a number"),
            "--only" => only = args.next().expect("--only needs text to match in paths").to_lowercase(),
            "--sheet" => sheet = Some(PathBuf::from(args.next().expect("--sheet needs a .png path"))),
            "--offset" => offset = args.next().and_then(|n| n.parse().ok()).expect("--offset needs a number"),
            _ => root = Some(PathBuf::from(arg)),
        }
    }
    let Some(root) = root else {
        eprintln!("usage: bench <folder> [--thumbs N] [--threads N] [--offset K] [--only TEXT] [--sheet OUT.png]");
        std::process::exit(2);
    };
    let root = std::path::absolute(&root).unwrap_or(root);
    let root_key = root_string(&root);

    println!("Folder: {}", root.display());
    let result = scan::scan(&root);
    let mut by_kind = [0usize; 3];
    for file in &result.files {
        by_kind[file.kind.index()] += 1;
    }
    println!(
        "Scan:          {} assets ({} images, {} sounds, {} models) in {} folders, {} other files skipped: {:.0} ms",
        grouped(result.files.len()),
        grouped(by_kind[0]),
        grouped(by_kind[1]),
        grouped(by_kind[2]),
        grouped(result.dirs_visited),
        grouped(result.skipped_files),
        ms(result.elapsed),
    );

    let db_path = std::env::temp_dir().join(format!("asset_browser_bench_{}.sqlite", std::process::id()));
    let mut conn = db::open(&db_path).expect("open bench database");
    let start = Instant::now();
    db::save_index(&mut conn, &root_key, &result.files).expect("save index");
    println!("Save index:    {:.0} ms", ms(start.elapsed()));
    let start = Instant::now();
    let loaded = db::load_index(&conn, &root_key).expect("load index").unwrap_or_default();
    println!(
        "Load index:    {:.0} ms ({} rows)  <- what reopening a folder waits for",
        ms(start.elapsed()),
        grouped(loaded.len())
    );
    let start = Instant::now();
    let library = Library::build(&root, loaded);
    println!("Build library: {:.0} ms ({} folders with assets)", ms(start.elapsed()), grouped(library.dirs.len()));
    let start = Instant::now();
    let shown = library.filter(0, [true; 3], "png");
    println!("Search 'png':  {:.1} ms ({} matches)", start.elapsed().as_secs_f64() * 1000.0, grouped(shown.len()));
    drop(conn);

    // An even spread across the whole library, not just the first folder.
    let images: Vec<_> =
        library.entries.iter().filter(|entry| entry.kind == AssetKind::Image && entry.lower.contains(&only)).collect();
    let step = (images.len() / thumb_count.max(1)).max(1);
    let sample: Vec<_> = images.into_iter().skip(offset % step).step_by(step).take(thumb_count).collect();
    let names: HashMap<i64, (&str, u64)> =
        sample.iter().map(|entry| (entry.key, (entry.rel.as_str(), entry.size))).collect();

    for pass in ["cold", "warm"] {
        let service = ThumbService::start_with_threads(db_path.clone(), threads, Arc::new(|| {}));
        let jobs = sample.iter().map(|entry| ThumbJob { key: entry.key, path: library.path_of(entry) }).collect();
        let start = Instant::now();
        service.set_wanted(jobs);
        let mut done = 0;
        let mut failed: Vec<(i64, String)> = Vec::new();
        let mut slowest: Vec<(u64, i64)> = Vec::new();
        let mut sheet_cells = Vec::new();
        while done < sample.len() {
            let Some(thumb) = service.recv_timeout(Duration::from_secs(120)) else {
                println!("  timed out with {} of {} done", done, sample.len());
                break;
            };
            done += 1;
            match thumb.result {
                Ok(pixels) if pass == "cold" && sheet.is_some() && sheet_cells.len() < 48 => sheet_cells.push(pixels),
                Ok(_) => {}
                Err(message) => failed.push((thumb.key, message)),
            }
            slowest.push((thumb.micros, thumb.key));
        }
        let elapsed = start.elapsed();
        let threads = service.thread_count();
        drop(service); // waits for the cache writer to commit
        println!(
            "Thumbnails ({pass}): {} in {:.2} s = {:.0}/s on {threads} threads, {} failed",
            grouped(done),
            elapsed.as_secs_f64(),
            done as f64 / elapsed.as_secs_f64(),
            failed.len(),
        );
        if let (Some(path), false) = (&sheet, sheet_cells.is_empty()) {
            write_sheet(path, &sheet_cells);
            println!("    contact sheet: {}", path.display());
        }
        if pass == "cold" {
            slowest.sort_unstable_by(|a, b| b.cmp(a));
            for (micros, key) in slowest.iter().take(5) {
                let (rel, size) = names[key];
                println!("    slowest: {:>7.0} ms  {:>9} KB  {rel}", *micros as f64 / 1000.0, size / 1024);
            }
            let mut reasons: HashMap<String, usize> = HashMap::new();
            for (key, message) in &failed {
                let ext = names[key].0.rsplit('.').next().unwrap_or("").to_lowercase();
                *reasons.entry(format!(".{ext}: {}", message.chars().take(70).collect::<String>())).or_default() += 1;
            }
            let mut reasons: Vec<_> = reasons.into_iter().collect();
            reasons.sort_unstable_by_key(|(_, count)| std::cmp::Reverse(*count));
            for (reason, count) in reasons.iter().take(6) {
                println!("    failed x{count}: {reason}");
            }
        }
    }
    let size: u64 = ["", "-wal", "-shm"]
        .iter()
        .filter_map(|suffix| std::fs::metadata(format!("{}{suffix}", db_path.display())).ok())
        .map(|meta| meta.len())
        .sum();
    println!("Cache size:    {:.1} MB for {} thumbnails", size as f64 / 1e6, grouped(sample.len()));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", db_path.display()));
    }
}

/// Lays thumbnails out on a checkerboard, 8 to a row, to check them by eye.
fn write_sheet(path: &std::path::Path, cells: &[asset_browser::thumbs::ThumbPixels]) {
    const CELL: u32 = 200;
    let columns = 8u32;
    let rows = (cells.len() as u32).div_ceil(columns);
    let mut sheet = image::RgbaImage::from_fn(columns * CELL, rows * CELL, |x, y| {
        let light = ((x / 8) + (y / 8)) % 2 == 0;
        let v = if light { 200 } else { 160 };
        image::Rgba([v, v, v, 255])
    });
    for (i, cell) in cells.iter().enumerate() {
        let (ox, oy) = ((i as u32 % columns) * CELL + 4, (i as u32 / columns) * CELL + 4);
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

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
