//! How paths are compared: a root's identity, a file's or folder's key across the app, and whether
//! one folder is inside another.

use std::path::{Path, PathBuf};

/// The form of a root path used as its identity in the index and cache keys.
pub fn root_string(root: &Path) -> String {
    let text = root.to_string_lossy();
    let trimmed = text.trim_end_matches(['/', '\\']);
    // Keep "C:\" rather than "C:" so a drive root stays a valid path.
    if trimmed.ends_with(':') { text.into_owned() } else { trimmed.to_owned() }
}

/// A file's or folder's identity across the app: its absolute path with `/` separators, in
/// lowercase, without a trailing separator. Favorites, collections and categories hold files by
/// it, and folders are found again by it after a rescan.
pub fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// The last part of a path ("Resources"), or the whole path for a drive ("F:\").
pub fn folder_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// Whether `inner` is `outer` or inside it.
pub fn is_within(inner: &Path, outer: &Path) -> bool {
    let (inner, outer) = (path_key(inner), path_key(outer));
    inner == outer || inner.strip_prefix(&outer).is_some_and(|rest| rest.starts_with('/'))
}

/// Adds `path` to a library's folders, keeping each file in one root: a folder that's in the
/// library already (a root, or inside one) isn't added, and `Err` has the root it's in. Otherwise
/// it goes in, in place of any roots inside it, and `Ok` has those.
pub fn add_root(roots: &mut Vec<PathBuf>, path: PathBuf) -> Result<Vec<PathBuf>, usize> {
    if let Some(index) = roots.iter().position(|root| is_within(&path, root)) {
        return Err(index);
    }
    let at = roots.iter().position(|root| is_within(root, &path)).unwrap_or(roots.len());
    let (inside, kept): (Vec<PathBuf>, Vec<PathBuf>) = roots.drain(..).partition(|root| is_within(root, &path));
    *roots = kept;
    roots.insert(at, path);
    Ok(inside)
}
