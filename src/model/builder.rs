//! What every reader builds its model with: vertices already in model space, triangles grouped by
//! material, and what else the file says (meshes, bones, animations). `Builder::finish` checks
//! it all and packs it into a `Model`.

use std::collections::HashMap;
use std::path::Path;

use super::textures::{self, Alpha};
use super::{
    Bounds, LoadOptions, MAX_TRIANGLES, Material, Model, ModelError, ModelInfo, Part, Texture, Vertex,
    check_triangle_count,
};

/// Alpha below this is a hole in a cut-out material: glTF's default, and what a texture whose alpha
/// looks like a mask gets.
pub(super) const CUTOUT_ALPHA: f32 = 0.5;

/// A colour whose channels are all above this is white: what exporters leave under a texture.
const NEARLY_WHITE: f32 = 0.95;

/// A material's base-colour texture, as a loader found it.
#[derive(Clone, Copy)]
pub(super) enum Slot {
    None,
    Missing,
    /// `cutout`: the texture's alpha is a mask to cut holes with (see `textures::Decoded`).
    Found {
        index: u32,
        cutout: bool,
    },
}

/// The texture files a model names, looked for beside it and read once each, however often they're
/// named.
pub(super) struct TextureFiles<'a> {
    /// The folder the model is in.
    dir: &'a Path,
    /// `LoadOptions::max_texture`: none are read when it's 0.
    max_texture: u32,
    /// Each name, as the model gives it, and what became of it.
    read: HashMap<String, Slot>,
}

impl<'a> TextureFiles<'a> {
    pub(super) fn new(file: &'a Path, options: &LoadOptions) -> Self {
        Self { dir: super::folder(file), max_texture: options.max_texture, read: HashMap::new() }
    }

    /// The texture file `name`: added to `builder` (or counted as missing) the first time it's
    /// named, `Slot::None` when textures aren't read.
    pub(super) fn slot(&mut self, builder: &mut Builder, name: &str) -> Slot {
        if self.max_texture == 0 {
            return Slot::None;
        }
        if let Some(&slot) = self.read.get(name) {
            return slot;
        }
        let decoded = textures::from_file(self.dir, name, self.max_texture, Alpha::Guess);
        let slot = builder.texture_slot(decoded.as_deref());
        self.read.insert(name.to_owned(), slot);
        slot
    }
}

/// What a loader has read so far: vertices already in model space, triangles grouped by material.
#[derive(Default)]
pub(super) struct Builder {
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
    /// Why some of the file was left out: the error when nothing else is read.
    skipped: Option<&'static str>,
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
    pub(super) fn material(&mut self, name: String, color: [f32; 4], texture: Slot) -> u32 {
        let (texture, alpha_cutoff, color) = match texture {
            Slot::None => (None, None, color),
            Slot::Found { index, cutout } => (Some(index), cutout.then_some(CUTOUT_ALPHA), color),
            Slot::Missing => {
                let white = color[..3].iter().all(|&c| c > NEARLY_WHITE);
                let [r, g, b, _] = Material::CLAY.color;
                (None, None, if white { [r, g, b, color[3]] } else { color })
            }
        };
        self.add_material(Material { name, color, texture, alpha_cutoff })
    }

    /// Adds a texture a loader decoded, or counts it as missing. Loaders keep the slot for each
    /// texture they've seen, so a shared texture is added and counted once.
    pub(super) fn texture_slot(&mut self, decoded: Option<&textures::Decoded>) -> Slot {
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
    pub(super) fn default_material(&mut self, vertex_colors: bool) -> u32 {
        let slot = usize::from(vertex_colors);
        if let Some(index) = self.defaults[slot] {
            return index;
        }
        let material = if vertex_colors { Material { color: [1.0; 4], ..Material::CLAY } } else { Material::CLAY };
        let index = self.add_material(material);
        self.defaults[slot] = Some(index);
        self.made_up.push(index);
        index
    }

    /// A white material wearing a texture the file doesn't name (`textures::companion`).
    pub(super) fn companion_material(&mut self, texture: Slot) -> u32 {
        let index = self.material(String::new(), [1.0; 4], texture);
        self.made_up.push(index);
        index
    }

    /// Sets how a material's alpha is used, where the file says (glTF's alpha mode) rather than it
    /// being guessed from the texture.
    pub(super) fn set_alpha_cutoff(&mut self, material: u32, cutoff: Option<f32>) {
        self.materials[material as usize].alpha_cutoff = cutoff;
    }

    /// One more of the file's meshes read, for `ModelInfo::meshes`.
    pub(super) fn count_mesh(&mut self) {
        self.meshes += 1;
    }

    /// The file's own vertex count, shown rather than the number of vertices drawn.
    pub(super) fn set_source_vertices(&mut self, count: u64) {
        self.source_vertices = Some(count);
    }

    /// A skeleton segment, from a joint to its child: drawn when the file has no triangles.
    pub(super) fn add_bone(&mut self, from: [f32; 3], to: [f32; 3]) {
        self.bones.push([from, to]);
    }

    /// The file's animations: their names and lengths in seconds.
    pub(super) fn set_animations(&mut self, animations: Vec<(String, f32)>) {
        self.animations = animations;
    }

    /// Notes why some of the file's geometry was left out, for the error when nothing else is
    /// read.
    pub(super) fn skip(&mut self, reason: &'static str) {
        self.skipped = Some(reason);
    }

    /// Appends vertices; returns the index of the first, for `add_triangles`.
    pub(super) fn add_vertices(&mut self, vertices: &[Vertex]) -> u32 {
        let base = self.vertices.len() as u32;
        // Past the triangle limit nothing more is kept; `finish` fails.
        if self.added_triangles <= MAX_TRIANGLES {
            self.vertices.extend_from_slice(vertices);
        }
        base
    }

    /// Adds triangles drawn with `material`; `indices` count from `base`. Past `MAX_TRIANGLES`
    /// they're dropped and `finish` fails.
    pub(super) fn add_triangles(&mut self, material: u32, base: u32, indices: &[u32]) {
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
    pub(super) fn finish(mut self, format: impl Into<String>) -> Result<Model, ModelError> {
        check_triangle_count(self.added_triangles)?;
        if self.triangle_count() == 0 && self.bones.is_empty() {
            return Err(self.skipped.map_or(ModelError::NoMeshes, ModelError::Skipped));
        }
        let unusable = self.repair_vertices();
        let (indices, parts) = self.usable_triangles(&unusable);
        // The skeleton is drawn only when there's nothing else.
        self.bones.retain(|segment| segment.iter().flatten().all(|c| c.is_finite()));
        if !indices.is_empty() {
            self.bones.clear();
        }
        if indices.is_empty() && self.bones.is_empty() {
            return Err(ModelError::NoTriangles);
        }
        let bounds = self.bounds(&indices);
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

    /// Gives broken normals, UVs and colours neutral values, and marks the vertices whose position
    /// is broken: those can't be drawn.
    fn repair_vertices(&mut self) -> Vec<bool> {
        let mut unusable = Vec::with_capacity(self.vertices.len());
        for vertex in &mut self.vertices {
            unusable.push(!vertex.position.iter().all(|c| c.is_finite()));
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
        unusable
    }

    /// The triangles that can be drawn, as one list with a part per material, in material order.
    fn usable_triangles(&self, unusable: &[bool]) -> (Vec<u32>, Vec<Part>) {
        let mut indices = Vec::with_capacity(self.triangle_count() * 3);
        let mut parts = Vec::new();
        for (material, list) in self.triangles.iter().enumerate() {
            let first = indices.len();
            for triangle in list.as_chunks::<3>().0 {
                if triangle.iter().all(|&index| unusable.get(index as usize) == Some(&false)) {
                    indices.extend_from_slice(triangle);
                }
            }
            if indices.len() > first {
                let count = (indices.len() - first) as u32;
                parts.push(Part { first: first as u32, count, material: material as u32 });
            }
        }
        (indices, parts)
    }

    /// The box around what's drawn: the triangles, or else the skeleton.
    fn bounds(&self, indices: &[u32]) -> Bounds {
        let mut bounds = Bounds::EMPTY;
        for &index in indices {
            bounds.add(self.vertices[index as usize].position);
        }
        for &[a, b] in &self.bones {
            bounds.add(a);
            bounds.add(b);
        }
        bounds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::test_files::{png_2x2, vertex};
    use crate::test_support::TempDir;

    #[test]
    fn a_texture_file_is_read_once_however_often_it_is_named() {
        let dir = TempDir::new("texture_files");
        dir.write("wood.png", &png_2x2());
        let model = dir.path().join("crate.3ds");
        let mut builder = Builder::default();
        let mut files = TextureFiles::new(&model, &LoadOptions { max_texture: 64 });
        for _ in 0..2 {
            assert!(matches!(files.slot(&mut builder, "wood.png"), Slot::Found { index: 0, .. }));
            assert!(matches!(files.slot(&mut builder, "gone.png"), Slot::Missing));
        }
        assert_eq!((builder.textures.len(), builder.missing_textures), (1, 1));
        let mut without = TextureFiles::new(&model, &LoadOptions::default());
        assert!(matches!(without.slot(&mut builder, "wood.png"), Slot::None));
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
        assert!(matches!(builder.finish("test"), Err(ModelError::TooManyTriangles(_))));

        // All in one go, so none were kept: still too many, not nothing to draw.
        let mut builder = Builder::default();
        let material = builder.default_material(false);
        let base = builder.add_vertices(&[vertex([0.0; 3]), vertex([1.0, 0.0, 0.0]), vertex([0.0, 1.0, 0.0])]);
        builder.add_triangles(material, base, &[many.as_slice(), &many].concat());
        assert!(matches!(builder.finish("test"), Err(ModelError::TooManyTriangles(_))));
    }

    #[test]
    fn builder_needs_something_to_draw() {
        assert!(Builder::default().finish("test").is_err());
        let mut bones = Builder::default();
        bones.add_bone([0.0; 3], [0.0, 1.0, 0.0]);
        let model = bones.finish("test").unwrap();
        assert!(!model.info.geometry);
        assert_eq!(model.info.size, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn a_skeleton_is_drawn_only_without_triangles() {
        let mut builder = Builder::default();
        let clay = builder.default_material(false);
        let base = builder.add_vertices(&[vertex([0.0; 3]), vertex([1.0, 0.0, 0.0]), vertex([0.0, 1.0, 0.0])]);
        builder.add_triangles(clay, base, &[0, 1, 2]);
        builder.add_bone([0.0; 3], [0.0, 5.0, 0.0]);
        let model = builder.finish("test").unwrap();
        assert!(model.bones.is_empty());
        assert_eq!(model.bounds.max, [1.0, 1.0, 0.0], "nor are the bones in the bounds");
    }

    #[test]
    fn missing_texture_on_white_falls_back_to_clay() {
        let mut builder = Builder::default();
        let slot = builder.texture_slot(None);
        let index = builder.material("m".into(), [1.0; 4], slot);
        let material = &builder.materials[index as usize];
        assert_eq!(material.color, Material::CLAY.color);
        assert_eq!(material.alpha_cutoff, None);
        assert_eq!(builder.missing_textures, 1);
        let red = builder.material("red".into(), [1.0, 0.0, 0.0, 1.0], slot);
        assert_eq!(builder.materials[red as usize].color, [1.0, 0.0, 0.0, 1.0], "colour kept");
    }

    #[test]
    fn nothing_read_says_why() {
        assert!(matches!(Builder::default().finish("test"), Err(ModelError::NoMeshes)));
        let mut skipped = Builder::default();
        skipped.skip("there are only points or lines, no surfaces to draw");
        assert_eq!(
            skipped.finish("test").err().unwrap().to_string(),
            "there are only points or lines, no surfaces to draw"
        );
        let mut broken = Builder::default();
        let clay = broken.default_material(false);
        let base = broken.add_vertices(&[vertex([f32::NAN; 3]), vertex([0.0; 3]), vertex([1.0, 0.0, 0.0])]);
        broken.add_triangles(clay, base, &[0, 1, 2]);
        assert!(matches!(broken.finish("test"), Err(ModelError::NoTriangles)));
    }
}
