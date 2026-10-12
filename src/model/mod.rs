//! 3D models: reads the common interchange formats into one simple form (`Model`) that both the
//! thumbnail renderer (`raster`) and the model viewer draw, and makes the grid's model thumbnails.
//!
//! Every loader produces the same space: right-handed, Y up, metres, the model's front facing +Z
//! (glTF's convention), with every node transform already applied, so nothing downstream looks at
//! the source format. FBX and OBJ are read by ufbx (`fbx`), glTF by the `gltf` crate (`gltf`); STL,
//! PLY, 3DS and Collada are small readers of our own. A .blend file can't be read without Blender,
//! but it carries the picture Blender saved with it (`blend`).

use std::path::Path;

use fast_image_resize as fr;

use crate::thumbs::{THUMB_MAX, ThumbPixels};

pub mod blend;
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

pub use textures::{TextureStats, texture_stats};

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

pub struct LoadOptions {
    /// Textures are scaled down to fit this many pixels on their longest side; 0 skips textures.
    pub max_texture: u32,
}

/// Longest side of the textures loaded for thumbnails. A model fills at most 192 pixels, so more
/// texture detail would only be averaged away.
pub const THUMB_TEXTURE: u32 = 256;

/// Models with more triangles than this aren't loaded. A vertex is 48 bytes and a faceted STL has
/// three per triangle, so this keeps one model under about 600 MB with a thumbnail made on every
/// core at once. The largest model in the test library has 151,000.
pub const MAX_TRIANGLES: u64 = 4_000_000;

fn check_triangle_count(triangles: u64) -> Result<(), String> {
    if triangles > MAX_TRIANGLES {
        let millions = triangles as f64 / 1e6;
        return Err(format!("too many triangles to preview: {millions:.1} million (the limit is 4 million)"));
    }
    Ok(())
}

/// Reads a model file. Returns an error for .blend files, whose geometry can't be read without
/// Blender (see `thumbnail` and `blend::read_preview` for their saved picture).
pub fn load(path: &Path, options: &LoadOptions) -> Result<Model, String> {
    match extension(path).as_str() {
        "fbx" | "obj" => fbx::load(path, options),
        "gltf" | "glb" => gltf::load(path, options),
        "stl" => stl::load(path),
        "ply" => ply::load(path, options),
        "3ds" => three_ds::load(path, options),
        "dae" => collada::load(path, options),
        "blend" => Err("Blender files can't be read without Blender, only their saved preview".into()),
        other => Err(format!("unsupported model format .{other}")),
    }
}

/// The grid thumbnail of a model file, `THUMB_MAX` square with a transparent background, and
/// what's in the file. For .blend files it's the picture Blender saved, cropped to what's drawn
/// (`blend::crop_to_content`) and scaled to fit.
///
/// Renders have `src_w` and `src_h` set to 0: they weren't scaled from a picture of known size.
pub fn thumbnail(path: &Path, resizer: &mut fr::Resizer) -> Result<(ThumbPixels, ModelInfo), String> {
    if extension(path) == "blend" {
        let preview = blend::read_preview(path)?;
        let (w, h, rgba) = blend::crop_to_content(&preview);
        let mut pixels = fit_picture(w, h, rgba, THUMB_MAX, resizer)?;
        (pixels.src_w, pixels.src_h) = (preview.width, preview.height);
        let info = ModelInfo { format: preview.format, ..ModelInfo::default() };
        return Ok((pixels, info));
    }
    let model = load(path, &LoadOptions { max_texture: THUMB_TEXTURE })?;
    let rgba = raster::render_thumbnail(&model, THUMB_MAX);
    Ok((ThumbPixels { w: THUMB_MAX, h: THUMB_MAX, src_w: 0, src_h: 0, rgba }, model.info))
}

fn extension(path: &Path) -> String {
    path.extension().and_then(|ext| ext.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// Scales a picture up or down so its longer side is `max`.
fn fit_picture(w: u32, h: u32, rgba: Vec<u8>, max: u32, resizer: &mut fr::Resizer) -> Result<ThumbPixels, String> {
    if w == 0 || h == 0 {
        return Err("the saved preview has no pixels".into());
    }
    let scale = max as f64 / w.max(h) as f64;
    let (fit_w, fit_h) = (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1));
    if (fit_w, fit_h) == (w, h) {
        return Ok(ThumbPixels { w, h, src_w: w, src_h: h, rgba });
    }
    let src = fr::images::Image::from_vec_u8(w, h, rgba, fr::PixelType::U8x4).map_err(|err| err.to_string())?;
    let mut dst = fr::images::Image::new(fit_w, fit_h, fr::PixelType::U8x4);
    let options = fr::ResizeOptions::new().resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::CatmullRom));
    resizer.resize(&src, &mut dst, &options).map_err(|err| err.to_string())?;
    Ok(ThumbPixels { w: fit_w, h: fit_h, src_w: w, src_h: h, rgba: dst.into_vec() })
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

/// A material's base-colour texture, as a loader found it.
#[derive(Clone, Copy)]
enum Slot {
    None,
    Missing,
    /// `cutout`: the texture's alpha is a mask to cut holes with (see `textures::Decoded`).
    Found {
        index: u32,
        cutout: bool,
    },
}

/// What a loader has read so far: vertices already in model space, triangles grouped by material.
#[derive(Default)]
struct Builder {
    vertices: Vec<Vertex>,
    /// Triangle indices into `vertices`, one list per material.
    triangles: Vec<Vec<u32>>,
    materials: Vec<Material>,
    textures: Vec<Texture>,
    /// Lazily made materials for geometry without one: [clay, white under vertex colours].
    defaults: [Option<u32>; 2],
    /// Materials made here rather than read from the file, left out of `ModelInfo::materials`.
    made_up: Vec<u32>,
    bones: Vec<[[f32; 3]; 2]>,
    meshes: u32,
    missing_textures: u32,
    /// The file's own vertex count, when it counts differently from us.
    source_vertices: Option<u64>,
    animations: Vec<(String, f32)>,
    /// Triangles offered to `add_triangles`, kept or not.
    added_triangles: u64,
}

impl Builder {
    fn add_material(&mut self, material: Material) -> u32 {
        self.materials.push(material);
        self.triangles.push(Vec::new());
        (self.materials.len() - 1) as u32
    }

    /// A material with a linear `color` and its texture as `texture_slot` gave it. If the colour
    /// was only white under a missing texture, clay is used instead: a pure white model shows no
    /// shape.
    fn material(&mut self, name: String, color: [f32; 4], texture: Slot) -> u32 {
        let (texture, alpha_cutoff, color) = match texture {
            Slot::None => (None, None, color),
            Slot::Found { index, cutout } => (Some(index), cutout.then_some(0.5), color),
            Slot::Missing => {
                let white = color[..3].iter().all(|&c| c > 0.95);
                let [r, g, b] = shading::DEFAULT_ALBEDO;
                (None, None, if white { [r, g, b, color[3]] } else { color })
            }
        };
        self.add_material(Material { name, color, texture, alpha_cutoff })
    }

    /// Adds a texture a loader decoded, or counts it as missing. Loaders keep the slot for each
    /// texture they've seen, so a shared texture is added and counted once.
    fn texture_slot(&mut self, decoded: Option<&textures::Decoded>) -> Slot {
        let Some(decoded) = decoded else {
            self.missing_textures += 1;
            return Slot::Missing;
        };
        self.textures.push(Texture {
            name: decoded.name.clone(),
            width: decoded.width,
            height: decoded.height,
            rgba: decoded.rgba.clone(),
        });
        Slot::Found { index: (self.textures.len() - 1) as u32, cutout: decoded.cutout }
    }

    /// The material for geometry the file gives none: clay, or white when vertex colours carry the
    /// colour.
    fn default_material(&mut self, vertex_colors: bool) -> u32 {
        let slot = usize::from(vertex_colors);
        if let Some(index) = self.defaults[slot] {
            return index;
        }
        let [r, g, b] = shading::DEFAULT_ALBEDO;
        let color = if vertex_colors { [1.0; 4] } else { [r, g, b, 1.0] };
        let index = self.add_material(Material { name: String::new(), color, texture: None, alpha_cutoff: None });
        self.defaults[slot] = Some(index);
        self.made_up.push(index);
        index
    }

    /// A white material wearing a texture the file doesn't name (`textures::companion`).
    fn companion_material(&mut self, texture: Slot) -> u32 {
        let index = self.material(String::new(), [1.0; 4], texture);
        self.made_up.push(index);
        index
    }

    /// Appends vertices; returns the index of the first, for `add_triangles`.
    fn add_vertices(&mut self, vertices: &[Vertex]) -> u32 {
        let base = self.vertices.len() as u32;
        // Past the triangle limit nothing more is kept; `finish` fails.
        if self.added_triangles <= MAX_TRIANGLES {
            self.vertices.extend_from_slice(vertices);
        }
        base
    }

    /// Adds triangles drawn with `material`; `indices` count from `base`. Past `MAX_TRIANGLES`
    /// they're dropped and `finish` fails.
    fn add_triangles(&mut self, material: u32, base: u32, indices: &[u32]) {
        self.added_triangles += (indices.len() / 3) as u64;
        if self.added_triangles > MAX_TRIANGLES {
            return;
        }
        let list = &mut self.triangles[material as usize];
        list.extend(indices.iter().map(|&index| base.saturating_add(index)));
    }

    fn triangle_count(&self) -> usize {
        self.triangles.iter().map(|list| list.len() / 3).sum()
    }

    /// Checks and packs everything into a `Model`. Triangles touching a vertex whose position
    /// isn't a finite number are dropped; other broken attributes get neutral values.
    fn finish(mut self, format: impl Into<String>) -> Result<Model, String> {
        check_triangle_count(self.added_triangles)?;
        let mut bad = vec![false; self.vertices.len()];
        for (vertex, bad) in self.vertices.iter_mut().zip(&mut bad) {
            *bad = !vertex.position.iter().all(|c| c.is_finite());
            if !vertex.normal.iter().all(|c| c.is_finite()) {
                vertex.normal = [0.0; 3];
            }
            if !vertex.uv.iter().all(|c| c.is_finite()) {
                vertex.uv = [0.0; 2];
            }
            if !vertex.color.iter().all(|c| c.is_finite()) {
                vertex.color = [1.0; 4];
            }
        }
        let mut indices = Vec::with_capacity(self.triangle_count() * 3);
        let mut parts = Vec::new();
        for (material, list) in self.triangles.iter().enumerate() {
            let first = indices.len();
            for triangle in list.as_chunks::<3>().0 {
                if triangle.iter().all(|&index| bad.get(index as usize) == Some(&false)) {
                    indices.extend_from_slice(triangle);
                }
            }
            if indices.len() > first {
                let count = (indices.len() - first) as u32;
                parts.push(Part { first: first as u32, count, material: material as u32 });
            }
        }
        let mut bounds = Bounds::EMPTY;
        for &index in &indices {
            bounds.add(self.vertices[index as usize].position);
        }
        self.bones.retain(|segment| segment.iter().flatten().all(|c| c.is_finite()));
        if indices.is_empty() {
            for &[a, b] in &self.bones {
                bounds.add(a);
                bounds.add(b);
            }
        } else {
            self.bones.clear();
        }
        if indices.is_empty() && self.bones.is_empty() {
            return Err("there are no triangles to draw".into());
        }

        let used_materials = parts.iter().filter(|part| !self.made_up.contains(&part.material)).count() as u32;
        let info = ModelInfo {
            format: format.into(),
            vertices: self.source_vertices.unwrap_or(self.vertices.len() as u64),
            triangles: (indices.len() / 3) as u64,
            meshes: self.meshes,
            materials: used_materials,
            textures: self.textures.len() as u32,
            missing_textures: self.missing_textures,
            size: bounds.size(),
            animations: self.animations,
            geometry: !indices.is_empty(),
        };
        Ok(Model {
            vertices: self.vertices,
            indices,
            parts,
            materials: self.materials,
            textures: self.textures,
            bones: self.bones,
            bounds,
            info,
        })
    }
}

#[cfg(test)]
mod test_files {
    //! Scratch folders for loader tests that need real files (external buffers, textures).

    use std::path::PathBuf;

    pub struct TempDir(pub PathBuf);

    impl TempDir {
        pub fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("asset_browser_{name}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create test folder");
            Self(dir)
        }

        pub fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create test subfolder");
            }
            std::fs::write(&path, bytes).expect("write test file");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Damaged copies of a good file, for parsers to survive: cut short at many points, and with
    /// bytes overwritten at random (a fixed seed, so a failure repeats).
    pub fn damaged(original: &[u8], copies: usize) -> Vec<Vec<u8>> {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut random = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut out: Vec<Vec<u8>> =
            (0..=original.len()).step_by(original.len() / 64 + 1).map(|len| original[..len].to_vec()).collect();
        for _ in 0..copies {
            let mut copy = original.to_vec();
            for _ in 0..1 + random() % 4 {
                let at = (random() % copy.len().max(1) as u64) as usize;
                if let Some(byte) = copy.get_mut(at) {
                    // Mostly extreme values, which find overflows; sometimes anything.
                    *byte = match random() % 3 {
                        0 => 0xff,
                        1 => 0x00,
                        _ => random() as u8,
                    };
                }
            }
            out.push(copy);
        }
        out
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
    use super::*;

    fn vertex(position: [f32; 3]) -> Vertex {
        Vertex { position, normal: [0.0, 0.0, 1.0], uv: [0.0; 2], color: [1.0; 4] }
    }

    #[test]
    fn builder_groups_by_material_and_drops_broken_triangles() {
        let mut builder = Builder::default();
        let red = builder.add_material(Material {
            name: "red".into(),
            color: [1.0, 0.0, 0.0, 1.0],
            texture: None,
            alpha_cutoff: None,
        });
        let clay = builder.default_material(false);
        let base = builder.add_vertices(&[
            vertex([0.0, 0.0, 0.0]),
            vertex([1.0, 0.0, 0.0]),
            vertex([0.0, 1.0, 0.0]),
            vertex([f32::NAN, 0.0, 0.0]),
            vertex([0.0, 0.0, 2.0]),
        ]);
        builder.add_triangles(clay, base, &[0, 1, 2]);
        builder.add_triangles(red, base, &[0, 2, 4, 0, 1, 3, 0, 1, 9]);
        builder.add_triangles(clay, base, &[1, 2, 4]);
        let model = builder.finish("test").unwrap();
        assert_eq!(model.indices, vec![0, 2, 4, 0, 1, 2, 1, 2, 4]);
        assert_eq!(
            model.parts,
            vec![Part { first: 0, count: 3, material: red }, Part { first: 3, count: 6, material: clay }]
        );
        assert_eq!(model.bounds, Bounds { min: [0.0; 3], max: [1.0, 1.0, 2.0] });
        assert_eq!(model.info.triangles, 3);
        assert_eq!(model.info.materials, 1, "the clay default isn't one of the file's materials");
        assert!(model.info.geometry);
    }

    #[test]
    fn builder_refuses_too_many_triangles() {
        let mut builder = Builder::default();
        let material = builder.default_material(false);
        let base = builder.add_vertices(&[vertex([0.0; 3]), vertex([1.0, 0.0, 0.0]), vertex([0.0, 1.0, 0.0])]);
        let many = [0u32, 1, 2].repeat((MAX_TRIANGLES / 2 + 1) as usize);
        builder.add_triangles(material, base, &many);
        builder.add_triangles(material, base, &many);
        assert_eq!(builder.vertices.len(), 3);
        assert!(builder.finish("test").err().unwrap().contains("too many triangles"));
    }

    #[test]
    fn builder_needs_something_to_draw() {
        assert!(Builder::default().finish("test").is_err());
        let mut bones = Builder::default();
        bones.bones.push([[0.0; 3], [0.0, 1.0, 0.0]]);
        let model = bones.finish("test").unwrap();
        assert!(!model.info.geometry);
        assert_eq!(model.info.size, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn missing_texture_on_white_falls_back_to_clay() {
        let mut builder = Builder::default();
        let slot = builder.texture_slot(None);
        let index = builder.material("m".into(), [1.0; 4], slot);
        let material = &builder.materials[index as usize];
        let [r, g, b] = shading::DEFAULT_ALBEDO;
        assert_eq!(material.color, [r, g, b, 1.0]);
        assert_eq!(material.alpha_cutoff, None);
        assert_eq!(builder.missing_textures, 1);
        let red = builder.material("red".into(), [1.0, 0.0, 0.0, 1.0], slot);
        assert_eq!(builder.materials[red as usize].color, [1.0, 0.0, 0.0, 1.0], "colour kept");
    }

    #[test]
    fn srgb_round_trip() {
        for value in [0.0, 0.01, 0.2, 0.5, 0.9, 1.0] {
            assert!((linear_to_srgb(srgb_to_linear(value)) - value).abs() < 1e-5);
        }
    }
}
