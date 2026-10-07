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
- Colours are linear RGB. A palette colour per triangle (`tri_color` →
  `colors`: MTL `Kd`, glTF base-colour factor) and, when the file colours
  its points, `corner_colors` (three per triangle) which win. They ride on
  corners so welding never has to pick between two colours at one point.
  8-bit PLY colours are sRGB and decoded; float colours are taken as linear.
- Errors are `String`s meant for a person: they name the line, the element
  or the buffer that was wrong.

## Not yet

glTF base-colour textures (milestone 5: the viewer has no textured
pipeline), PLY point clouds without faces (an error that says so), 3MF.

## Tests

`cargo test -p cce-mesh-io`. Unit tests sit beside each reader; glTF and
GLB fixtures are built in the test (base64 buffer, hand-assembled GLB
chunks). `tests/designer_round_trip.rs` reads the designer's default project
as its own exporter wrote it (`tests/fixtures/designer-default.{stl,obj}`,
made with `cce-designer --export default_project.json …`) and requires the
STL and the OBJ to come back as the same 362 points and 720 triangles with
the same winding. Regenerate the fixtures the same way if the exporter's
format changes, and keep the counts in the test's doc in step.
