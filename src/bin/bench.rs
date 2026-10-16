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
use asset_browser::asset::{AssetKind, KindSet};
use asset_browser::audio::{self, Source};
use asset_browser::imaging::Pixels;
use asset_browser::library::{Entry, Library, root_string};
use asset_browser::model::{self, LoadOptions, ModelInfo, TextureStats, raster};
use asset_browser::thumbs::{Preview, THUMB_MAX, THUMB_TEXTURE, Thumb, ThumbJob, ThumbService};
use asset_browser::work::Wake;
use asset_browser::{db, folder_index, scan};

/// Contact sheets: pictures to a row, and to a page.
const COLUMNS: usize = 8;
const PER_PAGE: usize = 48;

/// The cache database's files: the database, and its write-ahead log and index.
const CACHE_FILES: [&str; 3] = ["", "-wal", "-shm"];

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

impl Options {
    /// The folder and options from the command line; the usage, and exit, without a folder.
    fn from_args() -> (PathBuf, Self) {
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
        (std::path::absolute(&root).unwrap_or(root), options)
    }
}

fn main() {
    let (root, options) = Options::from_args();
    println!("Folder: {}", root.display());
    let db_path = std::env::temp_dir().join(format!("asset_browser_bench_{}.sqlite", std::process::id()));
    let library = index(&root, &db_path);

    let images = spread(&library, AssetKind::Image, &options, options.thumbs, 0);
    previews(AssetKind::Image, &library, &images, &db_path, &options);
    let sounds = spread(&library, AssetKind::Audio, &options, options.sounds, 0);
    previews(AssetKind::Audio, &library, &sounds, &db_path, &options);
    if options.sounds > 0 {
        start_latency(&library, &spread(&library, AssetKind::Audio, &options, 20, 1));
    }
    let models = spread(&library, AssetKind::Model, &options, options.models, 0);
    previews(AssetKind::Model, &library, &models, &db_path, &options);
    model_stages(&library, &models);
    gif_frames(&library, &options);

    let size: u64 = cache_files(&db_path).filter_map(|file| std::fs::metadata(file).ok()).map(|meta| meta.len()).sum();
    let count = images.len() + sounds.len() + models.len();
    println!("Cache size:    {:.1} MB for {} previews", size as f64 / 1e6, grouped(count));
    for file in cache_files(&db_path) {
        // Only there if the run got that far; a throwaway file left in the temp folder otherwise.
        let _ = std::fs::remove_file(file);
    }
}

fn cache_files(db_path: &Path) -> impl Iterator<Item = String> + '_ {
    CACHE_FILES.iter().map(move |suffix| format!("{}{suffix}", db_path.display()))
}

/// Scans `root` into the folder index at `db_path`, reads it back and builds the library, and
/// searches it, saying how long each takes.
fn index(root: &Path, db_path: &Path) -> Library {
    let root_key = root_string(root);
    let result = scan::scan(root);
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

    let conn = db::open_cache(db_path).expect("open bench database");
    let start = Instant::now();
    folder_index::save(&conn, &root_key, &result.files).expect("save index");
    println!("Save index:    {:.0} ms", ms(start.elapsed()));
    let start = Instant::now();
    let loaded = folder_index::load(&conn, &root_key).expect("load index").unwrap_or_default();
    println!(
        "Load index:    {:.0} ms ({} rows)  <- what reopening a folder waits for",
        ms(start.elapsed()),
        grouped(loaded.len())
    );
    let start = Instant::now();
    let library = Library::build(vec![(root.to_path_buf(), loaded)]);
    println!("Build library: {:.0} ms ({} folders with assets)", ms(start.elapsed()), grouped(library.dirs().len()));
    let start = Instant::now();
    let shown = library.filter(0, KindSet::ALL, "png");
    println!("Search 'png':  {:.1} ms ({} matches)", start.elapsed().as_secs_f64() * 1000.0, grouped(shown.len()));
    library
}

/// Loads animated GIFs' frames as the preview pane does (`animated::load_gif`): how long it takes
/// once the file is read, and the biggest sheet of frames it makes.
fn gif_frames(library: &Library, options: &Options) {
    let gifs: Vec<&Entry> = library
        .entries()
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
    let timings = Timings::of(&mut times.iter().map(|(time, _)| *time).collect::<Vec<_>>());
    let (median, worst) = timings.map_or((0.0, 0.0), |timings| (timings.median, timings.worst));
    let worst_name = times.last().map_or("", |(_, name)| name.as_str());
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
        library.entries().iter().filter(|entry| entry.kind == kind && entry.lower.contains(&options.only)).collect();
    let step = (all.len() / count).max(1);
    let skip = (options.offset + shift * step.div_ceil(2)) % step;
    all.into_iter().skip(skip).step_by(step).take(count).collect()
}

/// A preview pass: into an empty cache, or from it.
#[derive(Clone, Copy, PartialEq)]
enum Pass {
    Cold,
    Warm,
}

impl Pass {
    fn name(self) -> &'static str {
        match self {
            Pass::Cold => "cold",
            Pass::Warm => "warm",
        }
    }
}

/// What the previews of a kind are called.
fn label(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Image => "Thumbnails",
        AssetKind::Audio => "Waveforms",
        AssetKind::Model => "Models",
    }
}

/// Makes previews for `sample` twice: into an empty cache, then from it.
fn previews(kind: AssetKind, library: &Library, sample: &[&Entry], db_path: &Path, options: &Options) {
    if sample.is_empty() {
        return;
    }
    for pass in [Pass::Cold, Pass::Warm] {
        let textures_before = model::texture_stats();
        let keep_pictures = pass == Pass::Cold && options.sheet.is_some();
        let results = Results::of_pass(library, sample, db_path, options.threads, keep_pictures);
        println!(
            "{} ({}): {} in {:.2} s = {:.0}/s on {} threads, {} failed",
            label(kind),
            pass.name(),
            grouped(results.done),
            results.elapsed.as_secs_f64(),
            results.done as f64 / results.elapsed.as_secs_f64(),
            results.threads,
            results.failed.len(),
        );
        if let (Some(path), false) = (&options.sheet, results.pictures.is_empty()) {
            write_sheet(path, &results.pictures);
            println!("    contact sheet: {}", path.display());
        }
        if let (Some(path), AssetKind::Model, Pass::Cold) = (&options.sheet, kind, pass) {
            write_model_sheets(path, sample, &results.models, &results.failed);
        }
        if pass == Pass::Cold {
            if kind == AssetKind::Model {
                report_models(&results.contents, textures_before);
            }
            results.report_slowest_and_failures(sample, kind);
        }
    }
}

/// What a pass over a sample gave.
struct Results {
    done: usize,
    elapsed: Duration,
    threads: usize,
    failed: Vec<(i64, String)>,
    /// Each preview's time (microseconds) and key.
    times: Vec<(u64, i64)>,
    /// The first pictures, for a contact sheet.
    pictures: Vec<Pixels>,
    /// Every model's picture in sample order, for a sheet of them all.
    models: Vec<Option<Pixels>>,
    /// Models textured, missing a texture, picture only (.blend), skeleton only, animated.
    contents: [usize; 5],
}

impl Results {
    /// Makes the sample's previews with `threads` workers into the cache at `db_path`, keeping
    /// the pictures for contact sheets when asked.
    fn of_pass(library: &Library, sample: &[&Entry], db_path: &Path, threads: usize, keep_pictures: bool) -> Self {
        let order: HashMap<i64, usize> = sample.iter().enumerate().map(|(i, entry)| (entry.key, i)).collect();
        let nothing: Wake = Arc::new(|| {});
        let service = ThumbService::start_with_threads(db_path, threads, &nothing);
        let jobs = sample
            .iter()
            .map(|entry| ThumbJob { key: entry.key, path: library.path_of(entry), kind: entry.kind })
            .collect();
        let mut results = Results {
            done: 0,
            elapsed: Duration::ZERO,
            threads: 0,
            failed: Vec::new(),
            times: Vec::new(),
            pictures: Vec::new(),
            models: (0..sample.len()).map(|_| None).collect(),
            contents: [0; 5],
        };
        let start = Instant::now();
        service.set_wanted(jobs);
        while results.done < sample.len() {
            let Some(thumb) = service.recv_timeout(Duration::from_secs(120)) else {
                println!("  timed out with {} of {} done", results.done, sample.len());
                break;
            };
            results.done += 1;
            results.take(thumb, &order, keep_pictures);
        }
        results.elapsed = start.elapsed();
        results.threads = service.thread_count();
        drop(service); // waits for the cache writer to commit
        results
    }

    fn take(&mut self, thumb: Thumb, order: &HashMap<i64, usize>, keep_pictures: bool) {
        if let Ok(Preview::Model(_, info)) = &thumb.result {
            for (count, yes) in self.contents.iter_mut().zip(contents(info)) {
                *count += usize::from(yes);
            }
        }
        match thumb.result {
            Ok(Preview::Model(pixels, _)) if keep_pictures => self.models[order[&thumb.key]] = Some(pixels),
            Ok(Preview::Image(pixels)) if keep_pictures && self.pictures.len() < PER_PAGE => self.pictures.push(pixels),
            Ok(_) => {}
            Err(message) => self.failed.push((thumb.key, message)),
        }
        self.times.push((thumb.micros, thumb.key));
    }

    /// The five slowest previews, and the commonest reasons previews failed.
    fn report_slowest_and_failures(mut self, sample: &[&Entry], kind: AssetKind) {
        let names: HashMap<i64, (&str, u64)> =
            sample.iter().map(|entry| (entry.key, (entry.rel.as_str(), entry.size))).collect();
        self.times.sort_unstable_by(|a, b| b.cmp(a));
        for (micros, key) in self.times.iter().take(5) {
            let (rel, size) = names[key];
            println!("    slowest: {:>7.0} ms  {:>9} KB  {rel}", *micros as f64 / 1000.0, size / 1024);
        }
        let mut reasons: HashMap<String, usize> = HashMap::new();
        for (key, message) in &self.failed {
            let ext = names[key].0.rsplit('.').next().unwrap_or("").to_lowercase();
            *reasons.entry(format!(".{ext}: {}", message.chars().take(70).collect::<String>())).or_default() += 1;
        }
        let mut reasons: Vec<_> = reasons.into_iter().collect();
        reasons.sort_unstable_by_key(|(_, count)| std::cmp::Reverse(*count));
        for (reason, count) in reasons.iter().take(if kind == AssetKind::Model { 20 } else { 6 }) {
            println!("    failed x{count}: {reason}");
        }
    }
}

/// Whether a model is textured, missing a texture, only a picture (.blend), only a skeleton, and
/// animated.
fn contents(info: &ModelInfo) -> [bool; 5] {
    let picture_only = info.format.starts_with("Blender");
    [
        info.textures > 0,
        info.missing_textures > 0,
        picture_only,
        !info.geometry && !picture_only,
        !info.animations.is_empty(),
    ]
}

/// What the models were, and the textures decoded for them since `before`.
fn report_models(contents: &[usize; 5], before: TextureStats) {
    let [textured, missing, pictures, skeletons, animated] = *contents;
    println!(
        "    {textured} textured, {missing} missing a texture, {pictures} .blend pictures, \
         {skeletons} skeletons only, {animated} animated"
    );
    let textures = model::texture_stats();
    let decoded = textures.decoded - before.decoded;
    let micros = textures.decode_micros - before.decode_micros;
    println!(
        "    textures: {} decoded ({:.0} ms of work, {:.1} ms each), {} reused from memory",
        grouped(decoded as usize),
        micros as f64 / 1000.0,
        micros as f64 / 1000.0 / decoded.max(1) as f64,
        grouped((textures.reused - before.reused) as usize),
    );
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
    let (Some(loading), Some(rendering)) = (Timings::of(&mut load), Timings::of(&mut render)) else { return };
    let summary = |t: Timings| format!("median {:.1} ms, mean {:.1} ms, worst {:.1} ms", t.median, t.mean, t.worst);
    println!("Model stages, one thread, files in memory ({} models):", load.len());
    println!("    load:   {}", summary(loading));
    println!("    render: {}", summary(rendering));
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

/// Every model's thumbnail in sample order, a page at a time (`<sheet>-models-<page>.png`),
/// failures as red cells, with a list of which cell is which file (`<sheet>-models.txt`).
fn write_model_sheets(path: &Path, sample: &[&Entry], cells: &[Option<Pixels>], failed: &[(i64, String)]) {
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
    let summary = |times: &mut Vec<Duration>| match Timings::of(times) {
        Some(t) => format!("median {:.1} ms, worst {:.1} ms", t.median, t.worst),
        None => "no playable files".into(),
    };
    println!(
        "Sound start ({} files not read before): from disk {}; read ahead into memory {}",
        sample.len(),
        summary(&mut from_disk),
        summary(&mut from_memory),
    );
}

/// The middle, average and slowest of some timings, in milliseconds.
#[derive(Clone, Copy)]
struct Timings {
    median: f64,
    mean: f64,
    worst: f64,
}

impl Timings {
    /// Sorts `times`; `None` when there are none.
    fn of(times: &mut [Duration]) -> Option<Self> {
        times.sort_unstable();
        let worst = *times.last()?;
        let total: Duration = times.iter().sum();
        Some(Self { median: ms(times[times.len() / 2]), mean: ms(total) / times.len() as f64, worst: ms(worst) })
    }
}

/// Lays thumbnails out on a checkerboard, `COLUMNS` to a row, to check them by eye.
fn write_sheet(path: &std::path::Path, cells: &[Pixels]) {
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

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
