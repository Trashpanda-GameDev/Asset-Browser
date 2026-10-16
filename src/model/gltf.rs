//! glTF 2.0: the .gltf JSON form, with its buffers in files beside it or embedded as base64
//! `data:` URIs, and the binary .glb. The `gltf` crate parses the document; buffers and images are
//! resolved here, without its `import` feature. glTF is already in our space (Y up, metres, front
//! +Z, linear colours), so the work is walking the node tree.
//!
//! Every accessor is checked against its buffer before it's read: the crate trusts the file and
//! would panic on data that runs past the end.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use ::gltf::accessor::{DataType, Dimensions};
use ::gltf::mesh::Mode;
use ::gltf::{Accessor, Document, Semantic};

use super::geometry::{self, IDENTITY, Mat4, NormalMatrix, normalize};
use super::textures::{self, Alpha};
use super::{Builder, LoadOptions, Model, Slot, Vertex, percent_decode};

pub(super) fn load(path: &Path, options: &LoadOptions) -> Result<Model, String> {
    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    load_bytes(&bytes, path.parent().unwrap_or(Path::new(".")), options)
}

fn load_bytes(bytes: &[u8], dir: &Path, options: &LoadOptions) -> Result<Model, String> {
    let ::gltf::Gltf { document, mut blob } =
        ::gltf::Gltf::from_slice_without_validation(bytes).map_err(|err| format!("glTF: {err}"))?;
    check(&document)?;
    let buffers: Vec<Option<Vec<u8>>> = document
        .buffers()
        .map(|buffer| {
            let data = match buffer.source() {
                ::gltf::buffer::Source::Bin => blob.take(),
                ::gltf::buffer::Source::Uri(uri) => read_uri(uri, dir),
            };
            data.filter(|data| data.len() >= buffer.length())
        })
        .collect();
    let world = world_matrices(&document);

    let mut reader = Reader {
        root: document.as_json(),
        buffers: &buffers,
        dir,
        max_texture: options.max_texture,
        builder: Builder::default(),
        materials: HashMap::new(),
        images: HashMap::new(),
        skipped: None,
    };
    for node in document.nodes() {
        if let (Some(mesh), Some(matrix)) = (node.mesh(), world[node.index()]) {
            reader.add_mesh(&node, &mesh, &matrix, &world);
        }
    }
    let skipped = reader.skipped;
    let mut builder = reader.builder;

    if builder.triangle_count() == 0 {
        // Only a skeleton: draw the bones between joints.
        for skin in document.skins() {
            let joints: Vec<usize> = skin.joints().map(|joint| joint.index()).collect();
            for joint in skin.joints() {
                for child in joint.children().filter(|child| joints.contains(&child.index())) {
                    if let (Some(a), Some(b)) = (world[joint.index()], world[child.index()]) {
                        builder.bones.push([translation(&a), translation(&b)]);
                    }
                }
            }
        }
        if builder.bones.is_empty() {
            return Err(skipped.unwrap_or("there are no meshes in this file").into());
        }
    }
    builder.animations = document
        .animations()
        .map(|animation| {
            let name = animation.name().map_or_else(|| format!("Animation {}", animation.index() + 1), str::to_owned);
            let seconds = animation
                .channels()
                .filter_map(|channel| channel.sampler().input().max())
                .filter_map(|max| max.as_array()?.first()?.as_f64())
                .fold(0.0, f64::max);
            (name, seconds as f32)
        })
        .collect();
    let format = if bytes.starts_with(b"glTF") { "glTF 2.0 (GLB)" } else { "glTF 2.0" };
    builder.finish(format)
}

/// The crate's validation, minus what doesn't stop us drawing: positions missing their min/max
/// (bounds are computed from the data anyway), required extensions the crate doesn't know, and
/// textures whose picture only an extension gives (KTX2), which are then counted as missing.
fn check(document: &Document) -> Result<(), String> {
    use ::gltf::json::validation::Validate;
    let root = document.as_json();
    // The crate's own check of POSITION indexes the accessors before checking the index.
    let accessors = root.accessors.len();
    let attributes =
        root.meshes.iter().flat_map(|mesh| &mesh.primitives).flat_map(|primitive| primitive.attributes.values());
    if attributes.into_iter().any(|accessor| accessor.value() >= accessors) {
        return Err("glTF: a mesh names an accessor that isn't there".into());
    }
    let mut errors = Vec::new();
    root.validate(root, ::gltf::json::Path::new, &mut |path, error| errors.push((path(), error)));
    errors.retain(|(path, _)| {
        let path = path.as_str();
        let textureless = path.starts_with("textures[") && path.ends_with("].source");
        !(path.ends_with(".min") || path.ends_with(".max") || path.starts_with("extensionsRequired") || textureless)
    });
    if let Some((path, error)) = errors.first() {
        return Err(format!("glTF: {error} at {path}"));
    }
    for extension in document.extensions_required() {
        if matches!(extension, "KHR_draco_mesh_compression" | "EXT_meshopt_compression") {
            return Err(format!("glTF: the geometry is compressed with {extension}, which isn't supported"));
        }
    }
    Ok(())
}

/// Every node's transform to model space, or `None` for nodes outside the scene drawn: the
/// default scene, else the first, else every node that isn't another's child.
fn world_matrices(document: &Document) -> Vec<Option<Mat4>> {
    let count = document.nodes().count();
    let roots: Vec<::gltf::Node> = if let Some(scene) = document.default_scene().or_else(|| document.scenes().next()) {
        scene.nodes().collect()
    } else {
        let mut is_child = vec![false; count];
        for child in document.nodes().flat_map(|node| node.children()) {
            is_child[child.index()] = true;
        }
        document.nodes().filter(|node| !is_child[node.index()]).collect()
    };
    let mut world = vec![None; count];
    let mut stack: Vec<(::gltf::Node, Mat4)> = roots.into_iter().map(|node| (node, IDENTITY)).collect();
    while let Some((node, parent)) = stack.pop() {
        // Visited already: a broken file's cycle, or a node listed twice.
        if world[node.index()].is_some() {
            continue;
        }
        let matrix = geometry::mul(&parent, &node.transform().matrix());
        world[node.index()] = Some(matrix);
        stack.extend(node.children().map(|child| (child, matrix)));
    }
    world
}

fn translation(m: &Mat4) -> [f32; 3] {
    [m[3][0], m[3][1], m[3][2]]
}

struct Reader<'a> {
    /// The document as parsed, for what the crate's accessors would unwrap without checking.
    root: &'a ::gltf::json::Root,
    buffers: &'a [Option<Vec<u8>>],
    dir: &'a Path,
    max_texture: u32,
    builder: Builder,
    /// glTF material index → ours.
    materials: HashMap<usize, u32>,
    /// glTF image index (or `usize::MAX` for a texture with no picture) → what became of it.
    images: HashMap<usize, Slot>,
    /// Why geometry was left out, for the error when nothing is left.
    skipped: Option<&'static str>,
}

/// The data of each buffer, for the crate's readers.
fn buffer_data<'a>(buffers: &'a [Option<Vec<u8>>]) -> impl Clone + Fn(::gltf::Buffer) -> Option<&'a [u8]> {
    move |buffer| buffers.get(buffer.index())?.as_deref()
}

impl<'a> Reader<'a> {
    fn buffer(&self, index: usize) -> Option<&'a [u8]> {
        self.buffers.get(index)?.as_deref()
    }

    fn add_mesh(&mut self, node: &::gltf::Node, mesh: &::gltf::Mesh, world: &Mat4, all_world: &[Option<Mat4>]) {
        let skin = node.skin().and_then(|skin| self.joint_matrices(&skin, all_world));
        let mut drawn = false;
        for primitive in mesh.primitives() {
            if !matches!(primitive.mode(), Mode::Triangles | Mode::TriangleStrip | Mode::TriangleFan) {
                self.skipped = Some("there are only points or lines, no surfaces to draw");
                continue;
            }
            let Some(vertices) = self.read_vertices(&primitive, world, skin.as_deref()) else {
                self.skipped = Some("the geometry is stored in a way this app can't read");
                continue;
            };
            let count = vertices.len() as u32;
            let index_types = [DataType::U8, DataType::U16, DataType::U32];
            let indices: Vec<u32> = match primitive.indices() {
                Some(accessor) if self.readable(&accessor, &index_types, &[Dimensions::Scalar]) => {
                    match primitive.reader(buffer_data(self.buffers)).read_indices() {
                        Some(indices) => indices.into_u32().collect(),
                        None => continue,
                    }
                }
                Some(_) => {
                    self.skipped = Some("the geometry is stored in a way this app can't read");
                    continue;
                }
                None => (0..count).collect(),
            };
            let mut triangles = triangle_list(primitive.mode(), &indices);
            triangles.retain(|triangle| triangle.iter().all(|&index| index < count));
            if skin.is_none() && geometry::determinant(world) < 0.0 {
                // Mirrored: wind the other way so front faces stay counter-clockwise.
                for triangle in &mut triangles {
                    triangle.swap(1, 2);
                }
            }
            let mut indices: Vec<u32> = triangles.into_iter().flatten().collect();
            let mut vertices = vertices;
            if !self.has_normals(&primitive) {
                // The spec asks for flat shading when normals are missing.
                (vertices, indices) = geometry::flat_normals(&vertices, &indices);
            }
            let colors = primitive.get(&Semantic::Colors(0)).is_some();
            let material = self.material(&primitive.material(), colors);
            let base = self.builder.add_vertices(&vertices);
            self.builder.add_triangles(material, base, &indices);
            drawn = true;
        }
        if drawn {
            self.builder.meshes += 1;
        }
    }

    /// The accessor has a layout we read and lies inside its buffer, so reading it can't panic.
    fn readable(&self, accessor: &Accessor, types: &[DataType], dimensions: &[Dimensions]) -> bool {
        if !types.contains(&accessor.data_type())
            || !dimensions.contains(&accessor.dimensions())
            || accessor.count() == 0
        {
            return false;
        }
        if accessor.sparse().is_some() {
            // Sparse data patches a base; rarely used outside morph targets.
            return false;
        }
        let Some(view) = accessor.view() else { return false };
        let Some(buffer) = self.buffer(view.buffer().index()) else { return false };
        let size = accessor.size();
        let stride = view.stride().unwrap_or(size);
        let view_fits = view.offset().checked_add(view.length()).is_some_and(|end| end <= buffer.len());
        let end = (accessor.count() - 1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(accessor.offset()))
            .and_then(|n| n.checked_add(size));
        stride >= size && view_fits && end.is_some_and(|end| end <= view.length())
    }

    fn has_normals(&self, primitive: &::gltf::Primitive) -> bool {
        primitive
            .get(&Semantic::Normals)
            .is_some_and(|accessor| self.readable(&accessor, &[DataType::F32], &[Dimensions::Vec3]))
    }

    /// The primitive's vertices in model space, skinned to the rest pose when a skin is given.
    fn read_vertices(&self, primitive: &::gltf::Primitive, world: &Mat4, skin: Option<&[Mat4]>) -> Option<Vec<Vertex>> {
        let positions = primitive.get(&Semantic::Positions)?;
        if !self.readable(&positions, &[DataType::F32], &[Dimensions::Vec3]) {
            return None;
        }
        let reader = primitive.reader(buffer_data(self.buffers));
        let positions: Vec<[f32; 3]> = reader.read_positions()?.collect();
        let count = positions.len();
        let attribute_ok = |semantic: Semantic, types: &[DataType], dimensions: &[Dimensions]| {
            primitive
                .get(&semantic)
                .is_some_and(|accessor| accessor.count() == count && self.readable(&accessor, types, dimensions))
        };
        let floats = [DataType::F32, DataType::U8, DataType::U16];
        let normals: Option<Vec<[f32; 3]>> = if attribute_ok(Semantic::Normals, &[DataType::F32], &[Dimensions::Vec3]) {
            reader.read_normals().map(Iterator::collect)
        } else {
            None
        };
        let material = primitive.material();
        let uv_set = material.pbr_metallic_roughness().base_color_texture().map_or(0, |info| info.tex_coord());
        let uvs: Option<Vec<[f32; 2]>> = if attribute_ok(Semantic::TexCoords(uv_set), &floats, &[Dimensions::Vec2]) {
            reader.read_tex_coords(uv_set).map(|uvs| uvs.into_f32().collect())
        } else {
            None
        };
        let colors: Option<Vec<[f32; 4]>> =
            if attribute_ok(Semantic::Colors(0), &floats, &[Dimensions::Vec3, Dimensions::Vec4]) {
                reader.read_colors(0).map(|colors| colors.into_rgba_f32().collect())
            } else {
                None
            };
        let skinning = skin.and_then(|joints| {
            let ok = attribute_ok(Semantic::Joints(0), &[DataType::U8, DataType::U16], &[Dimensions::Vec4])
                && attribute_ok(Semantic::Weights(0), &floats, &[Dimensions::Vec4]);
            let indices: Vec<[u16; 4]> = ok.then(|| reader.read_joints(0)).flatten()?.into_u16().collect();
            let weights: Vec<[f32; 4]> = reader.read_weights(0)?.into_f32().collect();
            Some((joints, indices, weights))
        });

        let world_normals = NormalMatrix::new(world);
        let vertices = (0..count)
            .map(|i| {
                let skinned = skinning
                    .as_ref()
                    .and_then(|(joints, indices, weights)| blend_joints(joints, *indices.get(i)?, *weights.get(i)?));
                let (matrix, to_normal) = match skinned {
                    Some(matrix) => (matrix, NormalMatrix::new(&matrix)),
                    None => (*world, world_normals),
                };
                Vertex {
                    position: geometry::transform_point(&matrix, positions[i]),
                    normal: normals.as_ref().map_or([0.0; 3], |normals| normalize(to_normal.apply(normals[i]))),
                    uv: uvs.as_ref().map_or([0.0; 2], |uvs| uvs[i]),
                    color: colors.as_ref().map_or([1.0; 4], |colors| colors[i]),
                }
            })
            .collect();
        Some(vertices)
    }

    /// Each joint's transform from the bind pose to where the node tree puts it now: for the
    /// skeleton's rest pose, the mesh as it was bound.
    fn joint_matrices(&self, skin: &::gltf::Skin, world: &[Option<Mat4>]) -> Option<Vec<Mat4>> {
        let joints: Vec<::gltf::Node> = skin.joints().collect();
        let inverse_bind: Vec<Mat4> = match skin.inverse_bind_matrices() {
            Some(accessor) => {
                if !self.readable(&accessor, &[DataType::F32], &[Dimensions::Mat4]) || accessor.count() < joints.len() {
                    return None;
                }
                skin.reader(buffer_data(self.buffers)).read_inverse_bind_matrices()?.collect()
            }
            None => vec![IDENTITY; joints.len()],
        };
        joints
            .iter()
            .zip(&inverse_bind)
            .map(|(joint, inverse)| Some(geometry::mul(&world[joint.index()]?, inverse)))
            .collect()
    }

    fn material(&mut self, material: &::gltf::Material, vertex_colors: bool) -> u32 {
        let Some(index) = material.index() else { return self.builder.default_material(vertex_colors) };
        if let Some(&ours) = self.materials.get(&index) {
            return ours;
        }
        let pbr = material.pbr_metallic_roughness();
        let slot = match pbr.base_color_texture() {
            Some(info) => {
                let texture = self.root.textures.get(info.texture().index());
                // A texture whose picture only an extension gives has no index here.
                let image =
                    texture.map(|texture| texture.source.value()).filter(|&image| image < self.root.images.len());
                self.image(image.unwrap_or(usize::MAX))
            }
            None => Slot::None,
        };
        let name = material.name().unwrap_or_default().to_owned();
        let ours = self.builder.material(name, pbr.base_color_factor(), slot);
        // glTF says how alpha is used, rather than leaving it to be guessed from the texture.
        self.builder.materials[ours as usize].alpha_cutoff = match material.alpha_mode() {
            ::gltf::material::AlphaMode::Opaque => None,
            ::gltf::material::AlphaMode::Mask => Some(material.alpha_cutoff().unwrap_or(0.5)),
            // Blending isn't drawn; a cutout keeps foliage and hair cards readable.
            ::gltf::material::AlphaMode::Blend => Some(0.5),
        };
        self.materials.insert(index, ours);
        ours
    }

    /// Image `index` of the document, read from its JSON: the crate's `Image::source` unwraps a
    /// MIME type that exporters sometimes leave out.
    fn image(&mut self, index: usize) -> Slot {
        if self.max_texture == 0 {
            return Slot::None;
        }
        if let Some(&slot) = self.images.get(&index) {
            return slot;
        }
        let image = self.root.images.get(index);
        let name =
            image.and_then(|image| image.name.clone()).unwrap_or_else(|| format!("image {}", index.saturating_add(1)));
        let embedded = |bytes: &[u8]| textures::from_bytes(bytes, &name, self.max_texture, Alpha::Keep).map(Arc::new);
        let decoded: Option<Arc<textures::Decoded>> = match image {
            Some(image) if image.buffer_view.is_some() => {
                let view = image.buffer_view.and_then(|view| self.root.buffer_views.get(view.value()));
                let bytes = view.and_then(|view| {
                    let start = usize::try_from(view.byte_offset.map_or(0, |offset| offset.0)).ok()?;
                    let end = start.checked_add(usize::try_from(view.byte_length.0).ok()?)?;
                    self.buffer(view.buffer.value())?.get(start..end)
                });
                bytes.and_then(embedded)
            }
            Some(image) => match image.uri.as_deref() {
                Some(uri) => match data_uri(uri) {
                    Some(bytes) => bytes.as_deref().and_then(embedded),
                    None if !uri.contains("://") => {
                        textures::from_file(self.dir, &percent_decode(uri), self.max_texture, Alpha::Keep)
                    }
                    None => None,
                },
                None => None,
            },
            None => None,
        };
        let slot = self.builder.texture_slot(decoded.as_deref());
        self.images.insert(index, slot);
        slot
    }
}

/// Σ weightᵢ × joint matrixᵢ, or `None` when the weights are all zero.
fn blend_joints(joints: &[Mat4], indices: [u16; 4], weights: [f32; 4]) -> Option<Mat4> {
    let total: f32 = weights.iter().sum();
    if total.is_nan() || total <= 1e-6 {
        return None;
    }
    let mut out = [[0.0; 4]; 4];
    for (&joint, &weight) in indices.iter().zip(&weights) {
        let matrix = joints.get(joint as usize)?;
        for (out_column, column) in out.iter_mut().zip(matrix) {
            for (out, value) in out_column.iter_mut().zip(column) {
                *out += value * weight / total;
            }
        }
    }
    Some(out)
}

/// Strips and fans as a plain triangle list. Strips alternate their winding, so every other
/// triangle is turned back.
fn triangle_list(mode: Mode, indices: &[u32]) -> Vec<[u32; 3]> {
    match mode {
        Mode::TriangleStrip => indices
            .windows(3)
            .enumerate()
            .map(|(i, w)| if i % 2 == 0 { [w[0], w[1], w[2]] } else { [w[1], w[0], w[2]] })
            .collect(),
        Mode::TriangleFan => match indices.split_first() {
            Some((&center, rest)) => rest.windows(2).map(|w| [center, w[0], w[1]]).collect(),
            None => Vec::new(),
        },
        _ => indices.as_chunks::<3>().0.to_vec(),
    }
}

/// The bytes of a base64 `data:` URI; `None` when `uri` isn't one, `Some(None)` when it's broken.
fn data_uri(uri: &str) -> Option<Option<Vec<u8>>> {
    let rest = uri.strip_prefix("data:")?;
    let (kind, data) = rest.split_once(',')?;
    Some(if kind.ends_with(";base64") { base64(data) } else { None })
}

/// A buffer in a `data:` URI or a file beside the .gltf.
fn read_uri(uri: &str, dir: &Path) -> Option<Vec<u8>> {
    match data_uri(uri) {
        Some(bytes) => bytes,
        None if uri.contains("://") => None,
        None => std::fs::read(dir.join(percent_decode(uri))).ok(),
    }
}

fn base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut bits, mut count) = (0u32, 0u32);
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::test_files::png_2x2;
    use crate::test_support::TempDir;

    /// A unit quad in the XY plane facing +Z, as two triangles: positions, then normals, then UVs,
    /// then u16 indices.
    fn quad_buffer() -> Vec<u8> {
        let mut out = Vec::new();
        for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]] {
            out.extend(p.iter().flat_map(|c| c.to_le_bytes()));
        }
        for _ in 0..4 {
            out.extend([0.0f32, 0.0, 1.0].iter().flat_map(|c| c.to_le_bytes()));
        }
        for uv in [[0.0f32, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]] {
            out.extend(uv.iter().flat_map(|c| c.to_le_bytes()));
        }
        for index in [0u16, 1, 2, 0, 2, 3] {
            out.extend(index.to_le_bytes());
        }
        out
    }

    /// The document for `quad_buffer`, with `buffer` as the buffer's JSON and `extra` spliced
    /// into the root object (materials, images and so on).
    fn quad_json(buffer: &str, node: &str, material: &str, extra: &str) -> String {
        format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],
            "nodes":[{{"mesh":0{node}}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2}},"indices":3{material}}}]}}],
            "buffers":[{buffer}],
            "bufferViews":[
                {{"buffer":0,"byteOffset":0,"byteLength":48}},
                {{"buffer":0,"byteOffset":48,"byteLength":48}},
                {{"buffer":0,"byteOffset":96,"byteLength":32}},
                {{"buffer":0,"byteOffset":128,"byteLength":12}}],
            "accessors":[
                {{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}},
                {{"bufferView":1,"componentType":5126,"count":4,"type":"VEC3"}},
                {{"bufferView":2,"componentType":5126,"count":4,"type":"VEC2"}},
                {{"bufferView":3,"componentType":5123,"count":6,"type":"SCALAR"}}]
            {extra}}}"#
        )
    }

    fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
        let mut json = json.as_bytes().to_vec();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let mut bin = bin.to_vec();
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let mut out = Vec::new();
        out.extend(b"glTF");
        out.extend(2u32.to_le_bytes());
        out.extend((12 + 8 + json.len() as u32 + 8 + bin.len() as u32).to_le_bytes());
        out.extend((json.len() as u32).to_le_bytes());
        out.extend(b"JSON");
        out.extend(&json);
        out.extend((bin.len() as u32).to_le_bytes());
        out.extend(b"BIN\0");
        out.extend(&bin);
        out
    }

    fn encode_base64(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (u32::from(b) << (16 - 8 * i)));
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    fn options() -> LoadOptions {
        LoadOptions { max_texture: 64 }
    }

    #[test]
    fn glb_quad_with_material() {
        let json = quad_json(
            r#"{"byteLength":140}"#,
            r#","translation":[10,0,0]"#,
            r#","material":0"#,
            r#","materials":[{"name":"Paint","pbrMetallicRoughness":{"baseColorFactor":[0.5,0.25,1,1]}}]"#,
        );
        let model = load_bytes(&glb(&json, &quad_buffer()), Path::new("."), &options()).unwrap();
        assert_eq!(model.info.format, "glTF 2.0 (GLB)");
        assert_eq!((model.info.triangles, model.info.vertices, model.info.meshes), (2, 4, 1));
        assert_eq!(model.bounds.min, [10.0, 0.0, 0.0], "node translation applied");
        assert_eq!(model.bounds.max, [11.0, 1.0, 0.0]);
        assert_eq!(model.materials[0].name, "Paint");
        assert_eq!(model.materials[0].color, [0.5, 0.25, 1.0, 1.0], "linear already");
        assert_eq!(model.materials[0].alpha_cutoff, None);
        assert_eq!(model.vertices[0].normal, [0.0, 0.0, 1.0]);
        assert_eq!(model.vertices[2].uv, [1.0, 0.0]);
    }

    #[test]
    fn gltf_with_data_uri_buffer_and_texture() {
        let buffer = format!(
            r#"{{"byteLength":140,"uri":"data:application/octet-stream;base64,{}"}}"#,
            encode_base64(&quad_buffer())
        );
        let image = format!(r#"{{"uri":"data:image/png;base64,{}"}}"#, encode_base64(&png_2x2()));
        let extra = format!(
            r#","materials":[{{"pbrMetallicRoughness":{{"baseColorTexture":{{"index":0}}}},"alphaMode":"MASK","alphaCutoff":0.3}}],
            "textures":[{{"source":0}}],"images":[{image}]"#
        );
        let json = quad_json(&buffer, "", r#","material":0"#, &extra);
        let model = load_bytes(json.as_bytes(), Path::new("."), &options()).unwrap();
        assert_eq!(model.info.format, "glTF 2.0");
        assert_eq!((model.info.textures, model.info.missing_textures), (1, 0));
        assert_eq!(model.materials[0].texture, Some(0));
        assert_eq!(model.materials[0].alpha_cutoff, Some(0.3));
        assert_eq!((model.textures[0].width, model.textures[0].height), (2, 2));
    }

    #[test]
    fn external_buffer_and_image_beside_the_file() {
        let dir = TempDir::new("gltf_external");
        dir.write("quad data.bin", &quad_buffer());
        dir.write("wood.png", &png_2x2());
        let extra = r#","materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}],
            "textures":[{"source":0}],"images":[{"uri":"wood.png"},{"uri":"gone.png"}]"#;
        let json = quad_json(r#"{"byteLength":140,"uri":"quad%20data.bin"}"#, "", r#","material":0"#, extra);
        let path = dir.write("quad.gltf", json.as_bytes());
        let model = load(&path, &options()).unwrap();
        assert_eq!(model.info.triangles, 2);
        assert_eq!(model.info.textures, 1);
    }

    #[test]
    fn image_in_the_binary_without_mime_type_and_a_texture_without_picture() {
        let mut bin = quad_buffer();
        let png = png_2x2();
        bin.extend(&png);
        for (material, textures, missing) in [(0, 1, 0), (1, 0, 1)] {
            let extra = r#","materials":[
                    {"pbrMetallicRoughness":{"baseColorTexture":{"index":0}}},
                    {"pbrMetallicRoughness":{"baseColorTexture":{"index":1}}}],
                "textures":[{"source":0},{"extensions":{"KHR_texture_basisu":{"source":0}}}],
                "images":[{"bufferView":4}]"#;
            let json = quad_json(&format!(r#"{{"byteLength":{}}}"#, bin.len()), "", &format!(r#","material":{material}"#), extra)
                .replace(
                    r#"{"buffer":0,"byteOffset":128,"byteLength":12}]"#,
                    &format!(r#"{{"buffer":0,"byteOffset":128,"byteLength":12}},{{"buffer":0,"byteOffset":140,"byteLength":{}}}]"#, png.len()),
                );
            let model = load_bytes(&glb(&json, &bin), Path::new("."), &options()).unwrap();
            assert_eq!((model.info.textures, model.info.missing_textures), (textures, missing), "material {material}");
        }
    }

    #[test]
    fn mirrored_node_keeps_front_faces_counter_clockwise() {
        let json = quad_json(r#"{"byteLength":140}"#, r#","scale":[-1,1,1]"#, "", "");
        let model = load_bytes(&glb(&json, &quad_buffer()), Path::new("."), &options()).unwrap();
        for triangle in model.indices.as_chunks::<3>().0 {
            let [a, b, c] = triangle.map(|i| model.vertices[i as usize]);
            let winding = geometry::cross(geometry::sub(b.position, a.position), geometry::sub(c.position, a.position));
            assert!(geometry::dot(winding, a.normal) > 0.0, "winding and normal agree");
        }
        assert_eq!(model.bounds.min[0], -1.0);
    }

    #[test]
    fn missing_normals_give_flat_shading() {
        let json = quad_json(r#"{"byteLength":140}"#, "", "", "").replace(r#""NORMAL":1,"#, "");
        let model = load_bytes(&glb(&json, &quad_buffer()), Path::new("."), &options()).unwrap();
        assert_eq!(model.vertices.len(), 6);
        assert!(model.vertices.iter().all(|v| v.normal == [0.0, 0.0, 1.0]));
    }

    #[test]
    fn broken_files_are_errors_not_panics() {
        // Buffer shorter than the accessors claim.
        let json = quad_json(r#"{"byteLength":140}"#, "", "", "");
        let err = load_bytes(&glb(&json, &quad_buffer()[..100]), Path::new("."), &options()).err().unwrap();
        assert!(err.contains("can't read") || err.contains("no meshes"), "{err}");
        // Out-of-range index.
        let json = quad_json(r#"{"byteLength":140}"#, "", r#","material":7"#, "");
        assert!(load_bytes(&glb(&json, &quad_buffer()), Path::new("."), &options()).is_err());
        assert!(load_bytes(b"{ not json", Path::new("."), &options()).is_err());
        let draco =
            quad_json(r#"{"byteLength":140}"#, "", "", r#","extensionsRequired":["KHR_draco_mesh_compression"]"#);
        let err = load_bytes(&glb(&draco, &quad_buffer()), Path::new("."), &options()).err().unwrap();
        assert!(err.contains("KHR_draco_mesh_compression"), "{err}");
        // Extensions the crate doesn't know don't stop a file that draws without them.
        let unknown = quad_json(r#"{"byteLength":140}"#, "", "", r#","extensionsRequired":["KHR_texture_transform"]"#);
        assert!(load_bytes(&glb(&unknown, &quad_buffer()), Path::new("."), &options()).is_ok());
    }

    #[test]
    fn damaged_files_never_panic() {
        let json = quad_json(
            r#"{"byteLength":140}"#,
            r#","scale":[2,2,2]"#,
            r#","material":0"#,
            r#","materials":[{"pbrMetallicRoughness":{"baseColorFactor":[1,0,0,1]}}]"#,
        );
        for bytes in crate::model::test_files::damaged(&glb(&json, &quad_buffer()), 400) {
            let _ = load_bytes(&bytes, Path::new("."), &options());
        }
        // Valid JSON with different numbers: every digit in turn made 9, and 0.
        for (at, _) in json.char_indices().filter(|(_, c)| c.is_ascii_digit()) {
            for digit in ["9", "0"] {
                let mut changed = json.clone();
                changed.replace_range(at..at + 1, digit);
                let _ = load_bytes(&glb(&changed, &quad_buffer()), Path::new("."), &options());
            }
        }
        // Counts and offsets at their limits.
        for (from, to) in [
            (r#""count":6"#, r#""count":4294967295"#),
            (r#""count":4,"type":"VEC3","min""#, r#""count":0,"type":"VEC3","min""#),
            (r#""byteOffset":48"#, r#""byteOffset":18446744073709551615"#),
            (r#""byteLength":12"#, r#""byteLength":18446744073709551615"#),
        ] {
            let _ = load_bytes(&glb(&json.replace(from, to), &quad_buffer()), Path::new("."), &options());
        }
    }

    #[test]
    fn strips_fans_and_uri_helpers() {
        assert_eq!(triangle_list(Mode::TriangleStrip, &[0, 1, 2, 3]), vec![[0, 1, 2], [2, 1, 3]]);
        assert_eq!(triangle_list(Mode::TriangleFan, &[0, 1, 2, 3]), vec![[0, 1, 2], [0, 2, 3]]);
        assert_eq!(base64("SGVsbG8=").unwrap(), b"Hello");
        assert_eq!(base64(&encode_base64(&[1, 2, 3, 4, 5])).unwrap(), vec![1, 2, 3, 4, 5]);
        assert_eq!(percent_decode("my%20file%2Bx.bin"), "my file+x.bin");
        assert_eq!(data_uri("data:text/plain,hi"), Some(None));
        assert_eq!(data_uri("file.bin"), None);
    }

    #[test]
    fn animation_lengths() {
        let json = quad_json(
            r#"{"byteLength":140}"#,
            "",
            "",
            r#","animations":[
                {"name":"Wave","samplers":[{"input":4,"output":4}],"channels":[{"sampler":0,"target":{"node":0,"path":"scale"}}]},
                {"samplers":[{"input":5,"output":5}],"channels":[{"sampler":0,"target":{"node":0,"path":"scale"}}]}]"#,
        )
        .replace(
            r#""count":6,"type":"SCALAR"}]"#,
            r#""count":6,"type":"SCALAR"},
                {"bufferView":0,"componentType":5126,"count":2,"type":"SCALAR","min":[0],"max":[2.5]},
                {"bufferView":0,"componentType":5126,"count":2,"type":"SCALAR","min":[0],"max":[1]}]"#,
        );
        let model = load_bytes(&glb(&json, &quad_buffer()), Path::new("."), &options()).unwrap();
        assert_eq!(model.info.animations, vec![("Wave".to_owned(), 2.5), ("Animation 2".to_owned(), 1.0)]);
    }
}
