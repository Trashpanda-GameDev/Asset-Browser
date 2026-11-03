//! Errors that would otherwise vanish: a release build has no console. Once a program starts the
//! log (the app does, at start), they're appended to it; the latest one is shown in the status bar.
//! Until then nothing is written, so tests and the tools leave the user's log alone.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

static LAST: Mutex<Option<String>> = Mutex::new(None);

/// Where errors are appended, once a program has started the log.
static LOG: OnceLock<PathBuf> = OnceLock::new();

/// Appends errors to the file at `path` from now on, and panics once the hook is installed. For the
/// programs only: the library never calls it, so its tests write no log.
pub fn start_log(path: PathBuf) {
    // Started already: the log stays where it is.
    let _ = LOG.set(path);
}

/// Records an error that the app carries on after.
pub fn report(context: &str, error: impl std::fmt::Display) {
    let line = format!("{context}: {error}");
    append(&line);
    *LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(line);
}

/// Records an error that isn't worth showing, such as an update check while offline.
pub fn note(context: &str, error: impl std::fmt::Display) {
    append(&format!("{context}: {error}"));
}

/// The most recent error, for the status bar.
pub fn last() -> Option<String> {
    LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

/// Writes every panic, with a backtrace, to the log before the default handling runs.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unnamed");
        append(&format!("panic on thread '{name}': {info}\n{}", std::backtrace::Backtrace::force_capture()));
        default(info);
    }));
}

fn append(line: &str) {
    if let Some(path) = LOG.get() {
        append_to(path, line);
    }
}

/// Adds `line` at the end of the log at `path`, with the time.
fn append_to(path: &Path, line: &str) {
    if let Some(dir) = path.parent() {
        // The log can't report failing to write itself: opening the file below fails too.
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        // Nowhere left to tell.
        let _ = writeln!(file, "[unix {secs}] {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn each_error_is_a_line_at_the_end_of_the_log_with_the_time() {
        let dir = TempDir::new("diag");
        // In a folder that isn't there yet, as the cache's isn't before the app first runs.
        let log = dir.path().join("cache").join("asset_browser.log");
        append_to(&log, "Reading a picture: the file is gone");
        append_to(&log, "Saving previews: the disk is full");
        let text = std::fs::read_to_string(&log).expect("the log");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(
            lines[0].starts_with("[unix ") && lines[0].ends_with("] Reading a picture: the file is gone"),
            "{text}"
        );
        assert!(lines[1].ends_with("] Saving previews: the disk is full"), "{text}");
    }
}
