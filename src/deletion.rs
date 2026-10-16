//! Deleting assets and folders. They go to the Recycle Bin (the Trash on macOS and Linux), on a
//! thread of their own since the shell can take a while over many files, and what actually went
//! is reported back, so the library can drop just that.

use std::path::PathBuf;

use crossbeam_channel::Receiver;

use crate::{platform, work};

/// How a deletion went.
pub struct Deleted {
    /// No longer where they were.
    pub gone: Vec<PathBuf>,
    /// Still there.
    pub kept: Vec<PathBuf>,
    /// Why, when some were kept.
    pub error: Option<String>,
}

/// Starts moving `paths` to the Recycle Bin; `done` runs on that thread once it's over (to wake
/// the UI).
pub fn start(paths: Vec<PathBuf>, done: impl FnOnce() + Send + 'static) -> Receiver<Deleted> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    work::spawn("delete", move || {
        let result = platform::move_to_trash(&paths).map_err(|err| err.to_string());
        // Fails only when the window has closed.
        let _ = tx.send(sort_out(paths, result));
        done();
    });
    rx
}

/// After the shell has had its go, which paths are gone: what the shell reports isn't enough,
/// since it stops at the first failure, and the user can say no to its own "Delete for good?".
fn sort_out(paths: Vec<PathBuf>, result: Result<(), String>) -> Deleted {
    // `symlink_metadata` so a link is checked rather than what it points to.
    let (kept, gone): (Vec<PathBuf>, Vec<PathBuf>) =
        paths.into_iter().partition(|path| std::fs::symlink_metadata(path).is_ok());
    let error = (!kept.is_empty()).then(|| result.err().unwrap_or_else(|| "they're still there".to_owned()));
    Deleted { gone, kept, error }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn what_is_still_there_was_kept() {
        let dir = TempDir::new("deletion");
        let here = dir.write("here.png", b"x");
        let gone = dir.path().join("gone.png");

        let deleted = sort_out(vec![here.clone(), gone.clone()], Err("Access is denied".to_owned()));
        assert_eq!(deleted.gone, std::slice::from_ref(&gone));
        assert_eq!(deleted.kept, std::slice::from_ref(&here));
        assert_eq!(deleted.error.as_deref(), Some("Access is denied"));

        let deleted = sort_out(vec![gone.clone()], Ok(()));
        assert!(deleted.kept.is_empty() && deleted.error.is_none());
        let deleted = sort_out(vec![gone.clone()], Err("Some operations were aborted".to_owned()));
        assert!(deleted.error.is_none(), "an error after everything went is no failure");
        let deleted = sort_out(vec![here.clone()], Ok(()));
        assert!(deleted.error.is_some(), "a success that left the file behind is still a failure");
    }
}
