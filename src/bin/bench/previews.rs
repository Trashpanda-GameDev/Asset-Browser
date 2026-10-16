//! Making a sample's previews twice, into an empty cache and then from it, and what that showed.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asset_browser::app::grouped;
use asset_browser::asset::AssetKind;
use asset_browser::imaging::Pixels;
use asset_browser::library::{Entry, Library};
use asset_browser::model::{self, ModelInfo, TextureStats};
use asset_browser::thumbs::{Preview, Thumb, ThumbJob, ThumbService};
use asset_browser::work::Wake;

use super::sheets::{write_model_sheets, write_sheet};
use super::{Options, PER_PAGE};

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
pub(super) fn previews(kind: AssetKind, library: &Library, sample: &[&Entry], db_path: &Path, options: &Options) {
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
            Err(error) => self.failed.push((thumb.key, error.to_string())),
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
