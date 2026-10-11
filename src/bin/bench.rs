//! Measures the parts that decide how fast the browser feels, on a real folder.
//!
//!     cargo run --release --bin bench -- <folder> [--thumbs N] [--sounds N] [--threads N] [--offset K]
//!                                                 [--only TEXT] [--sheet OUT.png]
//!
//! Uses a throwaway cache database, so each first preview pass is a true cold run and the second
//! shows what reopening the folder costs. The OS keeps recently read files in memory, so compare
//! settings with a different `--offset` each time: every offset picks a separate set of files.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asset_browser::app::grouped;
use asset_browser::asset::AssetKind;
use asset_browser::audio::{self, Source};
use asset_browser::library::{Entry, Library, root_string};
use asset_browser::thumbs::{Preview, ThumbJob, ThumbPixels, ThumbService};
use asset_browser::{db, scan};

struct Options {
    thumbs: usize,
    sounds: usize,
    threads: usize,
    offset: usize,
    only: String,
    sheet: Option<PathBuf>,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut root = None;
    let mut options = Options {
        thumbs: 2000,
        sounds: 300,
        threads: asset_browser::thumbs::default_threads(),
        offset: 0,
        only: String::new(),
        sheet: None,
    };
    let number = |value: Option<String>, flag: &str| -> usize {
        value.and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("{flag} needs a number"))
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--thumbs" => options.thumbs = number(args.next(), "--thumbs"),
            "--sounds" => options.sounds = number(args.next(), "--sounds"),
            "--threads" => options.threads = number(args.next(), "--threads"),
            "--offset" => options.offset = number(args.next(), "--offset"),
            "--only" => options.only = args.next().expect("--only needs text to match in paths").to_lowercase(),
            "--sheet" => options.sheet = Some(PathBuf::from(args.next().expect("--sheet needs a .png path"))),
            _ => root = Some(PathBuf::from(arg)),
        }
    }
    let Some(root) = root else {
        eprintln!(
            "usage: bench <folder> [--thumbs N] [--sounds N] [--threads N] [--offset K] [--only TEXT] [--sheet OUT.png]"
        );
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

    let images = spread(&library, AssetKind::Image, &options, options.thumbs, 0);
    previews("Thumbnails", &library, &images, &db_path, &options);
    let sounds = spread(&library, AssetKind::Audio, &options, options.sounds, 0);
    previews("Waveforms", &library, &sounds, &db_path, &options);
    if options.sounds > 0 {
        start_latency(&library, &spread(&library, AssetKind::Audio, &options, 20, 1));
    }

    let size: u64 = ["", "-wal", "-shm"]
        .iter()
        .filter_map(|suffix| std::fs::metadata(format!("{}{suffix}", db_path.display())).ok())
        .map(|meta| meta.len())
        .sum();
    println!("Cache size:    {:.1} MB for {} previews", size as f64 / 1e6, grouped(images.len() + sounds.len()));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", db_path.display()));
    }
}

/// An even spread of `count` entries of `kind` across the whole library, not just the first
/// folder. `shift` picks a different set from the same spread.
fn spread<'a>(library: &'a Library, kind: AssetKind, options: &Options, count: usize, shift: usize) -> Vec<&'a Entry> {
    if count == 0 {
        return Vec::new();
    }
    let all: Vec<_> =
        library.entries.iter().filter(|entry| entry.kind == kind && entry.lower.contains(&options.only)).collect();
    let step = (all.len() / count).max(1);
    let skip = (options.offset + shift * step.div_ceil(2)) % step;
    all.into_iter().skip(skip).step_by(step).take(count).collect()
}

/// Makes previews for `sample` twice: into an empty cache, then from it.
fn previews(label: &str, library: &Library, sample: &[&Entry], db_path: &std::path::Path, options: &Options) {
    if sample.is_empty() {
        return;
    }
    let names: HashMap<i64, (&str, u64)> =
        sample.iter().map(|entry| (entry.key, (entry.rel.as_str(), entry.size))).collect();
    for pass in ["cold", "warm"] {
        let service = ThumbService::start_with_threads(db_path.to_path_buf(), options.threads, Arc::new(|| {}));
        let jobs = sample
            .iter()
            .map(|entry| ThumbJob { key: entry.key, path: library.path_of(entry), kind: entry.kind })
            .collect();
        let start = Instant::now();
        service.set_wanted(jobs);
        let mut done = 0;
        let mut failed: Vec<(i64, String)> = Vec::new();
        let mut slowest: Vec<(u64, i64)> = Vec::new();
        let mut sheet_cells: Vec<ThumbPixels> = Vec::new();
        while done < sample.len() {
            let Some(thumb) = service.recv_timeout(Duration::from_secs(120)) else {
                println!("  timed out with {} of {} done", done, sample.len());
                break;
            };
            done += 1;
            match thumb.result {
                Ok(Preview::Image(pixels)) if pass == "cold" && options.sheet.is_some() && sheet_cells.len() < 48 => {
                    sheet_cells.push(pixels);
                }
                Ok(_) => {}
                Err(message) => failed.push((thumb.key, message)),
            }
            slowest.push((thumb.micros, thumb.key));
        }
        let elapsed = start.elapsed();
        let threads = service.thread_count();
        drop(service); // waits for the cache writer to commit
        println!(
            "{label} ({pass}): {} in {:.2} s = {:.0}/s on {threads} threads, {} failed",
            grouped(done),
            elapsed.as_secs_f64(),
            done as f64 / elapsed.as_secs_f64(),
            failed.len(),
        );
        if let (Some(path), false) = (&options.sheet, sheet_cells.is_empty()) {
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
}

/// What playback waits for before the first audio: opening the file and decoding its first
/// packet, straight from disk versus from bytes already read ahead into memory.
fn start_latency(library: &Library, sample: &[&Entry]) {
    let mut from_disk = Vec::new();
    let mut from_memory = Vec::new();
    for entry in sample {
        let path = library.path_of(entry);
        let extension = entry.extension();
        if let Ok(time) = audio::time_to_first_audio(Source::File(path.clone()), extension) {
            from_disk.push(time);
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        if let Ok(time) = audio::time_to_first_audio(Source::Memory(bytes.into()), extension) {
            from_memory.push(time);
        }
    }
    let summary = |times: &mut Vec<Duration>| -> String {
        if times.is_empty() {
            return "no playable files".into();
        }
        times.sort_unstable();
        format!("median {:.1} ms, worst {:.1} ms", ms(times[times.len() / 2]), ms(*times.last().unwrap()))
    };
    println!(
        "Sound start ({} files not read before): from disk {}; read ahead into memory {}",
        sample.len(),
        summary(&mut from_disk),
        summary(&mut from_memory),
    );
}

/// Lays thumbnails out on a checkerboard, 8 to a row, to check them by eye.
fn write_sheet(path: &std::path::Path, cells: &[ThumbPixels]) {
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
