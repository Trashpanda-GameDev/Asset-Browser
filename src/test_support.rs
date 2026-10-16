//! Helpers shared by the tests.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A new, empty folder for one test, removed with everything in it when the test ends, failed or not. It sits two
/// folders deep in a folder of its own, so lookups that climb up to `../..` (texture searches do) stay inside it
/// instead of reaching whatever is in the system's temp folder.
pub struct TempDir {
    /// The folder of its own, removed at the end.
    root: PathBuf,
    path: PathBuf,
}

impl TempDir {
    pub fn new(name: &str) -> Self {
        static MADE: AtomicUsize = AtomicUsize::new(0);
        let unique = format!("asset_browser_{name}_{}_{}", std::process::id(), MADE.fetch_add(1, Ordering::Relaxed));
        let root = std::env::temp_dir().join(unique);
        let path = root.join("test").join(name);
        // Left over from a run that was killed, with the same process id.
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&path).expect("create the test's folder");
        Self { root, path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes a file at `name` in the folder (which may have folders in it, made as needed).
    pub fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create a folder in the test's folder");
        }
        std::fs::write(&path, bytes).expect("write a test file");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Nothing to do if it can't be removed: it's in the temp folder, which the OS clears.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
