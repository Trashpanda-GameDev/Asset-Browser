//! Walks a folder tree on every core and lists the asset files in it.
//!
//! `jwalk` would do the same job, but it is deprecated upstream. This is the same idea in a few
//! lines: one rayon task per directory. On Windows, `DirEntry::metadata` comes from the directory
//! listing itself, so each file costs no extra system call.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};

use crate::asset::AssetKind;

#[derive(Clone, Debug)]
pub struct ScannedFile {
    /// Path relative to the scanned root, with `/` separators on every platform.
    pub rel: String,
    pub kind: AssetKind,
    pub size: u64,
    /// Modification time in nanoseconds since the Unix epoch (0 when unknown).
    pub mtime: i64,
}

pub struct ScanResult {
    pub files: Vec<ScannedFile>,
    pub dirs_visited: usize,
    /// Files whose extension isn't an asset type we know.
    pub skipped_files: usize,
    pub elapsed: Duration,
}

struct Shared {
    files: Mutex<Vec<ScannedFile>>,
    dirs: AtomicUsize,
    skipped: AtomicUsize,
}

pub fn scan(root: &Path) -> ScanResult {
    let start = Instant::now();
    let shared = Shared { files: Mutex::new(Vec::new()), dirs: AtomicUsize::new(0), skipped: AtomicUsize::new(0) };
    rayon::scope(|scope| walk(scope, root.to_path_buf(), String::new(), &shared));
    ScanResult {
        files: shared.files.into_inner().unwrap_or_default(),
        dirs_visited: shared.dirs.into_inner(),
        skipped_files: shared.skipped.into_inner(),
        elapsed: start.elapsed(),
    }
}

fn walk<'s>(scope: &rayon::Scope<'s>, dir: PathBuf, rel: String, shared: &'s Shared) {
    let Ok(read) = std::fs::read_dir(&dir) else {
        return;
    };
    shared.dirs.fetch_add(1, Ordering::Relaxed);
    let mut found = Vec::new();
    let mut skipped = 0;
    for entry in read.flatten() {
        let name = entry.file_name();
        // Dot files are OS and tool clutter (.DS_Store, .git); non-UTF-8 names can't round-trip
        // through the index.
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') {
            continue;
        }
        // Symlinks and junctions report neither dir nor file here, so loops can't happen.
        let Ok(file_type) = entry.file_type() else { continue };
        let child_rel = if rel.is_empty() { name.to_owned() } else { format!("{rel}/{name}") };
        if file_type.is_dir() {
            let path = entry.path();
            scope.spawn(move |scope| walk(scope, path, child_rel, shared));
        } else if file_type.is_file() {
            let kind = Path::new(name).extension().and_then(|ext| ext.to_str()).and_then(AssetKind::from_extension);
            let Some(kind) = kind else {
                skipped += 1;
                continue;
            };
            let Ok(meta) = entry.metadata() else { continue };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_nanos() as i64);
            found.push(ScannedFile { rel: child_rel, kind, size: meta.len(), mtime });
        }
    }
    shared.skipped.fetch_add(skipped, Ordering::Relaxed);
    if !found.is_empty() {
        shared.files.lock().unwrap().extend(found);
    }
}
