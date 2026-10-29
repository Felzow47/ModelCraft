//! STEP / IGES through OpenCascade (OCCT 7.8, LGPL-2.1 with linking exception), via the small C
//! shim in `occt/shim.cpp`: XCAF assembly tree with names and colours, one tessellation per part.
use crate::{component, empty_scene, mat_mul, stem, Options};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;
use std::time::Instant;
use sw_format::{MeshSource, Scene, SceneMesh, TriMesh};

type NodeCb = extern "C" fn(
    *mut c_void,
    *const c_char,
    c_int,
    c_int,
    *const f64,
    c_int,
    c_int,
    *const f32,
) -> c_int;
type FaceCb = extern "C" fn(
    *mut c_void,
    c_int,
    c_int,
    *const f32,
    *const f32,
    *const f32,
    c_int,
    *const u32,
    c_int,
);
type ProgCb = extern "C" fn(*mut c_void, c_int, c_int, c_int);

extern "C" {
    fn oc_load(
        path: *const c_char,
        iges: c_int,
        quality: f64,
        user: *mut c_void,
        node: NodeCb,
        face: FaceCb,
        prog: ProgCb,
        err: *mut c_char,
        errlen: c_int,
    ) -> c_int;
}

struct Node {
    name: String,
    parent: i32,
    is_asm: bool,
    local: [f32; 16],
    part: i32,
    color: Option<[f32; 4]>,
}

#[derive(Default)]
struct Part {
    // colour key -> mesh under construction
    groups: Vec<(Option<[u8; 4]>, Option<[f32; 4]>, TriMesh)>,
}

struct State<'a> {
    nodes: Vec<Node>,
    parts: HashMap<i32, Part>,
    edges: bool,
    opts: &'a Options,
}

extern "C" fn on_node(
    user: *mut c_void,
    name: *const c_char,
    parent: c_int,
    is_asm: c_int,
    m: *const f64,
    part: c_int,
    has_col: c_int,
    rgba: *const f32,
) -> c_int {
    let st = unsafe { &mut *(user as *mut State) };
    let name = if name.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned()
    };
    let m = unsafe { std::slice::from_raw_parts(m, 16) };
    let mut local = [0f32; 16];
    for i in 0..16 {
        local[i] = m[i] as f32;
    }
    for i in 12..15 {
        local[i] *= 0.001; // mm -> m
    }
    let color = if has_col != 0 {
        let c = unsafe { std::slice::from_raw_parts(rgba, 4) };
        Some([c[0], c[1], c[2], c[3]])
    } else {
        None
    };
    st.nodes.push(Node {
        name,
        parent,
        is_asm: is_asm != 0,
        local,
        part,
        color,
    });
    (st.nodes.len() - 1) as c_int
}

extern "C" fn on_face(
    user: *mut c_void,
    part: c_int,
    has_col: c_int,
    rgba: *const f32,
    pos: *const f32,
    nrm: *const f32,
    nv: c_int,
    idx: *const u32,
    nt: c_int,
) {
    let st = unsafe { &mut *(user as *mut State) };
    let (nv, nt) = (nv.max(0) as usize, nt.max(0) as usize);
    let p = unsafe { std::slice::from_raw_parts(pos, nv * 3) };
    let n = unsafe { std::slice::from_raw_parts(nrm, nv * 3) };
    let ix = unsafe { std::slice::from_raw_parts(idx, nt * 3) };
    let pos: Vec<[f32; 3]> = p
        .chunks_exact(3)
        .map(|c| [c[0] * 0.001, c[1] * 0.001, c[2] * 0.001])
        .collect();
    let nrm: Vec<[f32; 3]> = n.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
    let tris: Vec<[u32; 3]> = ix.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
    let color = if has_col != 0 {
        let c = unsafe { std::slice::from_raw_parts(rgba, 4) };
        Some([c[0], c[1], c[2], c[3]])
    } else {
        None
    };
    let key = color.map(|c| c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8));
    let entry = st.parts.entry(part).or_default();
    let g = match entry.groups.iter().position(|g| g.0 == key) {
        Some(i) => i,
        None => {
            entry.groups.push((key, color, TriMesh::new_empty()));
            entry.groups.len() - 1
        }
    };
    entry.groups[g]
        .2
        .push_indexed_face(&pos, &nrm, &tris, st.edges);
}

extern "C" fn on_progress(user: *mut c_void, stage: c_int, done: c_int, total: c_int) {
    let st = unsafe { &mut *(user as *mut State) };
    if let Some(p) = &st.opts.progress {
        let (f, label) = match stage {
            0 => (0.02, "Lecture du fichier…"),
            1 => (0.35, "Conversion de la géométrie…"),
            _ => (
                0.4 + 0.58 * done as f32 / total.max(1) as f32,
                "Maillage des surfaces…",
            ),
        };
        p.report(f, label);
    }
}

pub fn load(path: &Path, opts: &Options) -> Result<Scene, String> {
    let t0 = Instant::now();
    let ext = crate::ext_of(path);
    let iges = matches!(ext.as_str(), "iges" | "igs");
    // OCCT takes a UTF-8 path on every platform (it converts to wide chars on Windows)
    let cpath = CString::new(path.to_string_lossy().as_bytes())
        .map_err(|_| "chemin invalide".to_string())?;
    let mut st = State {
        nodes: Vec::new(),
        parts: HashMap::new(),
        edges: opts.edges,
        opts,
    };
    let mut err = vec![0 as c_char; 512];
    // OCCT's IGES reader (IGESFile/analiges.c, structiges.c) keeps its parse state in C globals
    // (curp, curpage…): two concurrent reads corrupt each other and crash (seen in iges_Dsect with a
    // 16-thread scan). Serialise every OCCT read.
    static OCCT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = OCCT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let rc = unsafe {
        oc_load(
            cpath.as_ptr(),
            iges as c_int,
            opts.quality as f64,
            &mut st as *mut State as *mut c_void,
            on_node,
            on_face,
            on_progress,
            err.as_mut_ptr(),
            err.len() as c_int,
        )
    };
    if rc != 0 {
        let msg = unsafe { CStr::from_ptr(err.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        return Err(format!("OpenCascade : {msg}"));
    }
    let t_read = t0.elapsed();
    let mut s = empty_scene(path, if iges { "IGES" } else { "STEP" });
    // meshes, one per (part, colour)
    let mut part_meshes: HashMap<i32, Vec<usize>> = HashMap::new();
    let mut keys: Vec<i32> = st.parts.keys().copied().collect();
    keys.sort();
    for k in keys {
        let part = st.parts.remove(&k).unwrap();
        for (gi, (_, color, mut mesh)) in part.groups.into_iter().enumerate() {
            mesh.finish();
            if mesh.is_empty() {
                continue;
            }
            s.meshes.push(SceneMesh {
                name: format!("part{k}.{gi}"),
                mesh,
                source: MeshSource::Embedded,
                color,
            });
            part_meshes.entry(k).or_default().push(s.meshes.len() - 1);
        }
    }
    // tree (nodes arrive in pre-order, parents first)
    let mut comp_of: Vec<usize> = Vec::with_capacity(st.nodes.len());
    let mut world_of: Vec<[f32; 16]> = Vec::with_capacity(st.nodes.len());
    let mut depth_of: Vec<usize> = Vec::with_capacity(st.nodes.len());
    let root_name = stem(path);
    for (i, n) in st.nodes.iter().enumerate() {
        let (pw, pc, depth) = if n.parent >= 0 {
            let p = n.parent as usize;
            (world_of[p], Some(comp_of[p]), depth_of[p] + 1)
        } else {
            (crate::IDENTITY, None, 0)
        };
        let world = mat_mul(&pw, &n.local);
        let name = if n.name.is_empty() {
            if i == 0 {
                root_name.clone()
            } else {
                format!("Composant {}", i)
            }
        } else {
            n.name.clone()
        };
        let meshes = if n.part >= 0 {
            part_meshes.get(&n.part).cloned().unwrap_or_default()
        } else {
            Vec::new()
        };
        let is_asm = n.is_asm || meshes.len() > 1;
        let mut c = component(
            &name,
            depth,
            pc,
            is_asm,
            if is_asm {
                None
            } else {
                meshes.first().copied()
            },
        );
        c.world = world;
        c.color = n.color;
        s.components.push(c);
        let me = s.components.len() - 1;
        if !n.is_asm && meshes.len() > 1 {
            for (k, m) in meshes.iter().enumerate() {
                let mut c = component(
                    &format!("{name} · corps {}", k + 1),
                    depth + 1,
                    Some(me),
                    false,
                    Some(*m),
                );
                c.world = world;
                s.components.push(c);
            }
        }
        comp_of.push(me);
        world_of.push(world);
        depth_of.push(depth);
    }
    s.up_z = true;
    s.timings.mesh = t_read;
    if s.meshes.is_empty() {
        s.warnings.push(
            "Le fichier ne contient aucune surface maillable (fil de fer ou points uniquement ?)"
                .into(),
        );
    }
    Ok(s)
}
