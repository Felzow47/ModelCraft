//! Mesh / scene exchange formats through Open Asset Import Library (assimp 6, BSD-3-Clause), via
//! the C shim in `assimp/shim.cpp`: FBX, glTF/GLB, COLLADA, 3MF, PLY, AMF, DXF, IFC (2x3),
//! X3D, USD/USDZ (tinyusdz, Apache-2.0), Blender, DirectX .x, LightWave… Node tree with names,
//! transforms and material colours.
use crate::{component, empty_scene, mat_mul, stem, Options};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;
use std::time::Instant;
use sw_format::{MeshSource, Scene, SceneMesh};

type NodeCb = extern "C" fn(*mut c_void, *const c_char, c_int, *const f32, c_int) -> c_int;
type MeshCb = extern "C" fn(*mut c_void, c_int, c_int, *const c_char, c_int, *const f32, *const f32, c_int, *const u32, c_int);
type MetaCb = extern "C" fn(*mut c_void, *const c_char, f64);
type ProgCb = extern "C" fn(*mut c_void, f32) -> c_int;

extern "C" {
    fn as_load(path: *const c_char, user: *mut c_void, node: NodeCb, mesh: MeshCb, meta: MetaCb, prog: ProgCb, err: *mut c_char, errlen: c_int) -> c_int;
}

/// Extensions read through assimp, with (format label, metres per file unit, Z-up).
/// Unitless formats (PLY, OFF, DXF, 3MF, AMF) are taken as millimetres like STL.
pub const EXTS: &[(&str, &str, f32, bool)] = &[
    ("fbx", "FBX", 1.0, false), // converted to metres by assimp, axis from metadata
    ("gltf", "glTF", 1.0, false),
    ("glb", "glTF binaire", 1.0, false),
    ("dae", "COLLADA", 1.0, false), // <unit> and <up_axis> applied by assimp
    ("3mf", "3MF", 0.001, true),
    ("amf", "AMF", 0.001, true),
    ("ply", "PLY", 0.001, true),
    ("dxf", "DXF", 0.001, true),
    ("ifc", "IFC", 1.0, false), // metres, rotated to Y-up by assimp
    ("x3d", "X3D", 1.0, false),
    ("x3db", "X3D binaire", 1.0, false),
    ("usd", "USD", 0.01, false),
    ("usda", "USD", 0.01, false),
    ("usdc", "USD", 0.01, false),
    ("usdz", "USDZ", 0.01, false),
    ("blend", "Blender", 1.0, true),
    ("x", "DirectX", 1.0, false),
    ("lwo", "LightWave", 1.0, false),
    ("lws", "LightWave", 1.0, false),
    ("ac", "AC3D", 1.0, false),
    ("ase", "3ds Max ASE", 0.001, true),
    ("ms3d", "MilkShape 3D", 1.0, false),
    ("cob", "Caligari", 1.0, false),
    ("smd", "Valve SMD", 1.0, false),
    ("md2", "Quake 2", 1.0, false),
    ("md3", "Quake 3", 1.0, false),
    ("md5mesh", "Doom 3", 1.0, false),
    ("b3d", "Blitz3D", 1.0, false),
    ("nff", "NFF", 1.0, false),
    ("xgl", "XGL", 1.0, false),
    ("zgl", "XGL", 1.0, false),
    ("ogex", "OpenGEX", 1.0, false),
    ("irrmesh", "Irrlicht", 1.0, false),
    ("irr", "Irrlicht", 1.0, false),
];

pub fn handles(ext: &str) -> bool {
    EXTS.iter().any(|e| e.0 == ext)
}

struct Node {
    name: String,
    parent: i32,
    local: [f32; 16],
    meshes: Vec<usize>,
}

struct State<'a> {
    nodes: Vec<Node>,
    meshes: Vec<SceneMesh>,
    by_id: HashMap<i32, usize>,
    meta: HashMap<String, f64>,
    edges: bool,
    scale: f32,
    opts: &'a Options,
    tris: usize,
}

extern "C" fn on_node(user: *mut c_void, name: *const c_char, parent: c_int, m: *const f32, _n: c_int) -> c_int {
    let st = unsafe { &mut *(user as *mut State) };
    if st.opts.cancel.as_ref().is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
        return -1;
    }
    let name = if name.is_null() { String::new() } else { unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned() };
    let m = unsafe { std::slice::from_raw_parts(m, 16) };
    let mut local = [0f32; 16];
    local.copy_from_slice(m);
    for v in &mut local[12..15] {
        *v *= st.scale;
    }
    st.nodes.push(Node { name, parent, local, meshes: Vec::new() });
    (st.nodes.len() - 1) as c_int
}

extern "C" fn on_mesh(user: *mut c_void, node: c_int, id: c_int, name: *const c_char, has_col: c_int, rgba: *const f32, pos: *const f32, nv: c_int, idx: *const u32, nt: c_int) {
    let st = unsafe { &mut *(user as *mut State) };
    let mi = if let Some(&mi) = st.by_id.get(&id) {
        mi
    } else {
        let (nv, nt) = (nv.max(0) as usize, nt.max(0) as usize);
        let p = unsafe { std::slice::from_raw_parts(pos, nv * 3) };
        let ix = unsafe { std::slice::from_raw_parts(idx, nt * 3) };
        let s = st.scale;
        let pos: Vec<[f32; 3]> = p.chunks_exact(3).map(|c| [c[0] * s, c[1] * s, c[2] * s]).collect();
        let tris: Vec<[u32; 3]> = ix.chunks_exact(3).filter(|c| c.iter().all(|&i| (i as usize) < nv)).map(|c| [c[0], c[1], c[2]]).collect();
        st.tris += tris.len();
        let mesh = sw_format::mesh::build_from_triangles(&pos, &tris, 30.0, st.edges);
        let color = if has_col != 0 { let c = unsafe { std::slice::from_raw_parts(rgba, 4) }; Some([c[0], c[1], c[2], c[3]]) } else { None };
        let name = if name.is_null() { String::new() } else { unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned() };
        st.meshes.push(SceneMesh { name, mesh, source: MeshSource::Embedded, color });
        st.by_id.insert(id, st.meshes.len() - 1);
        st.meshes.len() - 1
    };
    if let Some(n) = st.nodes.get_mut(node as usize) {
        n.meshes.push(mi);
    }
}

extern "C" fn on_meta(user: *mut c_void, key: *const c_char, v: f64) {
    let st = unsafe { &mut *(user as *mut State) };
    let k = unsafe { CStr::from_ptr(key) }.to_string_lossy().into_owned();
    st.meta.insert(k, v);
}

extern "C" fn on_progress(user: *mut c_void, f: f32) -> c_int {
    let st = unsafe { &mut *(user as *mut State) };
    if let Some(p) = &st.opts.progress {
        p.report(0.05 + 0.6 * f.clamp(0.0, 1.0), "Conversion de la géométrie…");
    }
    let cancel = st.opts.cancel.as_ref().is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed));
    cancel as c_int
}

pub fn load(path: &Path, opts: &Options) -> Result<Scene, String> {
    let t0 = Instant::now();
    let ext = crate::ext_of(path);
    let &(_, label, scale, mut up_z) = EXTS.iter().find(|e| e.0 == ext).ok_or_else(|| format!("format « .{ext} » non géré par assimp"))?;
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    // assimp keeps the whole file plus its own scene graph in memory
    sw_format::ctl::check_memory(len, 12.0, "import").map_err(|e| e.to_string())?;
    let cpath = CString::new(path.to_string_lossy().as_bytes()).map_err(|_| "chemin invalide".to_string())?;
    let mut st = State { nodes: Vec::new(), meshes: Vec::new(), by_id: HashMap::new(), meta: HashMap::new(), edges: opts.edges, scale, opts, tris: 0 };
    let mut err = vec![0 as c_char; 1024];
    // assimp importers keep no global parse state, but its default logger is global: serialise
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let rc = {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { as_load(cpath.as_ptr(), &mut st as *mut State as *mut c_void, on_node, on_mesh, on_meta, on_progress, err.as_mut_ptr(), err.len() as c_int) }
    };
    opts.ctl().check().map_err(|e| e.to_string())?;
    if rc != 0 {
        let msg = unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy().into_owned();
        return Err(format!("{label} : {}", explain(&msg)));
    }
    if ext == "fbx" {
        // FBX stores its axis system; assimp leaves it as-is
        if let Some(&ax) = st.meta.get("UpAxis") {
            up_z = ax as i32 == 2;
        }
    }
    if let Some(p) = &opts.progress {
        p.report(0.9, "Construction de l'arbre…");
    }
    let mut s = empty_scene(path, label);
    s.meshes = std::mem::take(&mut st.meshes);
    build_tree(&mut s, &st.nodes, &stem(path));
    s.up_z = up_z;
    s.timings.mesh = t0.elapsed();
    if s.meshes.is_empty() {
        s.warnings.push("Le fichier ne contient aucune surface affichable (points, lignes ou animation seulement ?)".into());
    }
    s.properties.push(("Format".into(), label.into()));
    s.properties.push(("Lecteur".into(), "Open Asset Import Library (assimp)".into()));
    Ok(s)
}

/// Turn the assimp node tree into components, dropping the branches that hold no mesh and
/// collapsing single-child pass-through nodes so FBX / glTF helper nodes don't clutter the tree.
fn build_tree(s: &mut Scene, nodes: &[Node], root_name: &str) {
    let n = nodes.len();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, nd) in nodes.iter().enumerate() {
        if nd.parent >= 0 && (nd.parent as usize) < i {
            children[nd.parent as usize].push(i);
        }
    }
    // subtree mesh counts (nodes come in pre-order: children after parents)
    let mut sub = vec![0usize; n];
    for i in (0..n).rev() {
        sub[i] += nodes[i].meshes.len();
        if nodes[i].parent >= 0 {
            let p = nodes[i].parent as usize;
            if p < i {
                sub[p] += sub[i];
            }
        }
    }
    let mut world = vec![crate::IDENTITY; n];
    for i in 0..n {
        let pw = if nodes[i].parent >= 0 && (nodes[i].parent as usize) < i { world[nodes[i].parent as usize] } else { crate::IDENTITY };
        world[i] = mat_mul(&pw, &nodes[i].local);
    }
    fn emit(s: &mut Scene, nodes: &[Node], children: &[Vec<usize>], sub: &[usize], world: &[[f32; 16]], i: usize, parent: Option<usize>, depth: usize, root_name: &str) {
        if sub[i] == 0 || s.components.len() > 200_000 {
            return;
        }
        let kids: Vec<usize> = children[i].iter().copied().filter(|&c| sub[c] > 0).collect();
        let nd = &nodes[i];
        // pass-through node: no mesh of its own and a single meaningful child
        if nd.meshes.is_empty() && kids.len() == 1 {
            emit(s, nodes, children, sub, world, kids[0], parent, depth, root_name);
            return;
        }
        let mut name = nd.name.clone();
        if name.is_empty() || name == "RootNode" || name == "Scene" || name.starts_with("$Assimp") {
            name = if parent.is_none() { root_name.to_string() } else { format!("Objet {}", s.components.len() + 1) };
        }
        if nd.meshes.len() == 1 && kids.is_empty() {
            let mut c = component(&name, depth, parent, false, Some(nd.meshes[0]));
            c.world = world[i];
            s.components.push(c);
            return;
        }
        let mut c = component(&name, depth, parent, true, None);
        c.world = world[i];
        s.components.push(c);
        let me = s.components.len() - 1;
        for (k, &m) in nd.meshes.iter().enumerate() {
            let mname = &s.meshes[m].name;
            let label = if mname.is_empty() || nd.meshes.len() == 1 { format!("{name} · {}", k + 1) } else { mname.clone() };
            let mut c = component(&label, depth + 1, Some(me), false, Some(m));
            c.world = world[i];
            s.components.push(c);
        }
        for &k in &kids {
            emit(s, nodes, children, sub, world, k, Some(me), depth + 1, root_name);
        }
    }
    for i in 0..n {
        if nodes[i].parent < 0 {
            emit(s, nodes, &children, &sub, &world, i, None, 0, root_name);
        }
    }
}

/// assimp messages in English → a short French explanation for the common cases.
fn explain(msg: &str) -> String {
    let m = msg.to_ascii_lowercase();
    if msg.is_empty() {
        "lecture impossible".into()
    } else if m.contains("no meshes remaining") || m.contains("no faces") || m.contains("no meshes") {
        "le fichier ne contient que des points ou des lignes (aucune face à afficher)".into()
    } else if m.contains("unable to open file") || m.contains("failed to open") {
        format!("fichier illisible ({msg})")
    } else if m.contains("no suitable reader") || m.contains("unsupported") {
        format!("variante non prise en charge ({msg})")
    } else {
        msg.to_string()
    }
}
