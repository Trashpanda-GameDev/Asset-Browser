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
    pub error: Option<DeleteError>,
}

/// Why files were kept. The status bar shows the message.
#[derive(Debug, thiserror::Error)]
pub enum DeleteError {
    /// What the shell said.
    #[error("{0}")]
    Trash(#[from] trash::Error),
    /// The shell said nothing of a failure, yet they're there.
    #[error("they're still there")]
    StillThere,
}

/// Starts moving `paths` to the Recycle Bin; `done` runs on that thread once it's over (to wake
/// the UI).
pub fn start(paths: Vec<PathBuf>, done: impl FnOnce() + Send + 'static) -> Receiver<Deleted> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    work::spawn("delete", move || {
        let result = platform::move_to_trash(&paths);
        // Fails only when the window has closed.
        let _ = tx.send(sort_out(paths, result));
        done();
    });
    rx
}

/// After the shell has had its go, which paths are gone: what the shell reports isn't enough,
/// since it stops at the first failure, and the user can say no to its own "Delete for good?".
fn sort_out(paths: Vec<PathBuf>, result: Result<(), trash::Error>) -> Deleted {
    // `symlink_metadata` so a link is checked rather than what it points to.
    let (kept, gone): (Vec<PathBuf>, Vec<PathBuf>) =
        paths.into_iter().partition(|path| std::fs::symlink_metadata(path).is_ok());
    let error = (!kept.is_empty()).then(|| result.err().map_or(DeleteError::StillThere, DeleteError::from));
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
        let refused = |description: &str| Err(trash::Error::Unknown { description: description.to_owned() });

        let deleted = sort_out(vec![here.clone(), gone.clone()], refused("Access is denied"));
        assert_eq!(deleted.gone, std::slice::from_ref(&gone));
        assert_eq!(deleted.kept, std::slice::from_ref(&here));
        assert!(matches!(deleted.error, Some(DeleteError::Trash(_))), "the shell's reason");

        let deleted = sort_out(vec![gone.clone()], Ok(()));
        assert!(deleted.kept.is_empty() && deleted.error.is_none());
        let deleted = sort_out(vec![gone.clone()], refused("Some operations were aborted"));
        assert!(deleted.error.is_none(), "an error after everything went is no failure");
        let deleted = sort_out(vec![here.clone()], Ok(()));
        assert!(
            matches!(deleted.error, Some(DeleteError::StillThere)),
            "a success that left the file behind is still a failure"
        );
        assert_eq!(DeleteError::StillThere.to_string(), "they're still there");
    }
}
