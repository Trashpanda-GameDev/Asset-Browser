//! Scanning some of the library's folders again, saving what each holds to the folder index, and
//! building the library anew with what was found. Slow, so the app runs it on a thread of its own.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::library::{Library, root_string};
use crate::scan::{self, ScanResult, ScannedFile};
use crate::{db, diag, folder_index, platform};

/// What a rescan found.
pub struct Rescanned {
    /// The folders the library was built for.
    pub roots: Vec<PathBuf>,
    /// The library built with what was found and the files the other folders had when the scan
    /// started. When a folder couldn't be read, its files stay as they were, which only the app
    /// knows: then it's just what was found, for the app to build the library with.
    pub built: Result<Library, Vec<(PathBuf, Vec<ScannedFile>)>>,
    /// The folders scanned, and those that couldn't be read (unplugged, moved or renamed).
    pub scanned: Vec<PathBuf>,
    pub unreadable: Vec<PathBuf>,
    pub totals: ScanTotals,
}

/// How much a rescan went through, for the status bar.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScanTotals {
    pub elapsed: Duration,
    pub assets: usize,
    pub dirs_visited: usize,
    pub skipped: usize,
}

impl ScanTotals {
    fn add(&mut self, result: &ScanResult) {
        self.elapsed += result.elapsed;
        self.assets += result.files.len();
        self.dirs_visited += result.dirs_visited;
        self.skipped += result.skipped_files;
    }
}

/// Scans `targets`, some of `roots`, saving each one's index in the cache file at `cache`, and
/// builds the library from what was found and `others`: the other roots' files as they are.
pub fn rescan(
    cache: &Path,
    roots: Vec<PathBuf>,
    targets: &[PathBuf],
    others: Vec<(PathBuf, Vec<ScannedFile>)>,
) -> Rescanned {
    let found = scan_folders(cache, targets);
    let scanned = found.fresh.iter().map(|(root, _)| root.clone()).collect();
    let built =
        if found.unreadable.is_empty() { Ok(build(&roots, targets, found.fresh, others)) } else { Err(found.fresh) };
    Rescanned { roots, built, scanned, unreadable: found.unreadable, totals: found.totals }
}

/// What the folders scanned hold, and those that couldn't be read.
struct Found {
    fresh: Vec<(PathBuf, Vec<ScannedFile>)>,
    unreadable: Vec<PathBuf>,
    totals: ScanTotals,
}

/// Scans each folder and saves what's in it to the index. A folder that can't be read keeps what
/// was found there last time, in the index too: an unplugged drive mustn't empty its favorites
/// and collections.
fn scan_folders(cache: &Path, targets: &[PathBuf]) -> Found {
    let conn = db::open_cache(cache).map_err(|err| diag::report("Saving the index", err)).ok();
    let mut found = Found { fresh: Vec::new(), unreadable: Vec::new(), totals: ScanTotals::default() };
    for root in targets {
        if let Err(err) = std::fs::read_dir(root) {
            let context = format!("Can't read {} (showing what was found there last time)", platform::native(root));
            diag::report(&context, err);
            found.unreadable.push(root.clone());
            continue;
        }
        let result = scan::scan(root);
        if result.dirs_visited == 0 {
            found.unreadable.push(root.clone());
            continue;
        }
        if let Some(conn) = &conn
            && let Err(err) = folder_index::save(conn, &root_string(root), &result.files)
        {
            diag::report("Saving the index", err);
        }
        found.totals.add(&result);
        found.fresh.push((root.clone(), result.files));
    }
    found
}

/// The library from the folders just scanned and the others' files, in the roots' order.
fn build(
    roots: &[PathBuf],
    targets: &[PathBuf],
    mut fresh: Vec<(PathBuf, Vec<ScannedFile>)>,
    mut others: Vec<(PathBuf, Vec<ScannedFile>)>,
) -> Library {
    let files = roots
        .iter()
        .map(|root| {
            let from = if targets.contains(root) { &mut fresh } else { &mut others };
            let files = from.iter().position(|(path, _)| path == root).map(|at| from.swap_remove(at).1);
            (root.clone(), files.unwrap_or_default())
        })
        .collect();
    Library::build(files)
}
