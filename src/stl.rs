//! STL, binary and ASCII.
//!
//! Binary is told from ASCII by its size, not by its first word: the format
//! leaves the 80-byte header free, and plenty of binary files begin it with
//! `solid`, the word that opens an ASCII one. A binary file is exactly
//! 84 + 50 × its triangle count bytes long.
//!
//! Points are returned as written, with [`UpAxis::Z`], STL's convention;
//! turning them upright is the caller's call. The file's own facet normals
//! are ignored: normals come from the winding. Nor is the attribute word
//! read as a colour — the two vendor schemes for that disagree.

use glam::Vec3;

use crate::mesh::{Mesh, CLAY};
use crate::{Part, Scene, UpAxis};

pub fn read(bytes: &[u8]) -> Result<Scene, String> {
    let corners = if is_binary(bytes) { binary(bytes)? } else { ascii(bytes)? };
    let n = corners.len() / 3;
    let mesh = Mesh {
        positions: corners,
        triangles: (0..n as u32).map(|t| [t * 3, t * 3 + 1, t * 3 + 2]).collect(),
        tri_color: vec![0; n],
        colors: vec![CLAY],
        corner_colors: None,
    };
    Ok(Scene { parts: vec![Part { name: String::new(), mesh }], up: UpAxis::Z })
}

fn is_binary(bytes: &[u8]) -> bool {
    if bytes.len() < 84 {
        return false;
    }
    let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as u64;
    84 + 50 * count == bytes.len() as u64 || !bytes.starts_with(b"solid")
}

fn binary(bytes: &[u8]) -> Result<Vec<Vec3>, String> {
    let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
    let need = 84 + 50 * count;
    if bytes.len() < need {
        return Err(format!(
            "binary STL says {count} triangles ({need} bytes) but the file is {} bytes; it is cut short",
            bytes.len()
        ));
    }
    let f = |o: usize| f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let mut out = Vec::with_capacity(count * 3);
    for t in 0..count {
        // 12 bytes of facet normal, three 12-byte corners, 2 bytes attribute.
        let base = 84 + 50 * t + 12;
        for k in 0..3 {
            let o = base + 12 * k;
            out.push(Vec3::new(f(o), f(o + 4), f(o + 8)));
        }
    }
    Ok(out)
}

fn ascii(bytes: &[u8]) -> Result<Vec<Vec3>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "not a binary STL, and not text either".to_string())?;
    let mut out = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        let mut words = line.split_whitespace();
        if words.next() != Some("vertex") {
            continue;
        }
        let mut xyz = [0f32; 3];
        for v in &mut xyz {
            *v = words
                .next()
                .and_then(|w| w.parse().ok())
                .ok_or_else(|| format!("line {}: a vertex needs three numbers", line_no + 1))?;
        }
        out.push(Vec3::from(xyz));
    }
    if out.len() % 3 != 0 {
        return Err(format!("{} vertices is not a whole number of triangles", out.len()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binary_of(tris: &[[[f32; 3]; 3]]) -> Vec<u8> {
        let mut b = vec![0u8; 80];
        b[..5].copy_from_slice(b"solid"); // the header may say anything, this included
        b.extend((tris.len() as u32).to_le_bytes());
        for t in tris {
            b.extend([0u8; 12]);
            for p in t {
                for c in p {
                    b.extend(c.to_le_bytes());
                }
            }
            b.extend([0u8; 2]);
        }
        b
    }

    fn mesh(bytes: &[u8]) -> Mesh {
        read(bytes).unwrap().parts.remove(0).mesh
    }

    #[test]
    fn a_binary_file_headed_solid_is_read_as_binary() {
        let m = mesh(&binary_of(&[[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]]]));
        assert_eq!(m.triangles.len(), 1);
        assert_eq!(m.positions[1], Vec3::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn points_are_as_written_and_the_scene_says_z_is_up() {
        let s = read(&binary_of(&[[[0., 0., 2.], [1., 0., 0.], [0., 3., 0.]]])).unwrap();
        assert_eq!(s.up, UpAxis::Z);
        assert_eq!(s.parts[0].mesh.positions[0], Vec3::new(0.0, 0.0, 2.0));
    }

    #[test]
    fn ascii_is_read() {
        let text = "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n";
        assert_eq!(mesh(text.as_bytes()).triangles.len(), 1);
    }

    #[test]
    fn a_short_binary_file_is_an_error() {
        let mut b = binary_of(&[[[0.; 3], [1., 0., 0.], [0., 1., 0.]]; 2]);
        b.truncate(b.len() - 30);
        b[..5].copy_from_slice(b"model"); // not "solid": nothing to try as text
        let e = read(&b).unwrap_err();
        assert!(e.contains("cut short"), "{e}");
    }

    #[test]
    fn a_bad_ascii_vertex_is_an_error() {
        let e = read(b"solid t\nvertex 0 0\n").unwrap_err();
        assert!(e.contains("line 2"), "{e}");
    }
}
