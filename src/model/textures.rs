//! Finds and decodes the base-colour textures that models reference.
//!
//! Model files rarely point at their textures correctly: the path is an absolute one from the
//! artist's machine, or relative to a folder layout that wasn't kept. So after the file's own
//! relative path, the texture's file name is looked up in the few places asset packs put them.
//! Each probe can cost a seek on a spinning disk, so the list is short and stops at the first hit,
//! and absolute paths from the file are never tried (they can name network shares that take
//! seconds to time out).
//!
//! When no file has the name, those folders are listed once for a near miss: the same name as
//! another kind of picture (a .psd shipped as .png), or a name that differs only in the middle,
//! as when a pack is renamed (`PolygonCity_Texture_02_A` shipped as
//! `Polygon_Fantasy_Characters_Texture_02_A`). Some packs name no texture at all and pair them by
//! file name instead (`chestA.fbx` with `../PNG/chestA.png`, `build.fbx` with
//! `build_texture.tga`): `companion` finds those. On the test library, 15 of its 557 models found
//! their textures by the names in the files; with these, 77 do.
//!
//! Many models in a pack share one texture atlas, so decoded textures are kept in a small cache
//! shared by every thread. Measured on the test library: 23 of 77 texture loads are served from
//! it, each saving a decode of 30 ms on average.

use std::collections::VecDeque;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime};

use fast_image_resize as fr;
use image::ImageFormat;

pub(super) struct Decoded {
    /// The file name, for display.
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// sRGB, straight alpha, top row first.
    pub rgba: Vec<u8>,
    /// With `Alpha::Guess`: the alpha channel looks like a cutout mask.
    pub cutout: bool,
}

/// What a texture's alpha channel means.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Alpha {
    /// The format doesn't say (FBX, OBJ, 3DS): use alpha only if it looks like a cutout mask,
    /// clear holes in an otherwise opaque picture, as leaves and fences use. Otherwise it's made
    /// opaque: diffuse textures often carry unrelated data there (gloss, height) or an alpha that
    /// is zero everywhere.
    Guess,
    /// The material says how alpha is used (glTF): keep it as it is.
    Keep,
}

/// Decoded textures kept for reuse. A pack's atlas at the thumbnail size is 256 KB.
const CACHE_ENTRIES: usize = 32;
/// What those may hold in all: one at the preview pane's size is 16 MB. The memory setting
/// changes it (`set_cache_budget`).
static CACHE_BUDGET: AtomicU64 = AtomicU64::new(128 << 20);

/// How much the decoded textures kept may hold, from the next one decoded.
pub fn set_cache_budget(bytes: u64) {
    CACHE_BUDGET.store(bytes, Ordering::Relaxed);
}

/// A texture file as decoded once, or being decoded: every thread that wants it while it's
/// decoding waits for that one decode instead of starting its own. Workers take a folder's
/// models at the same time, so without this the 17 Synty models decoded their shared atlas
/// up to 17 times over (measured: 3 reuses in 47 lookups across the test library).
struct Cached {
    path: PathBuf,
    max: u32,
    alpha: Alpha,
    len: u64,
    modified: Option<SystemTime>,
    texture: Arc<OnceLock<Option<Arc<Decoded>>>>,
}

static CACHE: Mutex<VecDeque<Cached>> = Mutex::new(VecDeque::new());
static DECODED: AtomicU64 = AtomicU64::new(0);
static DECODE_MICROS: AtomicU64 = AtomicU64::new(0);
static REUSED: AtomicU64 = AtomicU64::new(0);

/// Texture decoding since the program started, for the benchmark.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextureStats {
    pub decoded: u64,
    pub decode_micros: u64,
    /// Served from the cache instead of being decoded again.
    pub reused: u64,
}

pub fn texture_stats() -> TextureStats {
    TextureStats {
        decoded: DECODED.load(Ordering::Relaxed),
        decode_micros: DECODE_MICROS.load(Ordering::Relaxed),
        reused: REUSED.load(Ordering::Relaxed),
    }
}

/// Finds the texture `reference` (as written in the model file: a relative or absolute path, or a
/// bare name) near the model in `model_dir`, and decodes it scaled to fit `max`.
pub(super) fn from_file(model_dir: &Path, reference: &str, max: u32, alpha: Alpha) -> Option<Arc<Decoded>> {
    let candidates = candidates(model_dir, reference);
    for path in &candidates {
        if let Some(texture) = open(path, max, alpha) {
            return Some(texture);
        }
    }
    let wanted = candidates.last()?.file_stem()?.to_str()?.to_lowercase();
    let mut folders: Vec<&Path> = Vec::new();
    for folder in candidates.iter().filter_map(|path| path.parent()) {
        if !folders.contains(&folder) {
            folders.push(folder);
        }
    }
    let mut best: Option<(usize, PathBuf)> = None;
    let mut tied = false;
    for folder in folders {
        for picture in pictures_in(folder) {
            let stem = stem_of(&picture);
            if stem == wanted {
                return open(&picture, max, alpha);
            }
            let Some(score) = similar(&wanted, &stem).filter(|_| !is_data_map(&stem)) else { continue };
            match &best {
                Some((best_score, _)) if score < *best_score => {}
                Some((best_score, _)) if score == *best_score => tied = true,
                _ => {
                    best = Some((score, picture));
                    tied = false;
                }
            }
        }
    }
    match best {
        Some((_, path)) if !tied => open(&path, max, alpha),
        _ => None,
    }
}

/// Common endings of a model's colour texture named after it: chest.png, build_texture.tga.
const COLOR_SUFFIXES: [&str; 9] =
    ["", "_texture", "_tex", "_diffuse", "_diff", "_albedo", "_basecolor", "_base_color", "_color"];

/// More folders beside the model's than this isn't one pack's layout (FBX/ beside PNG/): they
/// aren't searched.
const MAX_SIBLINGS: usize = 8;

/// A picture named after the model (see the module notes), for models whose file names no
/// texture or only one that's missing. Looked for beside the model, in the usual `Textures`
/// folders, and in the folders beside the model's own.
pub(super) fn companion(model: &Path, max: u32) -> Option<Arc<Decoded>> {
    let stem = model.file_stem()?.to_str()?.to_lowercase();
    let dir = model.parent()?;
    let mut folders: Vec<PathBuf> =
        ["", "Textures", "../Textures", "../../Textures"].iter().map(|folder| normalize(&dir.join(folder))).collect();
    if let Some(parent) = dir.parent()
        && let Ok(entries) = std::fs::read_dir(parent)
    {
        let mut siblings: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.path())
            .take(MAX_SIBLINGS + 2)
            .collect();
        siblings.sort();
        if siblings.len() <= MAX_SIBLINGS + 1 {
            folders.extend(siblings.into_iter().map(|sibling| normalize(&sibling)));
        }
    }
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut listings: Vec<Vec<PathBuf>> = Vec::new();
    for folder in folders {
        if !seen.contains(&folder) {
            listings.push(pictures_in(&folder));
            seen.push(folder);
        }
    }
    // The model's own name anywhere before a variant's name anywhere.
    for wanted in companion_names(&stem) {
        for pictures in &listings {
            let found = pictures.iter().filter(|p| stem_of(p) == wanted).find_map(|p| open(p, max, Alpha::Guess));
            if found.is_some() {
                return found;
            }
        }
    }
    None
}

/// The lowercase names a model's own texture may have, best first: its name with or without a
/// colour-map ending (`chest`, `build_texture`); then, as packs number variants of one object,
/// the texture of its variant by letter (`egg02` → `eggB`, `ground01Cracked` → `groundACracked`),
/// without the number (`player2` → `player`), or the first variant's (`coin` → `coinA`).
fn companion_names(stem: &str) -> Vec<String> {
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

/// A picture file at `path`, decoded or from the cache; `None` if there's no such file or it
/// can't be decoded.
fn open(path: &Path, max: u32, alpha: Alpha) -> Option<Arc<Decoded>> {
    let meta = std::fs::metadata(path).ok().filter(|meta| meta.is_file())?;
    let cell = cache_cell(path, max, alpha, meta.len(), meta.modified().ok());
    let mut decoded_here = false;
    let texture = cell
        .get_or_init(|| {
            decoded_here = true;
            let start = Instant::now();
            let decoded = decode_file(path, max, alpha).map(Arc::new);
            DECODED.fetch_add(1, Ordering::Relaxed);
            DECODE_MICROS.fetch_add(start.elapsed().as_micros() as u64, Ordering::Relaxed);
            decoded
        })
        .clone();
    if !decoded_here {
        REUSED.fetch_add(1, Ordering::Relaxed);
    }
    texture
}

/// The cache's cell for this version of the file, made empty if there's none yet.
fn cache_cell(
    path: &Path,
    max: u32,
    alpha: Alpha,
    len: u64,
    modified: Option<SystemTime>,
) -> Arc<OnceLock<Option<Arc<Decoded>>>> {
    let mut cache = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(index) = cache.iter().position(|entry| entry.path == path && entry.max == max && entry.alpha == alpha)
        && let Some(entry) = cache.remove(index)
        // Otherwise it changed on disk since, and is replaced.
        && entry.len == len
        && entry.modified == modified
    {
        let cell = entry.texture.clone();
        cache.push_front(entry);
        return cell;
    }
    // The oldest go first, past the count or the bytes (of those decoded by now).
    let budget = CACHE_BUDGET.load(Ordering::Relaxed);
    let held = |cache: &VecDeque<Cached>| -> u64 {
        cache.iter().filter_map(|entry| entry.texture.get()?.as_ref()).map(|decoded| decoded.rgba.len() as u64).sum()
    };
    while cache.len() >= CACHE_ENTRIES || (!cache.is_empty() && held(&cache) > budget) {
        cache.pop_back();
    }
    let cell = Arc::new(OnceLock::new());
    cache.push_front(Cached { path: path.to_path_buf(), max, alpha, len, modified, texture: cell.clone() });
    cell
}

/// File types a texture is saved as.
const PICTURES: [&str; 10] = ["png", "tga", "jpg", "jpeg", "bmp", "psd", "tif", "tiff", "webp", "gif"];

/// A folder huge enough to make listing it slow isn't a pack's texture folder.
const MAX_LISTED: usize = 10_000;

/// The picture files directly in `folder`.
fn pictures_in(folder: &Path) -> Vec<PathBuf> {
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

fn stem_of(path: &Path) -> String {
    path.file_stem().map(|stem| stem.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Lowercase names that agree at both ends and differ only in the middle, as when a pack is
/// renamed: how many characters they share, if at least 3 at the start and 6 at the end, the end
/// counted from a separator.
fn similar(a: &str, b: &str) -> Option<usize> {
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
fn is_data_map(stem: &str) -> bool {
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

/// Decodes texture bytes embedded in a model file. `name` gives the format when the bytes don't
/// (TGA has no signature).
pub(super) fn from_bytes(bytes: &[u8], name: &str, max: u32, alpha: Alpha) -> Option<Decoded> {
    let start = Instant::now();
    let decoded = decode_image(bytes, Path::new(name)).and_then(|(w, h, rgba)| finish(name, w, h, rgba, max, alpha));
    DECODED.fetch_add(1, Ordering::Relaxed);
    DECODE_MICROS.fetch_add(start.elapsed().as_micros() as u64, Ordering::Relaxed);
    decoded
}

/// Where to look, in order: the reference itself if relative, then its file name next to the
/// model and in `Textures` folders beside and above it, as asset packs ship.
fn candidates(model_dir: &Path, reference: &str) -> Vec<PathBuf> {
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
fn normalize(path: &Path) -> PathBuf {
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

fn decode_file(path: &Path, max: u32, alpha: Alpha) -> Option<Decoded> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("psd")) {
        let sampled = crate::psd_composite::read(path, max).ok()?;
        return finish(&name, sampled.w, sampled.h, sampled.rgba, max, alpha);
    }
    let bytes = std::fs::read(path).ok()?;
    let (w, h, rgba) = decode_image(&bytes, path)?;
    finish(&name, w, h, rgba, max, alpha)
}

/// Trusts the content over the extension; TGA has no signature, so it falls back to the name.
fn decode_image(bytes: &[u8], name: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let reader = || -> Option<image::ImageReader<Cursor<&[u8]>>> {
        let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
        if reader.format().is_none() {
            reader.set_format(ImageFormat::from_path(name).ok()?);
        }
        Some(reader)
    };
    // A model's textures are decoded at full size too: on the thumbnail workers they take their
    // turn in the decode budget, like pictures.
    let (w, h) = reader()?.into_dimensions().ok()?;
    let _decoding = crate::thumbs::reserve_decode(w, h);
    let image = reader()?.decode().ok()?.into_rgba8();
    let (w, h) = image.dimensions();
    Some((w, h, image.into_raw()))
}

/// Decides how alpha is used, then scales the picture to fit `max`.
#[expect(clippy::too_many_arguments, reason = "refactor: take fewer, grouped arguments")]
fn finish(name: &str, w: u32, h: u32, mut rgba: Vec<u8>, max: u32, alpha: Alpha) -> Option<Decoded> {
    if w == 0 || h == 0 || rgba.len() != w as usize * h as usize * 4 {
        return None;
    }
    let cutout = alpha == Alpha::Guess && is_cutout_mask(&rgba);
    if alpha == Alpha::Guess && !cutout {
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
    }
    let (fit_w, fit_h) = crate::thumbs::fit_size(w, h, max);
    if (fit_w, fit_h) != (w, h) {
        // Opaque (most textures, and every one whose alpha was just dropped): no premultiplied
        // full-size copy needed.
        let opaque = rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 255);
        let src = fr::images::Image::from_vec_u8(w, h, rgba, fr::PixelType::U8x4).ok()?;
        let mut dst = fr::images::Image::new(fit_w, fit_h, fr::PixelType::U8x4);
        let options = fr::ResizeOptions::new()
            .resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::CatmullRom))
            .use_alpha(!opaque);
        fr::Resizer::new().resize(&src, &mut dst, &options).ok()?;
        rgba = dst.into_vec();
    }
    Some(Decoded { name: name.to_owned(), width: fit_w, height: fit_h, rgba, cutout })
}

/// A mask is mostly fully opaque or fully clear, with some clear pixels: leaves, chain-link,
/// lace. Smooth alpha over most of the picture is packed data, and an alpha that is clear
/// everywhere was never meant to be seen.
fn is_cutout_mask(rgba: &[u8]) -> bool {
    let (mut clear, mut partial, mut total) = (0usize, 0usize, 0usize);
    for pixel in rgba.as_chunks::<4>().0 {
        let alpha = pixel[3];
        total += 1;
        if alpha < 128 {
            clear += 1;
        }
        if (8..248).contains(&alpha) {
            partial += 1;
        }
    }
    clear > 0 && clear < total && partial * 4 < total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::test_files::{TempDir, png_2x2};

    #[test]
    fn probes_relative_path_then_name_in_texture_folders() {
        let dir = Path::new("/packs/Pack/Models");
        let list = candidates(dir, "..\\Art\\wood.tga");
        let expected = [
            "/packs/Pack/Art/wood.tga",
            "/packs/Pack/Models/wood.tga",
            "/packs/Pack/Models/Textures/wood.tga",
            "/packs/Pack/Textures/wood.tga",
            "/packs/Textures/wood.tga",
        ];
        assert_eq!(list, expected.map(PathBuf::from));
        // An absolute path from another machine is only used for its file name.
        let list = candidates(dir, "C:\\Users\\artist\\wood.tga");
        assert_eq!(list[0], PathBuf::from("/packs/Pack/Models/wood.tga"));
        assert!(candidates(dir, "").is_empty());
    }

    #[test]
    fn finds_texture_beside_and_reuses_it() {
        let dir = TempDir::new("textures");
        dir.write("Textures/atlas.png", &png_2x2());
        let models = dir.0.join("Models");
        std::fs::create_dir_all(&models).unwrap();
        let first = from_file(&models, "D:/work/atlas.png", 256, Alpha::Guess).expect("found in ../Textures");
        assert_eq!((first.width, first.height), (2, 2));
        assert_eq!(&first.rgba[..4], &[255, 0, 0, 255]);
        assert!(!first.cutout);
        let again = from_file(&models, "atlas.png", 256, Alpha::Guess).unwrap();
        assert!(Arc::ptr_eq(&first, &again), "served from the cache");
        let kept = from_file(&models, "atlas.png", 256, Alpha::Keep).unwrap();
        assert!(!Arc::ptr_eq(&first, &kept), "decoded separately: alpha is treated differently");
        assert!(from_file(&models, "nothing.png", 256, Alpha::Guess).is_none());
    }

    #[test]
    fn near_misses_when_the_named_file_is_gone() {
        let dir = TempDir::new("texture_near_miss");
        dir.write("Pack/Textures/Polygon_Fantasy_Characters_Texture_02_A.png", &png_2x2());
        dir.write("Pack/Textures/Polygon_Fantasy_Characters_Texture_02_B.png", &png_2x2());
        dir.write("Pack/Textures/Polygon_Fantasy_Characters_Texture_02_A_Normals.png", &png_2x2());
        dir.write("Pack/Models/wood.png", &png_2x2());
        let models = dir.0.join("Pack/Models");
        // A renamed pack: the name differs only in the middle.
        let renamed = from_file(&models, "..\\..\\Texture\\PolygonCity_Texture_02_A.png", 64, Alpha::Guess).unwrap();
        assert_eq!(renamed.name, "Polygon_Fantasy_Characters_Texture_02_A.png");
        // The same name as another kind of picture.
        assert_eq!(from_file(&models, "C:/art/Wood.psd", 64, Alpha::Guess).unwrap().name, "wood.png");
        // Nothing alike: no guess.
        assert!(from_file(&models, "Stone_Texture_02_A.png", 64, Alpha::Guess).is_none());

        assert!(similar("polygoncity_texture_02_a", "polygon_fantasy_characters_texture_02_a").is_some());
        assert!(similar("polygoncity_texture_02_a", "polygon_fantasy_characters_texture_02_b").is_none());
        assert!(similar("rock_01_diffuse", "tree_01_diffuse").is_none(), "different objects");
        assert!(is_data_map("build_texture_nrm") && is_data_map("housenrm") && !is_data_map("house"));
    }

    #[test]
    fn texture_named_after_the_model() {
        let dir = TempDir::new("texture_companion");
        dir.write("Pack/PNG/chestA.png", &png_2x2());
        dir.write("Pack/PNG/eggB.png", &png_2x2());
        dir.write("Pack/PNG/groundACracked.png", &png_2x2());
        dir.write("Pack/PNG/player.png", &png_2x2());
        dir.write("Pack/PNG/coinA.png", &png_2x2());
        dir.write("Pack/FBX/build_texture.png", &png_2x2());
        dir.write("Pack/FBX/build_texture_nrm.png", &png_2x2());
        let fbx = dir.0.join("Pack/FBX");
        for (model, texture) in [
            ("chestA.fbx", "chestA.png"),
            ("egg02.fbx", "eggB.png"),
            ("ground01Cracked.fbx", "groundACracked.png"),
            ("player3.fbx", "player.png"),
            ("coin.fbx", "coinA.png"),
            ("build.fbx", "build_texture.png"),
        ] {
            assert_eq!(companion(&fbx.join(model), 64).map(|t| t.name.clone()).as_deref(), Some(texture), "{model}");
        }
        assert!(companion(&fbx.join("rock.fbx"), 64).is_none());
        assert_eq!(companion_names("egg02")[9..], ["eggb", "egg"]);
        assert_eq!(companion_names("coin")[9..], ["coina"]);
    }

    #[test]
    fn alpha_is_a_mask_only_when_it_looks_like_one() {
        let pixels = |alphas: &[u8]| -> Vec<u8> { alphas.iter().flat_map(|&a| [9, 9, 9, a]).collect() };
        assert!(is_cutout_mask(&pixels(&[255, 255, 0, 0, 255, 255, 255, 0])));
        assert!(!is_cutout_mask(&pixels(&[255, 255, 255, 255])), "opaque");
        assert!(!is_cutout_mask(&pixels(&[0, 0, 0, 0])), "clear everywhere");
        assert!(!is_cutout_mask(&pixels(&[40, 90, 120, 200, 230, 10, 100, 60])), "packed data");
        // Ignored alpha is made opaque, unless the material says how to use it.
        let decoded = finish("t", 2, 1, pixels(&[0, 0]), 256, Alpha::Guess).unwrap();
        assert_eq!(decoded.rgba[3], 255);
        let kept = finish("t", 2, 1, pixels(&[0, 0]), 256, Alpha::Keep).unwrap();
        assert_eq!(kept.rgba[3], 0);
    }

    #[test]
    fn large_textures_shrink_to_fit() {
        let decoded = finish("t", 512, 128, vec![200; 512 * 128 * 4], 256, Alpha::Guess).unwrap();
        assert_eq!((decoded.width, decoded.height), (256, 64));
        assert_eq!(decoded.rgba.len(), 256 * 64 * 4);
    }
}
