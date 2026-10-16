//! 3D models built from triangles and turned profiles, as OBJ files with their materials.

use std::f32::consts::TAU;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

use asset_browser::model::vector::{cross, dot, normalize, sub};

use super::paint::hex;

type Vec3 = [f32; 3];

/// Triangles, each with its own corners (so each face is shaded flat), and their materials.
#[derive(Default)]
struct Mesh {
    corners: Vec<Vec3>,
    faces: Vec<usize>,
    materials: Vec<(&'static str, u32)>,
}

impl Mesh {
    fn material(&mut self, name: &'static str, rgb: u32) -> usize {
        self.materials.push((name, rgb));
        self.materials.len() - 1
    }

    /// A triangle, turned to face away from `inside`.
    #[expect(clippy::too_many_arguments, reason = "refactor: take fewer, grouped arguments")]
    fn tri(&mut self, material: usize, a: Vec3, b: Vec3, c: Vec3, inside: Vec3) {
        let centre = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0];
        let (b, c) = if dot(cross(sub(b, a), sub(c, a)), sub(centre, inside)) < 0.0 { (c, b) } else { (b, c) };
        self.corners.extend([a, b, c]);
        self.faces.push(material);
    }

    fn quad(&mut self, material: usize, [a, b, c, d]: [Vec3; 4], inside: Vec3) {
        self.tri(material, a, b, c, inside);
        self.tri(material, a, c, d, inside);
    }

    fn cuboid(&mut self, material: usize, min: Vec3, max: Vec3) {
        let corner = |i: usize| {
            [
                if i & 1 == 0 { min[0] } else { max[0] },
                if i & 2 == 0 { min[1] } else { max[1] },
                if i & 4 == 0 { min[2] } else { max[2] },
            ]
        };
        let inside = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0, (min[2] + max[2]) / 2.0];
        for face in [[0, 1, 3, 2], [4, 5, 7, 6], [0, 1, 5, 4], [2, 3, 7, 6], [0, 2, 6, 4], [1, 3, 7, 5]] {
            self.quad(material, face.map(corner), inside);
        }
    }

    /// A shape turned around an upright axis through `at`: `profile` is (radius, height) from the
    /// bottom up, closed by flat caps where it doesn't end on the axis.
    #[expect(clippy::too_many_arguments, reason = "refactor: take fewer, grouped arguments")]
    fn lathe(&mut self, material: usize, at: Vec3, profile: &[(f32, f32)], segments: usize, caps: Option<usize>) {
        let point = |(r, h): (f32, f32), i: usize| {
            let a = i as f32 / segments as f32 * TAU;
            [at[0] + r * a.cos(), at[1] + h, at[2] + r * a.sin()]
        };
        for pair in profile.windows(2) {
            let inside = [at[0], at[1] + (pair[0].1 + pair[1].1) / 2.0, at[2]];
            for i in 0..segments {
                let (a, b) = (point(pair[0], i), point(pair[0], i + 1));
                let (c, d) = (point(pair[1], i + 1), point(pair[1], i));
                if pair[0].0 == 0.0 {
                    self.tri(material, a, c, d, inside);
                } else if pair[1].0 == 0.0 {
                    self.tri(material, a, b, c, inside);
                } else {
                    self.quad(material, [a, b, c, d], inside);
                }
            }
        }
        let (first, last) = (profile[0], profile[profile.len() - 1]);
        let cap = caps.unwrap_or(material);
        for (end, inward) in [(first, 0.01), (last, -0.01)] {
            if end.0 > 0.0 {
                let centre = [at[0], at[1] + end.1, at[2]];
                let inside = [at[0], at[1] + end.1 + inward, at[2]];
                for i in 0..segments {
                    self.tri(cap, centre, point(end, i), point(end, i + 1), inside);
                }
            }
        }
    }

    /// Moves and turns (around the upright axis, then tips over sideways) what was added since
    /// `from`.
    fn place(&mut self, from: usize, turn: f32, tip: f32, offset: Vec3) {
        let (st, ct) = turn.sin_cos();
        let (sp, cp) = tip.sin_cos();
        for p in &mut self.corners[from * 3..] {
            let (x, y) = (p[0] * cp - p[1] * sp, p[0] * sp + p[1] * cp);
            let (x, z) = (x * ct - p[2] * st, x * st + p[2] * ct);
            *p = [x + offset[0], y + offset[1], z + offset[2]];
        }
    }

    fn write(&self, dir: &Path, name: &str) -> io::Result<()> {
        let mut mtl = String::new();
        for &(material, rgb) in &self.materials {
            let [r, g, b, _] = hex(rgb);
            let _ = writeln!(mtl, "newmtl {material}\nKd {r:.3} {g:.3} {b:.3}\n");
        }
        fs::write(dir.join(format!("{name}.mtl")), mtl)?;
        let mut obj = format!("# Made up for the README's pictures (src/bin/shots/demo.rs)\nmtllib {name}.mtl\n");
        for p in &self.corners {
            let _ = writeln!(obj, "v {:.4} {:.4} {:.4}", p[0], p[1], p[2]);
        }
        for &[a, b, c] in self.corners.as_chunks::<3>().0 {
            let n = normalize(cross(sub(b, a), sub(c, a)));
            let _ = writeln!(obj, "vn {:.4} {:.4} {:.4}", n[0], n[1], n[2]);
        }
        let mut current = usize::MAX;
        for (i, &material) in self.faces.iter().enumerate() {
            if material != current {
                let _ = writeln!(obj, "usemtl {}", self.materials[material].0);
                current = material;
            }
            let _ = writeln!(obj, "f {0}//{n} {1}//{n} {2}//{n}", i * 3 + 1, i * 3 + 2, i * 3 + 3, n = i + 1);
        }
        fs::write(dir.join(format!("{name}.obj")), obj)
    }
}

#[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
pub(super) fn models(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;

    let mut crate_box = Mesh::default();
    let (wood, frame) = (crate_box.material("wood", 0xc08a4e), crate_box.material("frame", 0x7a4f2a));
    crate_box.cuboid(wood, [-0.5, 0.0, -0.5], [0.5, 1.0, 0.5]);
    // A beam along each of the box's twelve edges, standing out a little.
    let range = |axis: usize| if axis == 1 { (0.0, 1.0) } else { (-0.5, 0.5) };
    for along in 0..3 {
        for (low_p, low_q) in [(true, true), (true, false), (false, true), (false, false)] {
            let (mut min, mut max) = ([0.0; 3], [0.0; 3]);
            (min[along], max[along]) = range(along);
            for (axis, low) in [((along + 1) % 3, low_p), ((along + 2) % 3, low_q)] {
                let (lo, hi) = range(axis);
                (min[axis], max[axis]) = if low { (lo - 0.01, lo + 0.09) } else { (hi - 0.09, hi + 0.01) };
            }
            crate_box.cuboid(frame, min, max);
        }
    }
    crate_box.write(dir, "crate")?;

    let mut barrel = Mesh::default();
    let (staves, hoops, top) =
        (barrel.material("staves", 0xa86b38), barrel.material("hoops", 0x4b4f58), barrel.material("lid", 0x8a5a2f));
    barrel.lathe(staves, [0.0; 3], &[(0.36, 0.0), (0.42, 0.2), (0.45, 0.5), (0.42, 0.8), (0.36, 1.0)], 20, Some(top));
    for h in [0.16, 0.8] {
        barrel.lathe(hoops, [0.0; 3], &[(0.432, h), (0.432, h + 0.06)], 20, None);
    }
    barrel.write(dir, "barrel")?;

    let mut rock = Mesh::default();
    let stone = rock.material("stone", 0x8c8f96);
    icosphere(&mut rock, stone, 2, |p| {
        let bump = 0.16 * (3.1 * p[0] + 1.3).sin() * (2.7 * p[1] + 0.4).sin() * (3.3 * p[2] + 2.1).sin()
            + 0.07 * (7.0 * p[0]).sin() * (6.0 * p[1] + 1.0).sin() * (8.0 * p[2]).sin();
        let r = 0.55 * (1.0 + bump);
        [p[0] * r * 1.25, (p[1] * r * 0.8).max(-0.18) + 0.18, p[2] * r]
    });
    rock.write(dir, "rock")?;

    let mut crystal = Mesh::default();
    let (glow, deep) = (crystal.material("crystal", 0x7fe3ff), crystal.material("crystal_dark", 0x8a5cff));
    crystal.lathe(glow, [0.0; 3], &[(0.0, 0.0), (0.2, 0.22), (0.2, 1.05), (0.0, 1.4)], 6, None);
    for (turn, tip, scale, material) in [(0.4, 0.5, 0.6, deep), (2.6, 0.45, 0.5, glow), (4.4, 0.4, 0.42, deep)] {
        let from = crystal.faces.len();
        crystal.lathe(
            material,
            [0.0; 3],
            &[(0.0, 0.0), (0.2 * scale, 0.22 * scale), (0.2 * scale, 1.05 * scale), (0.0, 1.4 * scale)],
            6,
            None,
        );
        // Out from the big one's foot, the way it leans.
        let (sin, cos) = f32::sin_cos(turn);
        crystal.place(from, turn, tip, [-0.17 * cos, 0.02, -0.17 * sin]);
    }
    crystal.lathe(deep, [0.0; 3], &[(0.42, 0.0), (0.3, 0.08)], 6, Some(deep));
    crystal.write(dir, "crystal")?;

    let mut tree = Mesh::default();
    let (bark, leaves) = (tree.material("bark", 0x6e4a2c), tree.material("leaves", 0x3f9a4a));
    tree.lathe(bark, [0.0; 3], &[(0.11, 0.0), (0.09, 0.55)], 8, None);
    for (r, base, top) in [(0.62, 0.4, 1.15), (0.5, 0.8, 1.45), (0.36, 1.15, 1.75)] {
        tree.lathe(leaves, [0.0; 3], &[(r, base), (0.0, top)], 8, None);
    }
    tree.write(dir, "tree")?;

    let mut mushroom = Mesh::default();
    let (stem, cap, gills) =
        (mushroom.material("stem", 0xf3e6cf), mushroom.material("cap", 0xd9342b), mushroom.material("gills", 0xe9d3b0));
    mushroom.lathe(stem, [0.0; 3], &[(0.17, 0.0), (0.15, 0.4), (0.13, 0.58)], 14, None);
    mushroom.lathe(
        cap,
        [0.0; 3],
        &[(0.56, 0.52), (0.54, 0.62), (0.46, 0.74), (0.32, 0.84), (0.0, 0.9)],
        18,
        Some(gills),
    );
    mushroom.write(dir, "mushroom")?;

    let mut house = Mesh::default();
    let (walls, roof, door, glass) = (
        house.material("walls", 0xeadcc2),
        house.material("roof", 0xb5432f),
        house.material("door", 0x5e3a20),
        house.material("windows", 0x9fd2f2),
    );
    house.cuboid(walls, [-0.6, 0.0, -0.45], [0.6, 0.8, 0.45]);
    house.cuboid(door, [-0.12, 0.0, 0.45], [0.12, 0.46, 0.47]);
    for x in [-0.4, 0.28] {
        house.cuboid(glass, [x, 0.4, 0.45], [x + 0.14, 0.6, 0.47]);
    }
    house.cuboid(roof, [0.25, 1.0, -0.15], [0.4, 1.4, 0.0]);
    let ridge_inside = [0.0, 0.95, 0.0];
    for side in [-1.0f32, 1.0] {
        house.quad(
            roof,
            [[-0.7, 0.75, side * 0.58], [0.7, 0.75, side * 0.58], [0.7, 1.3, 0.0], [-0.7, 1.3, 0.0]],
            ridge_inside,
        );
        house.tri(walls, [side * 0.6, 0.8, -0.45], [side * 0.6, 0.8, 0.45], [side * 0.6, 1.24, 0.0], [0.0, 1.0, 0.0]);
    }
    house.write(dir, "house")?;

    let mut donut = Mesh::default();
    let (dough, icing) = (donut.material("dough", 0xd99a55), donut.material("icing", 0xff8fc1));
    let (big, small, around, across) = (0.42, 0.19, 32, 16);
    let at = |i: usize, j: usize| {
        let (a, b) = (i as f32 / around as f32 * TAU, j as f32 / across as f32 * TAU);
        let r = big + small * b.cos();
        [r * a.cos(), 0.2 + small * b.sin(), r * a.sin()]
    };
    for i in 0..around {
        for j in 0..across {
            let mid = (i as f32 + 0.5) / around as f32 * TAU;
            let inside = [big * mid.cos(), 0.2, big * mid.sin()];
            let b = (j as f32 + 0.5) / across as f32 * TAU;
            let material = if b.sin() > -0.15 { icing } else { dough };
            donut.quad(material, [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)], inside);
        }
    }
    donut.write(dir, "donut")
}

/// A ball of triangles, `subdivisions` times finer than an icosahedron, its points moved by `shape`.
fn icosphere(mesh: &mut Mesh, material: usize, subdivisions: usize, shape: impl Fn(Vec3) -> Vec3) {
    let g = (1.0 + 5.0f32.sqrt()) / 2.0;
    let points = [
        [-1.0, g, 0.0],
        [1.0, g, 0.0],
        [-1.0, -g, 0.0],
        [1.0, -g, 0.0],
        [0.0, -1.0, g],
        [0.0, 1.0, g],
        [0.0, -1.0, -g],
        [0.0, 1.0, -g],
        [g, 0.0, -1.0],
        [g, 0.0, 1.0],
        [-g, 0.0, -1.0],
        [-g, 0.0, 1.0],
    ];
    let mut triangles: Vec<[Vec3; 3]> = [
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ]
    .iter()
    .map(|t| t.map(|i| normalize(points[i])))
    .collect();
    for _ in 0..subdivisions {
        triangles = triangles
            .iter()
            .flat_map(|&[a, b, c]| {
                let mid = |p: Vec3, q: Vec3| normalize([(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0, (p[2] + q[2]) / 2.0]);
                let (ab, bc, ca) = (mid(a, b), mid(b, c), mid(c, a));
                [[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]]
            })
            .collect();
    }
    let centre = shape([0.0, 0.0, 0.0]);
    for [a, b, c] in triangles {
        mesh.tri(material, shape(a), shape(b), shape(c), centre);
    }
}
