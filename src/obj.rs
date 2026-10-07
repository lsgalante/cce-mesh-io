//! Wavefront OBJ, with its MTL colours.
//!
//! What is read: `v` points, `f` faces (any polygon, fanned into triangles;
//! `i`, `i/t`, `i//n` and `i/t/n` corners; negative indices counting back
//! from the newest point), `usemtl`, and `mtllib` for each material's `Kd`
//! diffuse colour. Texture coordinates and the file's normals are skipped:
//! the viewer derives normals itself and draws no textures yet. A missing
//! MTL file is not an error; its materials fall back to clay. One part for
//! the whole file, named by its first `o`; the scene is [`UpAxis::Y`].

use std::collections::HashMap;
use std::path::Path;

use glam::Vec3;

use crate::mesh::{Mesh, CLAY};
use crate::{Part, Scene, Unit, UpAxis};

pub fn read(bytes: &[u8], dir: Option<&Path>) -> Result<Scene, String> {
    let text = String::from_utf8_lossy(bytes);
    let mut mesh = Mesh { colors: vec![CLAY], ..Mesh::default() };
    // Material name → index into `mesh.colors`, filled as `mtllib`s are read.
    let mut library: HashMap<String, [f32; 3]> = HashMap::new();
    let mut slot: HashMap<String, u32> = HashMap::new();
    let mut current = 0u32;
    let mut corners: Vec<u32> = Vec::new();
    let mut name = String::new();

    for (line_no, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("");
        let mut words = line.split_whitespace();
        let err = |what: &str| format!("line {}: {what}", line_no + 1);
        match words.next() {
            Some("v") => {
                let mut xyz = [0f32; 3];
                for v in &mut xyz {
                    *v = words.next().and_then(|w| w.parse().ok()).ok_or_else(|| err("a point needs three numbers"))?;
                }
                mesh.positions.push(Vec3::from(xyz));
            }
            Some("f") => {
                corners.clear();
                for w in words {
                    let i: i64 = w.split('/').next().and_then(|s| s.parse().ok()).ok_or_else(|| err("a bad face corner"))?;
                    let n = mesh.positions.len() as i64;
                    let index = if i > 0 { i - 1 } else { n + i };
                    if i == 0 || !(0..n).contains(&index) {
                        return Err(err(&format!("face corner {i} names no point (there are {n} so far)")));
                    }
                    corners.push(index as u32);
                }
                if corners.len() < 3 {
                    return Err(err("a face needs three corners"));
                }
                for k in 1..corners.len() - 1 {
                    mesh.triangles.push([corners[0], corners[k], corners[k + 1]]);
                    mesh.tri_color.push(current);
                }
            }
            Some("o") if name.is_empty() => name = words.collect::<Vec<_>>().join(" "),
            Some("mtllib") => {
                // A library name may hold spaces, so take the rest of the line.
                let name = line.trim_start().strip_prefix("mtllib").unwrap_or("").trim();
                if let Some(dir) = dir {
                    match std::fs::read_to_string(dir.join(name)) {
                        Ok(mtl) => library.extend(read_mtl(&mtl)),
                        Err(e) => log::warn!("[model] material library {name}: {e}"),
                    }
                }
            }
            Some("usemtl") => {
                let name = words.next().unwrap_or("").to_string();
                current = *slot.entry(name.clone()).or_insert_with(|| {
                    mesh.colors.push(library.get(&name).copied().unwrap_or(CLAY));
                    (mesh.colors.len() - 1) as u32
                });
            }
            _ => {}
        }
    }
    Ok(Scene { parts: vec![Part { name, mesh }], up: UpAxis::Y, unit: Unit::Unspecified })
}

/// Each `newmtl` in an MTL file and its `Kd`.
fn read_mtl(text: &str) -> HashMap<String, [f32; 3]> {
    let mut out = HashMap::new();
    let mut name: Option<String> = None;
    for line in text.lines() {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("newmtl") => name = words.next().map(str::to_string),
            Some("Kd") => {
                let rgb: Vec<f32> = words.take(3).filter_map(|w| w.parse().ok()).collect();
                if let (Some(n), [r, g, b]) = (&name, rgb.as_slice()) {
                    out.insert(n.clone(), [*r, *g, *b]);
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quad_is_two_triangles_and_negative_indices_count_back() {
        let s = read(b"o quad\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf -4/1/1 -3/2/1 -2/3/1 -1/4/1\n", None).unwrap();
        assert_eq!(s.parts[0].name, "quad");
        assert_eq!(s.parts[0].mesh.triangles, vec![[0, 1, 2], [0, 2, 3]]);
    }

    #[test]
    fn materials_colour_their_faces() {
        let dir = std::env::temp_dir().join(format!("cce-model-obj-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("m.mtl"), "newmtl red\nKd 0.8 0.1 0.1\n").unwrap();
        let obj = b"mtllib m.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\nusemtl red\nf 1 3 2\nusemtl missing\nf 2 1 3\n";
        let m = read(obj, Some(&dir)).unwrap().parts.remove(0).mesh;
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(m.colors[m.tri_color[0] as usize], CLAY, "before any usemtl");
        assert_eq!(m.colors[m.tri_color[1] as usize], [0.8, 0.1, 0.1]);
        assert_eq!(m.colors[m.tri_color[2] as usize], CLAY, "a material the library lacks");
    }

    #[test]
    fn a_face_past_the_points_is_an_error() {
        let e = read(b"v 0 0 0\nv 1 0 0\nf 1 2 3\n", None).unwrap_err();
        assert!(e.contains("line 3") && e.contains("names no point"), "{e}");
    }
}
