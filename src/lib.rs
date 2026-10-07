//! Mesh files in, one scene type out, for every cce app that reads geometry:
//! cce-model shows it, cce-designer imports it.
//!
//! Each reader returns a [`Scene`] as the file has it: named parts, in the
//! file's own coordinates and units, with the up axis its format conventionally
//! uses, and the length [`Unit`] its format conventionally means. Nothing is
//! rotated, scaled or welded by a reader; [`load`] welds, and what to do about
//! the up axis is the caller's choice ([`Mesh::z_up_to_y_up`]).
//! A viewer turns a Z-up STL upright; an editor importing its own Y-up export
//! must not.
//!
//! This crate draws nothing and knows no renderer, so a headless tool can use
//! it as freely as an app.

mod gltf;
mod mesh;
mod obj;
mod ply;
mod stl;

use std::path::Path;

pub use mesh::{srgb_to_linear, Material, Mesh, CLAY};

/// File extensions [`load`] reads, lowercase.
pub const EXTENSIONS: &[&str] = &["stl", "obj", "gltf", "glb", "ply"];

/// Which way is up in a file's coordinates, by its format's convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpAxis {
    /// OBJ, glTF (by its spec) and PLY.
    Y,
    /// STL: it comes from CAD and goes to printers, which build along Z.
    Z,
}

/// What one unit of a file's coordinates is, by its format's convention.
/// None of these formats can be trusted to say: it is what a file of that
/// kind usually means, and a caller showing a size should say which it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// STL: what every slicer assumes.
    Millimetre,
    /// glTF: metres, by its spec.
    Metre,
    /// OBJ and PLY say nothing.
    Unspecified,
}

impl Unit {
    /// Millimetres per file unit, or `None` when the format does not say.
    pub fn millimetres(self) -> Option<f32> {
        match self {
            Unit::Millimetre => Some(1.0),
            Unit::Metre => Some(1000.0),
            Unit::Unspecified => None,
        }
    }
}

/// An image a material names: decoded, 8-bit RGBA, rows top to bottom,
/// sRGB-encoded as the file had it (a renderer that samples it through an
/// sRGB format gets linear values).
#[derive(Clone)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Texture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Texture({}x{})", self.width, self.height)
    }
}

impl Texture {
    /// Decode a PNG or JPEG.
    pub fn decode(bytes: &[u8]) -> Result<Texture, String> {
        let image = image::load_from_memory(bytes).map_err(|e| format!("an image that will not decode: {e}"))?;
        let rgba = image.to_rgba8();
        Ok(Texture { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() })
    }

    /// The texel nearest `uv` (image convention, repeating), sRGB-encoded.
    pub fn sample(&self, uv: [f32; 2]) -> [u8; 4] {
        let wrap = |v: f32, n: u32| ((v - v.floor()) * n as f32).floor().clamp(0.0, n as f32 - 1.0) as usize;
        let (x, y) = (wrap(uv[0], self.width), wrap(uv[1], self.height));
        let i = (y * self.width as usize + x) * 4;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
}

/// One named piece of a scene: a glTF node's mesh, a whole STL.
#[derive(Debug, Clone, Default)]
pub struct Part {
    pub name: String,
    pub mesh: Mesh,
}

/// What a file holds: its parts, already placed where the file's node tree
/// puts them, its up axis and its unit, and the images its materials name.
#[derive(Debug, Clone)]
pub struct Scene {
    pub parts: Vec<Part>,
    pub up: UpAxis,
    pub unit: Unit,
    /// Indexed by [`Material::texture`].
    pub textures: Vec<Texture>,
}

impl Scene {
    pub fn triangle_count(&self) -> usize {
        self.parts.iter().map(|p| p.mesh.triangles.len()).sum()
    }

    /// Every part in one mesh. Parts are not welded to each other, so two
    /// parts that touch keep separate normals where they meet.
    pub fn merged(&self) -> Mesh {
        let mut out = Mesh::default();
        for part in &self.parts {
            out.append(&part.mesh);
        }
        out
    }
}

/// Read `path` by its extension, weld each part, and name an unnamed part
/// after the file.
pub fn load(path: &Path) -> Result<Scene, String> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let dir = path.parent();
    let mut scene = match ext.as_str() {
        "stl" => stl::read(&bytes)?,
        "obj" => obj::read(&bytes, dir)?,
        "gltf" | "glb" => gltf::read(&bytes, dir)?,
        "ply" => ply::read(&bytes)?,
        "" => return Err("no file extension, so no way to tell the format".into()),
        other => return Err(format!("cannot read .{other} files")),
    };
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("model");
    for part in &mut scene.parts {
        part.mesh.weld();
        if part.name.is_empty() {
            part.name = stem.to_string();
        }
    }
    scene.parts.retain(|p| !p.mesh.triangles.is_empty());
    if scene.parts.is_empty() {
        return Err("the file holds no triangles".into());
    }
    Ok(scene)
}
