//! Errors that would otherwise vanish: a release build has no console. They're appended to a log
//! file next to the cache, and the latest one is shown in the status bar.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static LAST: Mutex<Option<String>> = Mutex::new(None);

pub fn log_path() -> PathBuf {
    crate::db::default_path().with_file_name("asset_browser.log")
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
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = writeln!(file, "[unix {secs}] {line}");
    }
}
