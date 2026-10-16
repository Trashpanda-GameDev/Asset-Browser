//! Collada (.dae): XML, read with roxmltree.
//!
//! Elements point at each other by id (`url="#id"`): the visual scene's nodes carry transforms
//! and instance geometries (or skin controllers wrapping them) with a material bound to each
//! primitive's symbol; a material instances an effect, whose diffuse is a colour or a texture
//! reached through sampler and surface parameters to an image file. Geometry is a set of float
//! sources and primitives (`triangles`, `polylist`, `polygons`, `trifans`, `tristrips`) whose
//! `p` lists interleave one index per input. `asset/up_axis` and `asset/unit` give the space.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use roxmltree::Node;

use super::geometry::{self, DEFAULT_CREASE, IDENTITY, Mat4, NormalMatrix};
use super::textures::{self, Alpha};
use super::vector::normalize;
use super::{Builder, LoadOptions, Model, Slot, Vertex, srgb_to_linear};

pub(super) fn parse(bytes: &[u8], file: &Path, options: &LoadOptions) -> Result<Model, String> {
    let (text, dir) = (String::from_utf8_lossy(bytes), super::folder(file));
    let parsing = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(&text, parsing).map_err(|err| format!("Collada: {err}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "COLLADA" {
        return Err("not a Collada file".into());
    }
    let ids: HashMap<&str, Node> = doc.descendants().filter_map(|node| Some((node.attribute("id")?, node))).collect();

    // The file's space to ours: units to metres, then its up axis to Y.
    let asset = child(root, "asset");
    let meters = asset
        .and_then(|asset| child(asset, "unit"))
        .and_then(|unit| unit.attribute("meter")?.trim().parse::<f32>().ok())
        .filter(|meters| meters.is_finite() && *meters > 0.0)
        .unwrap_or(1.0);
    let up = asset.and_then(|asset| child(asset, "up_axis")).and_then(|up| up.text()).unwrap_or("Y_UP").trim();
    let axes: Mat4 = match up {
        // Z up → Y up: (x, y, z) → (x, z, -y).
        "Z_UP" => [[1.0, 0.0, 0.0, 0.0], [0.0, 0.0, -1.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]],
        // X up → Y up: (x, y, z) → (-y, x, z).
        "X_UP" => [[0.0, 1.0, 0.0, 0.0], [-1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]],
        _ => IDENTITY,
    };
    let scale = [[meters, 0.0, 0.0, 0.0], [0.0, meters, 0.0, 0.0], [0.0, 0.0, meters, 0.0], [0.0, 0.0, 0.0, 1.0]];
    let space = geometry::mul(&axes, &scale);

    let mut reader = Reader {
        ids: &ids,
        dir,
        max_texture: options.max_texture,
        builder: Builder::default(),
        materials: HashMap::new(),
        textures: HashMap::new(),
        source_vertices: 0,
    };
    let scene = child(root, "scene")
        .and_then(|scene| child(scene, "instance_visual_scene"))
        .and_then(|instance| reader.target(instance.attribute("url")?))
        .or_else(|| descendant(root, "visual_scene"));
    match scene {
        Some(scene) => {
            for node in elements(scene, "node") {
                reader.walk(node, &space, 0);
            }
        }
        None => {
            // No scene: every geometry where it was modelled.
            for geometry in root.descendants().filter(|node| node.tag_name().name() == "geometry") {
                reader.add_geometry(geometry, &space, &HashMap::new());
            }
        }
    }
    let mut builder = reader.builder;
    builder.source_vertices = Some(reader.source_vertices);
    if builder.triangle_count() == 0 {
        return Err("there are no meshes in this file".into());
    }
    let version = root.attribute("version").unwrap_or("");
    builder.finish(format!("Collada {version}").trim_end().to_owned())
}

/// Child elements named `name`, ignoring the namespace.
fn elements<'a, 'input>(node: Node<'a, 'input>, name: &'static str) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children().filter(move |child| child.is_element() && child.tag_name().name() == name)
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &'static str) -> Option<Node<'a, 'input>> {
    elements(node, name).next()
}

fn descendant<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.descendants().find(|node| node.is_element() && node.tag_name().name() == name)
}

fn numbers<T: std::str::FromStr>(node: Option<Node>) -> Vec<T> {
    node.and_then(|node| node.text())
        .map(|text| text.split_ascii_whitespace().filter_map(|word| word.parse().ok()).collect())
        .unwrap_or_default()
}

/// A Collada matrix: 16 numbers, row by row.
fn matrix_from_rows(values: &[f32]) -> Option<Mat4> {
    (values.len() >= 16).then(|| std::array::from_fn(|column| std::array::from_fn(|row| values[row * 4 + column])))
}

/// A node's own transform: its transform elements multiplied in order.
fn local_transform(node: Node) -> Mat4 {
    let mut m = IDENTITY;
    for element in node.children().filter(Node::is_element) {
        let values: Vec<f32> = numbers(Some(element));
        let step: Option<Mat4> = match (element.tag_name().name(), values.as_slice()) {
            ("matrix", values) => matrix_from_rows(values),
            ("translate", [x, y, z, ..]) => {
                Some([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [*x, *y, *z, 1.0]])
            }
            ("scale", [x, y, z, ..]) => {
                Some([[*x, 0.0, 0.0, 0.0], [0.0, *y, 0.0, 0.0], [0.0, 0.0, *z, 0.0], [0.0, 0.0, 0.0, 1.0]])
            }
            ("rotate", [x, y, z, degrees, ..]) => Some(rotation(normalize([*x, *y, *z]), degrees.to_radians())),
            _ => None,
        };
        if let Some(step) = step {
            m = geometry::mul(&m, &step);
        }
    }
    m
}

/// Rotation by `angle` radians about the unit `axis`.
fn rotation(axis: [f32; 3], angle: f32) -> Mat4 {
    let [x, y, z] = axis;
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    [
        [t * x * x + c, t * x * y + s * z, t * x * z - s * y, 0.0],
        [t * x * y - s * z, t * y * y + c, t * y * z + s * x, 0.0],
        [t * x * z + s * y, t * y * z - s * x, t * z * z + c, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// One input of a primitive: which source, at which offset in each corner's indices.
struct Input<'a, 'input> {
    semantic: &'a str,
    source: Node<'a, 'input>,
    offset: usize,
}

/// Values of a `source`, with its accessor's stride.
struct Source {
    values: Vec<f32>,
    stride: usize,
    offset: usize,
}

fn source(node: Node) -> Source {
    let values: Vec<f32> = numbers(child(node, "float_array"));
    let accessor = child(node, "technique_common").and_then(|technique| child(technique, "accessor"));
    let attribute = |name: &str| accessor.and_then(|a| a.attribute(name)?.parse::<usize>().ok());
    Source { values, stride: attribute("stride").unwrap_or(1).max(1), offset: attribute("offset").unwrap_or(0) }
}

impl Source {
    fn get<const N: usize>(&self, index: usize) -> Option<[f32; N]> {
        let at = self.offset.checked_add(index.checked_mul(self.stride)?)?;
        let values = self.values.get(at..at.checked_add(N)?)?;
        Some(std::array::from_fn(|i| values[i]))
    }
}

struct Reader<'a, 'input> {
    ids: &'a HashMap<&'input str, Node<'a, 'input>>,
    dir: &'a Path,
    max_texture: u32,
    builder: Builder,
    /// Material id → ours.
    materials: HashMap<String, u32>,
    /// Image reference → what became of it.
    textures: HashMap<String, Slot>,
    source_vertices: u64,
}

/// Deep enough for any real scene; stops `instance_node` loops.
const MAX_DEPTH: usize = 64;

impl<'a, 'input> Reader<'a, 'input> {
    fn target(&self, url: &str) -> Option<Node<'a, 'input>> {
        self.ids.get(url.trim().trim_start_matches('#')).copied()
    }

    fn walk(&mut self, node: Node<'a, 'input>, parent: &Mat4, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let matrix = geometry::mul(parent, &local_transform(node));
        for element in node.children().filter(Node::is_element) {
            match element.tag_name().name() {
                "node" => self.walk(element, &matrix, depth + 1),
                "instance_node" => {
                    if let Some(target) = element.attribute("url").and_then(|url| self.target(url)) {
                        self.walk(target, &matrix, depth + 1);
                    }
                }
                "instance_geometry" => {
                    if let Some(geometry) = element.attribute("url").and_then(|url| self.target(url)) {
                        let bindings = self.bindings(element);
                        self.add_geometry(geometry, &matrix, &bindings);
                    }
                }
                "instance_controller" => {
                    // A skinned mesh: drawn as bound, through the controller's bind shape matrix.
                    let controller = element.attribute("url").and_then(|url| self.target(url));
                    let Some(skin) = controller.and_then(|controller| child(controller, "skin")) else { continue };
                    let Some(geometry) = skin.attribute("source").and_then(|url| self.target(url)) else { continue };
                    let bind_shape =
                        matrix_from_rows(&numbers::<f32>(child(skin, "bind_shape_matrix"))).unwrap_or(IDENTITY);
                    let bindings = self.bindings(element);
                    self.add_geometry(geometry, &geometry::mul(&matrix, &bind_shape), &bindings);
                }
                _ => {}
            }
        }
    }

    /// Material symbol → material element, from an instance's `bind_material`.
    fn bindings(&self, instance: Node<'a, 'input>) -> HashMap<&'a str, Node<'a, 'input>> {
        instance
            .descendants()
            .filter(|node| node.tag_name().name() == "instance_material")
            .filter_map(|node| Some((node.attribute("symbol")?, self.target(node.attribute("target")?)?)))
            .collect()
    }

    fn add_geometry(&mut self, geometry: Node<'a, 'input>, matrix: &Mat4, bindings: &HashMap<&str, Node<'a, 'input>>) {
        let Some(mesh) = child(geometry, "mesh") else { return };
        let normal_matrix = NormalMatrix::new(matrix);
        let mirrored = geometry::determinant(matrix) < 0.0;
        let mut drawn = false;
        for primitive in mesh.children().filter(Node::is_element) {
            let kind = primitive.tag_name().name();
            if !matches!(kind, "triangles" | "polylist" | "polygons" | "trifans" | "tristrips") {
                continue;
            }
            let Some((vertices, mut indices, has_normals)) = self.read_primitive(primitive) else { continue };
            if indices.is_empty() {
                continue;
            }
            let mut vertices: Vec<Vertex> = vertices
                .into_iter()
                .map(|vertex| Vertex {
                    position: geometry::transform_point(matrix, vertex.position),
                    normal: normalize(normal_matrix.apply(vertex.normal)),
                    ..vertex
                })
                .collect();
            if mirrored {
                for triangle in indices.as_chunks_mut::<3>().0 {
                    triangle.swap(1, 2);
                }
            }
            if !has_normals {
                (vertices, indices) = geometry::smooth_normals(&vertices, &indices, DEFAULT_CREASE);
            }
            let colors = vertices.iter().any(|vertex| vertex.color != [1.0; 4]);
            let material = match primitive.attribute("material").and_then(|symbol| bindings.get(symbol)) {
                Some(&material) => self.material(material, colors),
                // Files without bindings often name the material itself.
                None => match primitive.attribute("material").and_then(|id| self.target(id)) {
                    Some(material) if material.tag_name().name() == "material" => self.material(material, colors),
                    _ => self.builder.default_material(colors),
                },
            };
            let base = self.builder.add_vertices(&vertices);
            self.builder.add_triangles(material, base, &indices);
            drawn = true;
        }
        if drawn {
            self.builder.meshes += 1;
        }
    }

    /// The primitive's corners as vertices (merged where all their indices match) and triangles,
    /// in the geometry's own space; and whether normals came with it.
    #[expect(clippy::cognitive_complexity, clippy::too_many_lines, reason = "refactor: split into named steps")]
    fn read_primitive(&mut self, primitive: Node<'a, 'input>) -> Option<(Vec<Vertex>, Vec<u32>, bool)> {
        // Inputs of the primitive; VERTEX expands into the `vertices` element's inputs.
        let mut inputs: Vec<Input> = Vec::new();
        for input in elements(primitive, "input") {
            let semantic = input.attribute("semantic").unwrap_or("");
            let offset = input.attribute("offset").and_then(|o| o.parse().ok()).unwrap_or(0);
            let Some(source) = input.attribute("source").and_then(|url| self.target(url)) else { continue };
            if semantic == "VERTEX" {
                for shared in elements(source, "input") {
                    let Some(target) = shared.attribute("source").and_then(|url| self.target(url)) else { continue };
                    inputs.push(Input { semantic: shared.attribute("semantic").unwrap_or(""), source: target, offset });
                }
            } else {
                inputs.push(Input { semantic, source, offset });
            }
        }
        let stride = inputs.iter().map(|input| input.offset).max()?.checked_add(1)?;
        let find = |semantic: &str| {
            let input = inputs.iter().find(|input| input.semantic == semantic)?;
            Some((source(input.source), input.offset))
        };
        let (positions, position_offset) = find("POSITION")?;
        let (normals, uvs, colors) = (find("NORMAL"), find("TEXCOORD"), find("COLOR"));

        // Polygons as runs of corners, each corner `stride` indices.
        let lists: Vec<Vec<u32>> = elements(primitive, "p").map(|p| numbers(Some(p))).collect();
        let mut polygons: Vec<&[u32]> = Vec::new();
        let kind = primitive.tag_name().name();
        match kind {
            "triangles" => {
                for list in &lists {
                    polygons.extend(list.chunks_exact(stride.checked_mul(3)?));
                }
            }
            "polylist" => {
                let counts: Vec<usize> = numbers(child(primitive, "vcount"));
                let list = lists.first()?;
                let mut at = 0usize;
                for count in counts {
                    let end = count.checked_mul(stride).and_then(|len| len.checked_add(at));
                    let Some(polygon) = end.and_then(|end| list.get(at..end)) else { break };
                    polygons.push(polygon);
                    at += polygon.len();
                }
            }
            _ => polygons.extend(lists.iter().map(Vec::as_slice)),
        }

        let vertex = |corner: &[u32]| -> Option<Vertex> {
            let at = |offset: usize| corner.get(offset).map(|&index| index as usize);
            let attribute3 = |input: &Option<(Source, usize)>| {
                let (source, offset) = input.as_ref()?;
                source.get::<3>(at(*offset)?)
            };
            let uv = uvs.as_ref().and_then(|(source, offset)| source.get::<2>(at(*offset)?));
            let color = attribute3(&colors).map(|rgb| {
                let [r, g, b] = rgb.map(srgb_to_linear);
                [r, g, b, 1.0]
            });
            Some(Vertex {
                position: positions.get::<3>(at(position_offset)?)?,
                normal: attribute3(&normals).unwrap_or([0.0; 3]),
                // v counts up from the bottom of the picture.
                uv: uv.map_or([0.0; 2], |[u, v]| [u, 1.0 - v]),
                color: color.unwrap_or([1.0; 4]),
            })
        };
        let mut vertices: Vec<Vertex> = Vec::new();
        let mut merged: HashMap<&[u32], u32> = HashMap::new();
        let mut indices = Vec::new();
        let mut ids: Vec<u32> = Vec::new();
        for polygon in polygons {
            ids.clear();
            for corner in polygon.chunks_exact(stride) {
                let index = if let Some(&index) = merged.get(corner) {
                    index
                } else {
                    let Some(vertex) = vertex(corner) else { break };
                    vertices.push(vertex);
                    let index = (vertices.len() - 1) as u32;
                    merged.insert(corner, index);
                    index
                };
                ids.push(index);
            }
            if ids.len() != polygon.len() / stride {
                continue;
            }
            if kind == "tristrips" {
                for (i, w) in ids.windows(3).enumerate() {
                    indices.extend(if i % 2 == 0 { [w[0], w[1], w[2]] } else { [w[1], w[0], w[2]] });
                }
            } else {
                // Fans, and polygons cut into fans: convex in practice.
                for k in 1..ids.len().saturating_sub(1) {
                    indices.extend([ids[0], ids[k], ids[k + 1]]);
                }
            }
        }
        self.source_vertices += (positions.values.len() / positions.stride) as u64;
        Some((vertices, indices, normals.is_some()))
    }

    fn material(&mut self, material: Node<'a, 'input>, vertex_colors: bool) -> u32 {
        let id = material.attribute("id").unwrap_or("").to_owned();
        if let Some(&index) = self.materials.get(&id) {
            return index;
        }
        let name = material.attribute("name").unwrap_or(&id).to_owned();
        let effect = child(material, "instance_effect").and_then(|e| self.target(e.attribute("url")?));
        let profile = effect.and_then(|effect| child(effect, "profile_COMMON"));
        let shading = profile
            .and_then(|profile| child(profile, "technique"))
            .and_then(|technique| technique.children().find(Node::is_element));
        let diffuse = shading.and_then(|shading| child(shading, "diffuse"));
        let color = diffuse
            .and_then(|diffuse| child(diffuse, "color"))
            .map(|color| numbers::<f32>(Some(color)))
            .filter(|values| values.len() >= 3)
            .map(|values| [srgb_to_linear(values[0]), srgb_to_linear(values[1]), srgb_to_linear(values[2]), 1.0]);
        let image = diffuse.and_then(|diffuse| child(diffuse, "texture")).and_then(|texture| {
            let sampler = texture.attribute("texture")?;
            self.image_path(profile, sampler)
        });
        let slot = match image {
            Some(path) if self.max_texture > 0 => self.texture(&path),
            _ => Slot::None,
        };
        let index = match (color, slot) {
            (None, Slot::None) => self.builder.default_material(vertex_colors),
            (color, slot) => self.builder.material(name, color.unwrap_or([1.0; 4]), slot),
        };
        self.materials.insert(id, index);
        index
    }

    /// Follows a `texture` reference through the sampler and surface parameters to an image's
    /// file. Some exporters name the image directly instead.
    fn image_path(&self, profile: Option<Node<'a, 'input>>, reference: &str) -> Option<String> {
        let param = |sid: &str| {
            profile?
                .descendants()
                .find(|node| node.tag_name().name() == "newparam" && node.attribute("sid") == Some(sid))
        };
        let image_id = match param(reference) {
            Some(sampler) => {
                let surface_sid = descendant(sampler, "source")?.text()?.trim();
                match param(surface_sid) {
                    Some(surface) => descendant(surface, "init_from")?.text()?.trim().to_owned(),
                    // Collada 1.5 samplers point at the image themselves.
                    None => descendant(sampler, "instance_image")
                        .and_then(|image| image.attribute("url"))
                        .unwrap_or(surface_sid)
                        .to_owned(),
                }
            }
            None => reference.to_owned(),
        };
        let image = self.target(&image_id)?;
        let init = child(image, "init_from")?;
        // 1.4 has the path as text, 1.5 inside `ref`.
        let path = child(init, "ref").and_then(|r| r.text()).or_else(|| init.text())?.trim();
        let path = path.strip_prefix("file://").unwrap_or(path);
        Some(super::percent_decode(path))
    }

    fn texture(&mut self, path: &str) -> Slot {
        if let Some(&slot) = self.textures.get(path) {
            return slot;
        }
        let decoded = textures::from_file(self.dir, path, self.max_texture, Alpha::Guess);
        let slot = self.builder.texture_slot(decoded.as_ref().map(Arc::as_ref));
        self.textures.insert(path.to_owned(), slot);
        slot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::test_files::png_2x2;
    use crate::test_support::TempDir;

    /// A unit quad (Z up, centimetres) as a polylist with one textured material, placed by a
    /// translated node.
    const QUAD: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">
  <asset><unit name="centimeter" meter="0.01"/><up_axis>Z_UP</up_axis></asset>
  <library_images><image id="wood-image"><init_from>file:///C:/art/wood%20grain.png</init_from></image></library_images>
  <library_effects>
    <effect id="wood-effect"><profile_COMMON>
      <newparam sid="wood-surface"><surface type="2D"><init_from>wood-image</init_from></surface></newparam>
      <newparam sid="wood-sampler"><sampler2D><source>wood-surface</source></sampler2D></newparam>
      <technique sid="common"><lambert><diffuse><texture texture="wood-sampler" texcoord="UVMap"/></diffuse></lambert></technique>
    </profile_COMMON></effect>
    <effect id="red-effect"><profile_COMMON><technique sid="common"><phong>
      <diffuse><color sid="diffuse">1 0 0 1</color></diffuse>
    </phong></technique></profile_COMMON></effect>
  </library_effects>
  <library_materials>
    <material id="wood-material" name="Wood"><instance_effect url="#wood-effect"/></material>
    <material id="red-material" name="Red"><instance_effect url="#red-effect"/></material>
  </library_materials>
  <library_geometries>
    <geometry id="quad" name="Quad"><mesh>
      <source id="quad-positions"><float_array id="quad-positions-array" count="12">0 0 0 100 0 0 100 100 0 0 100 0</float_array>
        <technique_common><accessor source="#quad-positions-array" count="4" stride="3"/></technique_common></source>
      <source id="quad-uvs"><float_array id="quad-uvs-array" count="8">0 0 1 0 1 1 0 1</float_array>
        <technique_common><accessor source="#quad-uvs-array" count="4" stride="2"/></technique_common></source>
      <vertices id="quad-vertices"><input semantic="POSITION" source="#quad-positions"/></vertices>
      <polylist material="wood-symbol" count="1">
        <input semantic="VERTEX" source="#quad-vertices" offset="0"/>
        <input semantic="TEXCOORD" source="#quad-uvs" offset="1" set="0"/>
        <vcount>4</vcount><p>0 0 1 1 2 2 3 3</p>
      </polylist>
      <triangles material="red-symbol" count="1">
        <input semantic="VERTEX" source="#quad-vertices" offset="0"/>
        <p>0 2 1</p>
      </triangles>
    </mesh></geometry>
  </library_geometries>
  <library_visual_scenes><visual_scene id="scene">
    <node id="quad-node" name="Quad"><translate>0 0 200</translate>
      <instance_geometry url="#quad"><bind_material><technique_common>
        <instance_material symbol="wood-symbol" target="#wood-material"/>
        <instance_material symbol="red-symbol" target="#red-material"/>
      </technique_common></bind_material></instance_geometry>
    </node>
  </visual_scene></library_visual_scenes>
  <scene><instance_visual_scene url="#scene"/></scene>
</COLLADA>"##;

    #[test]
    fn textured_quad_in_centimetres_z_up() {
        let dir = TempDir::new("collada");
        dir.write("wood grain.png", &png_2x2());
        let path = dir.write("quad.dae", QUAD.as_bytes());
        let model = crate::model::load(&path, &LoadOptions { max_texture: 64 }).unwrap();
        assert_eq!(model.info.format, "Collada 1.4.1");
        assert_eq!(model.info.triangles, 3);
        assert_eq!((model.info.materials, model.info.textures, model.info.missing_textures), (2, 1, 0));
        // 100 cm square raised 200 cm along Z: Y up here, in metres.
        assert_eq!(model.bounds.min, [0.0, 2.0, -1.0]);
        assert_eq!(model.bounds.max, [1.0, 2.0, 0.0]);
        let wood = model.materials.iter().find(|m| m.name == "Wood").unwrap();
        assert_eq!(wood.texture, Some(0));
        let red = model.materials.iter().find(|m| m.name == "Red").unwrap();
        assert_eq!(red.color, [1.0, 0.0, 0.0, 1.0]);
        // Generated normals agree with the winding: the polylist faces up.
        let wood_index = model.materials.iter().position(|m| m.name == "Wood").unwrap() as u32;
        let part = model.parts.iter().find(|p| p.material == wood_index).unwrap();
        let first = model.vertices[model.indices[part.first as usize] as usize];
        assert!((first.normal[1] - 1.0).abs() < 1e-5, "{:?}", first.normal);
    }

    #[test]
    fn damaged_files_never_panic() {
        for bytes in crate::test_support::damaged(QUAD.as_bytes(), 300) {
            let _ = parse(&bytes, Path::new("test.dae"), &LoadOptions::default());
        }
        // Numbers that overflow arithmetic.
        let huge = QUAD.replace("<vcount>4</vcount>", "<vcount>18446744073709551615</vcount>").replace(
            r##"<input semantic="TEXCOORD" source="#quad-uvs" offset="1" set="0"/>"##,
            r##"<input semantic="TEXCOORD" source="#quad-uvs" offset="18446744073709551615" set="0"/>"##,
        );
        let _ = parse(huge.as_bytes(), Path::new("test.dae"), &LoadOptions::default());
    }

    #[test]
    fn broken_collada() {
        assert!(parse(b"<COLLADA><unclosed>", Path::new("test.dae"), &LoadOptions::default()).is_err());
        assert!(parse(b"<svg/>", Path::new("test.dae"), &LoadOptions::default()).is_err());
        let no_geometry =
            QUAD.replace("<p>0 0 1 1 2 2 3 3</p>", "<p>0 0 1</p>").replace("<p>0 2 1</p>", "<p>9 9 9</p>");
        assert!(parse(no_geometry.as_bytes(), Path::new("test.dae"), &LoadOptions::default()).is_err());
    }

    #[test]
    fn rotation_matches_right_hand_rule() {
        let quarter = rotation([0.0, 0.0, 1.0], std::f32::consts::FRAC_PI_2);
        let p = geometry::transform_point(&quarter, [1.0, 0.0, 0.0]);
        assert!((p[0]).abs() < 1e-6 && (p[1] - 1.0).abs() < 1e-6);
    }
}
