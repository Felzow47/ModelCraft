//! Indexed triangle meshes built from display-list faces, plus B-rep edges.

use crate::dl::FacePatch;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct TriMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Triangle list (3 indices per triangle).
    pub indices: Vec<u32>,
    /// Line list (2 indices per segment): boundaries of each B-rep face, i.e. model edges.
    pub edges: Vec<u32>,
    pub face_count: usize,
    /// First triangle of each face (B-rep face or smooth region); face i spans
    /// `face_start[i]..face_start[i+1]` (the last one runs to the end).
    pub face_start: Vec<u32>,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
}

impl TriMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
    /// Face index of a triangle (see `face_start`).
    pub fn face_of_triangle(&self, tri: u32) -> Option<usize> {
        if self.face_start.is_empty() {
            return None;
        }
        Some(self.face_start.partition_point(|&s| s <= tri).saturating_sub(1))
    }
    /// Triangle range of a face.
    pub fn face_triangles(&self, face: usize) -> std::ops::Range<u32> {
        let a = self.face_start.get(face).copied().unwrap_or(0);
        let b = self.face_start.get(face + 1).copied().unwrap_or(self.triangle_count() as u32);
        a..b
    }
    pub fn recompute_bbox(&mut self) {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in &self.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        if self.positions.is_empty() {
            lo = [0.0; 3];
            hi = [0.0; 3];
        }
        self.bbox_min = lo;
        self.bbox_max = hi;
    }
    /// Volume (signed, via the divergence theorem) and surface area of the mesh, in model units.
    /// The volume is only meaningful for closed meshes; its absolute value is returned.
    pub fn volume_area(&self) -> (f64, f64) {
        let mut vol = 0.0f64;
        let mut area = 0.0f64;
        for t in self.indices.chunks_exact(3) {
            let a = self.positions[t[0] as usize].map(|v| v as f64);
            let b = self.positions[t[1] as usize].map(|v| v as f64);
            let c = self.positions[t[2] as usize].map(|v| v as f64);
            let cr = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
            vol += (cr[0] * c[0] + cr[1] * c[1] + cr[2] * c[2]) / 6.0;
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
            area += 0.5 * (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        }
        (vol.abs(), area)
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    if l > 1e-30 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Build one mesh from a set of faces.
pub fn build(faces: &[FacePatch], with_edges: bool) -> TriMesh {
    let nv: usize = faces.iter().map(|f| f.positions.len()).sum();
    let mut m = TriMesh {
        positions: Vec::with_capacity(nv),
        normals: Vec::with_capacity(nv),
        indices: Vec::with_capacity(nv * 3),
        bbox_min: [f32::INFINITY; 3],
        bbox_max: [f32::NEG_INFINITY; 3],
        ..Default::default()
    };
    let mut edge_count: HashMap<(u32, u32), (u32, u32, u32)> = HashMap::new();
    for f in faces {
        let base = m.positions.len() as u32;
        let n = f.positions.len();
        m.positions.extend_from_slice(&f.positions);
        if f.normals.len() == n {
            m.normals.extend(f.normals.iter().map(|&x| normalize(x)));
        } else if f.normals.len() == 1 {
            let x = normalize(f.normals[0]);
            m.normals.extend(std::iter::repeat_n(x, n));
        } else {
            m.normals.extend(std::iter::repeat_n([0.0, 0.0, 0.0], n));
        }
        for p in &f.positions {
            for k in 0..3 {
                m.bbox_min[k] = m.bbox_min[k].min(p[k]);
                m.bbox_max[k] = m.bbox_max[k].max(p[k]);
            }
        }
        let tri_start = m.indices.len();
        m.face_start.push((tri_start / 3) as u32);
        let mut o = 0u32;
        for &sv in &f.strips {
            if sv >= 3 {
                for j in 0..sv - 2 {
                    let (a, b, c) = if j % 2 == 0 { (o + j, o + j + 1, o + j + 2) } else { (o + j + 1, o + j, o + j + 2) };
                    let (pa, pb, pc) = (f.positions[a as usize], f.positions[b as usize], f.positions[c as usize]);
                    if pa == pb || pb == pc || pa == pc {
                        continue; // degenerate (strip stitching)
                    }
                    m.indices.extend_from_slice(&[base + a, base + b, base + c]);
                }
            }
            o += sv;
        }
        // Missing normals -> flat normals from triangles.
        if f.normals.len() != n && f.normals.len() != 1 {
            for t in m.indices[tri_start..].chunks_exact(3) {
                let (a, b, c) = (m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
                let nn = normalize(cross(sub(b, a), sub(c, a)));
                for &i in t {
                    m.normals[i as usize] = nn;
                }
            }
        }
        // Edges: segments used by exactly one triangle of this face (welded by position).
        if with_edges {
            edge_count.clear();
            let mut weld: HashMap<[u32; 3], u32> = HashMap::new();
            let key = |p: [f32; 3]| [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
            let mut canon = |i: u32, pos: &[[f32; 3]]| -> u32 { *weld.entry(key(pos[i as usize])).or_insert(i) };
            for t in m.indices[tri_start..].chunks_exact(3) {
                let c = [canon(t[0], &m.positions), canon(t[1], &m.positions), canon(t[2], &m.positions)];
                for (x, y) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
                    let k = if x < y { (x, y) } else { (y, x) };
                    let e = edge_count.entry(k).or_insert((0, x, y));
                    e.0 += 1;
                }
            }
            for (_, (cnt, x, y)) in edge_count.iter() {
                if *cnt == 1 {
                    m.edges.push(*x);
                    m.edges.push(*y);
                }
            }
        }
        m.face_count += 1;
    }
    if m.positions.is_empty() {
        m.bbox_min = [0.0; 3];
        m.bbox_max = [0.0; 3];
    }
    m
}

/// Build a display mesh from a raw triangle soup (STL, OBJ, 3DS, STEP tessellation…).
///
/// Vertices are welded by position, normals are smoothed across edges whose dihedral angle is
/// below `crease_deg` and split elsewhere; "faces" are the smooth regions (used for picking) and
/// edges are the region boundaries plus open borders.
pub fn build_from_triangles(positions: &[[f32; 3]], tris: &[[u32; 3]], crease_deg: f32, with_edges: bool) -> TriMesh {
    // 1. weld
    let key = |p: [f32; 3]| [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
    let mut weld: HashMap<[u32; 3], u32> = HashMap::with_capacity(positions.len());
    let mut wpos: Vec<[f32; 3]> = Vec::with_capacity(positions.len());
    let remap: Vec<u32> = positions
        .iter()
        .map(|&p| {
            let p = [p[0] + 0.0, p[1] + 0.0, p[2] + 0.0]; // -0 → +0
            *weld.entry(key(p)).or_insert_with(|| {
                wpos.push(p);
                (wpos.len() - 1) as u32
            })
        })
        .collect();
    let mut t: Vec<[u32; 3]> = Vec::with_capacity(tris.len());
    let mut fnrm: Vec<[f32; 3]> = Vec::with_capacity(tris.len());
    for tr in tris {
        let (Some(&a), Some(&b), Some(&c)) = (remap.get(tr[0] as usize), remap.get(tr[1] as usize), remap.get(tr[2] as usize)) else { continue };
        if a == b || b == c || a == c {
            continue;
        }
        let n = cross(sub(wpos[b as usize], wpos[a as usize]), sub(wpos[c as usize], wpos[a as usize]));
        if dot(n, n) <= 0.0 {
            continue;
        }
        t.push([a, b, c]);
        fnrm.push(normalize(n));
    }
    // 2. edge → triangles adjacency
    let mut emap: HashMap<(u32, u32), Vec<u32>> = HashMap::with_capacity(t.len() * 2);
    for (ti, tr) in t.iter().enumerate() {
        for (x, y) in [(tr[0], tr[1]), (tr[1], tr[2]), (tr[2], tr[0])] {
            emap.entry(if x < y { (x, y) } else { (y, x) }).or_default().push(ti as u32);
        }
    }
    let cos_crease = crease_deg.to_radians().cos();
    let smooth = |a: u32, b: u32| dot(fnrm[a as usize], fnrm[b as usize]) >= cos_crease;
    // 3. smooth regions (union-find over smooth manifold edges)
    let mut parent: Vec<u32> = (0..t.len() as u32).collect();
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        while p[x as usize] != x {
            p[x as usize] = p[p[x as usize] as usize];
            x = p[x as usize];
        }
        x
    }
    for ts in emap.values() {
        if ts.len() == 2 && smooth(ts[0], ts[1]) {
            let (a, b) = (find(&mut parent, ts[0]), find(&mut parent, ts[1]));
            if a != b {
                parent[a as usize] = b;
            }
        }
    }
    let mut region: Vec<u32> = (0..t.len() as u32).map(|i| find(&mut parent, i)).collect();
    // compact ids, order triangles by region
    let mut ids: HashMap<u32, u32> = HashMap::new();
    for r in region.iter_mut() {
        let n = ids.len() as u32;
        *r = *ids.entry(*r).or_insert(n);
    }
    let mut order: Vec<u32> = (0..t.len() as u32).collect();
    order.sort_by_key(|&i| region[i as usize]);
    // 4. per-region vertex split with area-weighted smooth normals inside the region
    let mut m = TriMesh::default();
    let mut vmap: HashMap<(u32, u32), u32> = HashMap::new();
    let mut acc: Vec<[f32; 3]> = Vec::new();
    let mut last_region = u32::MAX;
    for &ti in &order {
        let r = region[ti as usize];
        if r != last_region {
            m.face_start.push((m.indices.len() / 3) as u32);
            last_region = r;
        }
        let tr = t[ti as usize];
        let a = wpos[tr[0] as usize];
        let n = cross(sub(wpos[tr[1] as usize], a), sub(wpos[tr[2] as usize], a));
        for &v in &tr {
            let idx = *vmap.entry((r, v)).or_insert_with(|| {
                m.positions.push(wpos[v as usize]);
                acc.push([0.0; 3]);
                (m.positions.len() - 1) as u32
            });
            let s = &mut acc[idx as usize];
            s[0] += n[0];
            s[1] += n[1];
            s[2] += n[2];
            m.indices.push(idx);
        }
    }
    m.normals = acc.into_iter().map(normalize).collect();
    m.face_count = m.face_start.len();
    // 5. edges: crease edges and open borders
    if with_edges {
        for (&(x, y), ts) in &emap {
            let feature = ts.len() != 2 || region[ts[0] as usize] != region[ts[1] as usize];
            if feature {
                let r = region[ts[0] as usize];
                if let (Some(&a), Some(&b)) = (vmap.get(&(r, x)), vmap.get(&(r, y))) {
                    m.edges.push(a);
                    m.edges.push(b);
                }
            }
        }
    }
    m.recompute_bbox();
    m
}

#[cfg(test)]
mod generic_tests {
    use super::*;
    #[test]
    fn cube_has_six_faces_and_twelve_edges() {
        let p = [[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.], [0., 0., 1.], [1., 0., 1.], [1., 1., 1.], [0., 1., 1.]];
        let q = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [2, 3, 7, 6], [1, 2, 6, 5], [0, 4, 7, 3]];
        let mut tris = Vec::new();
        for f in q {
            tris.push([f[0], f[1], f[2]]);
            tris.push([f[0], f[2], f[3]]);
        }
        let m = build_from_triangles(&p, &tris, 30.0, true);
        assert_eq!(m.face_count, 6);
        assert_eq!(m.edges.len() / 2, 12);
        let (v, a) = m.volume_area();
        assert!((v - 1.0).abs() < 1e-6 && (a - 6.0).abs() < 1e-6);
        assert_eq!(m.positions.len(), 24);
    }
}

impl TriMesh {
    /// An empty mesh ready for [`TriMesh::push_indexed_face`].
    pub fn new_empty() -> TriMesh {
        TriMesh { bbox_min: [f32::INFINITY; 3], bbox_max: [f32::NEG_INFINITY; 3], ..Default::default() }
    }

    /// Append one B-rep face given as an indexed triangulation with per-vertex normals.
    /// Edges are the face border (segments used by a single triangle, welded by position).
    pub fn push_indexed_face(&mut self, pos: &[[f32; 3]], nrm: &[[f32; 3]], tris: &[[u32; 3]], with_edges: bool) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(pos);
        if nrm.len() == pos.len() {
            self.normals.extend(nrm.iter().map(|&x| normalize(x)));
        } else {
            self.normals.extend(std::iter::repeat_n([0.0, 0.0, 1.0], pos.len()));
        }
        for p in pos {
            for k in 0..3 {
                self.bbox_min[k] = self.bbox_min[k].min(p[k]);
                self.bbox_max[k] = self.bbox_max[k].max(p[k]);
            }
        }
        self.face_start.push((self.indices.len() / 3) as u32);
        let n = pos.len() as u32;
        for t in tris {
            if t[0] >= n || t[1] >= n || t[2] >= n || t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
                continue;
            }
            self.indices.extend_from_slice(&[base + t[0], base + t[1], base + t[2]]);
        }
        if with_edges {
            let key = |p: [f32; 3]| [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
            let mut weld: HashMap<[u32; 3], u32> = HashMap::with_capacity(pos.len());
            let canon: Vec<u32> = (0..n).map(|i| *weld.entry(key(pos[i as usize])).or_insert(i)).collect();
            let mut cnt: HashMap<(u32, u32), (u32, u32, u32)> = HashMap::with_capacity(tris.len() * 2);
            for t in tris {
                if t[0] >= n || t[1] >= n || t[2] >= n {
                    continue;
                }
                let c = [canon[t[0] as usize], canon[t[1] as usize], canon[t[2] as usize]];
                for (x, y) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
                    if x == y {
                        continue;
                    }
                    let k = if x < y { (x, y) } else { (y, x) };
                    cnt.entry(k).or_insert((0, x, y)).0 += 1;
                }
            }
            for (_, (c, x, y)) in cnt {
                if c == 1 {
                    self.edges.push(base + x);
                    self.edges.push(base + y);
                }
            }
        }
        self.face_count += 1;
    }

    /// Fix up the bounding box of an empty mesh after the last `push_indexed_face`.
    pub fn finish(&mut self) {
        if self.positions.is_empty() {
            self.bbox_min = [0.0; 3];
            self.bbox_max = [0.0; 3];
        }
    }
}
