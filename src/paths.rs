//! Where the app keeps its files: the cache, the user's own data and the log.

use std::path::PathBuf;

/// Set to a folder, the app keeps everything in it (the cache, the log and the user's own data)
/// instead of in the user's folders. The screenshots tool's demo sets it, so it can't touch the
/// real library.
pub const HOME_VARIABLE: &str = "ASSET_BROWSER_HOME";

/// The cache, in the per-user cache location for this OS (`%LOCALAPPDATA%` on Windows,
/// `~/Library/Caches` on macOS, `~/.cache` on Linux).
pub fn cache_file() -> PathBuf {
    home("cache")
        .or_else(|| directories::ProjectDirs::from("", "", "AssetBrowser").map(|dirs| dirs.cache_dir().to_path_buf()))
        .unwrap_or_else(|| std::env::temp_dir().join("AssetBrowser"))
        .join("library.sqlite")
}

/// The user's own data, which isn't a cache and must survive one being cleared: favourites,
/// collections, categories (`groups`) and sprite sheet choices (`sheets`). The per-user data
/// folder: `%APPDATA%` on Windows, `~/Library/Application Support` on macOS, `~/.local/share`
/// on Linux.
pub fn user_data_file() -> PathBuf {
    home("data")
        .or_else(|| directories::ProjectDirs::from("", "", "AssetBrowser").map(|dirs| dirs.data_dir().to_path_buf()))
        .unwrap_or_else(|| std::env::temp_dir().join("AssetBrowser"))
        .join("user.sqlite")
}

/// The error log, next to the cache.
pub fn log_file() -> PathBuf {
    cache_file().with_file_name("asset_browser.log")
}

fn home(part: &str) -> Option<PathBuf> {
    std::env::var_os(HOME_VARIABLE).filter(|home| !home.is_empty()).map(|home| PathBuf::from(home).join(part))
}
