//! Wavefront OBJ, with its MTL materials.
//!
//! What is read: `v` points, `vt` texture coordinates, `vn` normals, `f`
//! faces (any polygon, fanned into triangles; `i`, `i/t`, `i//n` and
//! `i/t/n` corners; negative indices counting back from the newest), `usemtl`,
//! and `mtllib` for each material's `Kd` colour, `map_Kd` texture (PNG or
//! JPEG, relative to the MTL file), `Ns` shininess (as a roughness) and the
//! PBR extension's `Pm` / `Pr` when present. A `vt` is flipped in v on the
//! way in: OBJ's origin is the image's bottom-left, the crate's its top-left.
//! The file's normals are kept only if every corner has one. A missing MTL
//! file or texture is not an error; the material falls back to clay, or to
//! its colour. One part for the whole file, named by its first `o`; the
//! scene is [`UpAxis::Y`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glam::Vec3;

use crate::mesh::{Material, Mesh};
use crate::{Part, Scene, Texture, Unit, UpAxis};

/// A material as an MTL file states it, before its texture is loaded.
#[derive(Debug, Clone, Default)]
struct MtlEntry {
    kd: Option<[f32; 3]>,
    map_kd: Option<PathBuf>,
    ns: Option<f32>,
    pm: Option<f32>,
    pr: Option<f32>,
}

pub fn read(bytes: &[u8], dir: Option<&Path>) -> Result<Scene, String> {
    let text = String::from_utf8_lossy(bytes);
    let mut mesh = Mesh { materials: vec![Material::default()], ..Mesh::default() };
    let mut textures: Vec<Texture> = Vec::new();
    let mut texture_slot: HashMap<PathBuf, Option<usize>> = HashMap::new();
    let mut library: HashMap<String, MtlEntry> = HashMap::new();
    let mut slot: HashMap<String, u32> = HashMap::new();
    let mut current = 0u32;
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut normals: Vec<Vec3> = Vec::new();
    // Per corner, as triangles are emitted: its uv and normal, if named.
    let mut corner_uv: Vec<Option<[f32; 2]>> = Vec::new();
    let mut corner_n: Vec<Option<Vec3>> = Vec::new();
    let mut corners: Vec<(u32, Option<[f32; 2]>, Option<Vec3>)> = Vec::new();
    let mut name = String::new();

    for (line_no, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("");
        let mut words = line.split_whitespace();
        let err = |what: &str| format!("line {}: {what}", line_no + 1);
        let numbers = |words: &mut std::str::SplitWhitespace, n: usize, what: &str| -> Result<Vec<f32>, String> {
            let v: Vec<f32> = words.take(n).map(|w| w.parse::<f32>()).collect::<Result<_, _>>().map_err(|_| err(what))?;
            if v.len() < n {
                return Err(err(what));
            }
            Ok(v)
        };
        match words.next() {
            Some("v") => {
                let p = numbers(&mut words, 3, "a point needs three numbers")?;
                mesh.positions.push(Vec3::new(p[0], p[1], p[2]));
            }
            Some("vt") => {
                let t = numbers(&mut words, 1, "a texture coordinate needs a number")?;
                let v = words.next().and_then(|w| w.parse::<f32>().ok()).unwrap_or(0.0);
                uvs.push([t[0], 1.0 - v]);
            }
            Some("vn") => {
                let n = numbers(&mut words, 3, "a normal needs three numbers")?;
                normals.push(Vec3::new(n[0], n[1], n[2]).normalize_or_zero());
            }
            Some("f") => {
                corners.clear();
                for w in words {
                    let mut parts = w.split('/');
                    let resolve = |s: Option<&str>, n: usize, what: &str| -> Result<Option<usize>, String> {
                        let Some(s) = s.filter(|s| !s.is_empty()) else { return Ok(None) };
                        let i: i64 = s.parse().map_err(|_| err("a bad face corner"))?;
                        let index = if i > 0 { i - 1 } else { n as i64 + i };
                        if i == 0 || !(0..n as i64).contains(&index) {
                            return Err(err(&format!("face corner {i} names no {what} (there are {n} so far)")));
                        }
                        Ok(Some(index as usize))
                    };
                    let p = resolve(parts.next(), mesh.positions.len(), "point")?.ok_or_else(|| err("a bad face corner"))?;
                    let t = resolve(parts.next(), uvs.len(), "texture coordinate")?;
                    let n = resolve(parts.next(), normals.len(), "normal")?;
                    corners.push((p as u32, t.map(|t| uvs[t]), n.map(|n| normals[n])));
                }
                if corners.len() < 3 {
                    return Err(err("a face needs three corners"));
                }
                for k in 1..corners.len() - 1 {
                    let tri = [corners[0], corners[k], corners[k + 1]];
                    mesh.triangles.push(tri.map(|c| c.0));
                    mesh.tri_material.push(current);
                    corner_uv.extend(tri.map(|c| c.1));
                    corner_n.extend(tri.map(|c| c.2));
                }
            }
            Some("o") if name.is_empty() => name = words.collect::<Vec<_>>().join(" "),
            Some("mtllib") => {
                // A library name may hold spaces, so take the rest of the line.
                let lib = line.trim_start().strip_prefix("mtllib").unwrap_or("").trim().replace('\\', "/");
                if let Some(dir) = dir {
                    let path = dir.join(&lib);
                    match std::fs::read_to_string(&path) {
                        Ok(mtl) => library.extend(read_mtl(&mtl, path.parent().unwrap_or(dir))),
                        Err(e) => log::warn!("[mesh-io] material library {lib}: {e}"),
                    }
                }
            }
            Some("usemtl") => {
                let key = words.next().unwrap_or("").to_string();
                current = *slot.entry(key.clone()).or_insert_with(|| {
                    let entry = library.get(&key).cloned().unwrap_or_default();
                    let texture = entry.map_kd.as_ref().and_then(|path| {
                        *texture_slot.entry(path.clone()).or_insert_with(|| {
                            match std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| Texture::decode(&b)) {
                                Ok(t) => {
                                    textures.push(t);
                                    Some(textures.len() - 1)
                                }
                                Err(e) => {
                                    log::warn!("[mesh-io] texture {}: {e}", path.display());
                                    None
                                }
                            }
                        })
                    });
                    let mut m = Material::default();
                    if let Some(kd) = entry.kd {
                        m.color = kd;
                    } else if texture.is_some() {
                        // A texture with no Kd: the texture alone, untinted.
                        m.color = [1.0; 3];
                    }
                    m.texture = texture;
                    // Phong shininess to a roughness: sqrt(2 / (Ns + 2)).
                    if let Some(ns) = entry.ns {
                        m.roughness = (2.0 / (ns.max(0.0) + 2.0)).sqrt().clamp(0.04, 1.0);
                    }
                    m.roughness = entry.pr.unwrap_or(m.roughness);
                    m.metallic = entry.pm.unwrap_or(0.0);
                    mesh.materials.push(m);
                    (mesh.materials.len() - 1) as u32
                });
            }
            _ => {}
        }
    }
    if corner_uv.iter().any(Option::is_some) {
        mesh.corner_uvs = Some(corner_uv.into_iter().map(|t| t.unwrap_or([0.0; 2])).collect());
    }
    if !corner_n.is_empty() && corner_n.iter().all(|n| n.is_some_and(|n| n != Vec3::ZERO)) {
        mesh.file_normals = Some(corner_n.into_iter().map(Option::unwrap).collect());
    }
    Ok(Scene { parts: vec![Part { name, mesh }], up: UpAxis::Y, unit: Unit::Unspecified, textures })
}

/// Each `newmtl` in an MTL file and what it says; texture paths are
/// resolved against the MTL file's folder.
fn read_mtl(text: &str, dir: &Path) -> HashMap<String, MtlEntry> {
    let mut out: HashMap<String, MtlEntry> = HashMap::new();
    let mut name: Option<String> = None;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("");
        let mut words = line.split_whitespace();
        let Some(key) = words.next() else { continue };
        if key == "newmtl" {
            name = words.next().map(str::to_string);
            if let Some(n) = &name {
                out.entry(n.clone()).or_default();
            }
            continue;
        }
        let Some(entry) = name.as_ref().and_then(|n| out.get_mut(n)) else { continue };
        let rest: Vec<&str> = words.collect();
        let first = rest.first().and_then(|w| w.parse::<f32>().ok());
        match key {
            "Kd" => {
                let rgb: Vec<f32> = rest.iter().take(3).filter_map(|w| w.parse().ok()).collect();
                if let [r, g, b] = rgb.as_slice() {
                    entry.kd = Some([*r, *g, *b]);
                }
            }
            // Options (`-s 1 1 1`, `-bm 0.5`) come before the file name, so
            // the name is the last word (file names with spaces are rare in
            // MTL, and ambiguous there anyway).
            "map_Kd" => entry.map_kd = rest.last().map(|f| dir.join(f.replace('\\', "/"))),
            "Ns" => entry.ns = first,
            "Pm" => entry.pm = first,
            "Pr" => entry.pr = first,
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CLAY;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cce-mesh-io-obj-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_quad_is_two_triangles_and_negative_indices_count_back() {
        let s = read(b"o quad\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nf -4/-4 -3/-3 -2/-2 -1/-1\n", None).unwrap();
        assert_eq!(s.parts[0].name, "quad");
        let m = &s.parts[0].mesh;
        assert_eq!(m.triangles, vec![[0, 1, 2], [0, 2, 3]]);
        let uv = m.corner_uvs.as_ref().unwrap();
        assert_eq!(uv[0], [0.0, 1.0], "vt (0, 0) is the image's bottom-left");
        assert_eq!(uv[2], [1.0, 0.0]);
        assert!(m.file_normals.is_none());
    }

    #[test]
    fn normals_are_kept_when_every_corner_has_one() {
        let s = read(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nvn 0 0 2\nf 1//1 2//1 3//1\n", None).unwrap();
        assert_eq!(s.parts[0].mesh.file_normals.as_ref().unwrap()[1], Vec3::Z, "normalized");
        let s = read(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nvn 0 0 1\nf 1//1 2//1 3//1\nf 2 4 3\n", None).unwrap();
        assert!(s.parts[0].mesh.file_normals.is_none(), "one face without normals");
    }

    #[test]
    fn materials_colour_their_faces() {
        let dir = temp_dir("colour");
        std::fs::write(dir.join("m.mtl"), "newmtl red\nKd 0.8 0.1 0.1\nNs 0\n").unwrap();
        let obj = b"mtllib m.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\nusemtl red\nf 1 3 2\nusemtl missing\nf 2 1 3\n";
        let m = read(obj, Some(&dir)).unwrap().parts.remove(0).mesh;
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(m.material(0).color, CLAY, "before any usemtl");
        assert_eq!(m.material(1).color, [0.8, 0.1, 0.1]);
        assert_eq!(m.material(1).roughness, 1.0, "Ns 0 is fully rough");
        assert_eq!(m.material(2).color, CLAY, "a material the library lacks");
    }

    #[test]
    fn map_kd_loads_a_texture_once() {
        let dir = temp_dir("texture");
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(2, 1, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        std::fs::create_dir_all(dir.join("tex")).unwrap();
        std::fs::write(dir.join("tex/red.png"), &png).unwrap();
        std::fs::write(dir.join("m.mtl"), "newmtl a\nmap_Kd -s 1 1 1 tex\\red.png\nnewmtl b\nKd 0.5 0.5 0.5\nmap_Kd tex/red.png\n").unwrap();
        let obj = b"mtllib m.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl a\nf 1 2 3\nusemtl b\nf 1 3 2\n";
        let s = read(obj, Some(&dir)).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(s.textures.len(), 1, "one image, named twice");
        assert_eq!((s.textures[0].width, s.textures[0].height), (2, 1));
        let m = &s.parts[0].mesh;
        assert_eq!(m.material(0).texture, Some(0));
        assert_eq!(m.material(0).color, [1.0; 3], "no Kd: the texture untinted");
        assert_eq!(m.material(1).color, [0.5; 3]);
    }

    #[test]
    fn a_face_past_the_points_is_an_error() {
        let e = read(b"v 0 0 0\nv 1 0 0\nf 1 2 3\n", None).unwrap_err();
        assert!(e.contains("line 3") && e.contains("names no point"), "{e}");
    }
}
