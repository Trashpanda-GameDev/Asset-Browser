//! What kind of asset a file is, decided by its extension.

use eframe::egui::Color32;

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

    pub fn color(self) -> Color32 {
        match self {
            Self::Image => Color32::from_rgb(92, 168, 255),
            Self::Audio => Color32::from_rgb(255, 176, 72),
            Self::Model => Color32::from_rgb(84, 210, 146),
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Image => egui_phosphor::regular::IMAGE,
            Self::Audio => egui_phosphor::regular::WAVEFORM,
            Self::Model => egui_phosphor::regular::CUBE,
        }
    }
}
