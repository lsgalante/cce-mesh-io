//! PLY (Stanford polygon file), ASCII and binary of either byte order.
//!
//! A PLY file declares its own layout: a header lists elements (`vertex`,
//! `face`, and anything else a tool chose to add), each with a count and
//! typed properties, and the body follows in that order. So the reader
//! parses the header into that layout and walks the body by it, keeping
//! `x y z` and any `red green blue` of each vertex and the index list of each
//! face, and reading past everything else — an unknown element still has to
//! be read to find where the next one starts.
//!
//! Colours stored as 8-bit (the usual) are sRGB and are decoded to linear;
//! colours stored as floats are taken as linear already. They ride on
//! corners. A file of points with no faces is an error for now: the viewer
//! draws no point clouds yet. The scene is [`UpAxis::Y`].

use glam::Vec3;

use crate::mesh::{srgb_to_linear, Mesh, CLAY};
use crate::{Part, Scene, UpAxis};

#[derive(Debug, Clone, Copy, PartialEq)]
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
    fn parse(name: &str) -> Option<Scalar> {
        Some(match name {
            "char" | "int8" => Scalar::I8,
            "uchar" | "uint8" => Scalar::U8,
            "short" | "int16" => Scalar::I16,
            "ushort" | "uint16" => Scalar::U16,
            "int" | "int32" => Scalar::I32,
            "uint" | "uint32" => Scalar::U32,
            "float" | "float32" => Scalar::F32,
            "double" | "float64" => Scalar::F64,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Scalar::I8 | Scalar::U8 => 1,
            Scalar::I16 | Scalar::U16 => 2,
            Scalar::I32 | Scalar::U32 | Scalar::F32 => 4,
            Scalar::F64 => 8,
        }
    }
}

#[derive(Debug)]
enum Property {
    Scalar { name: String, ty: Scalar },
    List { name: String, count: Scalar, item: Scalar },
}

#[derive(Debug)]
struct Element {
    name: String,
    count: usize,
    properties: Vec<Property>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Encoding {
    Ascii,
    Little,
    Big,
}

/// The body, read one value at a time in whichever encoding the file uses.
struct Body<'a> {
    bytes: &'a [u8],
    at: usize,
    encoding: Encoding,
}

impl Body<'_> {
    fn value(&mut self, ty: Scalar) -> Result<f64, String> {
        if self.encoding == Encoding::Ascii {
            return self.word().and_then(|w| w.parse::<f64>().map_err(|_| format!("'{w}' is not a number")));
        }
        let n = ty.size();
        let raw = self.bytes.get(self.at..self.at + n).ok_or("the file ends in the middle of its data")?;
        self.at += n;
        let mut b = [0u8; 8];
        b[..n].copy_from_slice(raw);
        if self.encoding == Encoding::Big {
            b[..n].reverse();
        }
        Ok(match ty {
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

    fn word(&mut self) -> Result<&str, String> {
        let rest = &self.bytes[self.at..];
        let start = rest.iter().position(|c| !c.is_ascii_whitespace()).ok_or("the file ends in the middle of its data")?;
        let len = rest[start..].iter().position(|c| c.is_ascii_whitespace()).unwrap_or(rest.len() - start);
        self.at += start + len;
        std::str::from_utf8(&rest[start..start + len]).map_err(|_| "text that is not UTF-8".to_string())
    }
}

pub fn read(bytes: &[u8]) -> Result<Scene, String> {
    let (elements, encoding, body_at) = header(bytes)?;
    let mut body = Body { bytes, at: body_at, encoding };
    let mut positions: Vec<Vec3> = Vec::new();
    let mut point_colors: Vec<[f32; 3]> = Vec::new();
    let mut faces: Vec<[u32; 3]> = Vec::new();
    let mut corner: Vec<u32> = Vec::new();

    for el in &elements {
        match el.name.as_str() {
            "vertex" => {
                let at = |n: &str| {
                    el.properties.iter().position(|p| matches!(p, Property::Scalar { name, .. } if name == n))
                };
                let (Some(xi), Some(yi), Some(zi)) = (at("x"), at("y"), at("z")) else {
                    return Err("vertices without x, y and z".into());
                };
                let rgb = match (at("red"), at("green"), at("blue")) {
                    (Some(r), Some(g), Some(b)) => Some([r, g, b]),
                    _ => match (at("diffuse_red"), at("diffuse_green"), at("diffuse_blue")) {
                        (Some(r), Some(g), Some(b)) => Some([r, g, b]),
                        _ => None,
                    },
                };
                positions.reserve(el.count);
                let mut row = vec![0f64; el.properties.len()];
                for _ in 0..el.count {
                    for (i, p) in el.properties.iter().enumerate() {
                        row[i] = match p {
                            Property::Scalar { ty, .. } => body.value(*ty)?,
                            Property::List { count, item, .. } => {
                                skip_list(&mut body, *count, *item)?;
                                0.0
                            }
                        };
                    }
                    positions.push(Vec3::new(row[xi] as f32, row[yi] as f32, row[zi] as f32));
                    if let Some(idx) = rgb {
                        point_colors.push(idx.map(|i| match &el.properties[i] {
                            Property::Scalar { ty: Scalar::F32 | Scalar::F64, .. } => row[i] as f32,
                            _ => srgb_to_linear(row[i].clamp(0.0, 255.0) as u8),
                        }));
                    }
                }
            }
            "face" => {
                faces.reserve(el.count);
                for _ in 0..el.count {
                    for p in &el.properties {
                        match p {
                            Property::List { name, count, item } if name == "vertex_indices" || name == "vertex_index" => {
                                let n = body.value(*count)? as usize;
                                corner.clear();
                                for _ in 0..n {
                                    corner.push(body.value(*item)? as u32);
                                }
                                for k in 1..n.saturating_sub(1) {
                                    faces.push([corner[0], corner[k], corner[k + 1]]);
                                }
                            }
                            Property::List { count, item, .. } => skip_list(&mut body, *count, *item)?,
                            Property::Scalar { ty, .. } => {
                                body.value(*ty)?;
                            }
                        }
                    }
                }
            }
            _ => {
                for _ in 0..el.count {
                    for p in &el.properties {
                        match p {
                            Property::Scalar { ty, .. } => {
                                body.value(*ty)?;
                            }
                            Property::List { count, item, .. } => skip_list(&mut body, *count, *item)?,
                        }
                    }
                }
            }
        }
    }

    if faces.is_empty() {
        return Err(if positions.is_empty() {
            "no vertices and no faces".into()
        } else {
            format!("{} points and no faces: point clouds are not drawn yet", positions.len())
        });
    }
    let n = positions.len() as u32;
    if let Some(bad) = faces.iter().flatten().find(|&&i| i >= n) {
        return Err(format!("a face names vertex {bad}, but there are {n}"));
    }
    let corner_colors = (!point_colors.is_empty())
        .then(|| faces.iter().flat_map(|f| f.map(|i| point_colors[i as usize])).collect());
    let mesh = Mesh { tri_color: vec![0; faces.len()], positions, triangles: faces, colors: vec![CLAY], corner_colors };
    Ok(Scene { parts: vec![Part { name: String::new(), mesh }], up: UpAxis::Y })
}

fn skip_list(body: &mut Body, count: Scalar, item: Scalar) -> Result<(), String> {
    let n = body.value(count)? as usize;
    for _ in 0..n {
        body.value(item)?;
    }
    Ok(())
}

/// The elements, the encoding, and where the body starts.
fn header(bytes: &[u8]) -> Result<(Vec<Element>, Encoding, usize), String> {
    if !bytes.starts_with(b"ply") {
        return Err("not a PLY file (it does not begin with 'ply')".into());
    }
    let end = bytes
        .windows(10)
        .position(|w| w == b"end_header")
        .ok_or("the header never ends (no 'end_header')")?;
    // The body starts after the line end that follows end_header (\n or \r\n).
    let mut body_at = end + 10;
    while bytes.get(body_at).is_some_and(|&c| c == b' ' || c == b'\r') {
        body_at += 1;
    }
    if bytes.get(body_at) == Some(&b'\n') {
        body_at += 1;
    }
    let text = std::str::from_utf8(&bytes[..end]).map_err(|_| "a header that is not text".to_string())?;
    let mut encoding = None;
    let mut elements: Vec<Element> = Vec::new();
    for line in text.lines().skip(1) {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            ["format", "ascii", _] => encoding = Some(Encoding::Ascii),
            ["format", "binary_little_endian", _] => encoding = Some(Encoding::Little),
            ["format", "binary_big_endian", _] => encoding = Some(Encoding::Big),
            ["format", other, ..] => return Err(format!("unknown PLY format '{other}'")),
            ["element", name, count] => elements.push(Element {
                name: name.to_string(),
                count: count.parse().map_err(|_| format!("element {name} has count '{count}'"))?,
                properties: Vec::new(),
            }),
            ["property", "list", count, item, name] => {
                let el = elements.last_mut().ok_or("a property before any element")?;
                let ty = |t: &str| Scalar::parse(t).ok_or_else(|| format!("unknown property type '{t}'"));
                el.properties.push(Property::List { name: name.to_string(), count: ty(count)?, item: ty(item)? });
            }
            ["property", ty, name] => {
                let el = elements.last_mut().ok_or("a property before any element")?;
                let ty = Scalar::parse(ty).ok_or_else(|| format!("unknown property type '{ty}'"))?;
                el.properties.push(Property::Scalar { name: name.to_string(), ty });
            }
            ["comment", ..] | ["obj_info", ..] | [] => {}
            _ => log::debug!("[mesh-io] ply: skipped header line {line:?}"),
        }
    }
    Ok((elements, encoding.ok_or("no 'format' line")?, body_at))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASCII_QUAD: &str = "ply\nformat ascii 1.0\ncomment made by hand\nelement vertex 4\nproperty float x\nproperty float y\nproperty float z\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0 255 0 0\n1 0 0 255 0 0\n1 1 0 0 0 255\n0 1 0 0 0 255\n4 0 1 2 3\n";

    fn mesh(bytes: &[u8]) -> Mesh {
        read(bytes).unwrap().parts.remove(0).mesh
    }

    #[test]
    fn an_ascii_quad_is_two_coloured_triangles() {
        let m = mesh(ASCII_QUAD.as_bytes());
        assert_eq!(m.triangles, vec![[0, 1, 2], [0, 2, 3]]);
        let c = m.corner_colors.unwrap();
        assert_eq!(c[0], [1.0, 0.0, 0.0]);
        assert_eq!(c[2], [0.0, 0.0, 1.0], "corner 2 is vertex 2, blue");
    }

    /// The quad again, binary, with an extra element in the way and a
    /// per-face list it must read past.
    fn binary_quad(big: bool) -> Vec<u8> {
        let fmt = if big { "binary_big_endian" } else { "binary_little_endian" };
        let mut b = format!(
            "ply\r\nformat {fmt} 1.0\r\nelement vertex 4\r\nproperty double x\r\nproperty double y\r\nproperty double z\r\nelement face 1\r\nproperty list uchar uint vertex_indices\r\nproperty list uchar float texcoord\r\nelement extra 1\r\nproperty short junk\r\nend_header\r\n"
        )
        .into_bytes();
        let f64b = |v: f64| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        let u32b = |v: u32| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        for p in [[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.]] {
            for c in p {
                b.extend(f64b(c));
            }
        }
        b.push(4);
        for i in [0, 1, 2, 3] {
            b.extend(u32b(i));
        }
        b.push(2);
        b.extend([0u8; 8]);
        b.extend([7u8, 7]);
        b
    }

    #[test]
    fn binary_of_either_byte_order() {
        for big in [false, true] {
            let m = mesh(&binary_quad(big));
            assert_eq!(m.triangles, vec![[0, 1, 2], [0, 2, 3]], "big endian: {big}");
            assert_eq!(m.positions[2], Vec3::new(1.0, 1.0, 0.0));
            assert!(m.corner_colors.is_none());
        }
    }

    #[test]
    fn a_point_cloud_says_so() {
        let text = "ply\nformat ascii 1.0\nelement vertex 2\nproperty float x\nproperty float y\nproperty float z\nend_header\n0 0 0\n1 1 1\n";
        let e = read(text.as_bytes()).unwrap_err();
        assert!(e.contains("point clouds"), "{e}");
    }

    #[test]
    fn a_short_file_and_a_bad_index_are_errors() {
        let mut b = binary_quad(false);
        b.truncate(b.len() - 20);
        assert!(read(&b).unwrap_err().contains("ends in the middle"));
        let bad = ASCII_QUAD.replace("4 0 1 2 3", "3 0 1 9");
        assert!(read(bad.as_bytes()).unwrap_err().contains("names vertex 9"));
    }
}
