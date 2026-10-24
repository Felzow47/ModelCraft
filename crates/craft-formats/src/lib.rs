//! Readers for common 3D exchange formats, all producing a [`sw_format::Scene`] so the viewer
//! and the scanner handle every format the same way.
//!
//! | format | reader |
//! |---|---|
//! | .sldprt / .sldasm | `sw_format` (display lists cached by SolidWorks) |
//! | .stl | [`stl`] ASCII + binary |
//! | .obj (+ .mtl) | [`obj`] |
//! | .3ds | [`tds`] |
//! | .3dxml | [`xml3d`] (tessellated 3DRep) |
//! | .step / .stp / .iges / .igs | [`occt`] (OpenCascade, feature `occt`) |
//! | .easm | best effort through `sw_format` |
//! | .x_t / .x_b | not supported (Parasolid has no open reader) |

pub mod easm;
pub mod obj;
pub mod rvt;
pub mod stl;
pub mod tds;
pub mod xml3d;
#[cfg(feature = "occt")]
pub mod occt;

use std::path::Path;
use std::time::Instant;
use sw_format::{ContainerKind, DocKind, LoadOptions, MeshSource, Scene, SceneComponent, SceneMesh, Timings};

pub type Loaded = Scene;

/// Support level of a file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Full,
    Partial,
    None,
}

pub struct FormatInfo {
    pub exts: &'static [&'static str],
    pub name: &'static str,
    /// French note shown as a tooltip.
    pub note: &'static str,
    pub support: Support,
}

pub fn formats() -> Vec<FormatInfo> {
    let step = if cfg!(feature = "occt") { Support::Full } else { Support::None };
    vec![
        FormatInfo { exts: &["sldprt"], name: "SolidWorks pièce", support: Support::Full, note: "Géométrie d'affichage enregistrée par SolidWorks, couleurs comprises." },
        FormatInfo { exts: &["sldasm"], name: "SolidWorks assemblage", support: Support::Full, note: "Arbre et positions ; les pièces sont lues à côté de l'assemblage." },
        FormatInfo { exts: &["step", "stp"], name: "STEP AP203/AP214/AP242", support: step, note: "B-rep maillé par Open CASCADE, couleurs et assemblages." },
        FormatInfo { exts: &["iges", "igs"], name: "IGES", support: step, note: "Surfaces maillées par Open CASCADE." },
        FormatInfo { exts: &["stl"], name: "STL", support: Support::Full, note: "ASCII et binaire." },
        FormatInfo { exts: &["obj"], name: "Wavefront OBJ", support: Support::Full, note: "Groupes et couleurs des matériaux (.mtl)." },
        FormatInfo { exts: &["3ds"], name: "3D Studio", support: Support::Full, note: "Objets, matrices et couleurs des matériaux." },
        FormatInfo { exts: &["3dxml"], name: "3D XML", support: Support::Partial, note: "Représentations maillées 3DXML (pas de B-rep CATIA)." },
        FormatInfo { exts: &["easm", "eprt"], name: "eDrawings", support: Support::Partial, note: "Aperçu et maillages lisibles uniquement." },
        FormatInfo { exts: &["rvt", "rfa", "rte", "rft"], name: "Autodesk Revit", support: Support::Partial, note: "Vignette et informations du projet (géométrie propriétaire non lisible)." },
        FormatInfo { exts: &["x_t", "x_b"], name: "Parasolid", support: Support::None, note: "Format B-rep propriétaire ; exportez en STEP depuis SolidWorks." },
    ]
}

/// Every extension the viewer opens (lowercase, no dot).
pub fn open_extensions() -> Vec<&'static str> {
    formats().into_iter().filter(|f| f.support != Support::None || f.exts[0] == "x_t").flat_map(|f| f.exts.iter().copied()).collect()
}

pub fn ext_of(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

pub fn is_supported(path: &Path) -> bool {
    let e = ext_of(path);
    formats().iter().any(|f| f.support != Support::None && f.exts.contains(&e.as_str()))
}

/// Options shared by all readers.
#[derive(Clone, Debug)]
pub struct Options {
    pub edges: bool,
    pub resolve_external: bool,
    pub preview: bool,
    /// Chordal deflection for B-rep tessellation, relative to the model size (STEP/IGES).
    pub quality: f32,
    /// Optional progress sink (fraction 0..1, stage label), called from the loading thread.
    pub progress: Option<Progress>,
    /// Set to true to stop loading as soon as possible.
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl Default for Options {
    fn default() -> Self {
        Options { edges: true, resolve_external: true, preview: true, quality: 0.5, progress: None, cancel: None }
    }
}

impl Options {
    pub fn ctl(&self) -> sw_format::Ctl {
        sw_format::Ctl { progress: self.progress.clone(), cancel: self.cancel.clone() }
    }
}

pub use sw_format::Progress;

/// Read a whole file with byte progress, cancellation and fallible allocation.
fn read_file(path: &Path, opts: &Options, mem_factor: f64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let ctl = opts.ctl();
    let mut f = std::fs::File::open(path).map_err(|e| format!("lecture impossible : {e}"))?;
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    sw_format::ctl::check_memory(len, mem_factor, "lecture").map_err(|e| e.to_string())?;
    let mut v: Vec<u8> = Vec::new();
    sw_format::ctl::reserve(&mut v, len as usize, "lecture").map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 8 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("lecture impossible : {e}"))?;
        if n == 0 {
            break;
        }
        if v.capacity() - v.len() < n {
            let extra = n.max(v.len() / 4);
            sw_format::ctl::reserve(&mut v, extra, "lecture").map_err(|e| e.to_string())?;
        }
        v.extend_from_slice(&buf[..n]);
        ctl.report(0.01 + 0.3 * (v.len() as f32 / len.max(1) as f32).min(1.0), "Lecture du fichier…");
        ctl.bytes(v.len() as u64, len);
        ctl.check().map_err(|e| e.to_string())?;
    }
    Ok(v)
}

/// Load any supported file.
pub fn load_any(path: &Path, opts: &Options) -> Result<Scene, String> {
    let t0 = Instant::now();
    let ext = ext_of(path);
    if let Some(p) = &opts.progress {
        p.report(0.01, "Lecture du fichier…");
    }
    // triangle-soup formats weld vertices through hash maps: several times the file size in RAM
    let read = |factor: f64| read_file(path, opts, factor);
    let mut scene = match ext.as_str() {
        "easm" | "eprt" => easm::load(path, &read(4.0)?)?,
        "rvt" | "rfa" | "rte" | "rft" => rvt::load(path, opts)?,
        "sldprt" | "sldasm" => {
            let lo = LoadOptions { edges: opts.edges, resolve_external: opts.resolve_external, preview: opts.preview, ctl: opts.ctl() };
            sw_format::load_scene(path, &lo).map_err(|e| e.to_string())?
        }
        "stl" => stl::load(path, &read(6.0)?)?,
        "obj" => obj::load(path, &read(6.0)?)?,
        "3ds" => tds::load(path, &read(6.0)?)?,
        "3dxml" => xml3d::load(path, &read(4.0)?)?,
        #[cfg(feature = "occt")]
        "step" | "stp" | "iges" | "igs" => occt::load(path, opts)?,
        #[cfg(not(feature = "occt"))]
        "step" | "stp" | "iges" | "igs" => return Err("STEP/IGES : cette version a été compilée sans OpenCascade.".into()),
        "x_t" | "x_b" | "xmt_txt" => {
            return Err("Format Parasolid (.x_t) non supporté : aucun lecteur libre n'existe. Exportez le modèle en STEP depuis le logiciel d'origine.".into())
        }
        _ => return Err(format!("Format « .{ext} » non supporté")),
    };
    opts.ctl().check().map_err(|e| e.to_string())?;
    scene.timings.total = t0.elapsed();
    Ok(scene)
}

/// Wrap one triangle soup into a scene (positions are multiplied by `scale` to get metres).
pub fn single_mesh_scene(path: &Path, format: &'static str, pos: &[[f32; 3]], tris: &[[u32; 3]], color: Option<[f32; 4]>, scale: f32) -> Scene {
    let t = Instant::now();
    let scaled: Vec<[f32; 3]> = pos.iter().map(|p| [p[0] * scale, p[1] * scale, p[2] * scale]).collect();
    let mesh = sw_format::mesh::build_from_triangles(&scaled, tris, 30.0, true);
    let name = stem(path);
    let mut s = empty_scene(path, format);
    s.meshes.push(SceneMesh { name: name.clone(), mesh, source: MeshSource::Embedded, color });
    s.components.push(component(&name, 0, None, false, Some(0)));
    s.timings.mesh = t.elapsed();
    s.up_z = true;
    s
}

pub fn empty_scene(path: &Path, format: &'static str) -> Scene {
    Scene {
        path: path.to_path_buf(),
        kind: DocKind::Model,
        container: ContainerKind::Other(format),
        stream_count: 0,
        meshes: Vec::new(),
        components: Vec::new(),
        preview: None,
        warnings: Vec::new(),
        properties: Vec::new(),
        timings: Timings::default(),
        up_z: true,
    }
}

pub fn component(name: &str, depth: usize, parent: Option<usize>, is_assembly: bool, mesh: Option<usize>) -> SceneComponent {
    SceneComponent {
        name: name.to_string(),
        path: String::new(),
        depth,
        parent,
        is_assembly,
        world: IDENTITY,
        mesh,
        suppressed: false,
        hidden: false,
        color: None,
    }
}

pub const IDENTITY: [f32; 16] = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.];

pub fn stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Column-major 4x4 multiply (a × b).
pub fn mat_mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut r = [0f32; 16];
    for c in 0..4 {
        for row in 0..4 {
            r[c * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[c * 4 + k]).sum();
        }
    }
    r
}

/// Make the scene kind reflect its structure: several top-level objects read as an assembly tree.
pub fn finish_tree(s: &mut Scene) {
    if s.components.len() > 1 && s.kind == DocKind::Model {
        // keep Model; the tree shows the objects
    }
    if s.meshes.is_empty() {
        s.warnings.push("Aucune géométrie affichable trouvée dans le fichier".into());
    }
}
