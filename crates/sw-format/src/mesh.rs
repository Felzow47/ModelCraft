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
    /// Texture coordinates, one per vertex (empty when the mesh has no texture).
    pub uvs: Vec<[f32; 2]>,
    /// Diffuse texture (RGBA8, sRGB) sampled with `uvs`, multiplied by the mesh colour.
    pub texture: Option<std::sync::Arc<crate::preview::Rgba>>,
}

impl TriMesh {
    /// Texture coordinate of vertex `v` (0,0 when untextured).
    pub fn uv(&self, v: usize) -> [f32; 2] {
        self.uvs.get(v).copied().unwrap_or([0.0, 0.0])
    }
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
    try_build(faces, with_edges, &crate::Ctl::default()).unwrap_or_default()
}

/// Above this many vertices in one mesh, B-rep edges are not computed (memory / time).
pub const EDGE_VERTEX_LIMIT: usize = 12_000_000;

/// [`build`] with fallible allocation, overflow checks and cancellation.
pub fn try_build(faces: &[FacePatch], with_edges: bool, ctl: &crate::Ctl) -> crate::Result<TriMesh> {
    let nv: usize = faces.iter().map(|f| f.positions.len()).sum();
    let mut b = MeshBuilder::new(nv, faces.len(), with_edges, None)?;
    for (fi, f) in faces.iter().enumerate() {
        if fi % 4096 == 4095 {
            ctl.check()?;
        }
        b.add_face(f)?;
    }
    Ok(b.finish())
}

/// Size statistics of a set of faces, gathered in a first streaming pass so the mesh can be
/// allocated exactly (or simplified) in the second one.
#[derive(Debug, Clone, Copy)]
pub struct FaceStats {
    pub faces: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub area: f64,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
}

impl Default for FaceStats {
    fn default() -> Self {
        FaceStats { faces: 0, vertices: 0, triangles: 0, area: 0.0, bbox_min: [f32::INFINITY; 3], bbox_max: [f32::NEG_INFINITY; 3] }
    }
}

impl FaceStats {
    pub fn add(&mut self, f: &FacePatch) {
        self.faces += 1;
        self.vertices += f.positions.len();
        for p in &f.positions {
            for k in 0..3 {
                self.bbox_min[k] = self.bbox_min[k].min(p[k]);
                self.bbox_max[k] = self.bbox_max[k].max(p[k]);
            }
        }
        for_each_triangle(f, |a, b, c| {
            self.triangles += 1;
            let n = cross(sub(f.positions[b], f.positions[a]), sub(f.positions[c], f.positions[a]));
            self.area += 0.5 * (dot(n, n) as f64).sqrt();
        });
    }
}

/// Visit the non-degenerate triangles of a face's strips (local vertex indices).
fn for_each_triangle(f: &FacePatch, mut cb: impl FnMut(usize, usize, usize)) {
    let n = f.positions.len();
    let mut o = 0usize;
    for &sv in &f.strips {
        let sv = sv as usize;
        if sv >= 3 && o + sv <= n {
            for j in 0..sv - 2 {
                let (a, b, c) = if j % 2 == 0 { (o + j, o + j + 1, o + j + 2) } else { (o + j + 1, o + j, o + j + 2) };
                let (pa, pb, pc) = (f.positions[a], f.positions[b], f.positions[c]);
                if pa == pb || pb == pc || pa == pc {
                    continue; // degenerate (strip stitching)
                }
                cb(a, b, c);
            }
        }
        o += sv;
    }
}

/// Vertex clustering on a uniform grid (display simplification of huge meshes).
struct Cluster {
    lo: [f32; 3],
    inv: f32,
    map: HashMap<u64, u32>,
    count: Vec<u32>,
    local: Vec<u32>,
}

/// Incremental mesh construction: faces are added one at a time (and can be freed right
/// after), so a huge display list never needs all its faces in memory at once.
pub struct MeshBuilder {
    m: TriMesh,
    with_edges: bool,
    cluster: Option<Cluster>,
    edge_count: HashMap<(u32, u32), (u32, u32, u32)>,
}

impl MeshBuilder {
    /// `expect_vertices` / `expect_faces` size the buffers (fallibly). With `cluster =
    /// Some((origin, cell))`, vertices are merged per grid cell of size `cell` (simplified
    /// display mesh, no B-rep edges).
    pub fn new(expect_vertices: usize, expect_faces: usize, with_edges: bool, cluster: Option<([f32; 3], f32)>) -> crate::Result<Self> {
        use crate::ctl::reserve;
        if expect_vertices >= u32::MAX as usize / 2 {
            return Err(crate::Error::TooLarge(format!("{expect_vertices} sommets dans une seule pièce")));
        }
        let mut m = TriMesh { bbox_min: [f32::INFINITY; 3], bbox_max: [f32::NEG_INFINITY; 3], ..Default::default() };
        let cluster = cluster.map(|(lo, cell)| Cluster { lo, inv: 1.0 / cell.max(1e-12), map: HashMap::new(), count: Vec::new(), local: Vec::new() });
        if cluster.is_none() {
            reserve(&mut m.positions, expect_vertices, "sommets")?;
            reserve(&mut m.normals, expect_vertices, "normales")?;
            reserve(&mut m.indices, expect_vertices.saturating_mul(3), "triangles")?;
        }
        reserve(&mut m.face_start, expect_faces, "faces")?;
        let with_edges = with_edges && cluster.is_none() && expect_vertices <= EDGE_VERTEX_LIMIT;
        Ok(MeshBuilder { m, with_edges, cluster, edge_count: HashMap::new() })
    }

    pub fn vertex_count(&self) -> usize {
        self.m.positions.len()
    }

    fn grow<T>(v: &mut Vec<T>, extra: usize, what: &str) -> crate::Result<()> {
        if v.capacity() - v.len() < extra {
            crate::ctl::reserve(v, extra.max(v.len() / 4), what)?;
        }
        Ok(())
    }

    pub fn add_face(&mut self, f: &FacePatch) -> crate::Result<()> {
        let n = f.positions.len();
        if self.m.positions.len() + n >= u32::MAX as usize / 2 {
            return Err(crate::Error::TooLarge(format!("plus de {} sommets dans une seule pièce", self.m.positions.len())));
        }
        for p in &f.positions {
            for k in 0..3 {
                self.m.bbox_min[k] = self.m.bbox_min[k].min(p[k]);
                self.m.bbox_max[k] = self.m.bbox_max[k].max(p[k]);
            }
        }
        let face_normal = |i: usize| -> [f32; 3] {
            if f.normals.len() == n {
                normalize(f.normals[i])
            } else if f.normals.len() == 1 {
                normalize(f.normals[0])
            } else {
                [0.0, 0.0, 0.0]
            }
        };
        let tri_start = self.m.indices.len();
        self.m.face_start.push((tri_start / 3) as u32);
        self.m.face_count += 1;
        if let Some(cl) = self.cluster.as_mut() {
            // Map each vertex to its grid cell; positions/normals are accumulated and averaged
            // in `finish`.
            cl.local.clear();
            for (i, p) in f.positions.iter().enumerate() {
                let q = |k: usize| (((p[k] - cl.lo[k]) * cl.inv).floor() as i64).clamp(0, (1 << 21) - 1) as u64;
                let key = q(0) | (q(1) << 21) | (q(2) << 42);
                let idx = match cl.map.get(&key) {
                    Some(&v) => v,
                    None => {
                        let v = self.m.positions.len() as u32;
                        Self::grow(&mut self.m.positions, 1, "sommets")?;
                        Self::grow(&mut self.m.normals, 1, "normales")?;
                        Self::grow(&mut cl.count, 1, "sommets")?;
                        self.m.positions.push([0.0; 3]);
                        self.m.normals.push([0.0; 3]);
                        cl.count.push(0);
                        cl.map.insert(key, v);
                        v
                    }
                };
                let k = idx as usize;
                for c in 0..3 {
                    self.m.positions[k][c] += p[c];
                }
                let nn = face_normal(i);
                for c in 0..3 {
                    self.m.normals[k][c] += nn[c];
                }
                cl.count[k] += 1;
                cl.local.push(idx);
            }
            let local = std::mem::take(&mut cl.local);
            let mut tris = 0usize;
            for_each_triangle(f, |_, _, _| tris += 1);
            Self::grow(&mut self.m.indices, tris * 3, "triangles")?;
            let idx = &mut self.m.indices;
            for_each_triangle(f, |a, b, c| {
                let (a, b, c) = (local[a], local[b], local[c]);
                if a != b && b != c && a != c {
                    idx.extend_from_slice(&[a, b, c]);
                }
            });
            self.cluster.as_mut().unwrap().local = local;
            return Ok(());
        }
        let base = self.m.positions.len() as u32;
        Self::grow(&mut self.m.positions, n, "sommets")?;
        Self::grow(&mut self.m.normals, n, "normales")?;
        self.m.positions.extend_from_slice(&f.positions);
        for i in 0..n {
            self.m.normals.push(face_normal(i));
        }
        let mut tris = 0usize;
        for_each_triangle(f, |_, _, _| tris += 1);
        Self::grow(&mut self.m.indices, tris * 3, "triangles")?;
        let idx = &mut self.m.indices;
        for_each_triangle(f, |a, b, c| idx.extend_from_slice(&[base + a as u32, base + b as u32, base + c as u32]));
        let m = &mut self.m;
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
        if self.with_edges {
            let edge_count = &mut self.edge_count;
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
        Ok(())
    }

    pub fn finish(mut self) -> TriMesh {
        if let Some(cl) = self.cluster.take() {
            drop(cl.map);
            for (k, &c) in cl.count.iter().enumerate() {
                let c = c.max(1) as f32;
                let p = &mut self.m.positions[k];
                *p = [p[0] / c, p[1] / c, p[2] / c];
                self.m.normals[k] = normalize(self.m.normals[k]);
            }
            self.m.positions.shrink_to_fit();
            self.m.normals.shrink_to_fit();
            self.m.indices.shrink_to_fit();
        }
        if self.m.positions.is_empty() {
            self.m.bbox_min = [0.0; 3];
            self.m.bbox_max = [0.0; 3];
        }
        self.m
    }
}

/// Grid cell size giving roughly `target` vertices when clustering a surface of `area`
/// (each cell crossed by the surface gives one vertex; about `area / cell²` cells).
pub fn cluster_cell(area: f64, target: usize) -> f32 {
    ((area / target.max(1) as f64).sqrt() * 0.9).max(1e-9) as f32
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
