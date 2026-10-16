//! FBX (every version, binary and ASCII) and OBJ with its .mtl, through ufbx: a C library that
//! also converts axes and units while loading, so geometry arrives Y up in metres.

use std::collections::HashMap;
use std::path::Path;

use super::geometry::normalize;
use super::textures::{self, Alpha};
use super::{Builder, LoadOptions, Model, Slot, Vertex, srgb_to_linear};

/// ufbx's working memory per file. Only there to stop a corrupt file from exhausting memory: the
/// largest model in the test library (6 MB) needs a small fraction of it.
const MEMORY_LIMIT: usize = 1 << 30;

/// Corners gathered before they're merged into shared vertices, so a huge mesh never needs a
/// vertex per corner all at once: 3 MB of them.
const CHUNK_CORNERS: usize = 1 << 16;

#[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
pub(super) fn load(path: &Path, options: &LoadOptions) -> Result<Model, String> {
    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    let filename = path.to_string_lossy();
    let limit = || ufbx::AllocatorOpts { memory_limit: MEMORY_LIMIT, ..Default::default() };
    let opts = ufbx::LoadOpts {
        temp_allocator: limit(),
        result_allocator: limit(),
        target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
        target_unit_meters: 1.0,
        // Positions are taken through `geometry_to_world`, which includes the conversion either
        // way; this also scales the vertex data, so nothing is left in the root transform.
        space_conversion: ufbx::SpaceConversion::ModifyGeometry,
        generate_missing_normals: true,
        // An OBJ's .mtl.
        load_external_files: true,
        ignore_missing_external_files: true,
        obj_search_mtl_by_filename: true,
        ignore_embedded: options.max_texture == 0,
        // A skinned mesh is drawn as its skeleton holds it at rest: the bones' own transforms,
        // no animation. The mesh as stored (its bind pose) can face anywhere: in the test
        // library's Mixamo files it's turned 90° from its skeleton and showed its side.
        evaluate_skinning: true,
        // Lets ufbx find the .mtl relative to the file, since it reads from memory.
        filename: filename.as_ref().into(),
        ..Default::default()
    };
    let scene = ufbx::load_memory(&bytes, opts).map_err(|err| describe(&err))?;

    let mesh_nodes: Vec<&ufbx::Node> =
        scene.nodes.iter().map(|node| &**node).filter(|node| node.mesh.is_some()).collect();
    let mut reader = Reader {
        builder: Builder::default(),
        materials: HashMap::new(),
        textures: HashMap::new(),
        model_path: path,
        model_dir: path.parent().unwrap_or(Path::new(".")),
        max_texture: options.max_texture,
        texture_files: scene.texture_files.len(),
        has_uvs: mesh_nodes.iter().any(|node| node.mesh.as_ref().is_some_and(|mesh| mesh.vertex_uv.exists)),
        companion: None,
        companion_material: None,
    };
    // Hidden meshes are usually collision or helper shapes, unless everything is hidden.
    let any_visible = mesh_nodes.iter().any(|node| node.visible);
    let drawn: Vec<(&ufbx::Node, &ufbx::Mesh)> = mesh_nodes
        .iter()
        .filter_map(|node| Some((*node, &**node.mesh.as_ref()?)))
        .filter(|(node, mesh)| (node.visible || !any_visible) && mesh.num_triangles > 0)
        .collect();
    super::check_triangle_count(drawn.iter().map(|(_, mesh)| mesh.num_triangles as u64).sum())?;
    let mut source_vertices = 0;
    for (node, mesh) in drawn {
        reader.add_mesh(node, mesh)?;
        source_vertices += mesh.num_vertices as u64;
    }
    let mut builder = reader.builder;
    builder.source_vertices = Some(source_vertices);

    if builder.triangle_count() == 0 {
        // A skeleton and its animation, such as a Mixamo download without skin.
        for node in &scene.nodes {
            let Some(parent) = node.parent.as_ref() else { continue };
            if node.bone.is_some() && parent.bone.is_some() {
                builder.bones.push([translation(&parent.node_to_world), translation(&node.node_to_world)]);
            }
        }
        if builder.bones.is_empty() {
            return Err("there are no meshes in this file".into());
        }
    }
    builder.animations = scene
        .anim_stacks
        .iter()
        .filter(|stack| stack.layers.iter().any(|layer| !layer.anim_values.is_empty()))
        .map(|stack| (stack.element.name.to_string(), (stack.time_end - stack.time_begin).max(0.0) as f32))
        .collect();

    let meta = &scene.metadata;
    let format = if meta.file_format == ufbx::FileFormat::Obj {
        "OBJ".to_owned()
    } else {
        let encoding = if meta.ascii { "ASCII" } else { "binary" };
        format!("FBX {}.{} {encoding}", meta.version / 1000, meta.version % 1000 / 100)
    };
    builder.finish(format)
}

/// ufbx's short description, plus its detail when there is one.
fn describe(err: &ufbx::Error) -> String {
    let info = err.info();
    if info.is_empty() { err.description.to_string() } else { format!("{} ({info})", err.description) }
}

fn translation(m: &ufbx::Matrix) -> [f32; 3] {
    [m.m03 as f32, m.m13 as f32, m.m23 as f32]
}

/// How the bone holding most of a skinned mesh moves it from its stored form to the rest pose:
/// enough to tell whether skinning mirrors it.
fn skin_transform(mesh: &ufbx::Mesh) -> Option<ufbx::Matrix> {
    let cluster = mesh.skin_deformers.first()?.clusters.iter().max_by_key(|cluster| cluster.num_weights)?;
    Some(cluster.geometry_to_world)
}

struct Reader<'a> {
    builder: Builder,
    /// ufbx material element id → our material.
    materials: HashMap<u32, u32>,
    /// Texture file (or embedded texture's element id) → what became of it.
    textures: HashMap<String, Slot>,
    model_path: &'a Path,
    model_dir: &'a Path,
    max_texture: u32,
    /// Distinct texture files the scene names.
    texture_files: usize,
    has_uvs: bool,
    /// The picture named after the model (`textures::companion`), once looked for.
    companion: Option<Slot>,
    /// The white material wearing it, for faces without a material of their own.
    companion_material: Option<u32>,
}

impl Reader<'_> {
    /// The picture named after the model, looked for once. Only for files that name no texture,
    /// or just one that's missing: then the pairing is by name.
    fn companion(&mut self) -> Slot {
        if let Some(slot) = self.companion {
            return slot;
        }
        let decoded = if self.max_texture > 0 && self.has_uvs && self.texture_files <= 1 {
            textures::companion(self.model_path, self.max_texture)
        } else {
            None
        };
        let slot = match decoded {
            Some(decoded) => self.builder.texture_slot(Some(&*decoded)),
            None => Slot::None,
        };
        self.companion = Some(slot);
        slot
    }

    fn add_mesh(&mut self, node: &ufbx::Node, mesh: &ufbx::Mesh) -> Result<(), String> {
        let to_world = &node.geometry_to_world;
        let to_world_normals = ufbx::matrix_for_normals(to_world);
        // Skinned positions are in model space already.
        let skinned = mesh.skinned_position.exists && !mesh.skinned_is_local;
        let placement = if skinned { skin_transform(mesh).unwrap_or(*to_world) } else { *to_world };
        // A mirroring transform turns the winding around; swap it back so front faces stay
        // counter-clockwise.
        let order = if ufbx::matrix_determinant(&placement) < 0.0 { [0, 2, 1] } else { [0, 1, 2] };
        let colors = mesh.vertex_color.exists;
        let source = Source { mesh, to_world, to_world_normals: &to_world_normals, skinned, colors };

        let mut parts: Vec<(Option<&ufbx::Material>, &[u32])> = mesh
            .material_parts
            .iter()
            .filter(|part| part.num_triangles > 0)
            .map(|part| {
                let index = part.index as usize;
                // Materials belong to the node in FBX; the mesh's list can be another instance's.
                let material = node.materials.get(index).or_else(|| mesh.materials.get(index));
                (material.map(|material| &**material), &*part.face_indices)
            })
            .collect();
        let all_faces: Vec<u32>;
        if parts.is_empty() {
            all_faces = (0..mesh.faces.len() as u32).collect();
            parts.push((None, &all_faces));
        }

        let mut corners = vec![0u32; mesh.max_face_triangles.max(1) * 3];
        let mut vertices = Vec::with_capacity((mesh.num_triangles * 3).min(CHUNK_CORNERS + corners.len()));
        for (material, faces) in parts {
            let material = self.material(material, colors);
            for &face in faces {
                let Some(&face) = mesh.faces.get(face as usize) else { continue };
                let count = ufbx::triangulate_face(&mut corners, mesh, face) as usize;
                for triangle in corners[..count * 3].as_chunks::<3>().0 {
                    for k in order {
                        vertices.push(source.vertex(triangle[k] as usize));
                    }
                }
                if vertices.len() >= CHUNK_CORNERS {
                    self.add_corners(material, &mut vertices)?;
                }
            }
            self.add_corners(material, &mut vertices)?;
        }
        self.builder.meshes += 1;
        Ok(())
    }

    /// Adds triangles given one vertex per corner, merging the identical ones first, and empties
    /// `vertices`.
    fn add_corners(&mut self, material: u32, vertices: &mut Vec<Vertex>) -> Result<(), String> {
        if vertices.is_empty() {
            return Ok(());
        }
        let mut indices = vec![0u32; vertices.len()];
        let unique = ufbx::generate_indices(&mut [ufbx::VertexStream::new(vertices)], &mut indices, Default::default())
            .map_err(|err| describe(&err))?;
        let base = self.builder.add_vertices(&vertices[..unique]);
        self.builder.add_triangles(material, base, &indices);
        vertices.clear();
        Ok(())
    }

    fn material(&mut self, material: Option<&ufbx::Material>, vertex_colors: bool) -> u32 {
        let Some(material) = material else {
            if let Some(index) = self.companion_material {
                return index;
            }
            return match self.companion() {
                slot @ Slot::Found { .. } => {
                    let index = self.builder.companion_material(slot);
                    self.companion_material = Some(index);
                    index
                }
                _ => self.builder.default_material(vertex_colors),
            };
        };
        if let Some(&index) = self.materials.get(&material.element.element_id) {
            return index;
        }
        // ufbx maps every shading model onto PBR maps; the classic FBX diffuse is the fallback.
        // The PBR colour goes without its factor, as Blender, Unity and assimp read FBX: Synty's
        // white materials have a factor of 0.8 that would darken their atlas by 40% in light.
        let fbx = &material.fbx;
        let (base, factor) = if material.pbr.base_color.has_value {
            (&material.pbr.base_color, 1.0)
        } else {
            let factor = if fbx.diffuse_factor.has_value { fbx.diffuse_factor.value_vec4.x as f32 } else { 1.0 };
            (&fbx.diffuse_color, factor)
        };
        let c = base.value_vec4;
        // Stored as display colours.
        let color = [c.x, c.y, c.z].map(|c| srgb_to_linear((c as f32 * factor).clamp(0.0, 1.0)));
        let texture = base.texture.as_ref().or(fbx.diffuse_color.texture.as_ref());
        let slot = match texture {
            Some(texture) => self.texture(texture),
            None => Slot::None,
        };
        let index = self.builder.material(material.element.name.to_string(), [color[0], color[1], color[2], 1.0], slot);
        self.materials.insert(material.element.element_id, index);
        index
    }

    fn texture(&mut self, texture: &ufbx::Texture) -> Slot {
        if self.max_texture == 0 {
            return Slot::None;
        }
        // A layered texture lists its file textures; a file texture lists itself.
        let file = texture.file_textures.first().map_or(texture, |file| &**file);
        // The path as written in the file: relative, unless saved on another drive.
        let reference = [&file.relative_filename, &file.absolute_filename, &file.filename]
            .into_iter()
            .map(|name| name.as_ref())
            .find(|name: &&str| !name.is_empty());
        let embedded = !file.content.is_empty();
        // Several texture elements often name the same file.
        let key = match reference {
            Some(reference) if !embedded => reference.to_owned(),
            _ => format!("#{}", file.element.element_id),
        };
        if let Some(&slot) = self.textures.get(&key) {
            return slot;
        }
        let slot = if embedded {
            let decoded = textures::from_bytes(&file.content, reference.unwrap_or(""), self.max_texture, Alpha::Guess);
            self.builder.texture_slot(decoded.as_ref())
        } else if let Some(reference) = reference {
            match textures::from_file(self.model_dir, reference, self.max_texture, Alpha::Guess) {
                Some(decoded) => self.builder.texture_slot(Some(&*decoded)),
                // The one texture named is missing: the pack may pair them by name instead.
                None => match self.companion() {
                    found @ Slot::Found { .. } => found,
                    _ => self.builder.texture_slot(None),
                },
            }
        } else {
            Slot::None
        };
        self.textures.insert(key, slot);
        slot
    }
}

/// One mesh's vertex data and how it gets to model space.
struct Source<'a> {
    mesh: &'a ufbx::Mesh,
    to_world: &'a ufbx::Matrix,
    to_world_normals: &'a ufbx::Matrix,
    /// Take ufbx's skinned positions and normals, which are in model space already.
    skinned: bool,
    colors: bool,
}

impl Source<'_> {
    /// The vertex at a face corner (an index into the mesh's corners).
    fn vertex(&self, corner: usize) -> Vertex {
        let mesh = self.mesh;
        let p = if self.skinned {
            mesh.skinned_position[corner]
        } else {
            ufbx::transform_position(self.to_world, mesh.vertex_position[corner])
        };
        let normal = if self.skinned && mesh.skinned_normal.exists {
            let n = mesh.skinned_normal[corner];
            normalize([n.x as f32, n.y as f32, n.z as f32])
        } else if mesh.vertex_normal.exists {
            let n = ufbx::transform_direction(self.to_world_normals, mesh.vertex_normal[corner]);
            normalize([n.x as f32, n.y as f32, n.z as f32])
        } else {
            [0.0; 3]
        };
        let uv = if mesh.vertex_uv.exists {
            let uv = mesh.vertex_uv[corner];
            // FBX and OBJ count v up from the bottom of the picture.
            [uv.x as f32, 1.0 - uv.y as f32]
        } else {
            [0.0; 2]
        };
        let color = if self.colors {
            let c = mesh.vertex_color[corner];
            [srgb_to_linear(c.x as f32), srgb_to_linear(c.y as f32), srgb_to_linear(c.z as f32), c.w as f32]
        } else {
            [1.0; 4]
        };
        Vertex { position: [p.x as f32, p.y as f32, p.z as f32], normal, uv, color }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::test_files::{TempDir, png_2x2};

    #[test]
    fn obj_with_mtl_colour_and_texture() {
        let dir = TempDir::new("obj");
        dir.write(
            "quad.obj",
            b"mtllib quad.mtl\n\
              v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 -1\nv 0 1 -1\n\
              vt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\n\
              usemtl Red\nf 1/1 2/2 3/3 4/4\n\
              usemtl Wood\nf 1/1 4/4 6/3 5/2\n",
        );
        dir.write("quad.mtl", b"newmtl Red\nKd 1 0.5 0\nnewmtl Wood\nKd 1 1 1\nmap_Kd C:\\art\\wood.png\n");
        dir.write("Textures/wood.png", &png_2x2());
        let model = load(&dir.0.join("quad.obj"), &LoadOptions { max_texture: 64 }).unwrap();
        assert_eq!(model.info.format, "OBJ");
        assert_eq!(model.info.triangles, 4, "quads are split in two");
        assert_eq!(model.info.vertices, 6);
        assert_eq!((model.info.materials, model.info.textures, model.info.missing_textures), (2, 1, 0));
        assert_eq!(model.bounds.min, [0.0, 0.0, -1.0]);
        assert_eq!(model.bounds.max, [1.0, 1.0, 0.0]);

        let red = model.materials.iter().find(|m| m.name == "Red").unwrap();
        // Kd is a display colour; 0.5 is about 0.21 in linear light.
        assert!((red.color[1] - srgb_to_linear(0.5)).abs() < 1e-4, "{:?}", red.color);
        assert_eq!(red.texture, None);
        let wood = model.materials.iter().find(|m| m.name == "Wood").unwrap();
        assert_eq!(wood.texture, Some(0));
        assert_eq!(model.textures[0].width, 2);

        // The first quad faces +Z: its normals (generated) and winding agree.
        let red_index = model.materials.iter().position(|m| m.name == "Red").unwrap() as u32;
        let part = model.parts.iter().find(|part| part.material == red_index).unwrap();
        for &index in &model.indices[part.first as usize..][..part.count as usize] {
            let vertex = model.vertices[index as usize];
            assert!((vertex.normal[2] - 1.0).abs() < 1e-5, "{:?}", vertex.normal);
        }
        // v counts down from the top: the OBJ's (0, 0) is the picture's bottom-left.
        let origin = model.vertices.iter().find(|v| v.position == [0.0, 0.0, 0.0] && v.normal[2] > 0.5).unwrap();
        assert_eq!(origin.uv, [0.0, 1.0]);
    }

    #[test]
    fn missing_texture_is_counted_once() {
        let dir = TempDir::new("obj_missing");
        dir.write("tri.obj", b"mtllib tri.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl A\nf 1 2 3\nusemtl B\nf 1 3 2\n");
        dir.write("tri.mtl", b"newmtl A\nKd 1 1 1\nmap_Kd gone.png\nnewmtl B\nKd 0 0 1\nmap_Kd gone.png\n");
        let model = load(&dir.0.join("tri.obj"), &LoadOptions { max_texture: 64 }).unwrap();
        assert_eq!(model.info.missing_textures, 1);
        assert_eq!(model.info.textures, 0);
        let a = model.materials.iter().find(|m| m.name == "A").unwrap();
        let [r, g, b] = crate::model::shading::DEFAULT_ALBEDO;
        assert_eq!(a.color, [r, g, b, 1.0], "white under a missing texture shows as clay");
    }

    /// Three bones in a chain, in metres, with a 2-second animation and no mesh: what a Mixamo
    /// download "without skin" holds.
    const SKELETON: &str = r#"; FBX 7.4.0 project file
FBXHeaderExtension:  {
	FBXHeaderVersion: 1003
	FBXVersion: 7400
}
GlobalSettings:  {
	Version: 1000
	Properties70:  {
		P: "UpAxis", "int", "Integer", "",1
		P: "UpAxisSign", "int", "Integer", "",1
		P: "FrontAxis", "int", "Integer", "",2
		P: "FrontAxisSign", "int", "Integer", "",1
		P: "CoordAxis", "int", "Integer", "",0
		P: "CoordAxisSign", "int", "Integer", "",1
		P: "UnitScaleFactor", "double", "Number", "",100
	}
}
Objects:  {
	NodeAttribute: 1001, "NodeAttribute::", "LimbNode" {
		TypeFlags: "Skeleton"
	}
	NodeAttribute: 1002, "NodeAttribute::", "LimbNode" {
		TypeFlags: "Skeleton"
	}
	NodeAttribute: 1003, "NodeAttribute::", "LimbNode" {
		TypeFlags: "Skeleton"
	}
	Model: 2001, "Model::Hips", "LimbNode" {
		Version: 232
		Properties70:  {
			P: "Lcl Translation", "Lcl Translation", "", "A",0,1,0
		}
	}
	Model: 2002, "Model::Spine", "LimbNode" {
		Version: 232
		Properties70:  {
			P: "Lcl Translation", "Lcl Translation", "", "A",0,0.5,0
		}
	}
	Model: 2003, "Model::Head", "LimbNode" {
		Version: 232
		Properties70:  {
			P: "Lcl Translation", "Lcl Translation", "", "A",0.25,0.5,0
		}
	}
	AnimationStack: 3001, "AnimStack::Wave", "" {
		Properties70:  {
			P: "LocalStart", "KTime", "Time", "",0
			P: "LocalStop", "KTime", "Time", "",92372316000
		}
	}
	AnimationLayer: 3002, "AnimLayer::BaseLayer", "" {
	}
	AnimationCurveNode: 3003, "AnimCurveNode::T", "" {
		Properties70:  {
			P: "d|X", "Number", "", "A",0
			P: "d|Y", "Number", "", "A",0.5
			P: "d|Z", "Number", "", "A",0
		}
	}
	AnimationCurve: 3004, "AnimCurve::", "" {
		Default: 0
		KeyVer: 4009
		KeyTime: *2 {
			a: 0,92372316000
		}
		KeyValueFloat: *2 {
			a: 0,1
		}
		KeyAttrFlags: *1 {
			a: 24840
		}
		KeyAttrDataFloat: *4 {
			a: 0,0,218434821,0
		}
		KeyAttrRefCount: *1 {
			a: 2
		}
	}
}
Connections:  {
	C: "OO",2001,0
	C: "OO",2002,2001
	C: "OO",2003,2002
	C: "OO",1001,2001
	C: "OO",1002,2002
	C: "OO",1003,2003
	C: "OO",3002,3001
	C: "OO",3003,3002
	C: "OP",3003,2002, "Lcl Translation"
	C: "OP",3004,3003, "d|X"
}
"#;

    #[test]
    fn skeleton_only_file_gives_bones() {
        let dir = TempDir::new("fbx_skeleton");
        let path = dir.write("Wave.fbx", SKELETON.as_bytes());
        let model = load(&path, &LoadOptions { max_texture: 0 }).unwrap();
        assert_eq!(model.info.format, "FBX 7.4 ASCII");
        assert!(!model.info.geometry);
        assert_eq!(model.info.triangles, 0);
        assert_eq!(model.bones.len(), 2, "hips to spine, spine to head");
        // UnitScaleFactor 100: the file counts in metres, and the head sits 1 + 0.5 + 0.5 up.
        let top = model.bones.iter().flatten().map(|joint| joint[1]).fold(f32::MIN, f32::max);
        assert!((top - 2.0).abs() < 1e-4, "{top}");
        assert_eq!(model.info.animations.len(), 1);
        assert_eq!(model.info.animations[0].0, "Wave");
        assert!((model.info.animations[0].1 - 2.0).abs() < 1e-3, "{:?}", model.info.animations);
    }

    #[test]
    fn damaged_obj_and_fbx_never_panic() {
        let dir = TempDir::new("obj_damaged");
        let obj = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 1\nvn 0 0 1\nf 1/1/1 2/2/1 3/1/1 4/2/1\nf 1 3 4\n";
        let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
        for (at, _) in obj.char_indices().filter(|(_, c)| c.is_ascii_digit()) {
            for digit in ["9", "0", "-1"] {
                let mut changed = obj.to_owned();
                changed.replace_range(at..at + 1, digit);
                cases.push(("damaged.obj", changed.into_bytes()));
            }
        }
        cases.extend(
            crate::model::test_files::damaged(SKELETON.as_bytes(), 200).into_iter().map(|bytes| ("damaged.fbx", bytes)),
        );
        for (name, bytes) in cases {
            let path = dir.write(name, &bytes);
            let _ = load(&path, &LoadOptions { max_texture: 16 });
        }
    }

    #[test]
    fn broken_file_is_a_clear_error() {
        let dir = TempDir::new("fbx_broken");
        let path = dir.write("broken.fbx", b"Kaydara FBX Binary  \x00\x1a\x00\xe8\x1c\x00\x00");
        let err = load(&path, &LoadOptions { max_texture: 0 }).err().unwrap();
        assert!(!err.is_empty());
    }
}
