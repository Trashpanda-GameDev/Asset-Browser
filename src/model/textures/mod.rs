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

mod cache;
mod decode;
mod find;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use cache::{DECODE_MICROS, DECODED, open};
use decode::{decode_image, finish};
use find::{MAX_SIBLINGS, candidates, companion_names, is_data_map, normalize, pictures_in, similar, stem_of};

pub use cache::{TextureStats, set_cache_budget, texture_stats};

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

/// Decodes texture bytes embedded in a model file. `name` gives the format when the bytes don't
/// (TGA has no signature).
pub(super) fn from_bytes(bytes: &[u8], name: &str, max: u32, alpha: Alpha) -> Option<Decoded> {
    let start = Instant::now();
    let decoded = decode_image(bytes, Path::new(name)).and_then(|(w, h, rgba)| finish(name, w, h, rgba, max, alpha));
    DECODED.fetch_add(1, Ordering::Relaxed);
    DECODE_MICROS.fetch_add(start.elapsed().as_micros() as u64, Ordering::Relaxed);
    decoded
}

#[cfg(test)]
mod tests {
    use super::decode::is_cutout_mask;
    use super::*;
    use crate::model::test_files::png_2x2;
    use crate::test_support::TempDir;

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
        let models = dir.path().join("Models");
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
        let models = dir.path().join("Pack/Models");
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
        let fbx = dir.path().join("Pack/FBX");
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
