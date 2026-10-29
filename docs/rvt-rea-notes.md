# Revit (.rvt/.rfa) - reverse-engineering notes (clean room)

Started 2026-10-28. Goal: read the 3D of `.rvt` files in ModelCraft without Revit at run time.
Rules followed: no action on the Autodesk license / login / activation, Revit never launched by
us, nothing modified in the installation. Only **byte observations** on sample files shipped by
Autodesk with the installation or published by Autodesk, and the **exported names** (PE export
table) of a DLL. No disassembly or decompilation was used for these notes.
No Autodesk file and no customer model (NDA) is versioned here.

## Installation observed (read only)

- Revit 2025 build 25.0.2.419, installed in `D:\Programmes\AutoDesk\Launcher\Revit 2025\`
  (~25,000 files, 4.7 GB). Content (libraries, templates) in `C:\ProgramData\Autodesk\RVT 2025\`
  and family templates in `D:\Programmes\AutoDesk\Revit2042\Family Templates\`.
- DLLs related to files / geometry (size): `RevitDB.dll` (44 MB, object database),
  `PersistenceDB.dll` (7.6 MB, file storage), `EssentialsDB.dll`, `FamilyDB.dll`,
  `HostObjDB.dll`, `GeomUtil.dll`, `Graphics.dll`, `OGS*.dll` (display), `ASM*230A.dll`
  (solid modelling kernel), `Revit.IFC.Export.dll` / `IntfIFC.dll` (IFC export).
- Samples available: `Samples\*.rvt` (Snowdon Towers ×7, Golden Nugget ×2, 3 others, 21-134 MB),
  `RVT 2025\Libraries\**` (~8,300 `.rfa`, a few library `.rvt`), `DynamoSample_2025.rvt`
  (6.6 MB), `BatchPrintInitial.rvt` (156 KB).

## Container (confirmed on 3 files)

Files read: `BatchPrintInitial.rvt` (old format), `M_Chair-Desk.rfa` (2025), `DynamoSample_2025.rvt` (2025).

- Compound File (MS-CFB). Streams: `BasicFileInfo`, `Contents`, `Formats/Latest`,
  `Global/{ContentDocuments, DocumentIncrementTable, ElemTable, History, Latest, PartitionTable}`,
  `Partitions/NN`, `RevitPreview4.0`, `TransmissionData`, plus `PartAtom` (family) or
  `ProjectInformation` (project). A single `Partitions/NN` stream per file here (NN = 1, 70, 283:
  the number says nothing about the version).
- `BasicFileInfo`: UTF-16LE `Key: value`, 19 keys in 2025 (`Format: 2025`, `Build: 20240111_1515(x64)`,
  `Locale when saved`, document GUID…). The old file only has 5 keys, with `Revit Build`.
  `Last Save Path` holds a user path: not shown by default (privacy).
- Storage paging, found independently: after removing from each stream the bytes at offset
  `o` such that `o mod 65,249 >= 64,896`, **all** gzip members pass their CRC-32 / length check
  (a single failure out of 349 in the Dynamo partition, to look at: probably the last member).
- `Global/*`: 8 bytes then one gzip member. `Formats/Latest`: gzip at byte 0.
  2025 partition: 44-byte header then gzip members of ~128 KiB decompressed
  (Dynamo: 6.1 MB stored → 27.8 MB decompressed).

## Where the geometry is

- No ACIS/SAT/SAB stream in the file (no signature found in the chair's decompressed
  partition), and no obvious cached display mesh.
- The `Formats/Latest` schema (≈4,600 classes in 2025) names Revit's own geometry classes:
  `GBRep`, `BRep`, `ExternallyTaggedBRep`, `TaggedBReps`, `GFakeBRep`, `GPolyMesh`, `PolyMeshes`,
  `PartialPolyMeshes`, `TessLoop`, `Triangle`, `FaceTag*`, `EmbeddedSymbolGRep`.
  ⇒ The real 3D of families is a B-rep serialised by Revit (`G*Rep` classes) in the partitions.
  `GPolyMesh` meshes exist (toposolids, DWG/mesh imports): the simplest lead for a first
  triangulated render.
- `PersistenceDB.dll` (6,877 exports, names only): classes `RvtFileStorage`, `PartitionTable`,
  `PartitionInfo`, `StreamOpenOptions`, `CSimpleStorage`, `ElementStreamBuilder`, `SegmentIndex`,
  `SegmentAddresses`. No compression export: compression lives elsewhere (probably a static
  library). Imports `RevitDB.dll`, `Utility.dll`, `ole32.dll` (the CFB storage).

## ModelCraft status (V4)

`crates/craft-formats/src/rvt.rs` (V4) read `BasicFileInfo`, the PNG thumbnail of `RevitPreview4.0`
and the Atom of `PartAtom`/`ProjectInformation`. No geometry: `Support::Partial`.

## Inspiration: BIMCraft (26 branches, `main` = the most complete)

`crates/rvt` (Apache-2.0, partly ported from rvt-rs, github.com/DrunkOnJava/rvt-rs): reads the
schema, `ElemTable`, the element records of the partitions (`BuiltInCategory` category
at +0x12, 6×f64 bounding box in feet at +0x58 since 2024), then **rebuilds** walls, floors,
roofs, doors/windows, columns, rooms from location lines, sketches and layer types. It does not
read the `G*Rep` B-reps: parametric geometry is rebuilt, not the real mesh. Its
`docs/rvt-provenance.md` lists accepted and forbidden sources (disassembling Autodesk binaries
beyond export names is forbidden) - the same fence is adopted here.
Nothing is copied into ModelCraft.

## Next steps proposed at the time

1. **Robust container reader** in `rvt.rs`: de-paging (65,249 / 64,896), bounded gzip members,
   streaming reads of `Partitions/*` (bounded memory), tests on the 3 files + Snowdon.
2. **Schema**: parse `Formats/Latest` (classes, tags, fields) to get the tags of
   `GBRep`, `GPolyMesh`, `Outline`, `ElementParents` from the file itself.
3. **"Boxes" fallback** (planned for V5): one box per placed element, coloured by category -
   first real 3D display, low risk.
4. **GPolyMesh**: locate its instances in the partitions (simple families, DirectShape,
   toposolids) and decode vertices/faces.
5. **GBRep**: study it on tiny `.rfa` files (cube, cylinder) saved by the user in
   Revit 2025 (if they agree to save some: no license touched by us): planar faces first,
   then analytic surfaces; tessellation on the ModelCraft side.
6. Compare with an IFC export made by the user from Revit to validate positions/units.

## V5: bounding boxes (implemented, `crates/craft-formats/src/rvt_model.rs`)

Observations made here on the three samples (no code taken from BIMCraft or rvt-rs):

- The record chain at the start of the partition: `u64` id, `u32` size, `u16` constant,
  `u16` count, body, 16 bytes whose last `u32` repeats the size; next record at `o + size + 16`.
  Dynamo: 6,730 records = exactly the 6,730 ids of `Global/ElemTable` (`u64` id at +20 of each
  40-byte entry); the chain stops at 1.2 MB out of 38 MB (the rest: documents of the loaded
  families), so decompression stops there.
- Constant = 0x05c7 on all records of both 2025 files; + 12 = 0x05d3, the value read after the box.
- `Formats/Latest` defines `Outline` inline: `59 81 00 00 07 00 "Outline"` → tag 0x159
  (bit 0x8000 = inline definition). Old file (BatchPrintInitial): 0xb1.
- Marker `[0x159] ff ff ff ff [0x5d3]` at +0x50 of a frame, `i64` category at +0x12, `u32` kind
  at +0x42 (`0xffffef7f` = placed instance), 6×`f64` box in feet at +0x58: **hypothesis
  confirmed** on 2025. Only 4 of the 38 wall frames are top-level records: the others are nested
  in a record body, hence the marker search through the whole chain. In a family (`.rfa`), the
  family solids have category -1.
- Results: DynamoSample_2025.rvt → 235 boxes (38 walls 0.2 m × 2.7 m high, 6 floors, 2 ceilings,
  5 doors, 7 windows, 118 mullions, 36 panels, 3 stairs, 7 railings, 3 structural columns,
  3 foundations, 3 topography, 1 mass, 1 generic, 2 entourage); M_Chair-Desk.rfa → 4 boxes
  (seat, back, leg, base); BatchPrintInitial.rvt (old format) → 0, thumbnail fallback.
- Filters: finite, ordered box, < 3e5 feet, extended in at least two directions,
  physical categories only (lines, sketches, levels, annotations ignored), duplicates removed.

## User files (under NDA, never versioned)

Read only on the PC, `BasicFileInfo` only (no file name or content noted):
15 `.rvt`, 0.4 to 330 MB. 14 are Revit 2018 (13 workshared "Central", 1 not shared),
1 is a Revit 2025 "Central". The 2018 format does not have the 2024+ element frame: it stayed on
the thumbnail/information fallback; the next lead was the "up to 2023" frame (`u32` ids).

## V5.1: files older than 2024 (32-bit ids)

Checked on the sample projects published by Autodesk (revit.downloads.autodesk.com, not versioned):
`rst_basic_sample_project` 2018, `rstbasicsampleproject` 2023 and 2025, `rac_basic_sample_project`
2018 and `racbasicsampleproject` 2021.

- Revit 2024 moved to 64-bit `ElementId`s. Before: record header `u32` id, `u32`
  size, `u16` constant, `u16` count (12 bytes), 12-byte trailer whose last `u32` repeats the
  size; next record at `o + size + 12` (instead of `+ 16` in 2024+).
- The `[Outline] ff ff ff ff [tag]` marker remains, followed by the six `f64`; `Outline` tag = 0x109
  (2018), 0x13c (2023), 0x159 (2025), read from `Formats/Latest`. The following tag is no longer
  "constant + 12" in 2018 (0x4f4 for a 0x4eb constant): the most frequent one after the marker is used.
- Placement kind (`u32`) always 14 bytes before the marker; `i32` category 0x26 bytes before
  (0x3e and `i64` in 2024+: the six intermediate ids are half as long).
- Same model, same results from one version to the next: rst 786 boxes in 2018, 2023 and 2025;
  rac 458 in 2018 and 2021.
- Since 2019, the `Key: value` block of `BasicFileInfo` is at an odd offset: the alignment that
  contains the keys is used.
- Workshared files are read like the others (one partition per workset), read only: no lock, no
  temporary file, no synchronisation with the central.

## V6: project browser (implemented, `crates/craft-formats/src/rvt_browser.rs`)

Goal: when a .rvt is opened, the left panel shows the "Project Browser" like Revit (Views by
folders, Legends, Schedules/Quantities, Sheets), read only. Observations on the public samples only
(2018, 2021, 2023, 2025), no disassembly.

**Schema (`/Formats/Latest`).** Each class: `[u16 len][name][u16 parent]` (if the parent has the
0x8000 bit: `00 00` then its inline definition), `[u32][u32 field count]`, the fields
`[u32 len][name][type]`, then `u32 ng` and ng 16-byte GUIDs. Type: `b0 b1 00 00`, then
`u32 n` if b1 = 0x10, then `u16 class` if b0 = 0x0e and b1 ∈ {0x00, 0x50, 0x10}.
`0e 00 c` object by value (c = ElementId: a 4- or 8-byte id), `0e 01/02` owned pointer,
`0e 03` raw pointer (u32), `0e 50 c` set (u32 n then n items), `0e 51` pointer array,
`08 60` UTF-16 string (u32 length), `07` double, `04` int32, `01` boolean, `0b` 8 bytes
(2025, `TreeItemData`).

**Elements in the partitions.** `[id][u32][u32 size][u16 class]`, the body follows the class
directly; fields of the whole class chain in schema order (Element first: `m_id`, `m_famId`,
`m_ownerDBViewId`…). Non-null pointer: `ff ff ff ff [u16 class]` and its content is written
after the body, in encounter order (queue); `00000000` null; small value = back reference. An
element is valid when the decoded `m_id` = the header id. Views are past the end of the record
chain: the whole partitions are read.

**What is read.** `DBView*` (name, type, template, primary view for dependent views, viewports
on sheets, sheet number), their deferred parameter sets (`ParamValueSetInt/ElementId/AString/Double`:
discipline -1005163, phase -1012102…), `DBViewType` (`m_systemFamilyIdx`: 0x66 3D, 0x68 rendering,
0x69 schedule, 0x6b sheet, 0x6c drafting, 0x6d floor plan, 0x6e area, 0x6f ceiling, 0x70 section,
0x71 detail, 0x72 elevation, 0x75 legend, 0x76 table, 0x77 graphical column schedule,
0x78 structural; type name in `SymbolInfo`), `ProjectPhase`, `BrowserOrganization` (filters,
folder definitions `{parameter, character count}`, sorting; `m_type` 0 views, 1 sheets,
3 schedules) and, in `/Global/Latest`, `BrowserOrganizationTracking` (current organisation of
views, sheets and schedules; its `m_folderNameToIdMap` table holds Revit's folder names, which
were used to check the naming rule).

**Tree.** Excluded: templates, family views (`m_famId` ≠ -1), views owned by a sheet (revision
schedules), `DBViewProject`, system browser. "Family and type" folder: the plural of the
family, plus "(type)" when the type name is not the default name ("Sections (Wall Section)",
"Floor Plans"). Elevations older than 2019 without a type take the first type of the elevation
family. Empty value: "???" folder. Alphanumeric sort ignoring accents; dependent views under
their primary view; sheets as "NUM - Name". Filters: equal / not equal only ("not on sheets"
supported).

**Still unknown.** No legend or project parameter in the samples (values expected in
`ParamValueSetAString` with positive ids); labels of ElementId values other than phases;
filter relations other than equal/not equal.

### V6.1: families, groups, Revit links, simplified mode

- Loadable families: `FamilySurrogate` (`m_name`, `m_categoryId` built-in category,
  `m_elemId` = `Family` element) and `FamSymSurrogate` (`m_name` of the type, `m_famSurrogateId`).
  In-place families (`Family.m_inPlace`) are excluded; one family can have several surrogates
  (merged). Category names are not stored (localised by the UI): a table of the common built-in
  categories, annotations grouped under "Annotation Symbols", the rest under "Other families".
  System families (walls, pipes…) are not listed yet.
- Groups: `ElementGroupType` (name in `SymbolInfo`), detail group if `m_viewType != 0` or attached
  (`m_attachedParentId`, filed under its model group). Hypothesis to confirm on a model with
  detail groups.
- Revit links: `RvtLinkSymbol` (name `SymbolInfo.m_name`, nested links excluded via
  `m_parentSymbolId`), `RvtLinkInstance` (`m_name` = instance number, type via
  `InstanceInfo.m_symbolId`). State at the last save: `TransmissionData` stream
  (u32 + UTF-16 XML, `ExternalFileReference` of type "Revit Link", `LastSavedLoadState`
  Loaded / Unloaded / NotFound…). Unloaded link = red icon.
- Decoder: types `0x09` (GUID, 16 bytes), `0x0d` (XYZ, 24 bytes) and fixed object arrays `0e 10`.
- UI: a button next to the tree switches between the full browser (like Revit) and the
  simplified view (views, legends, schedules, sheets); the `browser_full` preference is saved in
  settings.json.
- Categories: ids checked against the public `BuiltInCategory` list of the Revit API (values
  stable across versions). All tag, head, symbol, callout, view title and boundary condition
  categories are filed under "Annotation Symbols". On the local file set, no unknown category is
  left among those found.

### V6.2: starting view and opening views

- Starting view (Revit "Manage > Starting View", API `StartingViewSettings`): class
  `InitialViewSettings : Element`, a single field `m_initialViewId` (ElementId of the view or
  sheet). "StartingView" hypothesis rejected: no class of that name in `/Formats/Latest`
  (3,981 classes, Revit 2018 to 2025). The element is in a partition, read by the same scan as the
  views. Missing or -1 when the project has no starting view (Revit then reopens the last active
  view, not read).
- Checked on 4 models of the user's file set (names not noted, NDA): each one opens on a sheet,
  at the same place in the browser as in Revit (user screenshot), under a sheet organisation
  with several folder levels.
- `BrowserNode` now carries `id` (ElementId of the view, -1 otherwise) and `start`.
- ModelCraft opens the file on that view: row selected, parent folders expanded, list scrolled
  to it. Clicking a view or sheet in the browser opens it: plans (and ceiling plans) seen from
  above, sections / elevations / details from the front, orthographic; 3D views isometric;
  sheets, drafting views, legends and schedules shown flat (white sheet with the
  `RevitPreview4.0` preview when there is one, which is often the active sheet at the last
  save). A "View: …" chip in the 3D view, with a cross to go back to the whole model.
- Not done at the time: 2D sheet content (title block lines, viewports, texts), framing on the
  view area (crop region, view range), per-view element filtering, real orientation of
  sections / elevations and saved 3D views (all done later, see below).

## V7: rebuilt shapes (implemented, `rvt_geom.rs` + `rvt_shape.rs`)

Elements are read object by object (generic decoder driven by `/Formats/Latest`), then rebuilt
parametrically (most elements store no B-rep: `SnapshotData` / `GElement` are empty).

Encoding completed (checked by declared size = consumed size):
- type `b0 b1 00 00`: if `b1 & 0xf0 == 0x10`, a `u32 n` follows (fixed array: `10` values,
  `13` n raw 4-byte pointers, `11`/`12` n owned pointers); `50` set / vector
  (`51` owned pointers, `53` raw pointers: `u32` count + n × 4); `0d` sub-descriptor
  `[u32 1][0x20][item type]`. Example: the edges (`GEdgeBase`, `0e 13` + n) were missing from the schema.
- owned pointer: `u32 v`; 0 = null (4 bytes); otherwise `u16` class (6 bytes); `0xffffffff` =
  new deferred object (FIFO queue); small `v` = local id (first mention: body queued,
  afterwards: reference); `v = 1` document, `v = 2` the element itself. Raw pointer (`0e 03`): 4 bytes.

Geometry read (feet, project frame):
- **Wall** (`SWall`): `VWallDriver.m_pCrv` (`GLine`: `m_origin`, `m_dirVec`, `m_endParams`);
  edited profile `VWallExtrusionHelper.m_pCurveLoops` (vertical plane of the base line); reference
  faces `m_pRefFaces` → `Plane` (`m_origin`, `m_xVec`, `m_yVec`, envelope `[umin,vmin,umax,vmax]`)
  which give the thickness, bottom, top and ends; `WallCutoutGStep` / `WallEmbedWallGStep.m_instId`
  = inserts that cut (opening = projected insert box). Joins are not stored with the wall: along
  an axis, the element box gives them.
- **Floor / ceiling / slab**: `m_bottom` / `m_top` (floors), horizontal outlines
  `*ExtrusionHelper.m_pCurveLoops`; otherwise horizontal reference planes.
- **Pipe / duct / cable tray** (`RbsSingleCurve` and subclasses): `m_pCurveDriver` →
  `RbsCurveDriver.m_pCrv` (axis), `m_dWidthOrDiameter`, `m_dHeight`, `m_vNormal` (height direction),
  `*DesignPropertyManager.m_sOverallSize`.
- **Family extrusion** (`ExtrusionElem`): `m_famId` (family document), `m_cutting` (void),
  `m_paramSet` (`ParamValueSetDouble`): `-1001800` start, `-1001801` end;
  `ExtrusionElemExtrusionHelper.m_pCurveLoops`; sketch plane `VarSketch.m_pPlane`.
- **Instance** (`FamilyInstance`): `m_pInstanceInfo` → `InstanceInfo.m_Trf` (`m_3x3`, columns =
  images of X, Y, Z; `m_or`) and `m_symbolId`; `FamilySymbol.m_famId` = embedded family document
  (otherwise `m_familyId` / `m_partitionSurrogateId`: loaded family whose geometry is not in the
  project, only the box remains). Check: each placed shape must stay inside the element box
  (otherwise box).

Validation (public samples + an NDA file, locally): out of 5,660 rebuilt shapes of the NDA MEP
file, 4 are more than 5 cm off the Revit box; `M_Chair-Desk.rfa`: 4/4.

### Solids of loaded families (revolutions, sweeps, blends, nested families)

- Family documents stored in the project: `/Global/ContentDocuments` (segments: ContentMarker
  tag, ContentKey GUID, then the root id after the NOBLE_SecondaryDataStorage tag). Elements of
  the document have `m_famId` = root. `Family.m_oFamDoc`: `m_contentDocGUID`,
  `m_big2SmallMap2` (project ids → document ids: the owner of a "first" id gives the root).
- `FamilySymbol`: (`m_famId`, `m_familyId`); nested symbols: `m_famId` = root of the document
  that holds them (do not use it as the family: self reference).
- `RevolutionElem`: sketch `m_sketchId`, angles -1001802 / -1001803. `SweepElem`:
  `m_profileData.m_profileSketchId` + `m_pathSketchId`, or `m_path3dId` → `Path3d`
  (`m_curveLoops` → `CurveLoop.m_curves`, `m_userId` = the sweep): this is the case of MEP elbows.
  `BlendElem`: two sketches whose `m_userId` = the blend, heights -1001804 / -1001805.
- Ignored shapes: integer parameter -1006205 ("Visible") = 0, or an
  `AddGeomToHostInFamEnvGStep` (8 ft × 8 ft sample host of hosted templates). The 9 / 11 flags of
  `GeomStepList` do not single out the host.
- Placement: per solid; the solids whose placed box falls in the instance's Revit box are kept if
  they cover it (≥ 60 % of each side ≥ 10 cm), otherwise uniform scaling about the insertion
  point (median ratio 0.1 to 10, sides within 8 %), otherwise per-axis stretching (axis-aligned
  placements, 0.33 to 3), otherwise, when all solids lie inside a larger box (a light fixture
  whose box also holds its light distribution), the solids as placed. The type can have other
  sizes than the saved family: this is an approximation.
- Validation (NDA MEP file): 9,721 shapes (5,660 before), 513 more than 5 cm off the Revit box,
  9 more than 30 cm. Pipe fittings 3,177 / 6,140, duct fittings 132 / 1,191, pipe accessories
  398 / 586; sprinklers and flex ducts: none.

## Sheets (2D content) and view framing

Read by `crates/craft-formats/src/rvt_sheet.rs`, in the same pass as the browser and the shapes.

- Elements owned by a sheet: `m_ownerDBViewId` = id of the `DBViewSheet`.
- Title block and 2D symbols: `FamilyInstance` (`m_pInstanceInfo.m_Trf`: `m_3x3`, `m_or`, feet),
  `FamilySymbol` symbol: curves `m_nonBRepGeomPtrs` (all at z = 0, flat `m_outline.m_minmax`),
  texts `m_textNoteData`, labels `m_tagNoteData` (value = a parameter of the sheet, of the project
  `ProjectInfo` or built in: -1007400 name, -1007401 number, -1007402 scale), tables
  `AllScheduleInstanceRegenData` (revision table).
- Texts: `TextNote` (RTF rich text reduced to plain text, `\'hh` Windows-1252, `\uN`);
  `m_height` = box: lines × size × ~1.57.
- Placed views: `Viewport` (`m_offset` centre on the sheet, `m_scale`, `m_dbViewId`,
  `m_labelOrigin` / `m_labelLength` title line, `m_detailNumber`). Crop region:
  `Viewer.m_boundedSpace` (`m_orig`, `m_basis` 3×3 = u, v, depth axes, `m_boundOffset` 6:
  u max, u min, v max, v min, far, near; `m_boundActive`). Sections, elevations
  (`Section`, `InteriorElevArrow`…) are subclasses of `Viewer`: test `isa("Viewer")`.
  The 3D view has the `DBView3d` class. Plan view range (`DBView`, feet): absolute values
  `m_cutPlaneElev`, `m_topClipElev`, `m_bottomClipElev`, `m_depthClipElev` when they are filled,
  otherwise `PlanViewRange2.m_offsets` (cut, top, bottom, depth) relative to
  `m_levelPos` (0 = level of the view `m_genElemId`, 1 = level above, -1 = unlimited; -1 wins
  over a value stored as 0, seen on a site plan). The old `PlanViewRange` can be all zeros. Plan
  looking up (ceiling plan, `m_viewDir.z < 0` or depth above the cut): from the cut to the depth;
  otherwise from min(depth, bottom) to the cut. Sections: the cut is always applied
  (`off[5]`), the far clip `off[4]` only when active. 3D section box: `ModelClipBox` (`m_isOn`,
  `m_viewer3dId` → `Viewer` of the 3D view, `m_boundedSpace`). `ViewFrame.clip`: oriented box
  (axes, bounds, ±1e7 for unlimited) applied by the shader (hiding + capping of cut faces) and by
  selection, and used to clip the edges of the views placed on sheets (3D Liang-Barsky).
  Rendering images: `ImageInstance` → `ImageSymbol` (`m_widthInFeet`, `m_heightInFeet`)
  → `ImageHolder.m_pRasterImage.m_compressedImage` (PNG / JPEG / BMP / GIF).
- View annotations (`rvt_annot.rs`, project elements with `m_famId = -1` whose
  `m_ownerDBViewId` is the view or the sheet):
  - detail lines: `CurveElem` (`m_detail = 1`), curve `m_pCurveDriver` →
    `CurveElemDriver.m_pCrv` (`GLine`, `GArc`… in feet, model coordinates); style
    `m_categoryId` → `GStyleElem.m_pGStyle` (`m_penNumber`, `m_color` red in the low byte,
    `m_linePatternId`: -3000010 solid, -1 "invisible lines", otherwise `LinePatternElem` →
    `LinePattern.m_segs`: `m_type` 0 dash, 1 space, 2 dot, `m_data` paper length in feet).
    User styles are subcategories of -2000051 (`CategoryElem.m_parentCategoryId`).
  - pens: `PenWidthTableElem` (id 2) → `PenWidthTable.m_draftPenInfo.m_pens` (16 paper widths
    in feet, annotations); `m_modelPenInfo` per scale (`m_invertedScale`) for the model.
  - filled regions: `FilledRegion` (`m_sketchId`, `m_attrId` → `FilledRegionAttributes.
    m_fillPatternElemId` → `FillPatternElem.m_pFillPattern`); outline = the `CurveElem`s whose
    `SketchMembership.m_groupId` = `m_sketchId` (`FilledRegionCurveData` data, often invisible
    lines). `FillPattern`: `m_name`, `m_scale`, `m_fpTarget` (0 drafting, paper sizes),
    `m_gridsArr` → `FillGrid` (`m_angle` rad, `m_origin`, `m_deltas` = [shift along, spacing],
    `m_segs` dashes / spaces); no grid = solid fill. Hatching clipped even-odd.
  - linear dimensions: `LinearDimString` (`m_refPnts` flat xyz list, dimension line = the
    `GLine` object of the tree `m_origin` / `m_dirVec`, `m_ArrSegInfo[].m_origin` text position of
    each segment, `m_values[0].m_value` value in feet); length recomputed by projecting the points
    on the line. Unit: `UnitsElem`, entry `autodesk.spec.aec:length-1.0.0` →
    `m_unitTypeId.m_typeId` (`autodesk.unit.unit:millimeters-1.0.1`…) and `m_accuracy`.
  - tags: `IndependentTag` (`m_symbolId`, `m_oTagOrientationCell` → `TagOrientationCell.
    m_headLocation` (model), `m_rotationAngle`; tagged element `TaggedElem.m_taggedId.
    m_linkInstOrHostId`), `RoomTag` (`m_point`, `m_symbolId`, `m_taggedRoomElemId.
    m_linkInstOrHostId`). The symbol is a 2D `FamilySymbol` (paper sizes) whose
    `m_tagNoteData` name the parameters: room -1006900 name, -1006901 number (strings of
    `RoomElem`), -1006902 area (computed, not stored).
  - drafting views: class `DBViewAnonDrafting`, default crop region (±100, inactive): no
    model, frame = extent of the annotations.
- View direction (`DBView`): `m_viewDir` towards the viewer (like `View.ViewDirection` in the
  API), `m_vertDir` screen up, `m_horzDir` right; checked on North (+y) / South (-y) elevations
  and plans (+z). `Scene.view_frames`: direction, up and crop box × depth (bounded to the model);
  the viewer derives the yaw / pitch of the Blender-style orbit from them (rotated plans: yaw
  taken from up).

### Hidden lines of placed views (`rvt_hlr.rs`)

No Revit data: computed by ModelCraft. View frame: u = `m_horzDir` (or the crop x axis),
v = up, w = `m_viewDir` (towards the viewer; the crop z axis of a section points the other way,
the far depth `off[4]` is positive along it). The faces of the rebuilt shapes (not the bounding
boxes: curtain walls, stairs, rooms would be solid) are clipped by what the view keeps (crop
region, view range, 3D section box), then drawn into a depth buffer (2,400 px on the long side).
An edge keeps the parts in front of the closest face (minimum over the 3 × 3 neighbouring
pixels, tolerance 0.02 ft + ¼ pixel). `RVT_NO_HLR=1`: all edges.

### Elements and categories hidden per view

In the view record (`DBView…`): `HiddenElementsViewSettings.m_hiddenElements`
(ids of the elements hidden in the view) and, among the `m_oaDrawFilters`,
`IckyExcludedCategoriesSetPtrWrapper.m_categoryIds` (categories unchecked in the view's
visibility, negative built-in category ids) with `m_analyticalModelsExcluded`,
`m_modelsExcluded`, `m_annotationsExcluded`. Public example rst_2023, 3D view "Central Pile"
without crop region or section box: only the foundations remain, the rest of the structure is in
hidden categories. Applied to the views placed on sheets (not yet to the interactive 3D view,
whose components group several elements).
`ExtentElem` (`m_extentElemId` of the view): `m_idExtents` empty on the samples.

## Radial, angular and spot dimensions

- `RadialDim`: `m_center` = arc centre, `m_refPnts[0]` = point on the arc,
  `m_ArrSegInfo[0].m_values[0].m_value` = radius (feet), `m_ArrSegInfo[0].m_origin` = text position.
  Drawing: line from the centre to the point (extended to the text), arrow on the arc, text "R <radius>".
- `AngularDim`: two points on the dimension arc in `m_refPnts`, centre and radius in the associated
  `GArc` object, `m_planeNormal` = dimension plane, value in radians. Accuracy read from `UnitsElem`
  (spec `autodesk.spec.aec:angle-1.0.0`). No public sample has one in a project view: not
  validated visually.
- `SpotElevation`: `m_refPnts` = dimensioned point, `m_bendOrPosPtOffset` = leader elbow,
  `m_horizLeaderLength` = shoulder, values = [elevation, east, north]. A `SpotCoordinateState`
  object marks a spot coordinate (text "N …" / "E …"). Checked on rst_2023 S101 (E 13062 / N 2799).

## Annotation crop

- Carried by the view's `Viewer` element (not by `DBView`): `m_bAnnotationCropActive` and
  `m_pAnnotationCropOffsets` → object with `m_leftRightTopBottomOffsets` (left, right, top, bottom),
  in **paper** feet beyond the crop region (default 0.0833 = 25.4 mm).
- As in Revit, an annotation entirely outside the frame is not drawn; an annotation that touches
  it stays whole (no clipping). Applied to cropped views only.
- On the public samples every annotation is already inside the frame: no visible change.

## Dimension styles and terminators

- A dimension (`LinearDimString`, `RadialDim`, `AngularDim`) points to its type through
  `m_styleSymbolId` → `DimensionStyle`. That gives the terminator `m_arrowHeadStyleId` →
  `LeaderStyle`, and the text size in double parameter -1006433 (paper feet; 0.0078 = 3/32",
  0.00787 = 2.4 mm). Other parameters seen (-1006465, -1006431, -1006407, -1006405, -1006404,
  -1006401): not identified with certainty (witness line gaps and extensions), not used.
- `LeaderStyle`: `m_tickType` 0 = oblique tick, 8 = arrow; `m_arrowFilled`, `m_arrowClosed`;
  parameter -1006426 = arrow opening angle (radians, 0.5236 = 30°), -1006414 = size
  (paper feet; "Diagonal 3mm" = 0.00984).
- Drawing: oblique tick at 45° of the given size; the arrow tip is on the witness line, its body
  towards the inside of the segment (two arrows back to back on intermediate witness lines),
  filled, closed or open depending on the type.

## Cut lines

- The cut plane of a view is the bound of the kept box on the viewer's side (plans: cut height
  of the view range; sections and elevations: their line). Rebuilt meshes that cross it are
  sectioned there (triangle/plane intersection) and the outline is drawn as a thick line
  (0.35 mm), never hidden.
- Categories that Revit does not cut (furniture, plumbing fixtures, lighting fixtures,
  equipment, planting, entourage, parking, site, topography) stay in projection.
- Limit: the line weight does not come from the project's object styles yet (built-in
  categories have no `GStyleElem` in the file; their styles remain to be located), a single cut
  weight is used, projection lines stay thin.

## Framing, mullions and curtain wall panels

Beams and braces (`FamilyInstance` with `m_pCurveDriver` → `FamInstCurveDriver`) carry their
location line: `m_pCrv` → `GLine` (`m_origin`, `m_dirVec`, `m_endParams`), with the setbacks
`m_setbacks` at both ends. When the family solids are not found (profiles driven by the
instance length), a box is built along that line: the width and height of the section are solved
(least squares) from the element's bounding box, each world extent being
`L·|n_i| + w·|y_i| + h·|z_i|`; the section is centred on the bounding box (justification offsets),
the length on the line.

Mullions (`MullionFamRef`) and panels (`CurtainCellFamRef`) are boxes in the axes of their
`m_Trf`: the edges solve `e_i = Σ_j |u_j[i]| · l_j` (Cramer), rejected when the system is
degenerate (45° rotation) or an edge is negative.

Limits: rectangular sections only (no I profile or tube), no mitre cuts, corner mullions and
non-rectangular panels stay approximate.

## V8: stored B-reps and the remaining element types (`rvt_shape.rs`)

Checked on the public samples only (rac 2018/2021, rst 2018/2023/2025).

- **Stored B-rep**: some elements do keep a planar B-rep in their tree (stairs runs and
  landings, profile roofs, parts, masses). `Face` → `m_pFirstLoop` → `EdgeLoop` (`m_next` = local
  id of an edge, `m_nextLoop`) → `Edge` (`m_pFace` = the two faces, `m_next` = next edge of the
  ring, which comes back to the loop, `m_firstAndLastEdgePnts` = `[u0, v0, u1, v1]` per side in
  the face's `Plane` parameters, feet). Local ids are mapped to tree objects (`Tree.local`).
  Parts (`PartMaker` → `DPartData.m_oGeometry`) and masses (`FamilyInstance` holding its own
  `Geometry`) use the same reader.
- **Roofs** (`Roof`): the reference layer faces `m_pRefLayerFaces` (planes) are grouped by
  direction and envelope; the thickness is the offset between parallel faces.
- **Doors and windows**: the family's default-type solids stretched to the type's width and
  height (`FamilySymbol` → `FamilyParams`: -1001301 width, -1001300 height, feet).
- **Railings**: `BaseRailing` (`m_sketchId` = path, `m_continuousRailIds` = top rail and
  handrails); `TopRail` / `HandRail` → `Path3dRailPathProvider.m_path3dId` → `Path3d` (z = top of
  the rail), swept as a rectangular band.
- **Curved walls**: the location arc swept by twice `m_keyRefOffset`, from the bottom to the top
  reference planes (unit test only: no public sample has one).
- **Topography** (`SiteSurface`): `m_drivers` → `m_pMementos` (last one) →
  `m_fixedVerts.m_loc` and `m_boundary`; ModelCraft triangulates (Delaunay, clipped to the
  boundary).
- **Rebar**: `Rebar` → `RebarInstance`: `m_oCacheData` → `RebarInstCache`
  (`m_noSelfIntersectingCenterlineCurves`, else `m_selfIntersectingCenterlineCurves`) →
  `OneBarCenterlineCurves.m_centerlineCurves` (`GLine` / `GArc`, world feet) = the first bar;
  bar k = that line through `m_individualBarsTrf[k]` then `m_barPositionsRelativeToFirstBarArr[k]`,
  skipped when `m_includedBars[k] = 0`; diameter `RebarBarType.m_barModelDiameter` (feet).
  Area / path reinforcement: `RebarInSystem` (`m_systemId`, `m_key {m_index, m_layer}`) →
  `RebarSystem.m_layers[layer]` (`m_uncutCurves` one line per bar, `m_individualLineTrf`,
  `m_includedUncutBars`, `m_barType`); the layer's `m_uncutRebarInstances` split its lines in
  consecutive runs of `m_numberOfBarPositions`, run `m_index` = the element. Bars are drawn as
  hexagonal tubes. rst 2023/2025: 334 / 334, all within 5 cm of their box.
- **Foundations**: wall footings (`ContFooting` → `ContFootingSegment.m_pProfile` (closed loop,
  world feet) swept along `m_pDriverCurve`); the driver stops at the wall ends, the footing at its
  joins, so an axis-aligned footing takes its length from its box. Isolated rectangular footings
  without family solids: the type's `m_outline` when it matches the type's
  STRUCTURAL_FOUNDATION_WIDTH / _LENGTH (-1001568 / -1001569). rst: 36 / 36.

Still boxes (or missing) on the public samples: stairs supports (`StairsSupport`, 2: the
stringer planes do not match the element box), planting and entourage (RPC content, no geometry
in the file), some families whose content document is not in the project (some plumbing
fixtures, a few mullions), and windows placed at an angle whose solids fall outside their box.
