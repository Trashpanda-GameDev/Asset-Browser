//! Geometry helpers the loaders share: node transforms, winding, polygons cut into triangles, and
//! normals for files that have none.

use std::collections::HashMap;

use super::Vertex;
use super::vector::{cross, dot, normalize, sub};

/// Twice the triangle's area along its normal: the area-weighted face normal.
fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    cross(sub(b, a), sub(c, a))
}

/// Column-major 4 × 4 matrix, `m[column][row]`, as glTF and our node walks use.
pub(super) type Mat4 = [[f32; 4]; 4];

pub(super) const IDENTITY: Mat4 =
    [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

pub(super) fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0; 4]; 4];
    for (column, out_column) in out.iter_mut().enumerate() {
        for (row, value) in out_column.iter_mut().enumerate() {
            *value = (0..4).map(|k| a[k][row] * b[column][k]).sum();
        }
    }
    out
}

/// 16 numbers row by row, as Collada stores a matrix; `None` when there are fewer.
pub(super) fn from_rows(values: &[f32]) -> Option<Mat4> {
    (values.len() >= 16).then(|| std::array::from_fn(|column| std::array::from_fn(|row| values[row * 4 + column])))
}

pub(super) fn translation([x, y, z]: [f32; 3]) -> Mat4 {
    [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [x, y, z, 1.0]]
}

pub(super) fn scaling([x, y, z]: [f32; 3]) -> Mat4 {
    [[x, 0.0, 0.0, 0.0], [0.0, y, 0.0, 0.0], [0.0, 0.0, z, 0.0], [0.0, 0.0, 0.0, 1.0]]
}

/// Rotation by `angle` radians about the unit `axis`, by the right-hand rule.
pub(super) fn rotation(axis: [f32; 3], angle: f32) -> Mat4 {
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

pub(super) fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|row| m[0][row] * p[0] + m[1][row] * p[1] + m[2][row] * p[2] + m[3][row])
}

/// Transforms normals by the inverse transpose of `m`'s 3 × 3 part, so they stay perpendicular
/// under non-uniform scale. Not normalised.
#[derive(Clone, Copy)]
pub(super) struct NormalMatrix([[f32; 3]; 3]);

impl NormalMatrix {
    pub(super) fn new(m: &Mat4) -> Self {
        // The cofactor matrix is the inverse transpose times the determinant. Directions only
        // need the determinant's sign, and this way a singular matrix doesn't divide by zero.
        let c = |col: usize| [m[col][0], m[col][1], m[col][2]];
        let (x, y, z) = (c(0), c(1), c(2));
        let sign = if dot(x, cross(y, z)) < 0.0 { -1.0 } else { 1.0 };
        Self([cross(y, z), cross(z, x), cross(x, y)].map(|column| column.map(|v| v * sign)))
    }

    pub(super) fn apply(&self, n: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        [0, 1, 2].map(|row| m[0][row] * n[0] + m[1][row] * n[1] + m[2][row] * n[2])
    }
}

/// Negative for a mirroring transform, whose triangles then wind the other way.
pub(super) fn determinant(m: &Mat4) -> f32 {
    let c = |col: usize| [m[col][0], m[col][1], m[col][2]];
    dot(c(0), cross(c(1), c(2)))
}

/// Reverses every triangle's winding. A mirroring transform reverses it (see `determinant`), and
/// reversing it back keeps front faces counter-clockwise.
pub(super) fn flip_winding(indices: &mut [u32]) {
    for triangle in indices.as_chunks_mut::<3>().0 {
        triangle.swap(1, 2);
    }
}

/// A convex polygon of `corners` corners cut into a fan from its first: each triangle as three of
/// the polygon's corners. Fewer than three corners make none.
pub(super) fn fan(corners: usize) -> impl Iterator<Item = [usize; 3]> {
    (1..corners.saturating_sub(1)).map(|k| [0, k, k + 1])
}

/// A triangle strip of `corners` corners as separate triangles, each as three of the strip's
/// corners. Every other one is turned back, so they all wind the same way.
pub(super) fn strip(corners: usize) -> impl Iterator<Item = [usize; 3]> {
    (0..corners.saturating_sub(2)).map(|i| if i % 2 == 0 { [i, i + 1, i + 2] } else { [i + 1, i, i + 2] })
}

/// Which faces around a shared position get one smooth normal.
#[derive(Clone, Copy)]
pub(super) enum Smoothing<'a> {
    /// Faces meeting at less than this angle (as its cosine) are smoothed together, so boxes
    /// keep their edges and curved surfaces look round.
    Crease(f32),
    /// 3DS smoothing groups, one bit set per group, per triangle: faces sharing a group are
    /// smoothed together, and a face in no group is flat.
    Groups(&'a [u32]),
}

/// The crease angle for files that give no smoothing information: 50°.
pub(super) const DEFAULT_CREASE: Smoothing<'static> = Smoothing::Crease(0.642_787_6);

/// Gives every triangle corner a normal: the area-weighted average of the faces around its
/// position that are smoothed together with its own, then merges corners that came out equal.
pub(super) fn smooth_normals(vertices: &[Vertex], indices: &[u32], smoothing: Smoothing) -> (Vec<Vertex>, Vec<u32>) {
    let (position_of, positions) = shared_positions(vertices);
    let normals: Vec<[f32; 3]> = indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            face_normal(
                vertices[t[0] as usize].position,
                vertices[t[1] as usize].position,
                vertices[t[2] as usize].position,
            )
        })
        .collect();
    let units: Vec<[f32; 3]> = normals.iter().map(|&n| normalize(n)).collect();
    let around = FacesAround::new(indices, &position_of, positions);
    let smooth = |a: usize, b: usize| match smoothing {
        Smoothing::Crease(cos_limit) => dot(units[a], units[b]) >= cos_limit,
        Smoothing::Groups(groups) => groups.get(a).zip(groups.get(b)).is_some_and(|(a, b)| a & b != 0),
    };
    let mut out_vertices = Vec::with_capacity(vertices.len());
    let mut out_indices = Vec::with_capacity(indices.len());
    let mut merged: HashMap<(u32, [u32; 3]), u32> = HashMap::with_capacity(vertices.len());
    for (corner, &index) in indices.iter().enumerate() {
        let face = corner / 3;
        let mut sum = normals[face];
        for &other in around.at(position_of[index as usize] as usize) {
            let other = other as usize;
            if other != face && smooth(face, other) {
                sum = [sum[0] + normals[other][0], sum[1] + normals[other][1], sum[2] + normals[other][2]];
            }
        }
        let mut normal = normalize(sum);
        if normal == [0.0; 3] {
            normal = units[face];
        }
        let key = (index, normal.map(f32::to_bits));
        let next = out_vertices.len() as u32;
        let out = *merged.entry(key).or_insert(next);
        if out == next {
            out_vertices.push(Vertex { normal, ..vertices[index as usize] });
        }
        out_indices.push(out);
    }
    (out_vertices, out_indices)
}

/// Each vertex's position as a number, the same for vertices at the same position: matched by
/// value, since files often split vertices at UV seams. And how many positions there are.
fn shared_positions(vertices: &[Vertex]) -> (Vec<u32>, usize) {
    let mut ids: HashMap<[u32; 3], u32> = HashMap::with_capacity(vertices.len());
    let position_of = vertices
        .iter()
        .map(|vertex| {
            // + 0.0 folds -0.0 into 0.0.
            let key = vertex.position.map(|c| (c + 0.0).to_bits());
            let next = ids.len() as u32;
            *ids.entry(key).or_insert(next)
        })
        .collect();
    (position_of, ids.len())
}

/// The faces around each position, as one list sliced by `start`.
struct FacesAround {
    start: Vec<u32>,
    faces: Vec<u32>,
}

impl FacesAround {
    fn new(indices: &[u32], position_of: &[u32], positions: usize) -> Self {
        let mut start = vec![0u32; positions + 1];
        for &index in indices {
            start[position_of[index as usize] as usize + 1] += 1;
        }
        for i in 1..start.len() {
            start[i] += start[i - 1];
        }
        let mut fill = start.clone();
        let mut faces = vec![0u32; indices.len()];
        for (corner, &index) in indices.iter().enumerate() {
            let slot = &mut fill[position_of[index as usize] as usize];
            faces[*slot as usize] = (corner / 3) as u32;
            *slot += 1;
        }
        Self { start, faces }
    }

    /// The faces with a corner at `position`.
    fn at(&self, position: usize) -> &[u32] {
        &self.faces[self.start[position] as usize..self.start[position + 1] as usize]
    }
}

/// Gives every triangle its own three vertices carrying the face normal: faceted shading, as glTF
/// asks for when normals are missing.
pub(super) fn flat_normals(vertices: &[Vertex], indices: &[u32]) -> (Vec<Vertex>, Vec<u32>) {
    let mut out = Vec::with_capacity(indices.len());
    for triangle in indices.as_chunks::<3>().0 {
        let [a, b, c] = triangle.map(|index| vertices[index as usize]);
        let normal = normalize(face_normal(a.position, b.position, c.position));
        out.extend([a, b, c].map(|vertex| Vertex { normal, ..vertex }));
    }
    let indices = (0..out.len() as u32).collect();
    (out, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fans_strips_and_winding() {
        assert_eq!(fan(5).collect::<Vec<_>>(), [[0, 1, 2], [0, 2, 3], [0, 3, 4]]);
        assert_eq!(strip(5).collect::<Vec<_>>(), [[0, 1, 2], [2, 1, 3], [2, 3, 4]]);
        for corners in 0..3 {
            assert_eq!((fan(corners).count(), strip(corners).count()), (0, 0), "{corners} corners");
        }
        let mut indices = [0, 1, 2, 3, 4, 5];
        flip_winding(&mut indices);
        assert_eq!(indices, [0, 2, 1, 3, 5, 4]);
    }

    #[test]
    fn matrices_place_points_as_named() {
        let quarter = rotation([0.0, 0.0, 1.0], std::f32::consts::FRAC_PI_2);
        let p = transform_point(&quarter, [1.0, 0.0, 0.0]);
        assert!((p[0]).abs() < 1e-6 && (p[1] - 1.0).abs() < 1e-6, "right-hand rule: {p:?}");
        let moved = mul(&translation([1.0, 2.0, 3.0]), &scaling([2.0; 3]));
        assert_eq!(transform_point(&moved, [1.0, 1.0, 1.0]), [3.0, 4.0, 5.0], "scaled, then moved");
        let rows: Vec<f32> = (0..16).map(|i| i as f32).collect();
        assert_eq!(from_rows(&rows).map(|m| m[3]), Some([3.0, 7.0, 11.0, 15.0]), "the last column holds the move");
        assert_eq!(from_rows(&rows[..15]), None);
    }

    fn vertex(position: [f32; 3]) -> Vertex {
        Vertex { position, normal: [0.0; 3], uv: [0.0; 2], color: [1.0; 4] }
    }

    /// A unit cube's 8 corners shared by 12 triangles, wound counter-clockwise from outside.
    fn cube() -> (Vec<Vertex>, Vec<u32>) {
        let vertices = (0..8).map(|i| vertex([(i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32])).collect();
        let indices = vec![
            0, 2, 1, 1, 2, 3, // -Z
            4, 5, 6, 5, 7, 6, // +Z
            0, 1, 4, 1, 5, 4, // -Y
            2, 6, 3, 3, 6, 7, // +Y
            0, 4, 2, 2, 4, 6, // -X
            1, 3, 5, 3, 7, 5, // +X
        ];
        (vertices, indices)
    }

    #[test]
    fn crease_keeps_cube_edges_sharp() {
        let (vertices, indices) = cube();
        let (out, out_indices) = smooth_normals(&vertices, &indices, DEFAULT_CREASE);
        assert_eq!(out.len(), 24, "each corner splits into one vertex per side");
        for triangle in out_indices.as_chunks::<3>().0 {
            let [a, b, c] = triangle.map(|i| out[i as usize]);
            let expected = normalize(face_normal(a.position, b.position, c.position));
            for vertex in [a, b, c] {
                assert_eq!(vertex.normal, expected);
            }
        }
    }

    #[test]
    fn smoothing_groups_round_a_shared_corner() {
        let (vertices, indices) = cube();
        let groups = vec![1u32; indices.len() / 3];
        let (out, _) = smooth_normals(&vertices, &indices, Smoothing::Groups(&groups));
        assert_eq!(out.len(), 8);
        let corner = out.iter().find(|v| v.position == [1.0, 1.0, 1.0]).unwrap();
        let expected = 1.0 / 3f32.sqrt();
        assert!(corner.normal.iter().all(|&c| (c - expected).abs() < 1e-5), "{:?}", corner.normal);
    }

    #[test]
    fn flat_normals_follow_winding() {
        let vertices = [vertex([0.0; 3]), vertex([1.0, 0.0, 0.0]), vertex([0.0, 1.0, 0.0])];
        let (out, indices) = flat_normals(&vertices, &[0, 1, 2]);
        assert_eq!(indices, vec![0, 1, 2]);
        assert!(out.iter().all(|v| v.normal == [0.0, 0.0, 1.0]));
    }

    #[test]
    fn matrices() {
        let translate = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [1.0, 2.0, 3.0, 1.0]];
        let mirror = [[-1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
        let both = mul(&translate, &mirror);
        assert_eq!(transform_point(&both, [1.0, 1.0, 1.0]), [0.0, 3.0, 4.0]);
        assert!(determinant(&both) < 0.0);
        let stretch = [[2.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
        let normal = normalize(NormalMatrix::new(&stretch).apply(normalize([1.0, 1.0, 0.0])));
        assert!(normal[0] < normal[1], "a stretched surface's normal leans away from the stretch");
        // Mirroring X leaves a +Z normal pointing +Z.
        assert_eq!(normalize(NormalMatrix::new(&mirror).apply([0.0, 0.0, 1.0])), [0.0, 0.0, 1.0]);
        assert_eq!(normalize(NormalMatrix::new(&mirror).apply([1.0, 0.0, 0.0])), [-1.0, 0.0, 0.0]);
        assert_eq!(mul(&IDENTITY, &translate), translate);
    }
}
