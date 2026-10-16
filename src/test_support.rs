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

/// Damaged copies of a good file, for parsers to survive: cut short at many points, and with
/// bytes overwritten at random (a fixed seed, so a failure repeats).
pub fn damaged(original: &[u8], copies: usize) -> Vec<Vec<u8>> {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut random = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut out: Vec<Vec<u8>> =
        (0..=original.len()).step_by(original.len() / 64 + 1).map(|len| original[..len].to_vec()).collect();
    for _ in 0..copies {
        let mut copy = original.to_vec();
        for _ in 0..1 + random() % 4 {
            let at = (random() % copy.len().max(1) as u64) as usize;
            if let Some(byte) = copy.get_mut(at) {
                // Mostly extreme values, which find overflows; sometimes anything.
                *byte = match random() % 3 {
                    0 => 0xff,
                    1 => 0x00,
                    _ => random() as u8,
                };
            }
        }
        out.push(copy);
    }
    out
}
