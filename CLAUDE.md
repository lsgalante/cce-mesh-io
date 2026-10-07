# cce-mesh-io

Mesh files in, one scene type out: STL, OBJ (+ MTL colours), glTF/GLB and
PLY. A library with no renderer and no cce-ui dependency, shared by
cce-model (shows files) and, from milestone 6 of the viewer's design doc,
cce-designer (imports them). Read the workspace guide
(`../cce-compositor/WORKSPACE.md`) first; this covers only what is
particular to this crate.

## The contract

- A reader returns a `Scene` **as the file has it**: parts in the file's own
  coordinates and units, and `up` saying which way its format calls up
  (`UpAxis::Z` for STL, `Y` for the rest). Readers never rotate, scale or
  weld. `load()` welds each part, drops empty parts and names unnamed ones
  after the file stem.
- **Callers decide orientation.** cce-model turns a Z-up scene upright
  (`Mesh::z_up_to_y_up`); an importer of the designer's own exports must
  not, because cce-designer writes its Y-up world into STL as is (its STL
  export is not Z-up — a slicer will lay those models on their side).
- `unit` is what the format conventionally means (STL mm, glTF metres, OBJ
  and PLY unspecified); a caller showing a size says which.
- **Materials, not a palette** (since milestone 5): each triangle names a
  `Material` (`tri_material` → `materials`): linear colour, an optional
  texture (an index into `Scene::textures`), metallic and roughness — glTF's
  model; an MTL's `Kd` / `map_Kd` / `Ns` (as roughness) / `Pm` / `Pr` map
  onto it. `Texture`s are decoded RGBA8, sRGB-encoded, top row first
  (`Texture::decode`, PNG or JPEG via the `image` crate; `sample` for the
  nearest texel, repeating).
- **Everything per corner rides on corners** (three per triangle, in
  triangle order), so welding never chooses between two values at a point:
  `corner_colors` (PLY, glTF `COLOR_0`, already multiplied by the material
  colour; `Mesh::corner_color` prefers them), `corner_uvs` (image convention,
  (0, 0) top-left; an OBJ `vt` is flipped in v; they may exceed 0..1 — textures
  repeat), and `file_normals` (glTF `NORMAL` through the node's
  inverse-transpose, OBJ `vn` only when every corner has one). `weld`,
  `append` and `z_up_to_y_up` keep all three in step; `append` drops file
  normals one side lacks.
- Errors are `String`s meant for a person: they name the line, the element
  or the buffer that was wrong.

## Not yet

Metallic-roughness, normal and occlusion TEXTURES (the factors are read),
KHR_texture_transform and other extensions, PLY texture coordinates, PLY
point clouds without faces (an error that says so), 3MF.

## Tests

`cargo test -p cce-mesh-io`. Unit tests sit beside each reader; glTF and
GLB fixtures are built in the test (base64 buffer, hand-assembled GLB
chunks). `tests/designer_round_trip.rs` reads the designer's default project
as its own exporter wrote it (`tests/fixtures/designer-default.{stl,obj}`,
made with `cce-designer --export default_project.json …`) and requires the
STL and the OBJ to come back as the same 362 points and 720 triangles with
the same winding. Regenerate the fixtures the same way if the exporter's
format changes, and keep the counts in the test's doc in step.
