# Projects studied for inspiration (clean room)

ModelCraft V4 took inspiration from the **behaviour** and **ideas** of two open-source projects
pointed out by the user. Rule followed: read them to understand what they do and how they are
organised, then rewrite everything in Rust ourselves. **Not a single line of their code was
copied** into ModelCraft (nor from rvt-rs, which BIMCraft partly derives from). Nothing from
eDrawings, SolidWorks or Autodesk either.

## tangency-labs/solidcraft

- **What it is**: parametric CAD in Rust / egui (sketches, constraints, features). It does not
  read SLDPRT/SLDASM or the SolidWorks display streams; it imports STEP/IGES/3MF/STL/OBJ/glTF.
- **Ideas kept (re-implemented our own way)**:
  - navigation presets (Blender / CAD style) and shortcuts documented inside the application;
  - middle double-click = zoom to fit; keyboard view rotations in 15° steps;
  - Ctrl+Z / Ctrl+Y on the view state;
  - animated exploded view;
  - a real Windows installer with a Start menu entry.
- **Not taken**: its parametric model (out of scope for a viewer), its MSI installer
  (we use Inno Setup, generated from our own extension list).

## IPedrax/BIMCraft (all branches)

- `main` is the most complete branch. The 25 other branches hold minor or older versions of
  `crates/rvt`.
- **RVT reader** (Apache-2.0, partly ported from rvt-rs): **semantic** reconstruction of the
  elements (levels, walls, floors/ceilings, roofs, doors/windows, columns, rooms), no native Revit
  mesh. Families, stairs and groups are ignored.
- **Format facts noted** (facts, not code): from Revit 2024 on, an element record carries a
  bounding box of 6 `f64` (feet) at +0x58 and a `BuiltInCategory` (`i64`) at +0x12.
- **V5 lead (not done there)**: an "element boxes" fallback for RVT files, written by us from
  these facts, keeping streaming reads and bounded memory.

## What came out of it in V4

| Idea | ModelCraft implementation |
|---|---|
| Navigation presets | `NavStyle` Blender (default) / SolidWorks / CAD, "Blender shortcuts" setting |
| 15° keyboard steps, opposite view | `view.orbit_*` (numpad 4/6/8/2), `view.flip` (numpad 9) |
| Middle double-click = zoom to fit | `viewer.rs`, mouse handling |
| Undo / redo | `ViewState` stack in `document.rs` (positions, explode, opacities, hiding) |
| Animated explode | `assembly.rs`: one fixed radial direction per part, never reversed |
| Installer | `installer/gen_iss.py` → `installer/modelcraft.iss` (Inno Setup) |
