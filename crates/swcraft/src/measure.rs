//! CPU-side geometry for picking and measurement: ray casts, face normals, cylinder fits
//! (hole diameters), edge chains (edge lengths) and the measurement state of a document.

use glam::{Mat4, Vec2, Vec3};
use std::collections::HashMap;
use sw_format::TriMesh;

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub comp: usize,
    pub mesh: usize,
    pub tri: u32,
    pub face: Option<usize>,
    pub t: f32,
    /// Hit point and geometric normal in display (world) space.
    pub point: Vec3,
    pub normal: Vec3,
}

/// Möller-Trumbore, two-sided. Returns the ray parameter.
pub fn ray_tri(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-20 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0).then_some(t)
}

fn ray_box(o: Vec3, d: Vec3, lo: Vec3, hi: Vec3) -> bool {
    let inv = Vec3::ONE / d;
    let t1 = (lo - o) * inv;
    let t2 = (hi - o) * inv;
    let tmin = t1.min(t2).max_element();
    let tmax = t1.max(t2).min_element();
    tmax >= tmin.max(0.0)
}

pub fn v3(p: [f32; 3]) -> Vec3 {
    Vec3::from(p)
}

/// Nearest hit over every drawn instance. `items` = (component, mesh, world matrix).
pub fn pick(meshes: &[&TriMesh], items: &[(usize, usize, Mat4)], o: Vec3, d: Vec3) -> Option<Hit> {
    let mut best: Option<Hit> = None;
    for &(comp, mi, world) in items {
        let m = meshes[mi];
        if m.indices.is_empty() {
            continue;
        }
        let inv = world.inverse();
        let lo = inv.transform_point3(o);
        let ld = inv.transform_vector3(d);
        let pad = (v3(m.bbox_max) - v3(m.bbox_min)).length() * 1e-3 + 1e-9;
        if !ray_box(lo, ld, v3(m.bbox_min) - Vec3::splat(pad), v3(m.bbox_max) + Vec3::splat(pad)) {
            continue;
        }
        for (ti, t3) in m.indices.chunks_exact(3).enumerate() {
            let a = v3(m.positions[t3[0] as usize]);
            let b = v3(m.positions[t3[1] as usize]);
            let c = v3(m.positions[t3[2] as usize]);
            if let Some(t) = ray_tri(lo, ld, a, b, c) {
                // ld is not normalised, but t is the same parameter for the world ray
                if best.is_none_or(|h| t < h.t) {
                    let n = world.transform_vector3((b - a).cross(c - a)).normalize_or_zero();
                    best = Some(Hit {
                        comp,
                        mesh: mi,
                        tri: ti as u32,
                        face: m.face_of_triangle(ti as u32),
                        t,
                        point: o + d * t,
                        normal: if n.dot(d) > 0.0 { -n } else { n },
                    });
                }
            }
        }
    }
    best
}

/// Triangles of a face (or the hit triangle alone when the mesh has no faces).
pub fn face_tris(m: &TriMesh, face: Option<usize>, tri: u32) -> std::ops::Range<u32> {
    match face {
        Some(f) => m.face_triangles(f),
        None => tri..tri + 1,
    }
}

/// Area-weighted normal, area and centroid of a face, in world space.
pub fn face_props(m: &TriMesh, tris: std::ops::Range<u32>, world: Mat4) -> (Vec3, f32, Vec3) {
    let mut n = Vec3::ZERO;
    let mut area = 0.0;
    let mut c = Vec3::ZERO;
    for t in tris {
        let i = t as usize * 3;
        let a = world.transform_point3(v3(m.positions[m.indices[i] as usize]));
        let b = world.transform_point3(v3(m.positions[m.indices[i + 1] as usize]));
        let cc = world.transform_point3(v3(m.positions[m.indices[i + 2] as usize]));
        let cr = (b - a).cross(cc - a);
        let ar = cr.length() * 0.5;
        n += cr;
        area += ar;
        c += (a + b + cc) / 3.0 * ar;
    }
    (n.normalize_or_zero(), area, if area > 0.0 { c / area } else { c })
}

#[derive(Clone, Copy, Debug)]
pub struct Cylinder {
    pub axis: Vec3,
    pub center: Vec3,
    pub radius: f32,
    /// RMS radial error relative to the radius.
    pub rel_err: f32,
    /// Normals point towards the axis (a hole / bore) rather than away (a boss / shaft).
    pub inward: bool,
    /// Swept angle around the axis (radians, 2π for a full cylinder).
    pub sweep: f32,
}

/// Symmetric 3×3 eigenvector of the smallest eigenvalue (Jacobi iterations).
fn smallest_eigvec(m: [[f64; 3]; 3]) -> Vec3 {
    let mut a = m;
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..32 {
        let (mut p, mut q, mut big) = (0, 1, 0.0);
        for i in 0..3 {
            for j in i + 1..3 {
                if a[i][j].abs() > big {
                    big = a[i][j].abs();
                    p = i;
                    q = j;
                }
            }
        }
        if big < 1e-15 {
            break;
        }
        let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
        let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
        let t = if theta == 0.0 { 1.0 } else { t };
        let c = 1.0 / (t * t + 1.0).sqrt();
        let s = t * c;
        let mut b = a;
        for k in 0..3 {
            b[p][k] = c * a[p][k] - s * a[q][k];
            b[q][k] = s * a[p][k] + c * a[q][k];
        }
        let a2 = b;
        for k in 0..3 {
            b[k][p] = c * a2[k][p] - s * a2[k][q];
            b[k][q] = s * a2[k][p] + c * a2[k][q];
        }
        a = b;
        for row in v.iter_mut() {
            let (vp, vq) = (row[p], row[q]);
            row[p] = c * vp - s * vq;
            row[q] = s * vp + c * vq;
        }
    }
    let k = (0..3).min_by(|&i, &j| a[i][i].partial_cmp(&a[j][j]).unwrap()).unwrap();
    Vec3::new(v[0][k] as f32, v[1][k] as f32, v[2][k] as f32).normalize_or_zero()
}

/// Least-squares (Kåsa) circle through 2D points: (centre, radius, rms error).
pub fn fit_circle_2d(pts: &[Vec2]) -> Option<(Vec2, f32, f32)> {
    if pts.len() < 3 {
        return None;
    }
    let mean = pts.iter().copied().sum::<Vec2>() / pts.len() as f32;
    let (mut suu, mut svv, mut suv, mut suuu, mut svvv, mut suvv, mut svuu) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
    for p in pts {
        let u = (p.x - mean.x) as f64;
        let v = (p.y - mean.y) as f64;
        suu += u * u;
        svv += v * v;
        suv += u * v;
        suuu += u * u * u;
        svvv += v * v * v;
        suvv += u * v * v;
        svuu += v * u * u;
    }
    let det = suu * svv - suv * suv;
    if det.abs() < 1e-30 {
        return None;
    }
    let r1 = 0.5 * (suuu + suvv);
    let r2 = 0.5 * (svvv + svuu);
    let uc = (r1 * svv - r2 * suv) / det;
    let vc = (r2 * suu - r1 * suv) / det;
    let n = pts.len() as f64;
    let r = (uc * uc + vc * vc + (suu + svv) / n).sqrt();
    let c = Vec2::new(uc as f32 + mean.x, vc as f32 + mean.y);
    let rms = (pts.iter().map(|p| ((p.distance(c) - r as f32) as f64).powi(2)).sum::<f64>() / n).sqrt();
    Some((c, r as f32, rms as f32))
}

fn basis(axis: Vec3) -> (Vec3, Vec3) {
    let u = axis.any_orthonormal_vector();
    (u, axis.cross(u))
}

/// Fit a cylinder to a face (hole or shaft). None when the face is not cylindrical.
pub fn fit_cylinder(m: &TriMesh, tris: std::ops::Range<u32>, world: Mat4) -> Option<Cylinder> {
    let mut idx: Vec<u32> = tris.flat_map(|t| (0..3).map(move |k| m.indices[t as usize * 3 + k])).collect();
    idx.sort_unstable();
    idx.dedup();
    if idx.len() < 6 {
        return None;
    }
    let pts: Vec<Vec3> = idx.iter().map(|&i| world.transform_point3(v3(m.positions[i as usize]))).collect();
    let nrm: Vec<Vec3> = idx.iter().map(|&i| world.transform_vector3(v3(m.normals[i as usize])).normalize_or_zero()).collect();
    let mut cov = [[0f64; 3]; 3];
    for n in &nrm {
        let a = [n.x as f64, n.y as f64, n.z as f64];
        for i in 0..3 {
            for j in 0..3 {
                cov[i][j] += a[i] * a[j];
            }
        }
    }
    let axis = smallest_eigvec(cov);
    // normals must be (nearly) perpendicular to the axis, and not all parallel (a plane)
    let max_dot = nrm.iter().map(|n| n.dot(axis).abs()).fold(0.0, f32::max);
    if max_dot > 0.25 {
        return None;
    }
    let (u, w) = basis(axis);
    let p2: Vec<Vec2> = pts.iter().map(|p| Vec2::new(p.dot(u), p.dot(w))).collect();
    let (c2, r, rms) = fit_circle_2d(&p2)?;
    let rel = rms / r.max(1e-12);
    if rel > 0.03 || !r.is_finite() {
        return None;
    }
    let along = pts.iter().map(|p| p.dot(axis)).sum::<f32>() / pts.len() as f32;
    let center = u * c2.x + w * c2.y + axis * along;
    // inward: normals point to the axis
    let mut inward = 0;
    let mut angles: Vec<f32> = Vec::with_capacity(pts.len());
    for (p, n) in pts.iter().zip(&nrm) {
        let radial = (*p - center) - axis * (*p - center).dot(axis);
        if radial.dot(*n) < 0.0 {
            inward += 1;
        }
        angles.push(radial.dot(w).atan2(radial.dot(u)));
    }
    angles.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut gap: f32 = angles[0] + std::f32::consts::TAU - angles[angles.len() - 1];
    for k in 1..angles.len() {
        gap = gap.max(angles[k] - angles[k - 1]);
    }
    // a full turn has only small gaps (one tessellation step)
    let sweep = std::f32::consts::TAU - gap;
    let sweep = if gap < 0.9 { std::f32::consts::TAU } else { sweep };
    if sweep < 0.6 {
        return None; // a sliver of arc is too uncertain to call a diameter
    }
    Some(Cylinder { axis, center, radius: r, rel_err: rel, inward: inward * 2 > pts.len(), sweep })
}

/// A chain of model-edge segments (one B-rep edge, or a closed loop such as a hole rim).
#[derive(Clone, Debug)]
pub struct EdgeChain {
    pub points: Vec<Vec3>,
    pub closed: bool,
}

impl EdgeChain {
    pub fn length(&self) -> f32 {
        let mut l: f32 = self.points.windows(2).map(|w| w[0].distance(w[1])).sum();
        if self.closed && self.points.len() > 2 {
            l += self.points[self.points.len() - 1].distance(self.points[0]);
        }
        l
    }
    /// Circle through the chain (centre, radius, normal) if it is one.
    pub fn circle(&self) -> Option<(Vec3, f32, Vec3)> {
        if self.points.len() < 5 {
            return None;
        }
        let mean = self.points.iter().copied().sum::<Vec3>() / self.points.len() as f32;
        let mut cov = [[0f64; 3]; 3];
        for p in &self.points {
            let d = *p - mean;
            let a = [d.x as f64, d.y as f64, d.z as f64];
            for i in 0..3 {
                for j in 0..3 {
                    cov[i][j] += a[i] * a[j];
                }
            }
        }
        let n = smallest_eigvec(cov);
        let flat = self.points.iter().map(|p| (*p - mean).dot(n).abs()).fold(0.0, f32::max);
        let (u, w) = basis(n);
        let p2: Vec<Vec2> = self.points.iter().map(|p| Vec2::new((*p - mean).dot(u), (*p - mean).dot(w))).collect();
        let (c, r, rms) = fit_circle_2d(&p2)?;
        if rms / r > 0.02 || flat > r * 0.02 {
            return None;
        }
        Some((mean + u * c.x + w * c.y, r, n))
    }
}

fn key(p: Vec3, q: f32) -> (i64, i64, i64) {
    ((p.x / q).round() as i64, (p.y / q).round() as i64, (p.z / q).round() as i64)
}

/// Distance from `p` to segment `ab`.
pub fn seg_dist(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-30)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// The model edge nearest to `p_world` (within `max_dist`), chained through vertices shared by
/// exactly two segments and stopping at sharp turns (> 50°), in world space.
pub fn edge_chain_near(m: &TriMesh, world: Mat4, p_world: Vec3, max_dist: f32) -> Option<EdgeChain> {
    if m.edges.is_empty() {
        return None;
    }
    let pts: Vec<Vec3> = m.positions.iter().map(|p| world.transform_point3(v3(*p))).collect();
    let diag = world.transform_vector3(v3(m.bbox_max) - v3(m.bbox_min)).length().max(1e-9);
    let q = diag * 1e-6;
    // weld positions so segments of neighbouring faces meet
    let mut ids: HashMap<(i64, i64, i64), usize> = HashMap::new();
    let mut node_pos: Vec<Vec3> = Vec::new();
    let mut node_of = |p: Vec3| -> usize {
        *ids.entry(key(p, q)).or_insert_with(|| {
            node_pos.push(p);
            node_pos.len() - 1
        })
    };
    let mut segs: Vec<(usize, usize)> = Vec::new();
    for e in m.edges.chunks_exact(2) {
        let a = node_of(pts[e[0] as usize]);
        let b = node_of(pts[e[1] as usize]);
        if a != b {
            segs.push((a.min(b), a.max(b)));
        }
    }
    segs.sort_unstable();
    segs.dedup();
    let (best, d) = segs
        .iter()
        .enumerate()
        .map(|(i, &(a, b))| (i, seg_dist(p_world, node_pos[a], node_pos[b])))
        .min_by(|x, y| x.1.partial_cmp(&y.1).unwrap())?;
    if d > max_dist {
        return None;
    }
    let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();
    for &(a, b) in &segs {
        adj.entry(a).or_default().push(b);
        adj.entry(b).or_default().push(a);
    }
    let (a0, b0) = segs[best];
    let walk = |from: usize, to: usize| -> (Vec<usize>, bool) {
        let mut out = vec![to];
        let (mut prev, mut cur) = (from, to);
        loop {
            let nb = &adj[&cur];
            if nb.len() != 2 {
                return (out, false);
            }
            let next = if nb[0] == prev { nb[1] } else { nb[0] };
            let d1 = (node_pos[cur] - node_pos[prev]).normalize_or_zero();
            let d2 = (node_pos[next] - node_pos[cur]).normalize_or_zero();
            if d1.dot(d2) < 50f32.to_radians().cos() {
                return (out, false);
            }
            if next == from || out.len() > 100_000 {
                return (out, next == from);
            }
            if next == a0 || next == b0 {
                return (out, true);
            }
            out.push(next);
            prev = cur;
            cur = next;
        }
    };
    let (fwd, closed) = walk(a0, b0);
    let mut chain: Vec<usize> = Vec::new();
    if closed {
        chain.push(a0);
        chain.extend(fwd);
    } else {
        let (back, _) = walk(b0, a0);
        chain.extend(back.iter().rev());
        chain.extend(fwd);
    }
    Some(EdgeChain { points: chain.iter().map(|&i| node_pos[i]).collect(), closed })
}

/// The mesh vertex of the hit triangle nearest to `p` (for snapping).
pub fn nearest_tri_vertex(m: &TriMesh, tri: u32, world: Mat4, p: Vec3) -> Vec3 {
    let i = tri as usize * 3;
    (0..3)
        .map(|k| world.transform_point3(v3(m.positions[m.indices[i + k] as usize])))
        .min_by(|a, b| a.distance(p).partial_cmp(&b.distance(p)).unwrap())
        .unwrap_or(p)
}

// ----- measurement state -----

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    Distance,
    Edge,
    Diameter,
    Angle,
}

#[derive(Clone, Debug)]
pub enum Measure {
    Distance { a: Vec3, b: Vec3 },
    Edge { chain: EdgeChain },
    Diameter { cyl: Cylinder, at: Vec3 },
    Angle { a: Vec3, na: Vec3, b: Vec3, nb: Vec3 },
    Face { at: Vec3, area: f32, normal: Vec3 },
}

#[derive(Default)]
pub struct MeasureState {
    pub tool: Tool,
    /// First point/face of a two-click measurement.
    pub pending: Option<(Vec3, Vec3)>,
    pub results: Vec<Measure>,
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_fit() {
        let pts: Vec<Vec2> = (0..20).map(|k| {
            let a = k as f32 * 0.3;
            Vec2::new(3.0 + 2.0 * a.cos(), -1.0 + 2.0 * a.sin())
        }).collect();
        let (c, r, rms) = fit_circle_2d(&pts).unwrap();
        assert!((c - Vec2::new(3.0, -1.0)).length() < 1e-3);
        assert!((r - 2.0).abs() < 1e-3);
        assert!(rms < 1e-3);
    }

    #[test]
    fn cylinder_fit_on_tube() {
        // tube of radius 5 along Z, normals inward (a hole)
        let mut pos = Vec::new();
        let mut nrm = Vec::new();
        let n = 32;
        for k in 0..n {
            let a = k as f32 / n as f32 * std::f32::consts::TAU;
            for z in [0.0f32, 10.0] {
                pos.push([5.0 * a.cos(), 5.0 * a.sin(), z]);
                nrm.push([-a.cos(), -a.sin(), 0.0]);
            }
        }
        let mut tris = Vec::new();
        for k in 0..n as u32 {
            let a = k * 2;
            let b = ((k + 1) % n as u32) * 2;
            tris.push([a, b, a + 1]);
            tris.push([b, b + 1, a + 1]);
        }
        let mut m = TriMesh::new_empty();
        m.push_indexed_face(&pos, &nrm, &tris, true);
        m.finish();
        let c = fit_cylinder(&m, 0..m.triangle_count() as u32, Mat4::IDENTITY).unwrap();
        assert!((c.radius - 5.0).abs() < 0.01, "{c:?}");
        assert!(c.axis.z.abs() > 0.99);
        assert!(c.inward);
        assert!((c.sweep - std::f32::consts::TAU).abs() < 1e-3);
        // the rims are closed circles of circumference 2π·5
        let chain = edge_chain_near(&m, Mat4::IDENTITY, Vec3::new(5.0, 0.0, 0.0), 0.5).unwrap();
        assert!(chain.closed);
        let (_, r, _) = chain.circle().unwrap();
        assert!((r - 5.0).abs() < 0.05);
        // pick through the tube wall
        let h = pick(&[&m], &[(0, 0, Mat4::IDENTITY)], Vec3::new(-20.0, 0.0, 5.0), Vec3::X).unwrap();
        assert!((h.point.x + 5.0).abs() < 0.1);
    }
}
