//! Measures the parts that decide how fast the browser feels, on a real folder.
//!
//!     cargo run --release --bin bench -- <folder> [--thumbs N] [--sounds N] [--models N] [--gifs N]
//!                                                 [--threads N] [--offset K] [--only TEXT] [--sheet OUT.png]
//!
//! Uses a throwaway cache database, so each first preview pass is a true cold run and the second
//! shows what reopening the folder costs. The OS keeps recently read files in memory, so compare
//! settings with a different `--offset` each time: every offset picks a separate set of files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use asset_browser::app::grouped;
use asset_browser::asset::AssetKind;
use asset_browser::audio::{self, Source};
use asset_browser::imaging::Pixels;
use asset_browser::library::{Entry, Library, root_string};
use asset_browser::model::{self, LoadOptions, raster};
use asset_browser::thumbs::{Preview, THUMB_MAX, THUMB_TEXTURE, ThumbJob, ThumbService};
use asset_browser::work::Wake;
use asset_browser::{db, scan};

struct Options {
    thumbs: usize,
    sounds: usize,
    models: usize,
    gifs: usize,
    threads: usize,
    offset: usize,
    only: String,
    sheet: Option<PathBuf>,
}

#[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
fn main() {
    let mut args = std::env::args().skip(1);
    let mut root = None;
    let mut options = Options {
        thumbs: 2000,
        sounds: 300,
        models: 600,
        gifs: 600,
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
            "--models" => options.models = number(args.next(), "--models"),
            "--gifs" => options.gifs = number(args.next(), "--gifs"),
            "--threads" => options.threads = number(args.next(), "--threads"),
            "--offset" => options.offset = number(args.next(), "--offset"),
            "--only" => options.only = args.next().expect("--only needs text to match in paths").to_lowercase(),
            "--sheet" => options.sheet = Some(PathBuf::from(args.next().expect("--sheet needs a .png path"))),
            _ => root = Some(PathBuf::from(arg)),
        }
    }
    let Some(root) = root else {
        eprintln!(
            "usage: bench <folder> [--thumbs N] [--sounds N] [--models N] [--gifs N] [--threads N] [--offset K] [--only TEXT] \
             [--sheet OUT.png]"
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
    let conn = db::open_cache(&db_path).expect("open bench database");
    let start = Instant::now();
    db::save_index(&conn, &root_key, &result.files).expect("save index");
    println!("Save index:    {:.0} ms", ms(start.elapsed()));
    let start = Instant::now();
    let loaded = db::load_index(&conn, &root_key).expect("load index").unwrap_or_default();
    println!(
        "Load index:    {:.0} ms ({} rows)  <- what reopening a folder waits for",
        ms(start.elapsed()),
        grouped(loaded.len())
    );
    let start = Instant::now();
    let library = Library::build(vec![(root.clone(), loaded)]);
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
    let models = spread(&library, AssetKind::Model, &options, options.models, 0);
    previews("Models", &library, &models, &db_path, &options);
    model_stages(&library, &models);
    gif_frames(&library, &options);

    let size: u64 = ["", "-wal", "-shm"]
        .iter()
        .filter_map(|suffix| std::fs::metadata(format!("{}{suffix}", db_path.display())).ok())
        .map(|meta| meta.len())
        .sum();
    let count = images.len() + sounds.len() + models.len();
    println!("Cache size:    {:.1} MB for {} previews", size as f64 / 1e6, grouped(count));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", db_path.display()));
    }
}

/// Loads animated GIFs' frames as the preview pane does (`animated::load_gif`): how long it takes
/// once the file is read, and the biggest sheet of frames it makes.
fn gif_frames(library: &Library, options: &Options) {
    let gifs: Vec<&Entry> = library
        .entries
        .iter()
        .filter(|entry| entry.extension().eq_ignore_ascii_case("gif") && entry.lower.contains(&options.only))
        .collect();
    let step = (gifs.len() as f64 / options.gifs.max(1) as f64).max(1.0);
    let sample: Vec<&Entry> = (0..options.gifs.min(gifs.len()))
        .map(|i| gifs[(i as f64 * step) as usize + options.offset % step as usize])
        .collect();
    if sample.is_empty() {
        return;
    }
    let mut resizer = fast_image_resize::Resizer::new();
    let (mut still, mut failed, mut times) = (0, Vec::new(), Vec::new());
    let mut biggest = (0u64, String::new());
    let start = Instant::now();
    for entry in &sample {
        let Ok(bytes) = std::fs::read(library.path_of(entry)) else {
            failed.push(entry.rel.clone());
            continue;
        };
        let loading = Instant::now();
        match asset_browser::animated::load_gif(&bytes, &mut resizer, &|| false) {
            Ok(Some((pixels, sheet))) => {
                times.push((loading.elapsed(), entry.rel.clone()));
                let size = u64::from(pixels.w) * u64::from(pixels.h);
                if size > biggest.0 {
                    biggest = (size, format!("{} x {}, {} frames", pixels.w, pixels.h, sheet.frames.len()));
                }
            }
            Ok(None) => still += 1,
            Err(err) => failed.push(format!("{}: {err}", entry.rel)),
        }
    }
    times.sort();
    let median = times.get(times.len() / 2).map_or(0.0, |(time, _)| ms(*time));
    let (worst, worst_name) = times.last().map_or((0.0, ""), |(time, name)| (ms(*time), name.as_str()));
    println!(
        "GIF frames:    {} animated, {} still, {} failed in {:.1} s; median {median:.1} ms, worst {worst:.0} ms ({}); \
         biggest sheet {}",
        grouped(times.len()),
        grouped(still),
        failed.len(),
        start.elapsed().as_secs_f64(),
        worst_name.rsplit('/').next().unwrap_or(worst_name),
        biggest.1,
    );
    for failure in failed.iter().take(5) {
        println!("    failed: {failure}");
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
#[expect(clippy::cognitive_complexity, clippy::too_many_lines, reason = "refactor: split into named steps")]
fn previews(label: &str, library: &Library, sample: &[&Entry], db_path: &Path, options: &Options) {
    if sample.is_empty() {
        return;
    }
    let models = label == "Models";
    let names: HashMap<i64, (&str, u64)> =
        sample.iter().map(|entry| (entry.key, (entry.rel.as_str(), entry.size))).collect();
    let order: HashMap<i64, usize> = sample.iter().enumerate().map(|(i, entry)| (entry.key, i)).collect();
    let nothing: Wake = Arc::new(|| {});
    for pass in ["cold", "warm"] {
        let service = ThumbService::start_with_threads(db_path, options.threads, &nothing);
        let jobs = sample
            .iter()
            .map(|entry| ThumbJob { key: entry.key, path: library.path_of(entry), kind: entry.kind })
            .collect();
        let textures_before = model::texture_stats();
        let start = Instant::now();
        service.set_wanted(jobs);
        let mut done = 0;
        let mut failed: Vec<(i64, String)> = Vec::new();
        let mut slowest: Vec<(u64, i64)> = Vec::new();
        let mut sheet_cells: Vec<Pixels> = Vec::new();
        // Models: every result in sample order, for a sheet of them all.
        let mut model_cells: Vec<Option<Pixels>> = (0..sample.len()).map(|_| None).collect();
        // Textured, missing a texture, picture only (.blend), skeleton only, animated.
        let mut contents = [0usize; 5];
        while done < sample.len() {
            let Some(thumb) = service.recv_timeout(Duration::from_secs(120)) else {
                println!("  timed out with {} of {} done", done, sample.len());
                break;
            };
            done += 1;
            if let Ok(Preview::Model(_, info)) = &thumb.result {
                let picture_only = info.format.starts_with("Blender");
                let counts = [
                    info.textures > 0,
                    info.missing_textures > 0,
                    picture_only,
                    !info.geometry && !picture_only,
                    !info.animations.is_empty(),
                ];
                for (count, yes) in contents.iter_mut().zip(counts) {
                    *count += usize::from(yes);
                }
            }
            let sheet = pass == "cold" && options.sheet.is_some();
            match thumb.result {
                Ok(Preview::Model(pixels, _)) if sheet => model_cells[order[&thumb.key]] = Some(pixels),
                Ok(Preview::Image(pixels)) if sheet && sheet_cells.len() < 48 => sheet_cells.push(pixels),
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
        if let (Some(path), true, "cold") = (&options.sheet, models, pass) {
            write_model_sheets(path, sample, &model_cells, &failed);
        }
        if pass == "cold" {
            if models {
                let [textured, missing, pictures, skeletons, animated] = contents;
                println!(
                    "    {textured} textured, {missing} missing a texture, {pictures} .blend pictures, \
                     {skeletons} skeletons only, {animated} animated"
                );
                let textures = model::texture_stats();
                let decoded = textures.decoded - textures_before.decoded;
                let micros = textures.decode_micros - textures_before.decode_micros;
                println!(
                    "    textures: {} decoded ({:.0} ms of work, {:.1} ms each), {} reused from memory",
                    grouped(decoded as usize),
                    micros as f64 / 1000.0,
                    micros as f64 / 1000.0 / decoded.max(1) as f64,
                    grouped((textures.reused - textures_before.reused) as usize),
                );
            }
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
            for (reason, count) in reasons.iter().take(if models { 20 } else { 6 }) {
                println!("    failed x{count}: {reason}");
            }
        }
    }
}

/// Where each model's time goes, one file after another with the files already in memory: reading
/// the file (with its textures, which the cold pass left in the texture cache) and rendering.
fn model_stages(library: &Library, sample: &[&Entry]) {
    let mut load = Vec::new();
    let mut render = Vec::new();
    let mut by_format: HashMap<String, (usize, Duration, Duration)> = HashMap::new();
    for entry in sample {
        let path = library.path_of(entry);
        if entry.extension().eq_ignore_ascii_case("blend") {
            continue;
        }
        let start = Instant::now();
        let Ok(model) = model::load(&path, &LoadOptions { max_texture: THUMB_TEXTURE }) else { continue };
        let loaded = start.elapsed();
        let start = Instant::now();
        let picture = raster::render_thumbnail(&model, THUMB_MAX);
        let rendered = start.elapsed();
        std::hint::black_box(picture);
        load.push(loaded);
        render.push(rendered);
        let format = model.info.format.split_whitespace().next().unwrap_or("").to_owned();
        let stats = by_format.entry(format).or_default();
        *stats = (stats.0 + 1, stats.1 + loaded, stats.2 + rendered);
    }
    if load.is_empty() {
        return;
    }
    let summary = |times: &mut Vec<Duration>| {
        times.sort_unstable();
        let total: Duration = times.iter().sum();
        format!(
            "median {:.1} ms, mean {:.1} ms, worst {:.1} ms",
            ms(times[times.len() / 2]),
            ms(total) / times.len() as f64,
            ms(*times.last().unwrap_or(&Duration::ZERO))
        )
    };
    println!("Model stages, one thread, files in memory ({} models):", load.len());
    println!("    load:   {}", summary(&mut load));
    println!("    render: {}", summary(&mut render));
    let mut formats: Vec<_> = by_format.into_iter().collect();
    formats.sort_unstable_by_key(|(_, (count, ..))| std::cmp::Reverse(*count));
    for (format, (count, loaded, rendered)) in formats {
        println!(
            "    {format:<8} x{count:<4} load {:.1} ms, render {:.1} ms on average",
            ms(loaded) / count as f64,
            ms(rendered) / count as f64
        );
    }
}

/// Every model's thumbnail in sample order, 48 to a page (`<sheet>-models-<page>.png`), failures
/// as red cells, with a list of which cell is which file (`<sheet>-models.txt`).
fn write_model_sheets(path: &Path, sample: &[&Entry], cells: &[Option<Pixels>], failed: &[(i64, String)]) {
    const PER_PAGE: usize = 48;
    let stem = path.with_extension("");
    let failures: HashMap<i64, &str> = failed.iter().map(|(key, message)| (*key, message.as_str())).collect();
    let mut legend = String::new();
    for (page, start) in (0..cells.len()).step_by(PER_PAGE).enumerate() {
        let end = (start + PER_PAGE).min(cells.len());
        let failed_cell = Pixels { w: 192, h: 192, src_w: 192, src_h: 192, rgba: [200, 40, 40, 160].repeat(192 * 192) };
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
            legend.push_str(&format!("{:02} r{} c{}  {}{failure}\n", page + 1, i / 8 + 1, i % 8 + 1, entry.rel));
        }
    }
    let legend_path = PathBuf::from(format!("{}-models.txt", stem.display()));
    std::fs::write(&legend_path, legend).expect("write contact sheet legend");
    println!("    contact sheets: {}-models-NN.png, legend {}", stem.display(), legend_path.display());
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
fn write_sheet(path: &std::path::Path, cells: &[Pixels]) {
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
