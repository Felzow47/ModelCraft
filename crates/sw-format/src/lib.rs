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
pub mod bodies;
pub mod container;
pub mod ctl;
pub mod dl;
pub mod mesh;
pub mod preview;

pub use container::{Container, ContainerKind, StreamInfo};
pub use ctl::{Ctl, Progress};
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
    #[error("chargement annulé")]
    Cancelled,
    #[error("fichier trop volumineux — {0}")]
    TooLarge(String),
    #[error("mémoire insuffisante — {0}")]
    OutOfMemory(String),
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
    /// How the DisplayLists stream was turned into meshes (diagnostics).
    pub dl: Option<DlStats>,
}

/// Statistics of the display-list → mesh conversion.
#[derive(Debug, Clone, Default)]
pub struct DlStats {
    /// Decompressed size of the DisplayLists stream.
    pub raw_bytes: u64,
    /// Stored (compressed) size.
    pub stored_bytes: u64,
    /// Decompressed piece by piece (two passes) instead of being held in memory.
    pub streamed: bool,
    pub faces: usize,
    /// Vertices in the file's tessellation.
    pub src_vertices: usize,
    pub src_triangles: usize,
    /// Vertices actually kept for display (lower if simplified).
    pub out_vertices: usize,
    pub out_triangles: usize,
    pub simplified: bool,
    pub blocks: usize,
    /// Vertex budget used to decide on simplification.
    pub budget: usize,
    pub pass1: Duration,
    pub pass2: Duration,
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
    /// Descriptive metadata shown in the info panel (label, value), e.g. Revit project data.
    pub properties: Vec<(String, String)>,
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
    /// Progress reporting and cancellation.
    pub ctl: Ctl,
}

impl Default for LoadOptions {
    fn default() -> Self {
        LoadOptions { edges: true, resolve_external: true, preview: true, ctl: Ctl::default() }
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

/// Locate a referenced component file on disk (all comparisons case-insensitive):
/// 1. the stored absolute path;
/// 2. the stored path's trailing folders re-rooted at the assembly folder and its ancestors
///    (a project tree that was moved or copied to another drive);
/// 3. the assembly's folder and its parent, by file name;
/// 4. the whole folder tree around the assembly (up to two levels above it, bounded walk),
///    preferring the candidate whose folders best match the stored path.
pub fn resolve_component_path(asm_dir: &Path, stored: &str, cache: &mut HashMap<PathBuf, Vec<PathBuf>>) -> Option<PathBuf> {
    let p = PathBuf::from(stored);
    if (cfg!(windows) || !stored.contains('\\')) && p.is_file() {
        return Some(p);
    }
    let parts: Vec<&str> = stored.split(['\\', '/']).filter(|s| !s.is_empty() && !s.ends_with(':')).collect();
    let fname = parts.last()?.to_lowercase();
    let ancestors: Vec<PathBuf> = asm_dir.ancestors().take(4).map(Path::to_path_buf).collect();
    // 2. re-rooted tails, longest first (only folders inside the tail are tried, not the drive)
    for k in (2..=parts.len().min(5)).rev() {
        let tail = &parts[parts.len() - k..];
        for root in &ancestors {
            if let Some(f) = walk_ci(root, tail, cache) {
                return Some(f);
            }
        }
    }
    // 3. same folder, parent
    for d in ancestors.iter().take(2) {
        if let Some(f) = dir_listing(d, cache).iter().find(|q| name_lc(q) == fname) {
            return Some(f.clone());
        }
    }
    // 4. the tree under the grandparent (or the highest ancestor available)
    let root = ancestors.get(2).or(ancestors.last())?.clone();
    let key = root.join("\0tree");
    let index = cache.entry(key).or_insert_with(|| tree_index(&root));
    let stored_dirs: Vec<String> = parts[..parts.len() - 1].iter().map(|s| s.to_lowercase()).collect();
    index
        .iter()
        .filter(|q| name_lc(q) == fname)
        .max_by_key(|q| {
            // trailing folder names in common with the stored path, then shallower paths
            let dirs: Vec<String> = q.parent().map(|d| d.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect()).unwrap_or_default();
            let common = dirs.iter().rev().zip(stored_dirs.iter().rev()).take_while(|(a, b)| a == b).count();
            (common, usize::MAX - dirs.len())
        })
        .cloned()
}

fn name_lc(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Entries (files and folders) of one folder, cached.
fn dir_listing<'a>(d: &Path, cache: &'a mut HashMap<PathBuf, Vec<PathBuf>>) -> &'a Vec<PathBuf> {
    cache.entry(d.to_path_buf()).or_insert_with(|| std::fs::read_dir(d).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect()).unwrap_or_default())
}

/// Follow `tail` (folder names then the file name) from `root`, case-insensitively.
fn walk_ci(root: &Path, tail: &[&str], cache: &mut HashMap<PathBuf, Vec<PathBuf>>) -> Option<PathBuf> {
    let mut cur = root.to_path_buf();
    for (i, seg) in tail.iter().enumerate() {
        let lc = seg.to_lowercase();
        let last = i + 1 == tail.len();
        let next = dir_listing(&cur, cache).iter().find(|q| name_lc(q) == lc && if last { q.is_file() } else { q.is_dir() })?.clone();
        cur = next;
    }
    Some(cur)
}

/// SolidWorks / 3D files below `root` (bounded: depth 6, 200 000 entries).
fn tree_index(root: &Path) -> Vec<PathBuf> {
    const EXTS: &[&str] = &["sldprt", "sldasm", "prt", "asm", "step", "stp", "igs", "iges", "x_t", "x_b"];
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            seen += 1;
            if seen > 200_000 {
                return out;
            }
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                if depth < 6 {
                    stack.push((p, depth + 1));
                }
            } else if p.extension().is_some_and(|x| EXTS.contains(&x.to_string_lossy().to_lowercase().as_str())) {
                out.push(p);
            }
        }
    }
    out
}

/// Load just the faces of a file's display list (used for external components).
fn load_faces(path: &Path) -> Result<Vec<dl::FacePatch>> {
    let c = Container::open(path)?;
    let Some(d) = read_display_lists(&c)? else { return Ok(Vec::new()) };
    let list = dl::decode(&d);
    // For a part file, take all faces.
    Ok(list.blocks.into_iter().flat_map(|b| b.faces).collect())
}

/// Above this decompressed size, the DisplayLists stream is decoded straight from the
/// decompressor (two passes) instead of being held in memory.
pub const STREAM_THRESHOLD: u64 = 128 << 20;

/// Largest number of display vertices kept for one file before the mesh is simplified
/// (vertex clustering). `MODELCRAFT_MAX_VERTICES` overrides it.
pub fn vertex_budget() -> usize {
    if let Some(v) = std::env::var("MODELCRAFT_MAX_VERTICES").ok().and_then(|v| v.trim().parse::<usize>().ok()) {
        return v.max(10_000);
    }
    // ~60 bytes per kept vertex on the CPU side (positions, normals, indices, ...) and as much
    // again transiently for the GPU upload: stay well inside the available memory.
    let by_mem = ctl::mem_available().map(|a| (a / 200) as usize).unwrap_or(usize::MAX);
    by_mem.clamp(2_000_000, 40_000_000)
}

/// Meshes decoded from a DisplayLists stream, one per document block (in order of first
/// appearance) or a single merged one.
struct DlMeshes {
    blocks: Vec<(Option<String>, mesh::TriMesh)>,
    stats: DlStats,
}

/// Decode a DisplayLists stream into meshes with bounded memory: a first pass only counts
/// (faces are dropped as soon as they are measured), the second builds exactly sized meshes,
/// freeing each face right after it is copied. Large streams are never held decompressed:
/// both passes decode straight from the decompressor. If the tessellation is larger than the
/// vertex budget (or the available memory), it is simplified by vertex clustering.
fn build_dl_meshes(c: &Container, info: &container::StreamInfo, merge: bool, opts: &LoadOptions, warnings: &mut Vec<String>) -> Result<DlMeshes> {
    let ctl = &opts.ctl;
    let est = info.raw_size().unwrap_or(info.stored_size.saturating_mul(4));
    let streamed = est > STREAM_THRESHOLD || std::env::var_os("MODELCRAFT_STREAM").is_some();
    let mut stats = DlStats { stored_bytes: info.stored_size, streamed, budget: vertex_budget(), ..Default::default() };
    let whole = if streamed { None } else { Some(c.read_info_ctl(info, ctl, (0.03, 0.35))?) };
    if let Some(d) = &whole {
        stats.raw_bytes = d.len() as u64;
    }
    let (r1, r2) = if streamed { ((0.03, 0.45), (0.45, 0.9)) } else { ((0.35, 0.5), (0.5, 0.9)) };

    // One decoding pass over the stream.
    let run_pass = |range: (f32, f32), stage: &str, visit: &mut dyn FnMut(Option<&str>, dl::FacePatch) -> Result<()>| -> Result<u64> {
        match &whole {
            Some(d) => {
                let len = d.len().max(1);
                let mut prog = |pos: usize| -> Result<()> {
                    ctl.report(range.0 + (range.1 - range.0) * (pos as f32 / len as f32), stage);
                    ctl.check()
                };
                dl::Decoder::default().run(d, 0, true, visit, &mut prog)?;
                Ok(d.len() as u64)
            }
            None => {
                let mut sd = dl::StreamDecoder::new(ctl, visit);
                let n = c.stream_info(info, ctl, range, stage, &mut |chunk| sd.feed(chunk))?;
                sd.finish()?;
                Ok(n)
            }
        }
    };

    // Pass 1: statistics per block.
    let t = Instant::now();
    let mut keys: Vec<Option<String>> = Vec::new();
    let mut paths: Vec<Option<String>> = Vec::new();
    let mut fstats: Vec<mesh::FaceStats> = Vec::new();
    {
        let mut visit = |path: Option<&str>, face: dl::FacePatch| -> Result<()> {
            let key = if merge { None } else { path.map(dl::file_key) };
            let k = match keys.iter().position(|x| *x == key) {
                Some(k) => k,
                None => {
                    keys.push(key);
                    paths.push(if merge { None } else { path.map(str::to_string) });
                    fstats.push(mesh::FaceStats::default());
                    keys.len() - 1
                }
            };
            fstats[k].add(&face);
            Ok(())
        };
        let n = run_pass(r1, "Analyse du maillage (1/2)…", &mut visit)?;
        stats.raw_bytes = n;
    }
    stats.pass1 = t.elapsed();
    stats.blocks = keys.len();
    stats.faces = fstats.iter().map(|s| s.faces).sum();
    stats.src_vertices = fstats.iter().map(|s| s.vertices).sum();
    stats.src_triangles = fstats.iter().map(|s| s.triangles).sum();
    if stats.faces == 0 {
        return Ok(DlMeshes { blocks: Vec::new(), stats });
    }

    // Decide: exact meshes, or simplified to fit the budget / the memory.
    let mut budget = stats.budget;
    let bytes_per_vertex = 24.0 + 12.0 * stats.src_triangles as f64 / stats.src_vertices.max(1) as f64 + 4.0;
    if let Some(avail) = ctl::mem_available() {
        // keep the meshes under ~45 % of the available memory (GPU upload copies come on top)
        let fit = (avail as f64 * 0.45 / bytes_per_vertex) as usize;
        budget = budget.min(fit.max(500_000));
    }
    let simplify = stats.src_vertices > budget;
    stats.simplified = simplify;
    let edge_limit_hit = opts.edges && !simplify && fstats.iter().any(|s| s.vertices > mesh::EDGE_VERTEX_LIMIT);
    if edge_limit_hit {
        warnings.push(format!("Maillage très dense ({} M sommets) : arêtes non calculées", stats.src_vertices / 1_000_000));
    }

    // Pass 2: build.
    let t = Instant::now();
    let mut builders: Vec<mesh::MeshBuilder> = Vec::with_capacity(keys.len());
    for s in &fstats {
        let cluster = simplify.then(|| {
            let target = ((budget as f64) * s.vertices as f64 / stats.src_vertices as f64).max(1000.0) as usize;
            (s.bbox_min, mesh::cluster_cell(s.area, target))
        });
        builders.push(mesh::MeshBuilder::new(s.vertices, s.faces, opts.edges, cluster)?);
    }
    {
        let mut visit = |path: Option<&str>, face: dl::FacePatch| -> Result<()> {
            let key = if merge { None } else { path.map(dl::file_key) };
            if let Some(k) = keys.iter().position(|x| *x == key) {
                builders[k].add_face(&face)?;
            }
            Ok(())
        };
        run_pass(r2, "Construction du maillage (2/2)…", &mut visit)?;
    }
    drop(whole);
    let blocks: Vec<(Option<String>, mesh::TriMesh)> = paths.into_iter().zip(builders.into_iter().map(|b| b.finish())).collect();
    stats.pass2 = t.elapsed();
    stats.out_vertices = blocks.iter().map(|(_, m)| m.positions.len()).sum();
    stats.out_triangles = blocks.iter().map(|(_, m)| m.triangle_count()).sum();
    if simplify {
        warnings.push(format!(
            "Maillage simplifié pour l'affichage : {:.1} M → {:.1} M sommets (le fichier n'est pas modifié)",
            stats.src_vertices as f64 / 1e6,
            stats.out_vertices as f64 / 1e6
        ));
    }
    Ok(DlMeshes { blocks, stats })
}

/// Open a file and build a renderable scene.
pub fn load_scene(path: &Path, opts: &LoadOptions) -> Result<Scene> {
    let ctl = &opts.ctl;
    let t0 = Instant::now();
    let mut timings = Timings::default();
    ctl.report(0.02, "Ouverture du conteneur…");
    let c = Container::open_ctl(path, ctl)?;
    timings.open = t0.elapsed();
    ctl.check()?;

    let ext_asm = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("sldasm"));
    let t1 = Instant::now();
    let mut warnings = Vec::new();
    let tree_xml = c.read(COMP_TREE).ok().flatten();
    let kind = if ext_asm || (tree_xml.is_some() && !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("sldprt"))) {
        DocKind::Assembly
    } else {
        DocKind::Part
    };
    let dl_info = c.find(DISPLAY_LISTS);
    let had_dl = dl_info.is_some();
    let built = match dl_info {
        None => None,
        Some(info) => match build_dl_meshes(&c, info, kind == DocKind::Part, opts, &mut warnings) {
            Ok(b) => Some(b),
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) => {
                warnings.push(format!("Maillage illisible ({e}) : seule la vignette est affichée"));
                None
            }
        },
    };
    if let Some(b) = &built {
        timings.read = t1.elapsed().saturating_sub(b.stats.pass1 + b.stats.pass2);
        timings.decode = b.stats.pass1;
        timings.dl = Some(b.stats.clone());
    }
    let blocks = built.map(|b| b.blocks).unwrap_or_default();
    if !had_dl && warnings.is_empty() {
        warnings.push("Pas de flux DisplayLists (fichier enregistré sans données graphiques)".into());
    } else if had_dl && blocks.is_empty() && warnings.is_empty() {
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
            if let Some((_, m)) = blocks.into_iter().next() {
                meshes.push(SceneMesh { name: file_stem.clone(), mesh: m, source: MeshSource::Embedded, color: None });
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
            for (bpath, m) in blocks {
                let name = bpath.as_deref().map(stem_of).unwrap_or_else(|| file_stem.clone());
                meshes.push(SceneMesh { name, mesh: m, source: MeshSource::Embedded, color: None });
                match bpath.as_deref().map(dl::file_key) {
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
            if opts.resolve_external {
                ctl.report(0.9, "Composants externes…");
            }
            for comp in &flat {
                ctl.check()?;
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
                                let m = mesh::try_build(&faces, opts.edges, ctl).ok()?;
                                meshes.push(SceneMesh {
                                    name: stem_of(&comp.path),
                                    mesh: m,
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

    ctl.check()?;
    ctl.report(0.95, "Vignette…");
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
        properties: Vec::new(),
        timings,
        up_z: false,
    })
}

#[cfg(test)]
mod resolve_tests {
    #[test]
    fn component_search_case_insensitive_tree() {
        let root = std::env::temp_dir().join(format!("mc_res_{}", std::process::id()));
        let asm = root.join("Projet").join("Assemblages");
        let sub = root.join("Projet").join("Pièces").join("Visserie");
        std::fs::create_dir_all(&asm).unwrap();
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("VIS M6.SLDPRT"), b"x").unwrap();
        let want = sub.join("VIS M6.SLDPRT");
        // moved project tree: the stored folders re-rooted next to the assembly
        let mut c = std::collections::HashMap::new();
        let r = super::resolve_component_path(&asm, r"D:\Old\Projet\pièces\visserie\vis m6.sldprt", &mut c);
        assert_eq!(r.as_deref(), Some(want.as_path()));
        // unrelated stored folders: found by the bounded tree walk
        let mut c = std::collections::HashMap::new();
        let r = super::resolve_component_path(&asm, r"C:\X\Y\Vis M6.sldprt", &mut c);
        assert_eq!(r.as_deref(), Some(want.as_path()));
        // absent file
        let mut c = std::collections::HashMap::new();
        assert!(super::resolve_component_path(&asm, r"C:\X\nope.sldprt", &mut c).is_none());
        std::fs::remove_dir_all(&root).ok();
    }
}
