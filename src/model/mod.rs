//! 3D models: reads the common interchange formats into one simple form (`Model`) that both the
//! thumbnail renderer (`raster`) and the model viewer draw.
//!
//! Every loader produces the same space: right-handed, Y up, metres, the model's front facing +Z
//! (glTF's convention), with every node transform already applied, so nothing downstream looks at
//! the source format. FBX and OBJ are read by ufbx (`fbx`), glTF by the `gltf` crate (`gltf`); STL,
//! PLY, 3DS and Collada are small readers of our own. A .blend file can't be read without Blender,
//! but it carries the picture Blender saved with it (`blend`).

use std::collections::HashSet;
use std::path::Path;

pub mod blend;
mod builder;

mod collada;

mod fbx;

mod geometry;

mod gltf;

mod ply;

pub mod raster;

pub mod shading;

mod stl;

mod textures;

mod three_ds;

pub mod vector;

pub use textures::{TextureStats, set_cache_budget as set_texture_cache_budget, texture_stats};

/// Everything about a model that the previews need, in one coordinate system:
/// right-handed, Y up, metres, with every node transform already applied.
pub struct Model {
    pub vertices: Vec<Vertex>,
    /// Triangle list into `vertices`. Front faces wind counter-clockwise.
    pub indices: Vec<u32>,
    /// Ranges of `indices` that share a material, in draw order: one per material.
    pub parts: Vec<Part>,
    pub materials: Vec<Material>,
    /// Decoded base-colour textures, referenced by `Material::texture`.
    pub textures: Vec<Texture>,
    /// Skeleton segments (parent joint, child joint), for files that are only a skeleton and its
    /// animation (e.g. Mixamo "without skin" downloads). Empty when the file has meshes.
    pub bones: Vec<[[f32; 3]; 2]>,
    pub bounds: Bounds,
    pub info: ModelInfo,
}

impl Model {
    /// Every triangle edge once, as a line list into `vertices`: the wireframe.
    pub fn edges(&self) -> Vec<u32> {
        let mut seen = HashSet::with_capacity(self.indices.len());
        let mut lines = Vec::with_capacity(self.indices.len() * 2);
        for triangle in self.indices.as_chunks::<3>().0 {
            for (a, b) in [(triangle[0], triangle[1]), (triangle[1], triangle[2]), (triangle[2], triangle[0])] {
                if seen.insert((a.min(b), a.max(b))) {
                    lines.extend([a, b]);
                }
            }
        }
        lines
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    /// Unit length, or zero where the file's normal was unusable.
    pub normal: [f32; 3],
    /// (0, 0) is the top-left corner of the texture (glTF's convention), so rows can be uploaded
    /// top first. Repeats outside 0..1.
    pub uv: [f32; 2],
    /// Linear vertex colour multiplier, [1, 1, 1, 1] when the file has none.
    pub color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    pub first: u32,
    pub count: u32,
    pub material: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub name: String,
    /// Linear RGBA base colour factor.
    pub color: [f32; 4],
    pub texture: Option<u32>,
    /// Fragments whose alpha (material × vertex × texture) is below this are discarded: leaves,
    /// fences and hair cards. `None` for opaque materials, whose alpha is ignored.
    pub alpha_cutoff: Option<f32>,
}

impl Material {
    /// Clay: what surfaces the file gives no material of their own are drawn with.
    pub const CLAY: Material = Material {
        name: String::new(),
        color: [shading::DEFAULT_ALBEDO[0], shading::DEFAULT_ALBEDO[1], shading::DEFAULT_ALBEDO[2], 1.0],
        texture: None,
        alpha_cutoff: None,
    };
}

#[derive(Clone)]
pub struct Texture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// sRGB-encoded, straight alpha, top row first.
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Bounds {
    const EMPTY: Bounds = Bounds { min: [f32::INFINITY; 3], max: [f32::NEG_INFINITY; 3] };

    fn add(&mut self, p: [f32; 3]) {
        self.min = [0, 1, 2].map(|axis| self.min[axis].min(p[axis]));
        self.max = [0, 1, 2].map(|axis| self.max[axis].max(p[axis]));
    }

    pub fn center(&self) -> [f32; 3] {
        [0, 1, 2].map(|axis| (self.min[axis] + self.max[axis]) * 0.5)
    }

    pub fn size(&self) -> [f32; 3] {
        [0, 1, 2].map(|axis| (self.max[axis] - self.min[axis]).max(0.0))
    }

    /// Radius of the sphere around `center` that holds the box.
    pub fn radius(&self) -> f32 {
        let [x, y, z] = self.size();
        (x * x + y * y + z * z).sqrt() * 0.5
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelInfo {
    /// "FBX 7.4 binary", "OBJ", "glTF 2.0 (GLB)", "STL binary", "Blender 2.79", ...
    pub format: String,
    /// As the file counts them (FBX and OBJ count positions); otherwise the vertices drawn.
    pub vertices: u64,
    pub triangles: u64,
    pub meshes: u32,
    pub materials: u32,
    pub textures: u32,
    /// Textures the materials reference that couldn't be found or decoded.
    pub missing_textures: u32,
    /// Bounds extent in metres.
    pub size: [f32; 3],
    /// (name, seconds)
    pub animations: Vec<(String, f32)>,
    /// False for .blend (picture only) and skeleton-only files.
    pub geometry: bool,
}

/// How a model is read. The default skips textures.
#[derive(Default)]
pub struct LoadOptions {
    /// Textures are scaled down to fit this many pixels on their longest side; 0 skips textures.
    pub max_texture: u32,
}

/// A format's reader: the file's bytes, the file itself (for what it refers to beside it, such as
/// textures), and how to read it.
type Parse = fn(&[u8], &Path, &LoadOptions) -> Result<Model, ModelError>;

/// The formats read, by extension. A new format is a reader and a line here (and its extension
/// in `asset`).
const FORMATS: [(&[&str], Parse); 6] = [
    (&["fbx", "obj"], fbx::parse),
    (&["gltf", "glb"], gltf::parse),
    (&["stl"], stl::parse),
    (&["ply"], ply::parse),
    (&["3ds"], three_ds::parse),
    (&["dae"], collada::parse),
];

/// Models with more triangles than this aren't loaded. A vertex is 48 bytes and a faceted STL has
/// three per triangle, so this keeps one model under about 600 MB with a thumbnail made on every
/// core at once. The largest model in the test library has 151,000.
const MAX_TRIANGLES: u64 = 4_000_000;

/// Why a model can't be shown. The message is what the user reads, and the thumbnail cache keeps
/// it with the failure, so a message doesn't change.
#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("{0}")]
    Read(#[from] std::io::Error),
    /// What the XML reader found wrong with a Collada file.
    #[error("Collada: {0}")]
    Collada(#[from] roxmltree::Error),
    /// What the glTF reader found wrong.
    #[error("glTF: {0}")]
    Gltf(#[from] ::gltf::Error),
    #[error("Blender files can't be read without Blender, only their saved preview")]
    Blender,
    #[error("unsupported model format .{0}")]
    Unsupported(String),
    #[error(
        "too many triangles to preview: {:.1} million (the limit is {} million)",
        *.0 as f64 / 1e6,
        MAX_TRIANGLES / 1_000_000
    )]
    TooManyTriangles(u64),
    #[error("there are no meshes in this file")]
    NoMeshes,
    /// Nothing was read, and this is why what the file has was left out.
    #[error("{0}")]
    Skipped(&'static str),
    /// Triangles were read, but none has a position that can be drawn.
    #[error("there are no triangles to draw")]
    NoTriangles,
    /// What a format's reader found wrong: a damaged file, or one stored in a way it can't read.
    #[error("{0}")]
    Unreadable(String),
}

impl From<String> for ModelError {
    fn from(message: String) -> Self {
        Self::Unreadable(message)
    }
}

impl From<&str> for ModelError {
    fn from(message: &str) -> Self {
        Self::Unreadable(message.to_owned())
    }
}

/// `Builder::finish` checks the count it was given; a reader checks first too where it would
/// otherwise read or make this much.
fn check_triangle_count(triangles: u64) -> Result<(), ModelError> {
    if triangles > MAX_TRIANGLES {
        return Err(ModelError::TooManyTriangles(triangles));
    }
    Ok(())
}

/// Reads a model file. Returns an error for .blend files, whose geometry can't be read without
/// Blender (see `blend::read_preview` for their saved picture).
pub fn load(path: &Path, options: &LoadOptions) -> Result<Model, ModelError> {
    let extension = extension(path);
    if extension == "blend" {
        return Err(ModelError::Blender);
    }
    let Some(&(_, parse)) = FORMATS.iter().find(|(extensions, _)| extensions.contains(&extension.as_str())) else {
        return Err(ModelError::Unsupported(extension));
    };
    let bytes = std::fs::read(path)?;
    parse(&bytes, path, options)
}

/// The folder a model file is in, where what it refers to (textures, an OBJ's .mtl) is looked for.
fn folder(file: &Path) -> &Path {
    file.parent().unwrap_or(Path::new("."))
}

fn extension(path: &Path) -> String {
    path.extension().and_then(|ext| ext.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// Display colour (sRGB-encoded, as FBX, MTL and 3DS files store material colours) to linear.
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// Z-up (STL, PLY, 3DS, Blender-made Collada) to Y-up: a rotation about X, so winding is kept and
/// the model's front (-Y in Z-up tools) ends up facing +Z.
fn z_up_to_y_up(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[2], -p[1]]
}

/// Most formats count v up from the bottom of the picture; ours counts down from the top (glTF's
/// convention).
fn flip_v([u, v]: [f32; 2]) -> [f32; 2] {
    [u, 1.0 - v]
}

/// File references in glTF and Collada are URIs, which escape spaces and more as %XX.
fn percent_decode(uri: &str) -> String {
    let bytes = uri.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |at: usize| bytes.get(at).and_then(|&b| (b as char).to_digit(16));
        if bytes[i] == b'%'
            && let (Some(high), Some(low)) = (hex(i + 1), hex(i + 2))
        {
            out.push((high * 16 + low) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod test_files {
    //! Small things for the loader tests: a texture, a vertex.

    use super::Vertex;

    /// A vertex at `position`, facing +Z, white.
    pub fn vertex(position: [f32; 3]) -> Vertex {
        Vertex { position, normal: [0.0, 0.0, 1.0], uv: [0.0; 2], color: [1.0; 4] }
    }

    /// A 2 × 2 PNG: red, green / blue, white.
    pub fn png_2x2() -> Vec<u8> {
        let pixels = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255];
        let mut out = Vec::new();
        image::ImageEncoder::write_image(
            image::codecs::png::PngEncoder::new(&mut out),
            &pixels,
            2,
            2,
            image::ExtendedColorType::Rgba8,
        )
        .expect("encode test PNG");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::builder::Builder;
    use super::test_files::vertex;
    use super::*;
    use crate::asset::AssetKind;

    #[test]
    fn every_format_read_is_a_model_in_the_library() {
        for extension in FORMATS.iter().flat_map(|(extensions, _)| *extensions) {
            assert_eq!(AssetKind::from_extension(extension), Some(AssetKind::Model), ".{extension}");
        }
    }

    #[test]
    fn edges_count_a_shared_side_once() {
        // A square of two triangles: four sides and the diagonal.
        let corners = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
        let mut builder = Builder::default();
        let clay = builder.default_material(false);
        let base = builder.add_vertices(&corners.map(vertex));
        builder.add_triangles(clay, base, &[0, 1, 2, 0, 2, 3]);
        let model = builder.finish("square").unwrap();
        let mut edges: Vec<[u32; 2]> =
            model.edges().as_chunks::<2>().0.iter().map(|&[a, b]| [a.min(b), a.max(b)]).collect();
        edges.sort_unstable();
        assert_eq!(edges, [[0, 1], [0, 2], [0, 3], [1, 2], [2, 3]]);
    }

    #[test]
    fn messages_stay_as_the_thumbnail_cache_keeps_them() {
        let messages = [
            (ModelError::Blender, "Blender files can't be read without Blender, only their saved preview"),
            (ModelError::Unsupported("xyz".into()), "unsupported model format .xyz"),
            (
                ModelError::TooManyTriangles(5_300_000),
                "too many triangles to preview: 5.3 million (the limit is 4 million)",
            ),
            (ModelError::NoMeshes, "there are no meshes in this file"),
            (ModelError::NoTriangles, "there are no triangles to draw"),
            (ModelError::Skipped("only lines"), "only lines"),
            (ModelError::from("not a 3DS file"), "not a 3DS file"),
        ];
        for (error, message) in messages {
            assert_eq!(error.to_string(), message);
        }
    }

    #[test]
    fn srgb_round_trip() {
        for value in [0.0, 0.01, 0.2, 0.5, 0.9, 1.0] {
            assert!((linear_to_srgb(srgb_to_linear(value)) - value).abs() < 1e-5);
        }
    }
}
