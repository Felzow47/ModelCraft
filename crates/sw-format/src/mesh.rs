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
