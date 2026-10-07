//! glTF 2.0, both forms: `.gltf` (JSON, buffers beside it or inline as
//! base64 data URIs) and `.glb` (one binary file).
//!
//! The scene's node tree is walked and every mesh placed where its node's
//! transforms put it, one [`Part`] per node with a mesh, named by the node
//! (else the mesh). Triangle lists, strips and fans are read; points and
//! lines are skipped. Each primitive's material contributes its base colour
//! factor, and a `COLOR_0` attribute (multiplied by that factor, as the spec
//! does) becomes corner colours. Base-colour TEXTURES are not read yet — a
//! textured model draws in its factor alone, often plain white — which is
//! milestone 5's work. A node whose transform mirrors (negative determinant)
//! has its winding reversed, so its faces still point out. glTF is Y-up by
//! its spec, in metres.
//!
//! Buffers are loaded here rather than by the crate's `import` feature,
//! which would also decode every texture image.

use std::path::Path;

use base64::Engine as _;
use glam::{Mat4, Vec3};
use gltf::buffer::Source;
use gltf::mesh::Mode;

use crate::mesh::Mesh;
use crate::{Part, Scene, Unit, UpAxis};

/// Deeper than any real node tree; a guard against a cycle the parser let by.
const MAX_DEPTH: usize = 128;

pub fn read(bytes: &[u8], dir: Option<&Path>) -> Result<Scene, String> {
    let file = gltf::Gltf::from_slice(bytes).map_err(|e| format!("not a readable glTF: {e}"))?;
    let buffers = load_buffers(&file, dir)?;
    let mut parts = Vec::new();
    let mut skipped_modes = 0usize;
    let mut textured = false;

    let roots: Vec<gltf::Node> = match file.default_scene().or_else(|| file.scenes().next()) {
        Some(scene) => scene.nodes().collect(),
        None => {
            // No scene: every node that is nobody's child is a root.
            let children: std::collections::HashSet<usize> =
                file.nodes().flat_map(|n| n.children().map(|c| c.index())).collect();
            file.nodes().filter(|n| !children.contains(&n.index())).collect()
        }
    };
    // Pushed in reverse, so the stack pops in pre-order: parts come out in
    // the order the file lists them, parents before children.
    let mut stack: Vec<(gltf::Node, Mat4, usize)> = roots.into_iter().rev().map(|n| (n, Mat4::IDENTITY, 0)).collect();
    while let Some((node, parent, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            return Err("the node tree is too deep (a cycle?)".into());
        }
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        let children: Vec<gltf::Node> = node.children().collect();
        for child in children.into_iter().rev() {
            stack.push((child, world, depth + 1));
        }
        let Some(gmesh) = node.mesh() else { continue };
        let mirrored = world.determinant() < 0.0;
        let mut mesh = Mesh::default();
        let mut corner_colors: Vec<[f32; 3]> = Vec::new();
        let mut any_colors = false;
        for prim in gmesh.primitives() {
            let reader = prim.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
            let Some(positions) = reader.read_positions() else { continue };
            let base = mesh.positions.len() as u32;
            mesh.positions.extend(positions.map(|p| world.transform_point3(Vec3::from(p))));
            let count = mesh.positions.len() as u32 - base;
            let indices: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..count).collect(),
            };
            if let Some(&bad) = indices.iter().find(|&&i| i >= count) {
                return Err(format!("mesh '{}' indexes point {bad} of {count}", gmesh.name().unwrap_or("?")));
            }
            let tris = triangles(prim.mode(), &indices);
            let Some(tris) = tris else {
                skipped_modes += 1;
                continue;
            };
            let material = prim.material();
            let pbr = material.pbr_metallic_roughness();
            textured |= pbr.base_color_texture().is_some();
            let [r, g, b, _] = pbr.base_color_factor();
            let slot = mesh.colors.len() as u32;
            mesh.colors.push([r, g, b]);
            let point_colors: Option<Vec<[f32; 3]>> = reader.read_colors(0).map(|c| c.into_rgb_f32().collect());
            for t in tris {
                let t = if mirrored { [t[0], t[2], t[1]] } else { t };
                for k in t {
                    corner_colors.push(match &point_colors {
                        Some(pc) => {
                            any_colors = true;
                            let c = pc.get(k as usize).copied().unwrap_or([1.0; 3]);
                            [c[0] * r, c[1] * g, c[2] * b]
                        }
                        None => [r, g, b],
                    });
                }
                mesh.triangles.push(t.map(|i| i + base));
                mesh.tri_color.push(slot);
            }
        }
        if any_colors {
            mesh.corner_colors = Some(corner_colors);
        }
        let name = node.name().or(gmesh.name()).map(str::to_string).unwrap_or_else(|| format!("node {}", node.index()));
        parts.push(Part { name, mesh });
    }

    if skipped_modes > 0 {
        log::info!("[mesh-io] glTF: skipped {skipped_modes} primitives of points or lines");
    }
    if textured {
        log::info!("[mesh-io] glTF: base-colour textures are not drawn yet; textured materials show their factor");
    }
    Ok(Scene { parts, up: UpAxis::Y, unit: Unit::Metre })
}

/// A primitive's index list as triangles, or `None` for points and lines.
fn triangles(mode: Mode, i: &[u32]) -> Option<Vec<[u32; 3]>> {
    Some(match mode {
        Mode::Triangles => i.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect(),
        // Every other triangle of a strip is reversed to keep the winding.
        Mode::TriangleStrip => (0..i.len().saturating_sub(2))
            .map(|k| if k % 2 == 0 { [i[k], i[k + 1], i[k + 2]] } else { [i[k + 1], i[k], i[k + 2]] })
            .collect(),
        Mode::TriangleFan => (1..i.len().saturating_sub(1)).map(|k| [i[0], i[k], i[k + 1]]).collect(),
        Mode::Points | Mode::Lines | Mode::LineLoop | Mode::LineStrip => return None,
    })
}

fn load_buffers(file: &gltf::Gltf, dir: Option<&Path>) -> Result<Vec<Vec<u8>>, String> {
    file.buffers()
        .map(|b| {
            let data = match b.source() {
                Source::Bin => file.blob.clone().ok_or("the file names a binary chunk it does not have")?,
                Source::Uri(uri) if uri.starts_with("data:") => {
                    let (_, payload) = uri.split_once(";base64,").ok_or("a data URI that is not base64")?;
                    base64::engine::general_purpose::STANDARD
                        .decode(payload)
                        .map_err(|e| format!("a bad base64 buffer: {e}"))?
                }
                Source::Uri(uri) => {
                    let path = dir.unwrap_or(Path::new(".")).join(percent_decode(uri));
                    std::fs::read(&path).map_err(|e| format!("buffer {}: {e}", path.display()))?
                }
            };
            if data.len() < b.length() {
                return Err(format!("buffer {} is {} bytes, shorter than the {} it declares", b.index(), data.len(), b.length()));
            }
            Ok(data)
        })
        .collect()
}

/// `my%20model.bin` → `my model.bin`: URIs in a glTF are percent-encoded.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One triangle's points and indices as a base64 buffer, and the JSON
    /// that describes it, with `nodes` and `extra` spliced in.
    fn gltf_json(nodes: &str, mode: u32, extra: &str) -> (String, Vec<u8>) {
        let mut bin = Vec::new();
        for p in [[0f32, 0., 0.], [1., 0., 0.], [0., 1., 0.], [1., 1., 0.]] {
            for c in p {
                bin.extend(c.to_le_bytes());
            }
        }
        for i in [0u16, 1, 2, 3] {
            bin.extend(i.to_le_bytes());
        }
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":{nodes},
            "meshes":[{{"name":"tri","primitives":[{{"attributes":{{"POSITION":0}},"indices":1,"mode":{mode},"material":0}}]}}],
            "materials":[{{"pbrMetallicRoughness":{{"baseColorFactor":[0.8,0.2,0.1,1.0]}}}}],
            "accessors":[{{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}},
                         {{"bufferView":1,"componentType":5123,"count":4,"type":"SCALAR"}}],
            "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":48}},{{"buffer":0,"byteOffset":48,"byteLength":8}}],
            "buffers":[{{"byteLength":56{extra}}}]}}"#
        );
        (json, bin)
    }

    fn inline(nodes: &str, mode: u32) -> Vec<u8> {
        let (_, bin) = gltf_json(nodes, mode, "");
        let uri = format!(r#","uri":"data:application/octet-stream;base64,{}""#, base64::engine::general_purpose::STANDARD.encode(&bin));
        gltf_json(nodes, mode, &uri).0.into_bytes()
    }

    #[test]
    fn a_node_transform_places_the_mesh_and_the_material_colours_it() {
        let s = read(&inline(r#"[{"name":"moved","mesh":0,"translation":[10,0,0],"scale":[2,2,2]}]"#, 5), None).unwrap();
        assert_eq!(s.up, UpAxis::Y);
        assert_eq!(s.unit, Unit::Metre);
        assert_eq!(Unit::Metre.millimetres(), Some(1000.0));
        let part = &s.parts[0];
        assert_eq!(part.name, "moved");
        assert_eq!(part.mesh.positions[1], Vec3::new(12.0, 0.0, 0.0));
        assert_eq!(part.mesh.triangles.len(), 2, "a strip of four points is two triangles");
        assert_eq!(part.mesh.corner_color(0, 0), [0.8, 0.2, 0.1]);
    }

    #[test]
    fn children_inherit_their_parents_transform() {
        let nodes = r#"[{"translation":[0,5,0],"children":[1]},{"mesh":0,"translation":[1,0,0]}]"#;
        let s = read(&inline(nodes, 4), None).unwrap();
        assert_eq!(s.parts.len(), 1);
        assert_eq!(s.parts[0].name, "tri", "an unnamed node takes its mesh's name");
        assert_eq!(s.parts[0].mesh.positions[0], Vec3::new(1.0, 5.0, 0.0));
    }

    #[test]
    fn a_mirroring_node_keeps_faces_pointing_out() {
        let plain = read(&inline(r#"[{"mesh":0}]"#, 4), None).unwrap();
        let mirrored = read(&inline(r#"[{"mesh":0,"scale":[-1,1,1]}]"#, 4), None).unwrap();
        let normal = |m: &Mesh| {
            let [a, b, c] = m.triangles[0].map(|i| m.positions[i as usize]);
            (b - a).cross(c - a).normalize()
        };
        assert_eq!(normal(&plain.parts[0].mesh), Vec3::Z);
        assert_eq!(normal(&mirrored.parts[0].mesh), Vec3::Z, "the mirror flipped the winding back");
    }

    #[test]
    fn a_glb_is_read_from_its_binary_chunk() {
        let (json, bin) = gltf_json(r#"[{"mesh":0}]"#, 4, "");
        let mut json = json.into_bytes();
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let mut glb = Vec::new();
        glb.extend(b"glTF");
        glb.extend(2u32.to_le_bytes());
        glb.extend(((12 + 8 + json.len() + 8 + bin.len()) as u32).to_le_bytes());
        glb.extend((json.len() as u32).to_le_bytes());
        glb.extend(b"JSON");
        glb.extend(&json);
        glb.extend((bin.len() as u32).to_le_bytes());
        glb.extend(b"BIN\0");
        glb.extend(&bin);
        let s = read(&glb, None).unwrap();
        assert_eq!(s.parts[0].mesh.triangles, vec![[0, 1, 2]]);
    }

    #[test]
    fn lines_are_skipped_and_a_missing_buffer_file_is_an_error() {
        let s = read(&inline(r#"[{"mesh":0}]"#, 1), None).unwrap();
        assert!(s.parts[0].mesh.triangles.is_empty());
        let (json, _) = gltf_json(r#"[{"mesh":0}]"#, 4, r#","uri":"nowhere.bin""#);
        let e = read(json.as_bytes(), Some(Path::new("/nonexistent"))).unwrap_err();
        assert!(e.contains("nowhere.bin"), "{e}");
    }

    #[test]
    fn percent_escapes_are_decoded() {
        assert_eq!(percent_decode("my%20model.bin"), "my model.bin");
        assert_eq!(percent_decode("100%"), "100%");
    }
}
