//! # sw-format
//!
//! Clean-room reader for SolidWorks part (`.SLDPRT`) and assembly (`.SLDASM`)
//! files, written from format observations only (no vendor code).
//!
//! * [`container`]: OLE (pre-2015) and disguised-ZIP (2015+) containers
//! * [`dl`]: cached display tessellation (`Contents/DisplayLists`)
//! * [`mesh`]: indexed triangle meshes + B-rep edges
//! * [`assembly`]: component tree and transforms (`COMPINSTANCETREE`)
//! * [`preview`]: embedded thumbnails
//! * [`Scene`]: everything a viewer needs, in one call.

pub mod assembly;
pub mod container;
pub mod dl;
pub mod mesh;
pub mod preview;

pub use container::{Container, ContainerKind, StreamInfo};
pub use mesh::TriMesh;
pub use preview::Rgba;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a SolidWorks file: {0}")]
    NotSolidWorks(String),
    #[error("corrupt data: {0}")]
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub const DISPLAY_LISTS: &str = "Contents/DisplayLists";
pub const COMP_TREE: &str = "swXmlContents/COMPINSTANCETREE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    Part,
    Assembly,
    /// Generic model from another format (STL, OBJ, STEP…), possibly with several objects.
    Model,
}

/// Read the display list of an opened container (decompressed), if any.
pub fn read_display_lists(c: &Container) -> Result<Option<Vec<u8>>> {
    c.read(DISPLAY_LISTS)
}

/// Best thumbnail: `PreviewPNG`, any `*PreviewPNG`, then `Preview` (DIB).
pub fn read_preview(c: &Container) -> Option<Rgba> {
    let mut names: Vec<&StreamInfo> = c.streams().iter().filter(|s| s.name.ends_with("PreviewPNG")).collect();
    names.sort_by_key(|s| (s.name != "PreviewPNG", s.name.len()));
    for s in names {
        if let Ok(b) = c.read_info(s) {
            if let Some(img) = preview::decode_png(&b) {
                return Some(img);
            }
        }
    }
    for s in c.streams().iter().filter(|s| s.name == "Preview" || s.name.ends_with("-Preview")) {
        if let Ok(b) = c.read_info(s) {
            if let Some(img) = preview::decode_dib(&b) {
                return Some(img);
            }
        }
    }
    None
}

#[derive(Debug, Clone, Default)]
pub struct Timings {
    /// Open container + list streams.
    pub open: Duration,
    /// Read + decompress DisplayLists (and XML tree).
    pub read: Duration,
    /// Decode tessellation records.
    pub decode: Duration,
    /// Build indexed meshes + edges.
    pub mesh: Duration,
    /// Loading external component files.
    pub external: Duration,
    pub total: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeshSource {
    /// Cached in the opened file itself.
    Embedded,
    /// Loaded from a sibling component file on disk.
    External(PathBuf),
}

#[derive(Debug, Clone)]
pub struct SceneMesh {
    pub name: String,
    pub mesh: TriMesh,
    pub source: MeshSource,
    /// Display colour (linear-ish sRGB 0..1, alpha) if the file defines one.
    pub color: Option<[f32; 4]>,
}

#[derive(Debug, Clone)]
pub struct SceneComponent {
    pub name: String,
    pub path: String,
    pub depth: usize,
    pub parent: Option<usize>,
    pub is_assembly: bool,
    /// Column-major 4x4 world transform (metres).
    pub world: [f32; 16],
    pub mesh: Option<usize>,
    pub suppressed: bool,
    pub hidden: bool,
    /// Colour override for this component (takes precedence over the mesh colour).
    pub color: Option<[f32; 4]>,
}

#[derive(Debug, Clone)]
pub struct Scene {
    pub path: PathBuf,
    pub kind: DocKind,
    pub container: ContainerKind,
    pub stream_count: usize,
    pub meshes: Vec<SceneMesh>,
    pub components: Vec<SceneComponent>,
    pub preview: Option<Rgba>,
    pub warnings: Vec<String>,
    pub timings: Timings,
    /// The model's vertical axis is +Z (most CAD exchange formats); SolidWorks is Y-up.
    pub up_z: bool,
}

impl Scene {
    /// Number of triangles actually drawn (instances counted).
    pub fn drawn_triangles(&self) -> usize {
        self.components
            .iter()
            .filter(|c| !c.suppressed)
            .filter_map(|c| c.mesh)
            .map(|m| self.meshes[m].mesh.triangle_count())
            .sum()
    }
    pub fn has_geometry(&self) -> bool {
        self.components.iter().any(|c| c.mesh.is_some_and(|m| !self.meshes[m].mesh.is_empty()))
    }
    /// Components that should have a mesh (parts) but none was found.
    pub fn missing_components(&self) -> usize {
        self.components.iter().filter(|c| !c.is_assembly && !c.suppressed && c.mesh.is_none()).count()
    }
    /// World-space bounding box of visible geometry.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        let mut any = false;
        for c in self.components.iter().filter(|c| !c.suppressed && !c.hidden) {
            let Some(mi) = c.mesh else { continue };
            let m = &self.meshes[mi].mesh;
            if m.is_empty() {
                continue;
            }
            let w = glam::Mat4::from_cols_array(&c.world);
            for k in 0..8 {
                let p = glam::Vec3::new(
                    if k & 1 == 0 { m.bbox_min[0] } else { m.bbox_max[0] },
                    if k & 2 == 0 { m.bbox_min[1] } else { m.bbox_max[1] },
                    if k & 4 == 0 { m.bbox_min[2] } else { m.bbox_max[2] },
                );
                let q = w.transform_point3(p);
                for i in 0..3 {
                    lo[i] = lo[i].min(q[i]);
                    hi[i] = hi[i].max(q[i]);
                }
                any = true;
            }
        }
        any.then_some((lo, hi))
    }
}

#[derive(Debug, Clone)]
pub struct LoadOptions {
    /// Compute B-rep edge lines.
    pub edges: bool,
    /// For assemblies: load component meshes from sibling files when not cached.
    pub resolve_external: bool,
    /// Decode the thumbnail.
    pub preview: bool,
}

impl Default for LoadOptions {
    fn default() -> Self {
        LoadOptions { edges: true, resolve_external: true, preview: true }
    }
}

fn identity() -> [f32; 16] {
    glam::Mat4::IDENTITY.to_cols_array()
}

fn stem_of(path: &str) -> String {
    let f = path.rsplit(['\\', '/']).next().unwrap_or(path);
    match f.rfind('.') {
        Some(i) => f[..i].to_string(),
        None => f.to_string(),
    }
}

/// Locate a referenced component file on disk: original path, then the
/// assembly's folder (and its parent) by file name (case-insensitive).
pub fn resolve_component_path(asm_dir: &Path, stored: &str, cache: &mut HashMap<PathBuf, Vec<PathBuf>>) -> Option<PathBuf> {
    let p = PathBuf::from(stored);
    if cfg!(windows) && p.is_file() {
        return Some(p);
    }
    let fname = stored.rsplit(['\\', '/']).next()?.to_lowercase();
    let mut dirs = vec![asm_dir.to_path_buf()];
    if let Some(parent) = asm_dir.parent() {
        dirs.push(parent.to_path_buf());
    }
    for d in dirs {
        let list = cache.entry(d.clone()).or_insert_with(|| {
            std::fs::read_dir(&d)
                .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file()).collect())
                .unwrap_or_default()
        });
        if let Some(found) = list.iter().find(|q| q.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase() == fname)) {
            return Some(found.clone());
        }
    }
    None
}

/// Load just the faces of a file's display list (used for external components).
fn load_faces(path: &Path) -> Result<Vec<dl::FacePatch>> {
    let c = Container::open(path)?;
    let Some(d) = read_display_lists(&c)? else { return Ok(Vec::new()) };
    let list = dl::decode(&d);
    // For a part file, take all faces.
    Ok(list.blocks.into_iter().flat_map(|b| b.faces).collect())
}

/// Open a file and build a renderable scene.
pub fn load_scene(path: &Path, opts: &LoadOptions) -> Result<Scene> {
    let t0 = Instant::now();
    let mut timings = Timings::default();
    let c = Container::open(path)?;
    timings.open = t0.elapsed();

    let ext_asm = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("sldasm"));
    let t1 = Instant::now();
    let mut warnings = Vec::new();
    let dl_bytes = match read_display_lists(&c) {
        Ok(d) => d,
        Err(e) => {
            warnings.push(format!("Maillage illisible ({e}) : seule la vignette est affichée"));
            None
        }
    };
    let tree_xml = c.read(COMP_TREE).ok().flatten();
    timings.read = t1.elapsed();
    let kind = if ext_asm || (tree_xml.is_some() && !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("sldprt"))) {
        DocKind::Assembly
    } else {
        DocKind::Part
    };

    let t2 = Instant::now();
    let list = dl_bytes.as_deref().map(dl::decode).unwrap_or_default();
    timings.decode = t2.elapsed();

    if dl_bytes.is_none() && warnings.is_empty() {
        warnings.push("Pas de flux DisplayLists (fichier enregistré sans données graphiques)".into());
    } else if list.is_empty() {
        warnings.push("DisplayLists présent mais aucun maillage décodé".into());
    }

    let t3 = Instant::now();
    let mut meshes: Vec<SceneMesh> = Vec::new();
    let mut components: Vec<SceneComponent> = Vec::new();
    let file_stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut ext_time = Duration::ZERO;

    match kind {
        DocKind::Model => {}
        DocKind::Part => {
            let faces: Vec<dl::FacePatch> = list.blocks.into_iter().flat_map(|b| b.faces).collect();
            if !faces.is_empty() {
                meshes.push(SceneMesh { name: file_stem.clone(), mesh: mesh::build(&faces, opts.edges), source: MeshSource::Embedded, color: None });
            }
            components.push(SceneComponent {
                name: file_stem.clone(),
                path: path.to_string_lossy().into_owned(),
                depth: 0,
                parent: None,
                is_assembly: false,
                world: identity(),
                mesh: (!meshes.is_empty()).then_some(0),
                suppressed: false,
                hidden: false,
                color: None,
            });
        }
        DocKind::Assembly => {
            // Index cached meshes by component file name.
            let mut by_key: HashMap<String, usize> = HashMap::new();
            let mut unkeyed: Vec<usize> = Vec::new();
            for b in &list.blocks {
                let name = b.path.as_deref().map(stem_of).unwrap_or_else(|| file_stem.clone());
                meshes.push(SceneMesh { name, mesh: mesh::build(&b.faces, opts.edges), source: MeshSource::Embedded, color: None });
                match b.key() {
                    Some(k) => {
                        by_key.insert(k, meshes.len() - 1);
                    }
                    None => unkeyed.push(meshes.len() - 1),
                }
            }
            let tree = match tree_xml.as_deref().map(assembly::parse) {
                Some(Ok(t)) => Some(t),
                Some(Err(e)) => {
                    warnings.push(e);
                    None
                }
                None => None,
            };
            let flat = tree.map(|t| t.flatten()).unwrap_or_default();
            let mut used = vec![false; meshes.len()];
            let asm_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            let mut dir_cache = HashMap::new();
            let mut ext_cache: HashMap<String, Option<usize>> = HashMap::new();
            for comp in &flat {
                let mut mesh_idx = None;
                if !comp.is_assembly {
                    let key = dl::file_key(&comp.path);
                    mesh_idx = by_key.get(&key).copied();
                    if mesh_idx.is_none() && opts.resolve_external && !comp.suppressed && !key.is_empty() {
                        mesh_idx = *ext_cache.entry(key.clone()).or_insert_with(|| {
                            let te = Instant::now();
                            let r = resolve_component_path(&asm_dir, &comp.path, &mut dir_cache).and_then(|p| {
                                let faces = load_faces(&p).ok()?;
                                if faces.is_empty() {
                                    return None;
                                }
                                meshes.push(SceneMesh {
                                    name: stem_of(&comp.path),
                                    mesh: mesh::build(&faces, opts.edges),
                                    source: MeshSource::External(p),
                                    color: None,
                                });
                                used.push(true);
                                Some(meshes.len() - 1)
                            });
                            ext_time += te.elapsed();
                            r
                        });
                    }
                    if let Some(i) = mesh_idx {
                        used[i] = true;
                    }
                }
                components.push(SceneComponent {
                    name: comp.name.clone(),
                    path: comp.path.clone(),
                    depth: comp.depth,
                    parent: comp.parent,
                    is_assembly: comp.is_assembly,
                    world: comp.world.as_mat4().to_cols_array(),
                    mesh: mesh_idx,
                    suppressed: comp.suppressed,
                    hidden: comp.hidden,
                    color: None,
                });
            }
            // Cached meshes not matched by any component: show them untransformed.
            for (i, u) in used.iter().enumerate() {
                if !*u {
                    if !flat.is_empty() && !unkeyed.contains(&i) {
                        warnings.push(format!("Maillage « {} » sans composant correspondant (affiché sans transformation)", meshes[i].name));
                    }
                    components.push(SceneComponent {
                        name: meshes[i].name.clone(),
                        path: String::new(),
                        depth: 0,
                        parent: None,
                        is_assembly: false,
                        world: identity(),
                        mesh: Some(i),
                        suppressed: false,
                        hidden: false,
                        color: None,
                    });
                }
            }
            let missing = components.iter().filter(|c| !c.is_assembly && !c.suppressed && c.mesh.is_none()).count();
            if missing > 0 {
                warnings.push(format!("{missing} composant(s) sans maillage (ni en cache, ni trouvé à côté de l'assemblage)"));
            }
        }
    }
    timings.external = ext_time;
    timings.mesh = t3.elapsed().saturating_sub(ext_time);

    let preview = if opts.preview { read_preview(&c) } else { None };
    timings.total = t0.elapsed();
    Ok(Scene {
        path: path.to_path_buf(),
        kind,
        container: c.kind(),
        stream_count: c.streams().len(),
        meshes,
        components,
        preview,
        warnings,
        timings,
        up_z: false,
    })
}
