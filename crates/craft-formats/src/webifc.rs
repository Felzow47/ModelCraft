//! IFC (2x3, 4, 4x3: the BIM exchange format Revit, ArchiCAD, Tekla… export) through web-ifc
//! (That Open Company, MPL-2.0, unmodified, linked statically) via the C shim in
//! `webifc/shim.cpp`. Tree: storey → building elements, with their IFC names and colours.
use crate::{component, empty_scene, stem, Options};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::path::Path;
use std::time::Instant;
use sw_format::{MeshSource, Scene, SceneMesh};

type ElemCb = extern "C" fn(*mut c_void, c_uint, *const c_char, *const c_char, *const c_char) -> c_int;
type GeomCb = extern "C" fn(*mut c_void, c_int, c_uint, *const f64, *const f64, *const f32, c_uint, *const u32, c_uint);
type ProgCb = extern "C" fn(*mut c_void, c_uint, c_uint) -> c_int;

extern "C" {
    fn wi_load(data: *const c_char, len: usize, user: *mut c_void, elem: ElemCb, geom: GeomCb, prog: ProgCb, schema: *mut c_char, schema_len: c_int, err: *mut c_char, errlen: c_int) -> c_int;
}

struct Elem {
    id: u32,
    ty: String,
    name: String,
    storey: String,
    parts: Vec<(usize, [f32; 16], [f32; 4])>,
}

struct State<'a> {
    elems: Vec<Elem>,
    meshes: Vec<SceneMesh>,
    by_geom: HashMap<u32, usize>,
    edges: bool,
    opts: &'a Options,
}

fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

fn cancelled(o: &Options) -> bool {
    o.cancel.as_ref().is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
}

extern "C" fn on_elem(user: *mut c_void, id: c_uint, ty: *const c_char, name: *const c_char, storey: *const c_char) -> c_int {
    let st = unsafe { &mut *(user as *mut State) };
    if cancelled(st.opts) {
        return -1;
    }
    st.elems.push(Elem { id, ty: cstr(ty), name: cstr(name), storey: cstr(storey), parts: Vec::new() });
    (st.elems.len() - 1) as c_int
}

extern "C" fn on_geom(user: *mut c_void, elem: c_int, gid: c_uint, rgba: *const f64, m: *const f64, v: *const f32, nv: c_uint, ix: *const u32, ni: c_uint) {
    let st = unsafe { &mut *(user as *mut State) };
    let mi = match st.by_geom.get(&gid) {
        Some(&mi) => mi,
        None => {
            let vs = unsafe { std::slice::from_raw_parts(v, nv as usize * 6) };
            let is = unsafe { std::slice::from_raw_parts(ix, ni as usize) };
            let pos: Vec<[f32; 3]> = vs.chunks_exact(6).map(|c| [c[0], c[1], c[2]]).collect();
            // web-ifc's winding is not always consistent (it relies on its vertex normals):
            // orient each triangle along the supplied normals
            let nrm = |i: u32| {
                let c = &vs[i as usize * 6 + 3..i as usize * 6 + 6];
                glam::Vec3::new(c[0], c[1], c[2])
            };
            let tris: Vec<[u32; 3]> = is
                .chunks_exact(3)
                .filter(|t| t.iter().all(|&i| i < nv))
                .map(|t| {
                    let p = |i: u32| glam::Vec3::from(pos[i as usize]);
                    let face = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
                    let n = nrm(t[0]) + nrm(t[1]) + nrm(t[2]);
                    if face.dot(n) < 0.0 { [t[0], t[2], t[1]] } else { [t[0], t[1], t[2]] }
                })
                .collect();
            let mesh = sw_format::mesh::build_from_triangles(&pos, &tris, 30.0, st.edges);
            st.meshes.push(SceneMesh { name: format!("#{gid}"), mesh, source: MeshSource::Embedded, color: None });
            st.by_geom.insert(gid, st.meshes.len() - 1);
            st.meshes.len() - 1
        }
    };
    let m = unsafe { std::slice::from_raw_parts(m, 16) };
    let mut w = [0f32; 16];
    for i in 0..16 {
        w[i] = m[i] as f32;
    }
    let c = unsafe { std::slice::from_raw_parts(rgba, 4) };
    let color = [c[0] as f32, c[1] as f32, c[2] as f32, (c[3] as f32).clamp(0.05, 1.0)];
    if let Some(e) = st.elems.get_mut(elem as usize) {
        e.parts.push((mi, w, color));
    }
}

extern "C" fn on_prog(user: *mut c_void, done: c_uint, total: c_uint) -> c_int {
    let st = unsafe { &mut *(user as *mut State) };
    if let Some(p) = &st.opts.progress {
        p.report(0.35 + 0.6 * done as f32 / total.max(1) as f32, "Maillage des éléments IFC…");
    }
    cancelled(st.opts) as c_int
}

/// Friendly label for an IFC entity name: IFCWALLSTANDARDCASE -> IfcWallStandardCase is not
/// recoverable from upper case, so keep a small French table for the common ones.
fn type_label(t0: &str) -> String {
    let up = t0.to_ascii_uppercase();
    let t = up.as_str();
    let fr = match t {
        "IFCWALL" | "IFCWALLSTANDARDCASE" | "IFCWALLELEMENTEDCASE" => "Mur",
        "IFCSLAB" | "IFCSLABSTANDARDCASE" => "Dalle",
        "IFCROOF" => "Toit",
        "IFCDOOR" | "IFCDOORSTANDARDCASE" => "Porte",
        "IFCWINDOW" | "IFCWINDOWSTANDARDCASE" => "Fenêtre",
        "IFCCOLUMN" | "IFCCOLUMNSTANDARDCASE" => "Poteau",
        "IFCBEAM" | "IFCBEAMSTANDARDCASE" => "Poutre",
        "IFCSTAIR" | "IFCSTAIRFLIGHT" => "Escalier",
        "IFCRAILING" => "Garde-corps",
        "IFCCOVERING" => "Revêtement",
        "IFCFURNISHINGELEMENT" | "IFCFURNITURE" => "Mobilier",
        "IFCPLATE" => "Plaque",
        "IFCMEMBER" => "Montant",
        "IFCCURTAINWALL" => "Mur-rideau",
        "IFCFOOTING" => "Fondation",
        "IFCPILE" => "Pieu",
        "IFCRAMP" | "IFCRAMPFLIGHT" => "Rampe",
        "IFCFLOWSEGMENT" | "IFCPIPESEGMENT" | "IFCDUCTSEGMENT" => "Conduit",
        "IFCFLOWFITTING" | "IFCPIPEFITTING" | "IFCDUCTFITTING" => "Raccord",
        "IFCBUILDINGELEMENTPROXY" => "Objet",
        _ => "",
    };
    // web-ifc gives the schema spelling (IfcWallStandardCase); rebuild something readable otherwise
    let camel = if t0.starts_with("Ifc") {
        t0.to_string()
    } else {
        let short = t.strip_prefix("IFC").unwrap_or(t);
        format!("Ifc{}{}", &short[..1.min(short.len())], short.get(1..).unwrap_or("").to_lowercase())
    };
    if fr.is_empty() {
        camel
    } else {
        format!("{fr} ({camel})")
    }
}

pub fn load(path: &Path, bytes: &[u8], opts: &Options) -> Result<Scene, String> {
    let t0 = Instant::now();
    // web-ifc keeps a token tape (about 3× the file) plus the geometry cache
    sw_format::ctl::check_memory(bytes.len() as u64, 10.0, "IFC").map_err(|e| e.to_string())?;
    if !bytes.starts_with(b"ISO-10303-21") {
        return Err("ce n'est pas un fichier IFC texte (en-tête ISO-10303-21 absent ; les .ifczip / .ifcxml ne sont pas gérés)".into());
    }
    if let Some(p) = &opts.progress {
        p.report(0.33, "Analyse du fichier IFC…");
    }
    let mut st = State { elems: Vec::new(), meshes: Vec::new(), by_geom: HashMap::new(), edges: opts.edges, opts };
    let mut err = vec![0 as c_char; 1024];
    let mut schema = vec![0 as c_char; 64];
    // web-ifc's boolean engine keeps some static state: one IFC at a time
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let rc = {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { wi_load(bytes.as_ptr() as *const c_char, bytes.len(), &mut st as *mut State as *mut c_void, on_elem, on_geom, on_prog, schema.as_mut_ptr(), schema.len() as c_int, err.as_mut_ptr(), err.len() as c_int) }
    };
    opts.ctl().check().map_err(|e| e.to_string())?;
    if rc != 0 {
        return Err(format!("IFC : {}", cstr(err.as_ptr())));
    }
    let schema = cstr(schema.as_ptr());
    let mut s = empty_scene(path, "IFC");
    s.meshes = std::mem::take(&mut st.meshes);
    let root = stem(path);
    s.components.push(component(&root, 0, None, true, None));
    // storeys in order of first appearance; elements without storey go last
    let mut storeys: Vec<String> = Vec::new();
    for e in &st.elems {
        if !e.parts.is_empty() && !storeys.contains(&e.storey) {
            storeys.push(e.storey.clone());
        }
    }
    storeys.sort_by_key(|n| n.is_empty());
    let flat = storeys.len() <= 1;
    for sname in &storeys {
        let parent = if flat {
            0
        } else {
            let label = if sname.is_empty() { "Hors niveau".to_string() } else { sname.clone() };
            s.components.push(component(&label, 1, Some(0), true, None));
            s.components.len() - 1
        };
        let depth = s.components[parent].depth + 1;
        for e in st.elems.iter().filter(|e| &e.storey == sname && !e.parts.is_empty()) {
            let label = if e.name.is_empty() { format!("{} #{}", type_label(&e.ty), e.id) } else { format!("{} · {}", e.name, type_label(&e.ty)) };
            if e.parts.len() == 1 {
                let (m, w, col) = e.parts[0];
                let mut c = component(&label, depth, Some(parent), false, Some(m));
                c.world = w;
                c.color = Some(col);
                s.components.push(c);
            } else {
                s.components.push(component(&label, depth, Some(parent), true, None));
                let me = s.components.len() - 1;
                for (k, &(m, w, col)) in e.parts.iter().enumerate() {
                    let mut c = component(&format!("{label} · {}", k + 1), depth + 1, Some(me), false, Some(m));
                    c.world = w;
                    c.color = Some(col);
                    s.components.push(c);
                }
            }
        }
    }
    s.up_z = false;
    s.properties.push(("Schéma IFC".into(), schema));
    s.properties.push(("Éléments".into(), st.elems.len().to_string()));
    s.properties.push(("Lecteur".into(), "web-ifc (That Open Company)".into()));
    if s.meshes.is_empty() {
        s.warnings.push("Aucun élément 3D trouvé dans le fichier IFC".into());
    }
    s.timings.mesh = t0.elapsed();
    Ok(s)
}

/// The first .ifc file of an .ifczip archive.
pub fn unzip_ifc(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("archive IFCZIP illisible : {e}"))?;
    let name = z.file_names().find(|n| n.to_ascii_lowercase().ends_with(".ifc")).map(str::to_string).ok_or("aucun fichier .ifc dans l'archive IFCZIP")?;
    let mut f = z.by_name(&name).map_err(|e| e.to_string())?;
    sw_format::ctl::check_memory(f.size(), 6.0, "IFC").map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(f.size() as usize);
    f.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    /// IFC_SAMPLES = a folder of public IFC files (e.g. web-ifc's tests/ifcfiles/public); skipped if absent.
    #[test]
    fn ifc_samples() {
        let Some(dir) = std::env::var_os("IFC_SAMPLES") else { return };
        let dir = std::path::PathBuf::from(dir);
        for (f, min_tris) in [("example.ifc", 5_000), ("AC20-FZK-Haus.ifc", 10_000), ("duplex.ifc", 10_000)] {
            let p = dir.join(f);
            if !p.exists() {
                continue;
            }
            let s = crate::load_any(&p, &crate::Options::default()).unwrap_or_else(|e| panic!("{f}: {e}"));
            let tris: usize = s.components.iter().filter_map(|c| c.mesh).map(|m| s.meshes[m].mesh.indices.len() / 3).sum();
            assert!(tris >= min_tris, "{f}: {tris} triangles");
            assert!(!s.up_z);
        }
    }
}
