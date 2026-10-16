//! Keeps the library current while the app is open. The OS says when something changes in the
//! library's folders (ReadDirectoryChangesW on Windows, FSEvents on macOS, inotify on Linux), and a
//! folder whose assets changed is rescanned once the changes settle, the way it is at the start.
//!
//! Only changes that can change the library count: an asset added, changed, renamed or removed,
//! or a folder made, renamed or removed. Reads (making thumbnails), other files (`Thumbs.db`,
//! a saving program's temporary files) and a folder's own date changing are ignored.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::event::{EventKind, ModifyKind};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::asset::AssetKind;
use crate::diag;
use crate::library::is_within;

/// A folder is rescanned once its changes have stopped for this long...
const QUIET: Duration = Duration::from_millis(750);
/// ...or this long after the first, while they keep coming (a long copy shows up as it goes).
const AT_MOST: Duration = Duration::from_secs(4);
/// How often a folder that couldn't be watched (an unplugged drive) is tried again.
const RETRY: Duration = Duration::from_secs(60);

/// When a folder's changes began and when the last one came.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Changes {
    first: Instant,
    last: Instant,
}

#[derive(Default)]
struct Shared {
    /// The folders watched, to tell which one a change is in: as the library has them, and as
    /// the OS may report them (macOS reports a folder reached through a link by its real path).
    roots: Vec<(PathBuf, PathBuf)>,
    changes: HashMap<PathBuf, Changes>,
}

impl Shared {
    fn root_of(&self, path: &Path) -> Option<PathBuf> {
        self.roots
            .iter()
            .find(|(root, real)| is_within(path, root) || is_within(path, real))
            .map(|(root, _)| root.clone())
    }
}

pub struct Watch {
    /// `None` when the OS's watching couldn't be started: the library is then only scanned at the
    /// start and when the app changes it.
    watcher: Option<RecommendedWatcher>,
    shared: Arc<Mutex<Shared>>,
    /// The folders being watched, and those that couldn't be (with when they were last tried).
    watched: Vec<PathBuf>,
    failed: HashMap<PathBuf, Instant>,
}

impl Watch {
    /// `wake` asks for a frame: called on the OS's thread when a change comes in, with how soon
    /// it'll be ready (`poll`).
    pub fn start(wake: impl Fn(Duration) + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let handler = {
            let shared = shared.clone();
            move |result: notify::Result<notify::Event>| {
                let Ok(event) = result else { return };
                let mut shared = shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let now = Instant::now();
                let roots: Vec<PathBuf> = if event.need_rescan() {
                    // Events were lost: any of them may have changed.
                    shared.roots.iter().map(|(root, _)| root.clone()).collect()
                } else {
                    event
                        .paths
                        .iter()
                        .filter(|path| counts(&event.kind, path))
                        .filter_map(|path| shared.root_of(path))
                        .collect()
                };
                if roots.is_empty() {
                    return;
                }
                for root in roots {
                    shared
                        .changes
                        .entry(root)
                        .and_modify(|changes| changes.last = now)
                        .or_insert(Changes { first: now, last: now });
                }
                drop(shared);
                wake(QUIET);
            }
        };
        let watcher = notify::recommended_watcher(handler)
            .map_err(|err| diag::report("Watching the library's folders for changes", err))
            .ok();
        Self { watcher, shared, watched: Vec::new(), failed: HashMap::new() }
    }

    /// Watches exactly `roots` (the library's folders), from now on. Cheap when they're the same.
    pub fn sync(&mut self, roots: &[PathBuf]) {
        let now = Instant::now();
        let retry: Vec<PathBuf> = self
            .failed
            .iter()
            .filter(|(_, at)| now.duration_since(**at) >= RETRY)
            .map(|(root, _)| root.clone())
            .collect();
        if self.watched.as_slice() == roots && retry.is_empty() {
            return;
        }
        let Some(watcher) = &mut self.watcher else { return };
        for root in &self.watched {
            if !roots.contains(root) {
                let _ = watcher.unwatch(root);
            }
        }
        self.failed.retain(|root, _| roots.contains(root));
        for root in roots {
            let new = !self.watched.contains(root);
            if new || retry.contains(root) {
                arm(watcher, root, &mut self.failed);
            }
        }
        self.watched = roots.to_vec();
        let real = |root: &PathBuf| std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        let mut shared = self.shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        shared.roots = roots.iter().map(|root| (root.clone(), real(root))).collect();
        shared.changes.retain(|root, _| roots.contains(root));
    }

    /// The folders whose changes have settled, to rescan now. Each is watched afresh first, so
    /// nothing is missed if the OS dropped its watch (a burst too big for its buffer can do that
    /// on Windows): what changes meanwhile, the rescan finds.
    pub fn poll(&mut self) -> Vec<PathBuf> {
        let ready = {
            let mut shared = self.shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let ready = settled(&shared.changes, Instant::now());
            shared.changes.retain(|root, _| !ready.contains(root));
            ready
        };
        if let Some(watcher) = &mut self.watcher {
            for root in &ready {
                let _ = watcher.unwatch(root);
                arm(watcher, root, &mut self.failed);
            }
        }
        ready
    }

    /// How soon the next folder will have settled, if any is waiting: to ask for a frame then.
    pub fn next_due(&self) -> Option<Duration> {
        let shared = self.shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        shared.changes.values().map(|changes| due(*changes).saturating_duration_since(now)).min()
    }
}

fn arm(watcher: &mut RecommendedWatcher, root: &Path, failed: &mut HashMap<PathBuf, Instant>) {
    match watcher.watch(root, RecursiveMode::Recursive) {
        Ok(()) => {
            failed.remove(root);
        }
        Err(err) => {
            // Logged once, not on every retry.
            if !failed.contains_key(root) {
                diag::note(&format!("Can't watch {} for changes", crate::platform::native(root)), err);
            }
            failed.insert(root.to_path_buf(), Instant::now());
        }
    }
}

/// When a folder's changes count as settled.
fn due(changes: Changes) -> Instant {
    (changes.last + QUIET).min(changes.first + AT_MOST)
}

/// The folders whose changes have settled by `now`.
fn settled(changes: &HashMap<PathBuf, Changes>, now: Instant) -> Vec<PathBuf> {
    changes.iter().filter(|(_, changes)| due(**changes) <= now).map(|(root, _)| root.clone()).collect()
}

/// Whether a change to `path` can change the library.
fn counts(kind: &EventKind, path: &Path) -> bool {
    // The scan leaves out dot names, so their changes don't matter either.
    if path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.')) {
        return false;
    }
    let asset = path.extension().is_some_and(|ext| AssetKind::from_extension(&ext.to_string_lossy()).is_some());
    match kind {
        // Reads, and the like: nothing changed.
        EventKind::Access(_) => false,
        // An asset written to; a folder's own date changing when something in it does isn't
        // enough (it may be a file the library leaves out).
        EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Metadata(_) | ModifyKind::Any | ModifyKind::Other) => asset,
        // Made, removed or renamed: an asset, or a folder (which may hold assets). A removed path
        // can't be looked at any more, so no extension counts as a folder.
        _ => asset || path.is_dir() || path.extension().is_none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use notify::event::{AccessKind, CreateKind, DataChange, RemoveKind};

    #[test]
    fn what_counts_as_a_change() {
        let create = EventKind::Create(CreateKind::Any);
        let write = EventKind::Modify(ModifyKind::Data(DataChange::Any));
        let remove = EventKind::Remove(RemoveKind::Any);
        assert!(counts(&create, Path::new("/a/hero.png")));
        assert!(counts(&write, Path::new("/a/hit.WAV")));
        assert!(counts(&remove, Path::new("/a/gone.fbx")));
        assert!(counts(&remove, Path::new("/a/a folder")), "a removed folder");
        assert!(!counts(&create, Path::new("/a/notes.txt")));
        assert!(!counts(&create, Path::new("/a/~psd1234.tmp")), "a saving program's temporary file");
        assert!(!counts(&create, Path::new("/a/.hidden.png")));
        assert!(!counts(&EventKind::Access(AccessKind::Any), Path::new("/a/hero.png")), "making its thumbnail");
        assert!(!counts(&EventKind::Modify(ModifyKind::Any), Path::new("/a/Thumbs.db")));
    }

    #[test]
    fn changes_settle_when_quiet_or_after_a_while() {
        let start = Instant::now();
        let mut changes = HashMap::new();
        changes.insert(PathBuf::from("/a"), Changes { first: start, last: start });
        assert!(settled(&changes, start + Duration::from_millis(100)).is_empty(), "still coming");
        assert_eq!(settled(&changes, start + QUIET), [PathBuf::from("/a")]);
        // A long copy: changes keep coming, but it's rescanned as it goes.
        changes.insert(PathBuf::from("/a"), Changes { first: start, last: start + AT_MOST - Duration::from_millis(1) });
        assert_eq!(settled(&changes, start + AT_MOST), [PathBuf::from("/a")]);
    }

    /// The OS's own events, for real: a picture dropped in is noticed, a text file isn't.
    #[test]
    fn notices_a_new_asset() {
        let temp = TempDir::new("watch");
        std::fs::create_dir_all(temp.path().join("sub")).unwrap();
        let dir = std::path::absolute(temp.path()).unwrap();
        let mut watch = Watch::start(|_| {});
        watch.sync(std::slice::from_ref(&dir));
        let changed = |watch: &mut Watch| {
            let until = Instant::now() + Duration::from_secs(10);
            while Instant::now() < until {
                let ready = watch.poll();
                if !ready.is_empty() {
                    return ready;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Vec::new()
        };
        std::fs::write(dir.join("sub").join("hero.png"), b"not really a png").unwrap();
        assert_eq!(changed(&mut watch), std::slice::from_ref(&dir), "the new picture's folder is rescanned");
        // Anything still coming from that write.
        std::thread::sleep(QUIET + Duration::from_millis(500));
        watch.poll();

        std::fs::write(dir.join("notes.txt"), b"hello").unwrap();
        std::thread::sleep(QUIET + Duration::from_millis(500));
        assert!(watch.poll().is_empty(), "a file the library leaves out changes nothing");

        watch.sync(&[]);
    }
}
