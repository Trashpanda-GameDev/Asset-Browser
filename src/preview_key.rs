//! The key a file's previews are cached under: one version of one file, made one way. The library
//! gives every file its key, and the thumbnail service caches what it makes under it.

use crate::asset::AssetKind;

/// Bump when the way image thumbnails or waveforms are made changes, so old cache entries stop
/// matching.
const THUMB_VERSION: u64 = 1;

/// The same for model previews, kept apart so they can be made again without throwing away the
/// image and sound caches. Starts at 2: version 1 is what model keys were before models had
/// previews.
const MODEL_PREVIEW_VERSION: u64 = 2;

/// Identifies one version of one file's preview: a different path, size or modification time is
/// a different key, and so is a new way of making previews of its kind. FNV-1a, because its
/// output never changes between Rust versions.
///
/// The root's separators are normalised so a file gets the same key whichever of its parent
/// folders was opened.
pub fn cache_key(root: &str, rel: &str, size: u64, mtime: i64, kind: AssetKind) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for &byte in bytes {
            hash ^= u64::from(if byte == b'\\' { b'/' } else { byte });
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    feed(root.trim_end_matches(['/', '\\']).as_bytes());
    feed(b"/");
    feed(rel.as_bytes());
    feed(&size.to_le_bytes());
    feed(&mtime.to_le_bytes());
    let version = match kind {
        AssetKind::Image | AssetKind::Audio => THUMB_VERSION,
        AssetKind::Model => MODEL_PREVIEW_VERSION,
    };
    feed(&version.to_le_bytes());
    hash as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_and_sound_keys_are_unchanged() {
        // Computed by the version before model previews: users have these thumbnails cached.
        let rock = cache_key("F:\\Resources", "3D/Textures/rock.png", 123456, 1700000000, AssetKind::Image);
        assert_eq!(rock, -9117893502861981840);
        let hit = cache_key("F:\\Resources\\", "Sounds/hit.wav", 98765, -5, AssetKind::Audio);
        assert_eq!(hit, -7692076259545324008);
        let bomb = "3D/Platformer Pack by Quaternius/FBX/Bomb.fbx";
        assert_eq!(cache_key("F:\\Resources", bomb, 4242, 1534000000, AssetKind::Model), -7762848195514578235);
    }
}
