//! 3D Studio .3ds: nested chunks, each a u16 id and a u32 length (header included), Z up. Units
//! aren't reliable (the master scale is rarely set), so the numbers are taken as metres.
//!
//! The editor chunk (0x3D3D) holds materials (0xAFFF: name, diffuse colour, texture map) and
//! objects (0x4000: name, then a triangle mesh 0x4100 with points, faces, per-face material
//! groups, smoothing groups, texture coordinates and the object's matrix). Points are stored in
//! world space already. Normals aren't stored: smoothing groups say which faces share them.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use super::geometry::{self, Smoothing};
use super::textures::{self, Alpha};
use super::vector::{cross, dot};
use super::{Builder, LoadOptions, Model, ModelError, Slot, Vertex, srgb_to_linear, z_up_to_y_up};

const MAIN: u16 = 0x4D4D;
const EDITOR: u16 = 0x3D3D;
const MATERIAL: u16 = 0xAFFF;
const MATERIAL_NAME: u16 = 0xA000;
const DIFFUSE: u16 = 0xA020;
const TEXTURE_MAP: u16 = 0xA200;
const MAP_NAME: u16 = 0xA300;
const COLOR_FLOAT: u16 = 0x0010;
const COLOR_24: u16 = 0x0011;
const OBJECT: u16 = 0x4000;
const TRIANGLES: u16 = 0x4100;
const POINTS: u16 = 0x4110;
const FACES: u16 = 0x4120;
const FACE_MATERIAL: u16 = 0x4130;
const SMOOTHING: u16 = 0x4150;
const UVS: u16 = 0x4140;
const MATRIX: u16 = 0x4160;

struct Chunk<'a> {
    id: u16,
    data: &'a [u8],
}

/// The chunks laid end to end in `data`. A damaged length ends the list; a last chunk cut short
/// keeps what's there.
fn chunks(mut data: &[u8]) -> impl Iterator<Item = Chunk<'_>> {
    std::iter::from_fn(move || {
        let header = data.get(..6)?;
        let id = u16::from_le_bytes([header[0], header[1]]);
        let len = u32::from_le_bytes([header[2], header[3], header[4], header[5]]) as usize;
        if len < 6 {
            return None;
        }
        let len = len.min(data.len());
        let chunk = Chunk { id, data: &data[6..len] };
        data = &data[len..];
        Some(chunk)
    })
}

/// A zero-terminated name and what follows it.
fn cstring(data: &[u8]) -> (String, &[u8]) {
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    let rest = data.get(end + 1..).unwrap_or(&[]);
    (String::from_utf8_lossy(&data[..end]).into_owned(), rest)
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn f32_at(data: &[u8], at: usize) -> Option<f32> {
    Some(f32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

#[derive(Default)]
struct MaterialDef {
    name: String,
    /// Linear.
    color: Option<[f32; 3]>,
    texture: Option<String>,
}

fn material_def(data: &[u8]) -> MaterialDef {
    let mut def = MaterialDef::default();
    for chunk in chunks(data) {
        match chunk.id {
            MATERIAL_NAME => def.name = cstring(chunk.data).0,
            DIFFUSE => {
                // The gamma-corrected colour comes first; a linear copy may follow.
                for color in chunks(chunk.data) {
                    let display = match color.id {
                        COLOR_24 if color.data.len() >= 3 => [0, 1, 2].map(|i| color.data[i] as f32 / 255.0),
                        COLOR_FLOAT => match [0, 4, 8].map(|at| f32_at(color.data, at)) {
                            [Some(r), Some(g), Some(b)] => [r, g, b],
                            _ => continue,
                        },
                        _ => continue,
                    };
                    def.color = Some(display.map(|c| srgb_to_linear(c.clamp(0.0, 1.0))));
                    break;
                }
            }
            TEXTURE_MAP => {
                if let Some(name) = chunks(chunk.data).find(|c| c.id == MAP_NAME) {
                    def.texture = Some(cstring(name.data).0).filter(|name| !name.is_empty());
                }
            }
            _ => {}
        }
    }
    def
}

pub(super) fn parse(bytes: &[u8], file: &Path, options: &LoadOptions) -> Result<Model, ModelError> {
    let dir = super::folder(file);
    let main = chunks(bytes).next().filter(|chunk| chunk.id == MAIN).ok_or("not a 3DS file")?;
    let editors: Vec<Chunk> = chunks(main.data).filter(|chunk| chunk.id == EDITOR).collect();
    let defs: Vec<MaterialDef> = editors
        .iter()
        .flat_map(|editor| chunks(editor.data))
        .filter(|chunk| chunk.id == MATERIAL)
        .map(|chunk| material_def(chunk.data))
        .collect();

    let mut reader = Reader {
        builder: Builder::default(),
        defs: &defs,
        materials: HashMap::new(),
        textures: HashMap::new(),
        dir,
        max_texture: options.max_texture,
        points: 0,
    };
    for object in editors.iter().flat_map(|editor| chunks(editor.data)).filter(|chunk| chunk.id == OBJECT) {
        let (_name, rest) = cstring(object.data);
        for mesh in chunks(rest).filter(|chunk| chunk.id == TRIANGLES) {
            reader.add_mesh(mesh.data);
        }
    }
    let points = reader.points;
    let mut builder = reader.builder;
    builder.set_source_vertices(points);
    builder.finish("3DS")
}

struct Reader<'a> {
    builder: Builder,
    defs: &'a [MaterialDef],
    /// Material name → ours.
    materials: HashMap<String, u32>,
    /// Texture file name → what became of it.
    textures: HashMap<String, Slot>,
    dir: &'a Path,
    max_texture: u32,
    points: u64,
}

impl Reader<'_> {
    #[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
    fn add_mesh(&mut self, data: &[u8]) {
        let mut points: Vec<[f32; 3]> = Vec::new();
        let mut uvs: Vec<[f32; 2]> = Vec::new();
        let mut faces: Vec<[u32; 3]> = Vec::new();
        let mut groups: Vec<(String, Vec<u16>)> = Vec::new();
        let mut smoothing: Vec<u32> = Vec::new();
        let mut mirrored = false;
        for chunk in chunks(data) {
            let data = chunk.data;
            let count = u16_at(data, 0).unwrap_or(0) as usize;
            match chunk.id {
                POINTS => {
                    points = (0..count)
                        .map_while(|i| {
                            Some([f32_at(data, 2 + i * 12)?, f32_at(data, 6 + i * 12)?, f32_at(data, 10 + i * 12)?])
                        })
                        .collect();
                }
                UVS => {
                    // v counts up from the bottom of the picture.
                    uvs = (0..count)
                        .map_while(|i| Some([f32_at(data, 2 + i * 8)?, 1.0 - f32_at(data, 6 + i * 8)?]))
                        .collect();
                }
                FACES => {
                    faces = (0..count)
                        .map_while(|i| {
                            let at = 2 + i * 8;
                            Some([u16_at(data, at)? as u32, u16_at(data, at + 2)? as u32, u16_at(data, at + 4)? as u32])
                        })
                        .collect();
                    let rest = data.get(2 + count * 8..).unwrap_or(&[]);
                    for sub in chunks(rest) {
                        match sub.id {
                            FACE_MATERIAL => {
                                let (name, list) = cstring(sub.data);
                                let n = u16_at(list, 0).unwrap_or(0) as usize;
                                let faces = (0..n).map_while(|i| u16_at(list, 2 + i * 2)).collect();
                                groups.push((name, faces));
                            }
                            SMOOTHING => {
                                smoothing =
                                    sub.data.as_chunks::<4>().0.iter().map(|b| u32::from_le_bytes(*b)).collect();
                            }
                            _ => {}
                        }
                    }
                }
                MATRIX => {
                    // Rows: the X, Y and Z axes, then the origin.
                    let m: Vec<f32> = (0..9).map_while(|i| f32_at(data, i * 4)).collect();
                    if let [x0, x1, x2, y0, y1, y2, z0, z1, z2] = m[..] {
                        mirrored = dot([x0, x1, x2], cross([y0, y1, y2], [z0, z1, z2])) < 0.0;
                    }
                }
                _ => {}
            }
        }
        faces.retain(|face| face.iter().all(|&index| (index as usize) < points.len()));
        if faces.is_empty() {
            return;
        }
        self.points += points.len() as u64;

        // Faces in material order, each group's faces once; faces in no group last.
        let mut order: Vec<(Option<usize>, u32)> = Vec::with_capacity(faces.len());
        let mut placed = vec![false; faces.len()];
        for (group, (_, list)) in groups.iter().enumerate() {
            for &face in list {
                if let Some(seen @ false) = placed.get_mut(face as usize).map(|p| *p) {
                    let _ = seen;
                    placed[face as usize] = true;
                    order.push((Some(group), face as u32));
                }
            }
        }
        order.extend((0..faces.len() as u32).filter(|&face| !placed[face as usize]).map(|face| (None, face)));

        let vertices: Vec<Vertex> = points
            .iter()
            .enumerate()
            .map(|(i, &p)| Vertex {
                position: z_up_to_y_up(p),
                normal: [0.0; 3],
                uv: uvs.get(i).copied().unwrap_or([0.0; 2]),
                color: [1.0; 4],
            })
            .collect();
        let mut indices = Vec::with_capacity(order.len() * 3);
        let mut face_groups = Vec::with_capacity(order.len());
        for &(_, face) in &order {
            let [a, b, c] = faces[face as usize];
            // An object mirrored in the editor keeps its faces' winding from before the mirror.
            indices.extend(if mirrored { [a, c, b] } else { [a, b, c] });
            face_groups.push(smoothing.get(face as usize).copied().unwrap_or(0));
        }
        let smoothing =
            if smoothing.len() >= faces.len() { Smoothing::Groups(&face_groups) } else { geometry::DEFAULT_CREASE };
        let (vertices, indices) = geometry::smooth_normals(&vertices, &indices, smoothing);

        let base = self.builder.add_vertices(&vertices);
        let mut start = 0;
        while start < order.len() {
            let group = order[start].0;
            let end = start + order[start..].iter().take_while(|(g, _)| *g == group).count();
            let material = match group {
                Some(group) => self.material(&groups[group].0),
                None => self.builder.default_material(false),
            };
            self.builder.add_triangles(material, base, &indices[start * 3..end * 3]);
            start = end;
        }
        self.builder.count_mesh();
    }

    fn material(&mut self, name: &str) -> u32 {
        if let Some(&index) = self.materials.get(name) {
            return index;
        }
        let def = self.defs.iter().find(|def| def.name == name);
        let index = match def {
            None => self.builder.default_material(false),
            Some(def) => {
                let [r, g, b] = def.color.unwrap_or([1.0; 3]);
                let slot = match &def.texture {
                    Some(texture) if self.max_texture > 0 => self.texture(texture),
                    _ => Slot::None,
                };
                self.builder.material(def.name.clone(), [r, g, b, 1.0], slot)
            }
        };
        self.materials.insert(name.to_owned(), index);
        index
    }

    fn texture(&mut self, name: &str) -> Slot {
        if let Some(&slot) = self.textures.get(name) {
            return slot;
        }
        let decoded = textures::from_file(self.dir, name, self.max_texture, Alpha::Guess);
        let slot = self.builder.texture_slot(decoded.as_ref().map(Arc::as_ref));
        self.textures.insert(name.to_owned(), slot);
        slot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: u16, body: &[u8]) -> Vec<u8> {
        let mut out = id.to_le_bytes().to_vec();
        out.extend((body.len() as u32 + 6).to_le_bytes());
        out.extend(body);
        out
    }

    /// A unit square in the XY plane (Z up), as two faces with one material and one smoothing
    /// group, plus UVs.
    fn file(with_material: bool) -> Vec<u8> {
        let mut points = 4u16.to_le_bytes().to_vec();
        for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]] {
            points.extend(p.iter().flat_map(|c| c.to_le_bytes()));
        }
        let mut uvs = 4u16.to_le_bytes().to_vec();
        for uv in [[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]] {
            uvs.extend(uv.iter().flat_map(|c| c.to_le_bytes()));
        }
        let mut faces = 2u16.to_le_bytes().to_vec();
        for face in [[0u16, 1, 2], [0, 2, 3]] {
            faces.extend(face.iter().flat_map(|i| i.to_le_bytes()));
            faces.extend(0u16.to_le_bytes());
        }
        if with_material {
            let mut group = b"Brick\0".to_vec();
            group.extend(2u16.to_le_bytes());
            group.extend([0u8, 0, 1, 0]);
            faces.extend(chunk(FACE_MATERIAL, &group));
        }
        faces.extend(chunk(SMOOTHING, &[1, 0, 0, 0, 1, 0, 0, 0]));
        let mut mesh = chunk(POINTS, &points);
        mesh.extend(chunk(UVS, &uvs));
        mesh.extend(chunk(FACES, &faces));
        let mut object = b"Wall\0".to_vec();
        object.extend(chunk(TRIANGLES, &mesh));

        let mut material = chunk(MATERIAL_NAME, b"Brick\0");
        material.extend(chunk(DIFFUSE, &chunk(COLOR_24, &[255, 128, 0])));
        let mut editor = chunk(MATERIAL, &material);
        editor.extend(chunk(OBJECT, &object));
        let mut main = chunk(0x0002, &3u32.to_le_bytes());
        main.extend(chunk(EDITOR, &editor));
        chunk(MAIN, &main)
    }

    #[test]
    fn square_with_material_and_smoothing() {
        let model = parse(&file(true), Path::new("test.3ds"), &LoadOptions::default()).unwrap();
        assert_eq!(model.info.format, "3DS");
        assert_eq!((model.info.triangles, model.info.vertices, model.info.materials), (2, 4, 1));
        let brick = &model.materials[model.parts[0].material as usize];
        assert_eq!(brick.name, "Brick");
        assert_eq!(brick.color[0], 1.0);
        assert!((brick.color[1] - srgb_to_linear(128.0 / 255.0)).abs() < 1e-5);
        // Z up: the square faces +Y here, smoothed into one normal.
        assert_eq!(model.vertices.len(), 4);
        assert!(model.vertices.iter().all(|v| (v.normal[1] - 1.0).abs() < 1e-6));
        assert_eq!(model.bounds.min, [0.0, 0.0, -1.0]);
        let corner = model.vertices.iter().find(|v| v.position == [1.0, 0.0, -1.0]).unwrap();
        assert_eq!(corner.uv, [1.0, 0.0], "v flipped to count from the top");
    }

    #[test]
    fn damaged_files_never_panic() {
        for bytes in crate::test_support::damaged(&file(true), 500) {
            let _ = parse(&bytes, Path::new("test.3ds"), &LoadOptions::default());
        }
    }

    #[test]
    fn faces_without_material_and_broken_files() {
        let model = parse(&file(false), Path::new("test.3ds"), &LoadOptions::default()).unwrap();
        assert_eq!(model.info.materials, 0);
        assert_eq!(model.info.triangles, 2);
        let data = file(true);
        for cut in [10, 40, data.len() / 2] {
            // Cut short: an error or fewer triangles, never a panic.
            let _ = parse(&data[..cut], Path::new("test.3ds"), &LoadOptions::default());
        }
        assert!(parse(b"MM not 3ds", Path::new("test.3ds"), &LoadOptions::default()).is_err());
    }
}
