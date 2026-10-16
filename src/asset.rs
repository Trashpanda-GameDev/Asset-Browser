//! What kind of asset a file is, decided by its extension.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AssetKind {
    Image,
    Audio,
    Model,
}

impl AssetKind {
    pub const ALL: [AssetKind; 3] = [Self::Image, Self::Audio, Self::Model];

    /// Files with any other extension are left out of the library.
    pub fn from_extension(ext: &str) -> Option<Self> {
        let mut buf = [0u8; 8];
        let bytes = ext.as_bytes();
        if bytes.len() > buf.len() {
            return None;
        }
        let lower = &mut buf[..bytes.len()];
        lower.copy_from_slice(bytes);
        lower.make_ascii_lowercase();
        Some(match &*lower {
            b"png" | b"jpg" | b"jpeg" | b"gif" | b"bmp" | b"tga" | b"tif" | b"tiff" | b"webp" | b"ico" | b"qoi"
            | b"psd" => Self::Image,
            b"wav" | b"ogg" | b"mp3" | b"flac" | b"aif" | b"aiff" | b"opus" | b"m4a" => Self::Audio,
            b"fbx" | b"obj" | b"gltf" | b"glb" | b"dae" | b"blend" | b"3ds" | b"stl" | b"ply" => Self::Model,
            _ => return None,
        })
    }

    pub fn index(self) -> usize {
        match self {
            Self::Image => 0,
            Self::Audio => 1,
            Self::Model => 2,
        }
    }

    pub fn to_db(self) -> i64 {
        self.index() as i64
    }

    pub fn from_db(value: i64) -> Option<Self> {
        Self::ALL.get(usize::try_from(value).ok()?).copied()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Image => "Images",
            Self::Audio => "Sounds",
            Self::Model => "Models",
        }
    }

    pub fn singular(self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Audio => "Sound",
            Self::Model => "Model",
        }
    }
}

/// Which kinds of asset are shown: every kind, or a mode's one (`Mode::kinds`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KindSet([bool; 3]);

impl KindSet {
    pub const ALL: Self = Self([true; 3]);

    pub fn of(kinds: &[AssetKind]) -> Self {
        let mut set = [false; 3];
        for kind in kinds {
            set[kind.index()] = true;
        }
        Self(set)
    }

    pub fn only(kind: AssetKind) -> Self {
        Self::of(&[kind])
    }

    pub fn contains(self, kind: AssetKind) -> bool {
        self.0[kind.index()]
    }

    /// How many of `counts` (one per kind, in `AssetKind::index` order) are of these kinds.
    pub fn count(self, counts: [u32; 3]) -> u32 {
        counts.iter().zip(self.0).filter(|(_, on)| *on).map(|(count, _)| count).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every cached folder index holds these numbers: changing one would file people's assets under
    /// the wrong kind until their folders are scanned again.
    #[test]
    fn stored_numbers_never_change() {
        assert_eq!(AssetKind::ALL.map(AssetKind::to_db), [0, 1, 2]);
        for kind in AssetKind::ALL {
            assert_eq!(AssetKind::from_db(kind.to_db()), Some(kind));
        }
        assert_eq!(AssetKind::from_db(3), None);
        assert_eq!(AssetKind::from_db(-1), None);
    }

    #[test]
    fn a_kind_set_counts_only_its_kinds() {
        let counts = [5, 7, 11];
        assert_eq!(KindSet::ALL.count(counts), 23);
        assert_eq!(KindSet::only(AssetKind::Audio).count(counts), 7);
        assert_eq!(KindSet::of(&[AssetKind::Image, AssetKind::Model]).count(counts), 16);
        assert!(KindSet::only(AssetKind::Model).contains(AssetKind::Model));
        assert!(!KindSet::only(AssetKind::Model).contains(AssetKind::Image));
    }

    #[test]
    fn the_extension_decides_the_kind() {
        assert_eq!(AssetKind::from_extension("png"), Some(AssetKind::Image));
        assert_eq!(AssetKind::from_extension("PSD"), Some(AssetKind::Image), "any case");
        assert_eq!(AssetKind::from_extension("Ogg"), Some(AssetKind::Audio));
        assert_eq!(AssetKind::from_extension("glb"), Some(AssetKind::Model));
        assert_eq!(AssetKind::from_extension("txt"), None);
        assert_eq!(AssetKind::from_extension(""), None);
        assert_eq!(AssetKind::from_extension("pngpngpng"), None, "longer than any asset's extension");
    }
}
