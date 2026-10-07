//! The geometry every reader produces: a triangle mesh with a material per
//! triangle and, when the file has them, a colour, a texture coordinate and a
//! normal per corner.
//!
//! ## Why meshes are welded
//!
//! An STL has no shared points: each triangle carries its own corners. A
//! smooth normal is the average of the faces meeting at a point, which needs
//! those faces to name the same point, so [`crate::load`] welds every part by
//! position. Colours, texture coordinates and the file's own normals ride on
//! CORNERS, not points, so welding never has to choose between two of them
//! meeting at one place (a texture seam, a hard edge).

use std::collections::HashMap;

use glam::Vec3;

/// The colour of a surface that names none (an STL; an OBJ without a
/// material), linear RGB: a warm clay.
pub const CLAY: [f32; 3] = [0.42, 0.40, 0.36];

/// How a surface looks: glTF's metallic-roughness model, which the other
/// formats map onto (an MTL's `Kd` is the colour, its `Ns` a roughness).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Material {
    /// Linear RGB, multiplying the texture when there is one.
    pub color: [f32; 3],
    /// An index into the scene's `textures`.
    pub texture: Option<usize>,
    /// 0 a dielectric, 1 a metal.
    pub metallic: f32,
    /// 0 a mirror, 1 fully rough.
    pub roughness: f32,
}

impl Default for Material {
    fn default() -> Self {
        Self { color: CLAY, texture: None, metallic: 0.0, roughness: 0.8 }
    }
}

impl Material {
    /// A plain surface of this colour.
    pub fn colour(color: [f32; 3]) -> Self {
        Self { color, ..Self::default() }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    /// Counter-clockwise seen from outside.
    pub triangles: Vec<[u32; 3]>,
    /// One per triangle: an index into `materials`.
    pub tri_material: Vec<u32>,
    pub materials: Vec<Material>,
    /// Three per triangle, in `triangles` order, linear RGB, when the file
    /// colours its points (PLY, glTF `COLOR_0`). They multiply the
    /// material's colour (and replace it in [`Mesh::corner_color`]).
    pub corner_colors: Option<Vec<[f32; 3]>>,
    /// Three per triangle, when the file has texture coordinates. Image
    /// convention: (0, 0) the top-left texel (an OBJ `vt` is flipped in v
    /// on the way in). They may run past 0..1: textures repeat.
    pub corner_uvs: Option<Vec<[f32; 2]>>,
    /// Three per triangle, unit length, when the file brings its own
    /// normals; a caller without them derives them ([`Mesh::corner_normals`]).
    pub file_normals: Option<Vec<Vec3>>,
}

impl Mesh {
    /// The axis-aligned box around every point, or `None` for no points.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let first = *self.positions.first()?;
        Some(self.positions.iter().fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p))))
    }

    /// The material of triangle `t`.
    pub fn material(&self, t: usize) -> Material {
        self.materials.get(self.tri_material[t] as usize).copied().unwrap_or_default()
    }

    /// The colour of corner `k` (0..3) of triangle `t`, without its texture:
    /// the corner's own colour if the file gives one, else its material's.
    pub fn corner_color(&self, t: usize, k: usize) -> [f32; 3] {
        if let Some(c) = &self.corner_colors {
            return c[t * 3 + k];
        }
        self.material(t).color
    }

    /// Add `other`'s triangles to this mesh, its points and materials after
    /// this one's (texture indices are the scene's, so they are kept).
    /// Corner colours survive: if either side has them, the other's
    /// triangles are given their material colours as corner colours. Corner
    /// texture coordinates survive likewise, (0, 0) where a side had none;
    /// the file's normals survive only if both sides have them.
    pub fn append(&mut self, other: &Mesh) {
        let base = self.positions.len() as u32;
        let first = self.materials.len() as u32;
        if self.corner_colors.is_some() || other.corner_colors.is_some() {
            let mine = self.expanded_corner_colors();
            let theirs = other.expanded_corner_colors();
            self.corner_colors = Some(mine.into_iter().chain(theirs).collect());
        }
        if self.corner_uvs.is_some() || other.corner_uvs.is_some() {
            let uvs = |m: &Mesh| m.corner_uvs.clone().unwrap_or_else(|| vec![[0.0; 2]; m.triangles.len() * 3]);
            let mut mine = uvs(self);
            mine.extend(uvs(other));
            self.corner_uvs = Some(mine);
        }
        let empty = self.triangles.is_empty();
        self.file_normals = match (self.file_normals.take(), &other.file_normals) {
            (Some(mut mine), Some(theirs)) => {
                mine.extend_from_slice(theirs);
                Some(mine)
            }
            (None, Some(theirs)) if empty => Some(theirs.clone()),
            _ => None,
        };
        self.positions.extend_from_slice(&other.positions);
        self.triangles.extend(other.triangles.iter().map(|t| t.map(|i| i + base)));
        self.tri_material.extend(other.tri_material.iter().map(|c| c + first));
        self.materials.extend_from_slice(&other.materials);
    }

    fn expanded_corner_colors(&self) -> Vec<[f32; 3]> {
        if let Some(c) = &self.corner_colors {
            return c.clone();
        }
        (0..self.triangles.len()).flat_map(|t| [0, 1, 2].map(|k| self.corner_color(t, k))).collect()
    }

    /// Turn a Z-up mesh Y-up: (x, y, z) → (x, z, −y). A rotation, not a
    /// mirror, so the winding and with it the outward side of every face are
    /// kept.
    pub fn z_up_to_y_up(&mut self) {
        let turn = |p: &mut Vec3| *p = Vec3::new(p.x, p.z, -p.y);
        self.positions.iter_mut().for_each(turn);
        if let Some(n) = &mut self.file_normals {
            n.iter_mut().for_each(turn);
        }
    }

    /// Move and scale the mesh so its bounding box is centred on the origin
    /// and its farthest point is 1 from it, and return the (centre, radius)
    /// it had, so its real size can still be reported. The radius is the
    /// farthest point's, not the box's half-diagonal: a sphere's box corners
    /// sit 1.7 times farther out than any of its points.
    pub fn fit_to_unit(&mut self) -> (Vec3, f32) {
        let Some((lo, hi)) = self.bounds() else { return (Vec3::ZERO, 1.0) };
        let centre = (lo + hi) / 2.0;
        let radius = self.positions.iter().map(|p| p.distance(centre)).fold(0.0, f32::max).max(f32::MIN_POSITIVE);
        for p in &mut self.positions {
            *p = (*p - centre) / radius;
        }
        (centre, radius)
    }

    /// Merge points that sit at the same place (to a millionth of the
    /// mesh's size) and drop the triangles that collapse doing so.
    pub fn weld(&mut self) {
        let Some((lo, hi)) = self.bounds() else { return };
        let cell = ((hi - lo).length() * 1e-6).max(f32::MIN_POSITIVE);
        let key = |p: Vec3| {
            let q = (p - lo) / cell;
            [q.x.round() as i64, q.y.round() as i64, q.z.round() as i64]
        };
        // Sized for a closed mesh's usual one point per six corners: an STL
        // of millions of triangles would otherwise rehash its way up.
        let mut index: HashMap<[i64; 3], u32> = HashMap::with_capacity(self.positions.len() / 4);
        let mut positions = Vec::with_capacity(self.positions.len() / 4);
        let remap: Vec<u32> = self
            .positions
            .iter()
            .map(|p| {
                *index.entry(key(*p)).or_insert_with(|| {
                    positions.push(*p);
                    (positions.len() - 1) as u32
                })
            })
            .collect();
        drop(index);
        let mut triangles = Vec::with_capacity(self.triangles.len());
        let mut tri_material = Vec::with_capacity(self.triangles.len());
        let mut kept = Vec::with_capacity(self.triangles.len());
        for (t, tri) in self.triangles.iter().enumerate() {
            let [a, b, c] = tri.map(|i| remap[i as usize]);
            if a != b && b != c && a != c {
                triangles.push([a, b, c]);
                tri_material.push(self.tri_material[t]);
                kept.push(t);
            }
        }
        // The corner attributes of the triangles that stayed.
        fn keep<T: Copy>(src: &Option<Vec<T>>, kept: &[usize]) -> Option<Vec<T>> {
            src.as_ref().map(|s| kept.iter().flat_map(|&t| [s[t * 3], s[t * 3 + 1], s[t * 3 + 2]]).collect())
        }
        let corner_colors = keep(&self.corner_colors, &kept);
        let corner_uvs = keep(&self.corner_uvs, &kept);
        let file_normals = keep(&self.file_normals, &kept);
        positions.shrink_to_fit();
        self.positions = positions;
        self.triangles = triangles;
        self.tri_material = tri_material;
        self.corner_colors = corner_colors;
        self.corner_uvs = corner_uvs;
        self.file_normals = file_normals;
    }

    /// A normal for each triangle corner (three per triangle, in order):
    /// the area-weighted average of the faces at that point which meet this
    /// triangle within `crease_degrees`, so a cube keeps its edges and a
    /// sphere's facets blend.
    pub fn corner_normals(&self, crease_degrees: f32) -> Vec<Vec3> {
        let crease_cos = crease_degrees.to_radians().cos();
        // The cross product's length is twice the area, so summing raw
        // crosses weights each face by its area.
        let cross: Vec<Vec3> = self
            .triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| self.positions[i as usize]);
                (b - a).cross(c - a)
            })
            .collect();
        let unit: Vec<Vec3> = cross.iter().map(|n| n.normalize_or_zero()).collect();

        // Faces at each point, as one flat list with offsets.
        let mut start = vec![0u32; self.positions.len() + 1];
        for t in &self.triangles {
            for &v in t {
                start[v as usize + 1] += 1;
            }
        }
        for i in 1..start.len() {
            start[i] += start[i - 1];
        }
        let mut fill = start.clone();
        let mut faces = vec![0u32; self.triangles.len() * 3];
        for (f, t) in self.triangles.iter().enumerate() {
            for &v in t {
                faces[fill[v as usize] as usize] = f as u32;
                fill[v as usize] += 1;
            }
        }

        let mut out = Vec::with_capacity(self.triangles.len() * 3);
        for (f, t) in self.triangles.iter().enumerate() {
            for &v in t {
                let around = &faces[start[v as usize] as usize..start[v as usize + 1] as usize];
                let sum: Vec3 = around
                    .iter()
                    .filter(|&&g| unit[g as usize].dot(unit[f]) >= crease_cos)
                    .map(|&g| cross[g as usize])
                    .sum();
                out.push(sum.try_normalize().unwrap_or(unit[f]));
            }
        }
        out
    }
}

/// An 8-bit sRGB channel (as PLY colours are stored) as linear 0..1.
pub fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A unit cube as an STL would carry it: twelve triangles, every corner
    /// its own point.
    pub(crate) fn soup_cube() -> Mesh {
        let c = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
        let quads = [
            [c(1., 0., 0.), c(1., 1., 0.), c(1., 1., 1.), c(1., 0., 1.)],
            [c(0., 0., 1.), c(0., 1., 1.), c(0., 1., 0.), c(0., 0., 0.)],
            [c(0., 1., 0.), c(0., 1., 1.), c(1., 1., 1.), c(1., 1., 0.)],
            [c(0., 0., 1.), c(0., 0., 0.), c(1., 0., 0.), c(1., 0., 1.)],
            [c(0., 0., 1.), c(1., 0., 1.), c(1., 1., 1.), c(0., 1., 1.)],
            [c(1., 0., 0.), c(0., 0., 0.), c(0., 1., 0.), c(1., 1., 0.)],
        ];
        let mut m = Mesh::default();
        for q in quads {
            for i in [0, 1, 2, 0, 2, 3] {
                m.positions.push(q[i]);
            }
        }
        m.triangles = (0..12).map(|t| [t * 3, t * 3 + 1, t * 3 + 2]).collect();
        m.tri_material = vec![0; 12];
        m.materials = vec![Material::default()];
        m
    }

    #[test]
    fn welding_a_cube_leaves_its_eight_corners() {
        let mut m = soup_cube();
        assert_eq!(m.positions.len(), 36);
        m.weld();
        assert_eq!(m.positions.len(), 8);
        assert_eq!(m.triangles.len(), 12);
    }

    #[test]
    fn a_cube_keeps_its_edges_and_its_faces_point_out() {
        let mut m = soup_cube();
        m.weld();
        let normals = m.corner_normals(40.0);
        let (lo, hi) = m.bounds().unwrap();
        let center = (lo + hi) / 2.0;
        for (f, t) in m.triangles.iter().enumerate() {
            let [a, b, c] = t.map(|i| m.positions[i as usize]);
            let face = (b - a).cross(c - a).normalize();
            assert!(face.dot((a + b + c) / 3.0 - center) > 0.0, "triangle {f} faces inward");
            for k in 0..3 {
                assert!(normals[f * 3 + k].dot(face) > 0.999, "triangle {f} corner {k} was smoothed");
            }
        }
    }

    #[test]
    fn a_shallow_fold_is_smoothed() {
        let mut m = Mesh::default();
        let lift = 20f32.to_radians().tan();
        m.positions = vec![Vec3::ZERO, Vec3::Z, Vec3::new(-1.0, 0.0, 0.5), Vec3::new(1.0, lift, 0.5)];
        m.triangles = vec![[0, 1, 2], [0, 3, 1]];
        m.tri_material = vec![0, 0];
        let n = m.corner_normals(40.0);
        assert!(n[0].dot(n[3]) > 0.9999, "the shared point has two normals");
    }

    #[test]
    fn welding_drops_a_collapsed_triangle_and_its_corner_colours() {
        let mut m = Mesh::default();
        m.positions = vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::X, Vec3::X * (1.0 + 1e-9), Vec3::Y];
        m.triangles = vec![[0, 1, 2], [3, 4, 5]];
        m.tri_material = vec![0, 0];
        m.corner_colors = Some(vec![[1.0, 0.0, 0.0]; 3].into_iter().chain(vec![[0.0, 1.0, 0.0]; 3]).collect());
        m.weld();
        assert_eq!(m.triangles, vec![[0, 1, 2]]);
        assert_eq!(m.corner_colors.unwrap(), vec![[1.0, 0.0, 0.0]; 3]);
    }

    #[test]
    fn a_fitted_mesh_lies_in_the_unit_sphere() {
        let mut m = soup_cube();
        for p in &mut m.positions {
            *p = *p * 25.0 + Vec3::new(-12.5, -12.5, 0.0);
        }
        let (centre, radius) = m.fit_to_unit();
        assert_eq!(centre, Vec3::new(0.0, 0.0, 12.5));
        assert!((radius - 25.0 * 3f32.sqrt() / 2.0).abs() < 1e-3, "a cube's corners are its farthest points");
        assert!(m.positions.iter().all(|p| p.length() <= 1.0 + 1e-5));
    }

    #[test]
    fn z_up_becomes_y_up_without_turning_faces_inside_out() {
        let mut m = soup_cube();
        m.z_up_to_y_up();
        let mut w = m.clone();
        w.weld();
        let (lo, hi) = w.bounds().unwrap();
        assert_eq!((lo, hi), (Vec3::new(0.0, 0.0, -1.0), Vec3::new(1.0, 1.0, 0.0)));
        let centre = (lo + hi) / 2.0;
        for t in &w.triangles {
            let [a, b, c] = t.map(|i| w.positions[i as usize]);
            assert!((b - a).cross(c - a).dot((a + b + c) / 3.0 - centre) > 0.0);
        }
    }

    #[test]
    fn appending_keeps_colours_on_both_sides() {
        let mut a = soup_cube();
        let mut b = soup_cube();
        b.materials = vec![Material::colour([0.0, 0.0, 1.0])];
        b.corner_colors = Some(vec![[0.0, 1.0, 0.0]; 36]);
        a.append(&b);
        assert_eq!(a.triangles.len(), 24);
        assert_eq!(a.triangles[12], [36, 37, 38]);
        assert_eq!(a.corner_color(0, 0), CLAY, "the first mesh's material colour, as a corner colour");
        assert_eq!(a.corner_color(12, 0), [0.0, 1.0, 0.0]);
    }

    #[test]
    fn welding_keeps_each_kept_triangles_uvs_and_normals() {
        let mut m = Mesh::default();
        m.positions = vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::X, Vec3::X * (1.0 + 1e-9), Vec3::Y, Vec3::Z, Vec3::X, Vec3::Y];
        m.triangles = vec![[0, 1, 2], [3, 4, 5], [6, 7, 8]];
        m.tri_material = vec![0; 3];
        m.corner_uvs = Some((0..9).map(|i| [i as f32, 0.0]).collect());
        m.file_normals = Some((0..9).map(|i| Vec3::splat(i as f32)).collect());
        m.weld();
        assert_eq!(m.triangles.len(), 2, "the middle one collapsed");
        assert_eq!(m.corner_uvs.unwrap()[3], [6.0, 0.0], "the third triangle's uvs moved up");
        assert_eq!(m.file_normals.unwrap()[3], Vec3::splat(6.0));
    }

    #[test]
    fn appending_keeps_uvs_and_drops_normals_one_side_lacks() {
        let mut a = soup_cube();
        a.corner_uvs = Some(vec![[1.0, 1.0]; 36]);
        a.file_normals = Some(vec![Vec3::Y; 36]);
        a.append(&soup_cube());
        assert_eq!(a.corner_uvs.as_ref().unwrap()[36], [0.0, 0.0]);
        assert_eq!(a.corner_uvs.unwrap().len(), 72);
        assert!(a.file_normals.is_none());
    }

    #[test]
    fn srgb_round_numbers() {
        assert_eq!(srgb_to_linear(0), 0.0);
        assert!((srgb_to_linear(255) - 1.0).abs() < 1e-6);
        assert!((srgb_to_linear(188) - 0.5).abs() < 0.01);
    }
}
