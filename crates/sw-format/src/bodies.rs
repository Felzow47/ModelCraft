//! Split a single display mesh into its separate bodies (connected pieces), so a part with
//! several bodies can be exploded, selected and made transparent body by body.
//!
//! Faces in a SolidWorks display list do not share vertices, so connectivity is found by welding
//! vertex positions (quantised to a tiny fraction of the part size) as well as by index.

use crate::{Scene, SceneComponent, SceneMesh, TriMesh};
use std::collections::HashMap;

/// Largest mesh (vertices) that is analysed; bigger ones stay whole.
pub const MAX_SPLIT_VERTICES: usize = 6_000_000;
/// More pieces than this (e.g. a scanned or text-heavy mesh) is not worth listing as bodies.
pub const MAX_BODIES: usize = 2000;

fn find(p: &mut [u32], mut x: u32) -> u32 {
    while p[x as usize] != x {
        let n = p[p[x as usize] as usize];
        p[x as usize] = n;
        x = n;
    }
    x
}

fn union(p: &mut [u32], a: u32, b: u32) {
    let (ra, rb) = (find(p, a), find(p, b));
    if ra != rb {
        let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
        p[hi as usize] = lo;
    }
}

/// Body index of every vertex, and the number of bodies (vertices used by no triangle get the
/// body of nothing: `u32::MAX`).
pub fn body_labels(m: &TriMesh) -> (Vec<u32>, usize) {
    let n = m.positions.len();
    let mut p: Vec<u32> = (0..n as u32).collect();
    for t in m.indices.chunks_exact(3) {
        union(&mut p, t[0], t[1]);
        union(&mut p, t[0], t[2]);
    }
    let ext = (0..3).map(|k| m.bbox_max[k] - m.bbox_min[k]).fold(0f32, f32::max).max(1e-12);
    let q = 1.0 / (ext * 2e-6);
    let mut seen: HashMap<[i32; 3], u32> = HashMap::with_capacity(n / 2);
    for (i, v) in m.positions.iter().enumerate() {
        let key = [((v[0] - m.bbox_min[0]) * q).round() as i32, ((v[1] - m.bbox_min[1]) * q).round() as i32, ((v[2] - m.bbox_min[2]) * q).round() as i32];
        match seen.entry(key) {
            std::collections::hash_map::Entry::Occupied(e) => union(&mut p, *e.get(), i as u32),
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(i as u32);
            }
        }
    }
    drop(seen);
    let mut used = vec![false; n];
    for &i in &m.indices {
        used[i as usize] = true;
    }
    let mut id = vec![u32::MAX; n];
    let mut label = vec![u32::MAX; n];
    let mut count = 0u32;
    for i in 0..n {
        if !used[i] {
            continue;
        }
        let r = find(&mut p, i as u32) as usize;
        if id[r] == u32::MAX {
            id[r] = count;
            count += 1;
        }
        label[i] = id[r];
    }
    (label, count as usize)
}

/// Split `m` into one mesh per body (in order of first appearance). Returns None for 0/1 body.
pub fn split_mesh(m: &TriMesh) -> Option<Vec<TriMesh>> {
    if m.positions.len() > MAX_SPLIT_VERTICES || m.indices.is_empty() {
        return None;
    }
    let (label, count) = body_labels(m);
    if !(2..=MAX_BODIES).contains(&count) {
        return None;
    }
    let mut out: Vec<TriMesh> = (0..count).map(|_| TriMesh::default()).collect();
    let mut remap = vec![u32::MAX; m.positions.len()];
    let tri_body = |t: usize| label[m.indices[t * 3] as usize] as usize;
    let push_vertex = |out: &mut Vec<TriMesh>, remap: &mut Vec<u32>, v: u32| -> u32 {
        let b = label[v as usize] as usize;
        if remap[v as usize] == u32::MAX {
            remap[v as usize] = out[b].positions.len() as u32;
            out[b].positions.push(m.positions[v as usize]);
            out[b].normals.push(m.normals.get(v as usize).copied().unwrap_or([0.0, 0.0, 1.0]));
        }
        remap[v as usize]
    };
    let ntri = m.triangle_count();
    let faces: Vec<std::ops::Range<u32>> = if m.face_start.is_empty() { vec![0..ntri as u32] } else { (0..m.face_start.len()).map(|f| m.face_triangles(f)).collect() };
    for r in faces {
        // a face may (rarely, e.g. a whole mesh without faces) span several bodies
        let mut started = vec![];
        for t in r.clone() {
            let b = tri_body(t as usize);
            if !started.contains(&b) {
                started.push(b);
                let s0 = (out[b].indices.len() / 3) as u32;
                out[b].face_start.push(s0);
            }
            for k in 0..3 {
                let v = m.indices[t as usize * 3 + k];
                let nv = push_vertex(&mut out, &mut remap, v);
                out[b].indices.push(nv);
            }
        }
    }
    for e in m.edges.chunks_exact(2) {
        let (a, b) = (e[0] as usize, e[1] as usize);
        if a >= label.len() || b >= label.len() || label[a] == u32::MAX || label[a] != label[b] || remap[a] == u32::MAX || remap[b] == u32::MAX {
            continue;
        }
        let body = label[a] as usize;
        out[body].edges.push(remap[a]);
        out[body].edges.push(remap[b]);
    }
    for o in out.iter_mut() {
        if m.face_start.is_empty() {
            o.face_start.clear();
        }
        o.face_count = o.face_start.len().max(if o.indices.is_empty() { 0 } else { 1 });
        o.recompute_bbox();
    }
    Some(out)
}

/// Most faces a single-body part is split into for its exploded view.
pub const MAX_FACE_PIECES: usize = 1500;

/// Split `m` into one mesh per face (B-rep face or smooth region), for exploding a part that
/// is a single body. Returns None when the mesh has no faces, one face or too many.
pub fn split_faces(m: &TriMesh) -> Option<Vec<TriMesh>> {
    if m.positions.len() > MAX_SPLIT_VERTICES || m.indices.is_empty() || !(2..=MAX_FACE_PIECES).contains(&m.face_start.len()) {
        return None;
    }
    let nv = m.positions.len();
    let mut out = Vec::with_capacity(m.face_start.len());
    let mut remap = vec![u32::MAX; nv];
    let mut owner = vec![u32::MAX; nv];
    for f in 0..m.face_start.len() {
        let r = m.face_triangles(f);
        let mut o = TriMesh::default();
        for t in r {
            for k in 0..3 {
                let Some(&v) = m.indices.get(t as usize * 3 + k) else { continue };
                let v = (v as usize).min(nv - 1);
                if owner[v] != f as u32 {
                    owner[v] = f as u32;
                    remap[v] = o.positions.len() as u32;
                    o.positions.push(m.positions[v]);
                    o.normals.push(m.normals.get(v).copied().unwrap_or([0.0, 0.0, 1.0]));
                }
                o.indices.push(remap[v]);
            }
        }
        if o.indices.is_empty() {
            continue;
        }
        // the face's boundary edges: segments whose two ends belong to this face
        for e in m.edges.chunks_exact(2) {
            let (a, b) = (e[0] as usize, e[1] as usize);
            if a < nv && b < nv && owner[a] == f as u32 && owner[b] == f as u32 {
                o.edges.push(remap[a]);
                o.edges.push(remap[b]);
            }
        }
        o.face_start = vec![0];
        o.face_count = 1;
        o.recompute_bbox();
        out.push(o);
    }
    (out.len() >= 2).then_some(out)
}

/// A part that is one body: one child component per face ("Face 1", "Face 2"…), so it can be
/// exploded face by face. Returns the number of faces (0 = unchanged).
pub fn split_single_part_faces(scene: &mut Scene) -> usize {
    split_single_with(scene, split_faces, "Face")
}

/// When the scene has exactly one component with a mesh, replace that mesh by one child
/// component per body ("Corps 1", "Corps 2"…). Returns the number of bodies (0 = unchanged).
pub fn split_single_part(scene: &mut Scene) -> usize {
    split_single_with(scene, split_mesh, "Corps")
}

fn split_single_with(scene: &mut Scene, split: fn(&TriMesh) -> Option<Vec<TriMesh>>, label: &str) -> usize {
    let with_mesh: Vec<usize> = scene.components.iter().enumerate().filter(|(_, c)| c.mesh.is_some_and(|m| !scene.meshes[m].mesh.is_empty())).map(|(i, _)| i).collect();
    let [ci] = with_mesh[..] else { return 0 };
    let mi = scene.components[ci].mesh.unwrap();
    // the mesh must not be shared with another (suppressed / empty) component
    if scene.components.iter().enumerate().any(|(k, c)| k != ci && c.mesh == Some(mi)) {
        return 0;
    }
    let Some(parts) = split(&scene.meshes[mi].mesh) else { return 0 };
    let n = parts.len();
    let base = scene.meshes[mi].clone_meta();
    let mut parts = parts.into_iter();
    scene.meshes[mi].mesh = parts.next().unwrap();
    let mut mesh_ids = vec![mi];
    for p in parts {
        mesh_ids.push(scene.meshes.len());
        scene.meshes.push(SceneMesh { mesh: p, ..base.clone() });
    }
    let parent = scene.components[ci].clone();
    let kids: Vec<SceneComponent> = mesh_ids
        .iter()
        .enumerate()
        .map(|(k, &m)| SceneComponent {
            name: format!("{label} {}", k + 1),
            path: parent.path.clone(),
            depth: parent.depth + 1,
            parent: Some(ci),
            is_assembly: false,
            world: parent.world,
            mesh: Some(m),
            suppressed: false,
            hidden: false,
            color: None,
        })
        .collect();
    scene.components[ci].mesh = None;
    // insert right after the part (depth-first order) and shift later parent indices
    for c in scene.components.iter_mut().skip(ci + 1) {
        if let Some(p) = c.parent.as_mut() {
            if *p > ci {
                *p += n;
            }
        }
    }
    scene.components.splice(ci + 1..ci + 1, kids);
    n
}

/// Bodies per part above which an assembly part is left whole (keeps big trees readable).
pub const MAX_BODIES_PER_ASM_PART: usize = 32;

/// Assemblies: give every multi-body part one child per body, so opacity, selection and
/// explosion work per body inside a part too. Meshes shared by several instances are split once.
/// Stops when the vertex budget is spent. Returns the number of body components added.
pub fn split_assembly_parts(scene: &mut Scene) -> usize {
    use std::collections::HashMap;
    let leaves: Vec<usize> = scene.components.iter().enumerate().filter(|(_, c)| c.mesh.is_some_and(|m| !scene.meshes[m].mesh.is_empty()) && !c.suppressed).map(|(i, _)| i).collect();
    if leaves.len() < 2 {
        return 0;
    }
    let mut budget = MAX_SPLIT_VERTICES;
    let mut split: HashMap<usize, Option<Vec<usize>>> = HashMap::new();
    let mut added = 0usize;
    // from the end so inserting children never moves a component still to visit
    for &ci in leaves.iter().rev() {
        let mi = scene.components[ci].mesh.unwrap();
        if !split.contains_key(&mi) {
            let nv = scene.meshes[mi].mesh.positions.len();
            let res = if nv <= budget {
                budget -= nv;
                split_mesh(&scene.meshes[mi].mesh).filter(|p| p.len() <= MAX_BODIES_PER_ASM_PART).map(|parts| {
                    let base = scene.meshes[mi].clone_meta();
                    parts
                        .into_iter()
                        .map(|p| {
                            scene.meshes.push(SceneMesh { mesh: p, ..base.clone() });
                            scene.meshes.len() - 1
                        })
                        .collect::<Vec<_>>()
                })
            } else {
                None
            };
            split.insert(mi, res);
        }
        let Some(Some(ids)) = split.get(&mi) else { continue };
        let ids = ids.clone();
        let n = ids.len();
        let parent = scene.components[ci].clone();
        let kids: Vec<SceneComponent> = ids
            .iter()
            .enumerate()
            .map(|(k, &m)| SceneComponent {
                name: format!("Corps {}", k + 1),
                path: parent.path.clone(),
                depth: parent.depth + 1,
                parent: Some(ci),
                is_assembly: false,
                world: parent.world,
                mesh: Some(m),
                suppressed: false,
                hidden: parent.hidden,
                color: parent.color,
            })
            .collect();
        scene.components[ci].mesh = None;
        for c in scene.components.iter_mut().skip(ci + 1) {
            if let Some(p) = c.parent.as_mut() {
                if *p > ci {
                    *p += n;
                }
            }
        }
        scene.components.splice(ci + 1..ci + 1, kids);
        added += n;
        if added > 50_000 {
            break;
        }
    }
    // the original whole-part meshes are no longer referenced; leave them empty to free memory
    for (mi, r) in split {
        if r.is_some() && !scene.components.iter().any(|c| c.mesh == Some(mi)) {
            scene.meshes[mi].mesh = TriMesh::default();
        }
    }
    added
}

impl SceneMesh {
    fn clone_meta(&self) -> SceneMesh {
        SceneMesh { name: self.name.clone(), mesh: TriMesh::default(), source: self.source.clone(), color: self.color }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembly_parts_split_per_instance() {
        let two = concat(quad(0.0, 2), quad(10.0, 1));
        let mesh = SceneMesh { name: "p".into(), mesh: two, source: crate::MeshSource::Embedded, color: None };
        let comp = |name: &str, depth: usize, parent: Option<usize>, m: Option<usize>| SceneComponent { name: name.into(), path: String::new(), depth, parent, is_assembly: m.is_none(), world: [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.], mesh: m, suppressed: false, hidden: false, color: None };
        let mut s = crate::Scene { meshes: vec![mesh], components: vec![comp("asm", 0, None, None), comp("a", 1, Some(0), Some(0)), comp("b", 1, Some(0), Some(0))], path: Default::default(), kind: crate::DocKind::Assembly, container: crate::ContainerKind::Other("t"), stream_count: 0, preview: None, warnings: vec![], properties: vec![], timings: Default::default(), up_z: false };
        let n = split_assembly_parts(&mut s);
        assert_eq!(n, 4, "two instances × two bodies");
        let names: Vec<&str> = s.components.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["asm", "a", "Corps 1", "Corps 2", "b", "Corps 1", "Corps 2"]);
        assert_eq!(s.components[5].parent, Some(4));
        assert_eq!(s.components[4].parent, Some(0));
        assert!(s.meshes[0].mesh.is_empty());
    }

    fn quad(off: f32, faces: usize) -> TriMesh {
        // `faces` separate faces forming a strip (vertices duplicated per face, like a display list)
        let mut m = TriMesh::default();
        for f in 0..faces {
            let x = off + f as f32;
            let b = m.positions.len() as u32;
            m.positions.extend([[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x + 1.0, 1.0, 0.0], [x, 1.0, 0.0]]);
            m.normals.extend([[0.0, 0.0, 1.0]; 4]);
            m.face_start.push((m.indices.len() / 3) as u32);
            m.indices.extend([b, b + 1, b + 2, b, b + 2, b + 3]);
            m.edges.extend([b, b + 1]);
        }
        m.face_count = faces;
        m.recompute_bbox();
        m
    }

    fn concat(a: TriMesh, b: TriMesh) -> TriMesh {
        let mut m = a;
        let o = m.positions.len() as u32;
        let t = (m.indices.len() / 3) as u32;
        m.positions.extend(b.positions);
        m.normals.extend(b.normals);
        m.indices.extend(b.indices.iter().map(|i| i + o));
        m.edges.extend(b.edges.iter().map(|i| i + o));
        m.face_start.extend(b.face_start.iter().map(|s| s + t));
        m.face_count += b.face_count;
        m.recompute_bbox();
        m
    }

    #[test]
    fn welded_faces_make_one_body_and_gaps_split() {
        let m = concat(quad(0.0, 3), quad(10.0, 2));
        let parts = split_mesh(&m).expect("two bodies");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].face_count, 3);
        assert_eq!(parts[1].face_count, 2);
        assert_eq!(parts[0].triangle_count() + parts[1].triangle_count(), m.triangle_count());
        assert_eq!(parts[0].edges.len() + parts[1].edges.len(), m.edges.len());
        assert!(split_mesh(&quad(0.0, 4)).is_none());
    }
}

#[cfg(test)]
mod face_tests {
    use super::*;
    #[test]
    fn single_body_splits_into_faces() {
        // two triangles sharing an edge, each its own face
        let m = TriMesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            indices: vec![0, 1, 2, 1, 3, 2],
            edges: vec![0, 1, 1, 3],
            face_count: 2,
            face_start: vec![0, 1],
            bbox_min: [0.0; 3],
            bbox_max: [1.0, 1.0, 0.0],
        };
        let f = split_faces(&m).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].indices.len(), 3);
        assert_eq!(f[1].positions.len(), 3);
        assert_eq!(f[0].edges.len(), 2);
        assert!(split_mesh(&m).is_none());
    }
}
