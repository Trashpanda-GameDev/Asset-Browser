//! Finding a model's texture files: the path the file names, the folders textures are usually kept
//! in, near-misses of the name, and pictures named after the model.

use std::path::{Component, Path, PathBuf};

/// Common endings of a model's colour texture named after it: chest.png, build_texture.tga.
const COLOR_SUFFIXES: [&str; 9] =
    ["", "_texture", "_tex", "_diffuse", "_diff", "_albedo", "_basecolor", "_base_color", "_color"];

/// More folders beside the model's than this isn't one pack's layout (FBX/ beside PNG/): they
/// aren't searched.
pub(super) const MAX_SIBLINGS: usize = 8;

/// The lowercase names a model's own texture may have, best first: its name with or without a
/// colour-map ending (`chest`, `build_texture`); then, as packs number variants of one object,
/// the texture of its variant by letter (`egg02` → `eggB`, `ground01Cracked` → `groundACracked`),
/// without the number (`player2` → `player`), or the first variant's (`coin` → `coinA`).
pub(super) fn companion_names(stem: &str) -> Vec<String> {
    let mut names: Vec<String> = COLOR_SUFFIXES.iter().map(|suffix| format!("{stem}{suffix}")).collect();
    // The last run of digits.
    let end = stem.rfind(|c: char| c.is_ascii_digit()).map(|at| at + 1);
    let start = end.map(|end| stem[..end].trim_end_matches(|c: char| c.is_ascii_digit()).len());
    match (start, end) {
        (Some(start), Some(end)) if start > 0 => {
            let (before, after) = (&stem[..start], &stem[end..]);
            if let Ok(number @ 1..=26) = stem[start..end].parse::<u8>() {
                names.push(format!("{before}{}{after}", (b'a' + number - 1) as char));
            }
            names.push(format!("{}{after}", before.trim_end_matches(['_', '-', ' '])));
        }
        _ => names.push(format!("{stem}a")),
    }
    names
}

/// File types a texture is saved as.
const PICTURES: [&str; 10] = ["png", "tga", "jpg", "jpeg", "bmp", "psd", "tif", "tiff", "webp", "gif"];

/// A folder huge enough to make listing it slow isn't a pack's texture folder.
const MAX_LISTED: usize = 10_000;

/// The picture files directly in `folder`.
pub(super) fn pictures_in(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    entries
        .take(MAX_LISTED)
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| PICTURES.iter().any(|picture| ext.eq_ignore_ascii_case(picture)))
        })
        .collect()
}

pub(super) fn stem_of(path: &Path) -> String {
    path.file_stem().map(|stem| stem.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Lowercase names that agree at both ends and differ only in the middle, as when a pack is
/// renamed: how many characters they share, if at least 3 at the start and 6 at the end, the end
/// counted from a separator.
pub(super) fn similar(a: &str, b: &str) -> Option<usize> {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let room = a.len().min(b.len()).saturating_sub(prefix);
    let shared = a.iter().rev().zip(b.iter().rev()).take(room).take_while(|(x, y)| x == y).count();
    let end = &a[a.len() - shared..];
    let from = end.iter().position(|&c| matches!(c, b'_' | b'-' | b' ' | b'.'))?;
    let suffix = shared - from;
    (prefix >= 3 && suffix >= 6).then_some(prefix + suffix)
}

/// Normal, specular, roughness and similar maps sit beside colour textures with names close to
/// theirs; they're never what a near miss means.
pub(super) fn is_data_map(stem: &str) -> bool {
    const DATA: [&str; 14] = [
        "normal",
        "nrm",
        "bump",
        "height",
        "disp",
        "rough",
        "gloss",
        "spec",
        "metal",
        "occlusion",
        "emis",
        "emmis",
        "mask",
        "opacity",
    ];
    stem.split(['_', '-', ' ', '.']).any(|word| DATA.iter().any(|data| word.starts_with(data)))
        || DATA.iter().any(|data| stem.ends_with(data))
}

/// Where to look, in order: the reference itself if relative, then its file name next to the
/// model and in `Textures` folders beside and above it, as asset packs ship.
pub(super) fn candidates(model_dir: &Path, reference: &str) -> Vec<PathBuf> {
    let parts: Vec<&str> = reference.split(['/', '\\']).filter(|part| !part.is_empty() && *part != ".").collect();
    let Some(&name) = parts.last() else { return Vec::new() };
    let absolute = reference.starts_with(['/', '\\']) || reference.contains(':');
    let mut list = Vec::with_capacity(6);
    if !absolute && parts.len() > 1 {
        list.push(parts.iter().fold(model_dir.to_path_buf(), |path, part| path.join(part)));
    }
    for folder in ["", "Textures", "../Textures", "../../Textures"] {
        let mut path = model_dir.to_path_buf();
        for part in folder.split('/').filter(|part| !part.is_empty()) {
            if part == ".." {
                path.pop();
            } else {
                path.push(part);
            }
        }
        list.push(path.join(name));
    }
    let mut unique: Vec<PathBuf> = Vec::with_capacity(list.len());
    for path in list {
        let path = normalize(&path);
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    unique
}

/// Folds `..` so the same file isn't probed twice under different spellings.
pub(super) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir if out.file_name().is_some() => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}
