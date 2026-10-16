//! Decoded textures kept for reuse, within a memory budget, so models sharing an atlas decode it once.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime};

use super::decode::decode_file;
use super::{Alpha, Decoded};
use crate::work;

/// Decoded textures kept for reuse. A pack's atlas at the thumbnail size is 256 KB.
const CACHE_ENTRIES: usize = 32;

/// What those may hold in all: one at the preview pane's size is 16 MB. The memory setting
/// changes it (`set_cache_budget`).
static CACHE_BUDGET: AtomicU64 = AtomicU64::new(128 << 20);

/// How much the decoded textures kept may hold, from the next one decoded.
pub fn set_cache_budget(bytes: u64) {
    CACHE_BUDGET.store(bytes, Ordering::Relaxed);
}

/// A texture file as decoded once, or being decoded: every thread that wants it while it's
/// decoding waits for that one decode instead of starting its own. Workers take a folder's
/// models at the same time, so without this the 17 Synty models decoded their shared atlas
/// up to 17 times over (measured: 3 reuses in 47 lookups across the test library).
struct Cached {
    path: PathBuf,
    max: u32,
    alpha: Alpha,
    len: u64,
    modified: Option<SystemTime>,
    texture: Arc<OnceLock<Option<Arc<Decoded>>>>,
}

static CACHE: Mutex<VecDeque<Cached>> = Mutex::new(VecDeque::new());

pub(super) static DECODED: AtomicU64 = AtomicU64::new(0);

pub(super) static DECODE_MICROS: AtomicU64 = AtomicU64::new(0);

static REUSED: AtomicU64 = AtomicU64::new(0);

/// Texture decoding since the program started, for the benchmark.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextureStats {
    pub decoded: u64,
    pub decode_micros: u64,
    /// Served from the cache instead of being decoded again.
    pub reused: u64,
}

pub fn texture_stats() -> TextureStats {
    TextureStats {
        decoded: DECODED.load(Ordering::Relaxed),
        decode_micros: DECODE_MICROS.load(Ordering::Relaxed),
        reused: REUSED.load(Ordering::Relaxed),
    }
}

/// A picture file at `path`, decoded or from the cache; `None` if there's no such file or it
/// can't be decoded.
pub(super) fn open(path: &Path, max: u32, alpha: Alpha) -> Option<Arc<Decoded>> {
    let meta = std::fs::metadata(path).ok().filter(|meta| meta.is_file())?;
    let cell = cache_cell(path, max, alpha, meta.len(), meta.modified().ok());
    let mut decoded_here = false;
    let texture = cell
        .get_or_init(|| {
            decoded_here = true;
            let start = Instant::now();
            let decoded = decode_file(path, max, alpha).map(Arc::new);
            DECODED.fetch_add(1, Ordering::Relaxed);
            DECODE_MICROS.fetch_add(start.elapsed().as_micros() as u64, Ordering::Relaxed);
            decoded
        })
        .clone();
    if !decoded_here {
        REUSED.fetch_add(1, Ordering::Relaxed);
    }
    texture
}

/// The cache's cell for this version of the file, made empty if there's none yet.
fn cache_cell(
    path: &Path,
    max: u32,
    alpha: Alpha,
    len: u64,
    modified: Option<SystemTime>,
) -> Arc<OnceLock<Option<Arc<Decoded>>>> {
    let mut cache = work::lock(&CACHE);
    if let Some(index) = cache.iter().position(|entry| entry.path == path && entry.max == max && entry.alpha == alpha)
        && let Some(entry) = cache.remove(index)
        // Otherwise it changed on disk since, and is replaced.
        && entry.len == len
        && entry.modified == modified
    {
        let cell = entry.texture.clone();
        cache.push_front(entry);
        return cell;
    }
    // The oldest go first, past the count or the bytes (of those decoded by now).
    let budget = CACHE_BUDGET.load(Ordering::Relaxed);
    let held = |cache: &VecDeque<Cached>| -> u64 {
        cache.iter().filter_map(|entry| entry.texture.get()?.as_ref()).map(|decoded| decoded.rgba.len() as u64).sum()
    };
    while cache.len() >= CACHE_ENTRIES || (!cache.is_empty() && held(&cache) > budget) {
        cache.pop_back();
    }
    let cell = Arc::new(OnceLock::new());
    cache.push_front(Cached { path: path.to_path_buf(), max, alpha, len, modified, texture: cell.clone() });
    cell
}
