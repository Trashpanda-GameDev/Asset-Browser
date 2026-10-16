//! PLY (Stanford polygon files), ASCII or binary of either byte order, as 3D scanners and
//! MeshLab write them: Z up, units unknown and taken as metres.
//!
//! A text header names the elements in order, each with a count and typed properties, then the
//! data follows. We read `vertex` (x y z, normals, colours, texture coordinates under their usual
//! names) and `face` (a list of corner indices, optionally a `texcoord` list per face as MeshLab
//! writes), and step over any other element. A `comment TextureFile <name>` line names the texture.

use std::path::Path;
use std::sync::Arc;

use super::geometry::{self, DEFAULT_CREASE};
use super::textures::{self, Alpha};
use super::vector;
use super::{Builder, LoadOptions, Model, Vertex, z_up_to_y_up};

pub(super) fn load(path: &Path, options: &LoadOptions) -> Result<Model, String> {
    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    let dir = path.parent().unwrap_or(Path::new("."));
    parse(&bytes, dir, options)
}

#[derive(Clone, Copy, PartialEq)]
enum Encoding {
    Ascii,
    Little,
    Big,
}

#[derive(Clone, Copy, PartialEq)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "char" | "int8" => Self::I8,
            "uchar" | "uint8" => Self::U8,
            "short" | "int16" => Self::I16,
            "ushort" | "uint16" => Self::U16,
            "int" | "int32" => Self::I32,
            "uint" | "uint32" => Self::U32,
            "float" | "float32" => Self::F32,
            "double" | "float64" => Self::F64,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }

    /// Colours stored as integers count up to the type's maximum.
    fn color_scale(self) -> f64 {
        match self {
            Self::U8 | Self::I8 => 255.0,
            Self::U16 | Self::I16 => 65535.0,
            _ => 1.0,
        }
    }
}

struct Property {
    name: String,
    /// `Some(count type)` for a list.
    list: Option<Scalar>,
    scalar: Scalar,
}

struct Element {
    name: String,
    count: usize,
    properties: Vec<Property>,
}

struct Header {
    encoding: Encoding,
    elements: Vec<Element>,
    texture: Option<String>,
    /// Where the data starts.
    data: usize,
}

fn parse_header(bytes: &[u8]) -> Result<Header, String> {
    if !bytes.starts_with(b"ply") {
        return Err("not a PLY file".into());
    }
    let end = bytes.windows(10).position(|w| w == b"end_header").ok_or("the PLY header never ends")?;
    let line_end = bytes[end..].iter().position(|&b| b == b'\n').map_or(bytes.len(), |at| end + at + 1);
    let text = String::from_utf8_lossy(&bytes[..end]);
    let mut header = Header { encoding: Encoding::Ascii, elements: Vec::new(), texture: None, data: line_end };
    for line in text.lines() {
        let words: Vec<&str> = line.split_ascii_whitespace().collect();
        match words.as_slice() {
            ["format", format, ..] => {
                header.encoding = match *format {
                    "ascii" => Encoding::Ascii,
                    "binary_little_endian" => Encoding::Little,
                    "binary_big_endian" => Encoding::Big,
                    other => return Err(format!("unknown PLY format {other}")),
                }
            }
            ["comment", key, name @ ..] if key.eq_ignore_ascii_case("TextureFile") && !name.is_empty() => {
                header.texture = Some(name.join(" "));
            }
            ["element", name, count] => header.elements.push(Element {
                name: name.to_string(),
                count: count.parse().map_err(|_| format!("bad PLY element count {count}"))?,
                properties: Vec::new(),
            }),
            ["property", "list", count, item, name] => {
                let element = header.elements.last_mut().ok_or("a PLY property comes before any element")?;
                let (count, item) = (Scalar::parse(count), Scalar::parse(item));
                let (Some(count), Some(scalar)) = (count, item) else { return Err("unknown PLY list type".into()) };
                element.properties.push(Property { name: name.to_string(), list: Some(count), scalar });
            }
            ["property", kind, name] => {
                let element = header.elements.last_mut().ok_or("a PLY property comes before any element")?;
                let scalar = Scalar::parse(kind).ok_or_else(|| format!("unknown PLY type {kind}"))?;
                element.properties.push(Property { name: name.to_string(), list: None, scalar });
            }
            _ => {}
        }
    }
    Ok(header)
}

/// Reads numbers in the file's encoding.
struct Values<'a> {
    encoding: Encoding,
    bytes: &'a [u8],
    at: usize,
}

impl Values<'_> {
    fn next(&mut self, scalar: Scalar) -> Result<f64, String> {
        const SHORT: &str = "the PLY data ends early";
        if self.encoding == Encoding::Ascii {
            let rest = &self.bytes[self.at.min(self.bytes.len())..];
            let start = rest.iter().position(|b| !b.is_ascii_whitespace()).ok_or(SHORT)?;
            let len = rest[start..].iter().position(|b| b.is_ascii_whitespace()).unwrap_or(rest.len() - start);
            self.at += start + len;
            let word = std::str::from_utf8(&rest[start..start + len]).map_err(|_| "bad number in PLY data")?;
            return word.parse::<f64>().map_err(|_| format!("bad number in PLY data: {word}"));
        }
        let size = scalar.size();
        let raw = self.bytes.get(self.at..self.at + size).ok_or(SHORT)?;
        self.at += size;
        let mut b = [0u8; 8];
        b[..size].copy_from_slice(raw);
        if self.encoding == Encoding::Big {
            b[..size].reverse();
        }
        Ok(match scalar {
            Scalar::I8 => b[0] as i8 as f64,
            Scalar::U8 => b[0] as f64,
            Scalar::I16 => i16::from_le_bytes([b[0], b[1]]) as f64,
            Scalar::U16 => u16::from_le_bytes([b[0], b[1]]) as f64,
            Scalar::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
            Scalar::U32 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
            Scalar::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
            Scalar::F64 => f64::from_le_bytes(b),
        })
    }
}

/// More list entries than this means a damaged file, not a polygon.
const MAX_LIST: f64 = 4096.0;

#[expect(clippy::cognitive_complexity, clippy::too_many_lines, reason = "refactor: split into named steps")]
fn parse(bytes: &[u8], dir: &Path, options: &LoadOptions) -> Result<Model, String> {
    let header = parse_header(bytes)?;
    let mut values = Values { encoding: header.encoding, bytes, at: header.data };
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut has_normals = false;
    let mut has_colors = false;
    // Triangles as (vertex, optional per-corner texture coordinate).
    let mut corners: Vec<(u32, Option<[f32; 2]>)> = Vec::new();
    let mut points_only = true;
    for element in &header.elements {
        let names: Vec<&str> = element.properties.iter().map(|p| p.name.as_str()).collect();
        let find = |aliases: &[&str]| aliases.iter().find_map(|alias| names.iter().position(|name| name == alias));
        match element.name.as_str() {
            "vertex" => {
                let position = [find(&["x"]), find(&["y"]), find(&["z"])];
                let normal = [find(&["nx"]), find(&["ny"]), find(&["nz"])];
                let color = [
                    find(&["red", "r", "diffuse_red"]),
                    find(&["green", "g", "diffuse_green"]),
                    find(&["blue", "b", "diffuse_blue"]),
                ];
                let alpha = find(&["alpha", "a"]);
                let uv = [find(&["s", "u", "texture_u", "texture_s"]), find(&["t", "v", "texture_v", "texture_t"])];
                has_normals = normal.iter().all(Option::is_some);
                has_colors = color.iter().all(Option::is_some);
                vertices.reserve(element.count.min(1 << 24));
                let mut row = vec![0.0f64; element.properties.len()];
                for _ in 0..element.count {
                    for (value, property) in row.iter_mut().zip(&element.properties) {
                        *value = match property.list {
                            None => values.next(property.scalar)?,
                            Some(count) => {
                                // A list on a vertex isn't something we use: step over it.
                                let n = values.next(count)?;
                                for _ in 0..n.clamp(0.0, MAX_LIST) as usize {
                                    values.next(property.scalar)?;
                                }
                                0.0
                            }
                        };
                    }
                    let get = |index: Option<usize>| index.map_or(0.0, |i| row[i] as f32);
                    let scale = |index: Option<usize>| {
                        index.map_or(1.0, |i| (row[i] / element.properties[i].scalar.color_scale()) as f32)
                    };
                    let color = if has_colors {
                        // Stored as display colours.
                        let [r, g, b] = color.map(|index| super::srgb_to_linear(scale(index)));
                        [r, g, b, scale(alpha)]
                    } else {
                        [1.0; 4]
                    };
                    let uv = uv.map(get);
                    vertices.push(Vertex {
                        position: z_up_to_y_up(position.map(get)),
                        normal: if has_normals { vector::normalize(z_up_to_y_up(normal.map(get))) } else { [0.0; 3] },
                        uv: [uv[0], 1.0 - uv[1]],
                        color,
                    });
                }
            }
            "face" => {
                let indices = find(&["vertex_indices", "vertex_index"]).ok_or("PLY faces without vertex indices")?;
                let texcoord = find(&["texcoord"]);
                let mut face: Vec<u32> = Vec::new();
                let mut uvs: Vec<f32> = Vec::new();
                for _ in 0..element.count {
                    face.clear();
                    uvs.clear();
                    for (i, property) in element.properties.iter().enumerate() {
                        match property.list {
                            Some(count) => {
                                let n = values.next(count)?;
                                if !(0.0..=MAX_LIST).contains(&n) {
                                    return Err("damaged PLY face".into());
                                }
                                for _ in 0..n as usize {
                                    let value = values.next(property.scalar)?;
                                    if i == indices {
                                        face.push(value as u32);
                                    } else if Some(i) == texcoord {
                                        uvs.push(value as f32);
                                    }
                                }
                            }
                            None => {
                                values.next(property.scalar)?;
                            }
                        }
                    }
                    if face.len() >= 3 {
                        points_only = false;
                    }
                    let uv_of = |k: usize| (uvs.len() == face.len() * 2).then(|| [uvs[k * 2], 1.0 - uvs[k * 2 + 1]]);
                    // A fan: faces are convex polygons in practice.
                    for k in 1..face.len().saturating_sub(1) {
                        corners.extend([(face[0], uv_of(0)), (face[k], uv_of(k)), (face[k + 1], uv_of(k + 1))]);
                    }
                }
            }
            _ => {
                for _ in 0..element.count {
                    for property in &element.properties {
                        match property.list {
                            Some(count) => {
                                let n = values.next(count)?;
                                for _ in 0..n.clamp(0.0, MAX_LIST) as usize {
                                    values.next(property.scalar)?;
                                }
                            }
                            None => {
                                values.next(property.scalar)?;
                            }
                        }
                    }
                }
            }
        }
    }
    if points_only {
        return Err("there are only points, no faces to draw".into());
    }

    // Faces with their own texture coordinates get their own corners.
    let count = vertices.len() as u32;
    let mut indices = Vec::with_capacity(corners.len());
    for triangle in corners.as_chunks::<3>().0 {
        if triangle.iter().any(|&(index, _)| index >= count) {
            continue;
        }
        for &(index, uv) in triangle {
            match uv {
                Some(uv) => {
                    indices.push(vertices.len() as u32);
                    vertices.push(Vertex { uv, ..vertices[index as usize] });
                }
                None => indices.push(index),
            }
        }
    }
    if !has_normals {
        (vertices, indices) = geometry::smooth_normals(&vertices, &indices, DEFAULT_CREASE);
    }

    let mut builder = Builder::default();
    let texture = header.texture.as_deref().filter(|_| options.max_texture > 0).map(|name| {
        let decoded = textures::from_file(dir, name, options.max_texture, Alpha::Guess);
        builder.texture_slot(decoded.as_ref().map(Arc::as_ref))
    });
    let material = match texture {
        Some(slot) => builder.material(header.texture.clone().unwrap_or_default(), [1.0; 4], slot),
        None => builder.default_material(has_colors),
    };
    let base = builder.add_vertices(&vertices);
    builder.add_triangles(material, base, &indices);
    builder.meshes = 1;
    builder.source_vertices = Some(count as u64);
    let format = if header.encoding == Encoding::Ascii { "PLY ASCII" } else { "PLY binary" };
    builder.finish(format)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> LoadOptions {
        LoadOptions { max_texture: 64 }
    }

    const ASCII: &str = "ply\nformat ascii 1.0\ncomment made by hand\nelement vertex 4\n\
        property float x\nproperty float y\nproperty float z\n\
        property uchar red\nproperty uchar green\nproperty uchar blue\n\
        element face 1\nproperty list uchar int vertex_indices\nend_header\n\
        0 0 0 255 0 0\n1 0 0 255 0 0\n1 1 0 255 0 0\n0 1 0 255 0 0\n4 0 1 2 3\n";

    #[test]
    fn ascii_quad_with_colours() {
        let model = parse(ASCII.as_bytes(), Path::new("."), &options()).unwrap();
        assert_eq!(model.info.format, "PLY ASCII");
        assert_eq!(model.info.triangles, 2);
        assert_eq!(model.info.vertices, 4);
        assert!(model.vertices.iter().all(|v| v.color == [1.0, 0.0, 0.0, 1.0]));
        // Vertex colours carry the colour: the material is white.
        assert_eq!(model.materials[model.parts[0].material as usize].color, [1.0; 4]);
        // Z up: the quad lying in the file's XY plane faces +Y here.
        assert!(model.vertices.iter().all(|v| v.normal == [0.0, 1.0, 0.0]));
    }

    /// A triangle with normals as doubles and floats, a face with a flag before its list, and an
    /// edge element to step over.
    fn binary(big: bool) -> Vec<u8> {
        let mut out = format!(
            "ply\nformat {} 1.0\nelement vertex 3\nproperty double x\nproperty double y\nproperty double z\n\
             property float nx\nproperty float ny\nproperty float nz\n\
             element face 1\nproperty uchar flags\nproperty list uchar uint vertex_indices\n\
             element edge 1\nproperty int vertex1\nproperty int vertex2\nend_header\n",
            if big { "binary_big_endian" } else { "binary_little_endian" }
        )
        .into_bytes();
        let f64s = |v: f64| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        let f32s = |v: f32| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        let u32s = |v: u32| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        for p in [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]] {
            for c in p {
                out.extend(f64s(c));
            }
            for c in [0.0, 0.0, 1.0] {
                out.extend(f32s(c));
            }
        }
        out.push(7);
        out.push(3);
        for i in [0, 1, 2] {
            out.extend(u32s(i));
        }
        out.extend([0u8; 8]);
        out
    }

    #[test]
    fn binary_both_byte_orders_with_extra_elements() {
        for big in [false, true] {
            let model = parse(&binary(big), Path::new("."), &options()).unwrap();
            assert_eq!(model.info.format, "PLY binary");
            assert_eq!(model.info.triangles, 1);
            assert_eq!(model.bounds.max, [2.0, 0.0, 0.0]);
            assert_eq!(model.bounds.min, [0.0, 0.0, -2.0]);
            assert!(model.vertices.iter().all(|v| v.normal == [0.0, 1.0, 0.0]), "file normals, turned Y up");
        }
    }

    #[test]
    fn damaged_files_never_panic() {
        for original in [ASCII.as_bytes().to_vec(), binary(false), binary(true)] {
            for bytes in crate::model::test_files::damaged(&original, 300) {
                let _ = parse(&bytes, Path::new("."), &options());
            }
        }
    }

    #[test]
    fn point_clouds_and_broken_files() {
        let points = "ply\nformat ascii 1.0\nelement vertex 2\nproperty float x\nproperty float y\nproperty float z\nend_header\n0 0 0\n1 1 1\n";
        assert_eq!(
            parse(points.as_bytes(), Path::new("."), &options()).err().unwrap(),
            "there are only points, no faces to draw"
        );
        assert!(parse(b"ply\nformat ascii 1.0\nelement vertex 2\n", Path::new("."), &options()).is_err());
        assert!(parse(&ASCII.as_bytes()[..ASCII.len() - 6], Path::new("."), &options()).is_err());
        assert!(parse(b"not ply", Path::new("."), &options()).is_err());
    }
}
