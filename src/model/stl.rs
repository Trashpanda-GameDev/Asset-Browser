//! STL, binary and ASCII: a list of triangles, each with its own three corners, Z up. The
//! unit isn't stored; the numbers are taken as metres.
//!
//! Binary: an 80-byte header, a u32 triangle count, then 50 bytes per triangle (normal, three
//! corners, a u16 attribute). ASCII: "solid", then `facet normal … outer loop vertex x y z (×3)
//! endloop endfacet` per triangle. Some binary files start their header with "solid" too, so the
//! size decides: a binary file is exactly 84 + 50 × count bytes.

use std::path::Path;

use super::builder::Builder;
use super::vector::{cross, normalize, sub};
use super::{LoadOptions, Model, ModelError, Vertex, z_up_to_y_up};

/// STL has nothing beside it to read, and no textures.
pub(super) fn parse(bytes: &[u8], _file: &Path, _options: &LoadOptions) -> Result<Model, ModelError> {
    let count = bytes.get(80..84).map(|count| u32::from_le_bytes([count[0], count[1], count[2], count[3]]) as u64);
    // Some exporters add a few bytes after the last triangle.
    let binary_size = count.map(|count| 84 + 50 * count).filter(|&size| size <= bytes.len() as u64);
    let ascii = bytes.trim_ascii_start().starts_with(b"solid");
    // Before reading them, when the file really holds that many.
    if let (Some(count), Some(_)) = (count, binary_size)
        && !ascii
    {
        super::check_triangle_count(count)?;
    }
    let as_binary = |size: u64| (binary(&bytes[84..size as usize]), "STL binary");
    let (corners, format) = match binary_size {
        Some(size) if size == bytes.len() as u64 || !ascii => as_binary(size),
        _ if ascii => match (text(bytes), binary_size) {
            (Ok(corners), _) if !corners.is_empty() => (corners, "STL ASCII"),
            (_, Some(size)) => as_binary(size),
            (result, None) => (result?, "STL ASCII"),
        },
        _ => return Err("not an STL file, or it's cut short".into()),
    };
    if corners.is_empty() {
        return Err("the file has no triangles".into());
    }
    super::check_triangle_count(corners.len() as u64 / 3)?;

    // The stored facet normals are often zero or stale, so the corners decide: faceted shading,
    // as STL viewers show it.
    let mut vertices = Vec::with_capacity(corners.len());
    for triangle in corners.as_chunks::<3>().0 {
        let [a, b, c] = triangle.map(z_up_to_y_up);
        let normal = normalize(cross(sub(b, a), sub(c, a)));
        vertices.extend([a, b, c].map(|position| Vertex { position, normal, uv: [0.0; 2], color: [1.0; 4] }));
    }
    let mut builder = Builder::default();
    let material = builder.default_material(false);
    let base = builder.add_vertices(&vertices);
    let indices: Vec<u32> = (0..vertices.len() as u32).collect();
    builder.add_triangles(material, base, &indices);
    builder.count_mesh();
    builder.finish(format)
}

fn binary(data: &[u8]) -> Vec<[f32; 3]> {
    let mut corners = Vec::with_capacity(data.len() / 50 * 3);
    for record in data.as_chunks::<50>().0 {
        for corner in 0..3 {
            let at = 12 + corner * 12;
            corners.push(std::array::from_fn(|axis| {
                let b = &record[at + axis * 4..][..4];
                f32::from_le_bytes([b[0], b[1], b[2], b[3]])
            }));
        }
    }
    corners
}

fn text(bytes: &[u8]) -> Result<Vec<[f32; 3]>, String> {
    let text = String::from_utf8_lossy(bytes);
    let mut corners = Vec::new();
    let mut words = text.split_ascii_whitespace();
    while let Some(word) = words.next() {
        if word.eq_ignore_ascii_case("vertex") {
            let mut corner = [0.0; 3];
            for value in &mut corner {
                let word = words.next().ok_or("an ASCII STL vertex is cut short")?;
                *value = word.parse().map_err(|_| format!("not a number in an ASCII STL vertex: {word}"))?;
            }
            corners.push(corner);
        }
    }
    // A last triangle missing corners is dropped.
    corners.truncate(corners.len() / 3 * 3);
    Ok(corners)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(bytes: &[u8]) -> Result<Model, ModelError> {
        parse(bytes, Path::new("test.stl"), &LoadOptions::default())
    }

    fn binary_file(triangles: &[[[f32; 3]; 3]], header: &[u8]) -> Vec<u8> {
        let mut out = header.to_vec();
        out.resize(80, 0);
        out.extend((triangles.len() as u32).to_le_bytes());
        for triangle in triangles {
            out.extend([0u8; 12]); // facet normal left zero, as many exporters do
            for corner in triangle {
                out.extend(corner.iter().flat_map(|c| c.to_le_bytes()));
            }
            out.extend([0u8; 2]);
        }
        out
    }

    const TRIANGLE: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 3.0, 1.0]];

    #[test]
    fn binary_even_when_the_header_says_solid() {
        for header in [&b"exported by a CAD tool"[..], b"solid but binary"] {
            let model = read(&binary_file(&[TRIANGLE, TRIANGLE], header)).unwrap();
            assert_eq!(model.info.format, "STL binary");
            assert_eq!(model.info.triangles, 2);
            // Z up becomes Y up: (0, 3, 1) → (0, 1, -3).
            assert_eq!(model.bounds.min, [0.0, 0.0, -3.0]);
            assert_eq!(model.bounds.max, [2.0, 1.0, 0.0]);
        }
    }

    #[test]
    fn ascii_with_flat_normals() {
        let text = b"solid part\n facet normal 0 0 0\n  outer loop\n   vertex 0 0 0\n   vertex 1 0 0\n   vertex 0 1 0\n  endloop\n endfacet\nendsolid part\n";
        let model = read(text).unwrap();
        assert_eq!(model.info.format, "STL ASCII");
        assert_eq!(model.info.triangles, 1);
        // Facing up in the file (+Z) is facing up here (+Y).
        assert!(model.vertices.iter().all(|v| v.normal == [0.0, 1.0, 0.0]), "{:?}", model.vertices[0].normal);
    }

    #[test]
    fn damaged_files_never_panic() {
        let ascii = b"solid part\n facet normal 0 0 1\n  outer loop\n   vertex 0 0 0\n   vertex 1 0 0\n   vertex 0 1 0\n  endloop\n endfacet\nendsolid part\n";
        for original in [binary_file(&[TRIANGLE, TRIANGLE], b""), ascii.to_vec()] {
            for bytes in crate::test_support::damaged(&original, 300) {
                let _ = read(&bytes);
            }
        }
    }

    #[test]
    fn broken_files() {
        assert!(read(b"").is_err());
        assert!(read(b"solid x\n facet normal 0 0 0 outer loop vertex 1 nope").is_err());
        let mut cut = binary_file(&[TRIANGLE], b"");
        cut.truncate(100);
        assert!(read(&cut).is_err());
        assert!(read(&binary_file(&[], b"")).is_err());
    }
}
