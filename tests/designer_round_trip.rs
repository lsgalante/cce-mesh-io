//! What cce-designer exports, this crate reads back as the same mesh.
//!
//! The fixtures are the designer's default project written by its own
//! exporter (`cce-designer --export default_project.json out.stl|out.obj`,
//! 2026-10-07): 362 points and 384 primitives, 48 triangles and 336 quads, so
//! 720 triangles. STL keeps only triangles and no shared points; OBJ keeps
//! both. Read and welded, the two must agree on every point and triangle.

use std::path::Path;

use cce_mesh_io::{load, Mesh, UpAxis};
use glam::Vec3;

fn fixture(name: &str) -> cce_mesh_io::Scene {
    load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
}

/// Every triangle as its three corners, starting from the smallest so the
/// same triangle written from another corner compares equal; sorted.
fn triangle_set(m: &Mesh) -> Vec<[[i64; 3]; 3]> {
    // Exact-enough keys: the exporter writes shortest round-trip floats.
    let key = |p: Vec3| [(p.x * 1e5).round() as i64, (p.y * 1e5).round() as i64, (p.z * 1e5).round() as i64];
    let mut out: Vec<[[i64; 3]; 3]> = m
        .triangles
        .iter()
        .map(|t| {
            let k = t.map(|i| key(m.positions[i as usize]));
            let first = (0..3).min_by_key(|&i| k[i]).unwrap();
            [k[first], k[(first + 1) % 3], k[(first + 2) % 3]]
        })
        .collect();
    out.sort();
    out
}

#[test]
fn the_stl_export_reads_back_as_the_designer_wrote_it() {
    let s = fixture("designer-default.stl");
    assert_eq!(s.up, UpAxis::Z, "STL's convention, reported, not applied");
    assert_eq!(s.parts.len(), 1);
    assert_eq!(s.parts[0].name, "designer-default");
    let m = &s.parts[0].mesh;
    assert_eq!(m.triangles.len(), 720);
    assert_eq!(m.positions.len(), 362, "welding recovers the designer's shared points");
}

#[test]
fn the_obj_export_reads_back_as_the_designer_wrote_it() {
    let s = fixture("designer-default.obj");
    assert_eq!(s.up, UpAxis::Y);
    assert_eq!(s.parts[0].name, "default", "the exporter's `o` line");
    let m = &s.parts[0].mesh;
    assert_eq!(m.triangles.len(), 720, "48 triangles and 336 quads");
    assert_eq!(m.positions.len(), 362);
}

#[test]
fn both_exports_are_the_same_mesh() {
    let stl = fixture("designer-default.stl").merged();
    let obj = fixture("designer-default.obj").merged();
    assert_eq!(stl.bounds(), obj.bounds());
    assert_eq!(triangle_set(&stl), triangle_set(&obj), "same triangles, same winding");
}
