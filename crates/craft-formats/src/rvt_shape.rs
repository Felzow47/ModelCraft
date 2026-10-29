//! Revit element shapes rebuilt from the decoded element data (`rvt_geom`): Revit does not
//! store the meshes of walls, floors or family solids in the file, only what drives them, so
//! the solids are rebuilt here the way they are defined:
//!
//! - walls: the elevation profile (`VWallExtrusionHelper` curve loops, in the vertical plane
//!   of the location line) extruded across the wall between its outermost side faces
//!   (`m_pRefFaces` planes), minus the openings of the inserts that cut it (doors, windows,
//!   embedded walls: their boxes projected on the wall);
//! - floors, ceilings, pads: the sketch loops between the bottom and top faces;
//! - family solids (`ExtrusionElem`): the sketch loops extruded along the sketch plane normal
//!   between the start and end parameters, placed by every instance's transform (families
//!   whose document is in the project file: in-place families and the like).
//!
//! Regions are tessellated by a strip (trapezoid) decomposition, which handles holes,
//! several loops and rectangular cut-outs without a general polygon boolean.
//!
//! Clean-room, from byte observations only (see docs/rvt-rea-notes.md).

use crate::rvt_browser::Schema;
use crate::rvt_geom::{elements, Obj, Tree, G};
use std::collections::HashMap;

type V3 = [f64; 3];
const FEET: f64 = 0.3048;

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: V3) -> Option<V3> {
    let l = dot(a, a).sqrt();
    (l > 1e-12 && l.is_finite()).then(|| mul(a, 1.0 / l))
}
fn v3(s: Option<&[f64]>) -> Option<V3> {
    let s = s?;
    (s.len() >= 3 && s[..3].iter().all(|x| x.is_finite())).then(|| [s[0], s[1], s[2]])
}

/// A triangle mesh in metres.
#[derive(Debug, Default, Clone)]
pub struct Mesh {
    pub pos: Vec<[f32; 3]>,
    pub tri: Vec<[u32; 3]>,
}

impl Mesh {
    pub fn bbox(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in &self.pos {
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
        (!self.pos.is_empty()).then_some((lo, hi))
    }
}

/// A frame: origin and three axes (feet). Points (u, v, w) map to `o + u·U + v·V + w·N`.
#[derive(Clone, Copy, Debug)]
struct Frame {
    o: V3,
    u: V3,
    v: V3,
    n: V3,
}

impl Frame {
    fn at(&self, u: f64, v: f64, w: f64) -> V3 {
        add(add(add(self.o, mul(self.u, u)), mul(self.v, v)), mul(self.n, w))
    }
    fn uv(&self, p: V3) -> [f64; 2] {
        let d = sub(p, self.o);
        [dot(d, self.u), dot(d, self.v)]
    }
}

// ------------------------------------------------------------------ curves and loops

/// Points along a curve object (feet), start to end.
pub(crate) fn curve_points(c: &Obj, s: &Schema) -> Vec<V3> {
    let ep = c.fs("m_endParams").unwrap_or(&[]);
    let (t0, t1) = match ep {
        [a, b, ..] if a.is_finite() && b.is_finite() => (*a, *b),
        _ => return vec![],
    };
    match s.name(c.tag) {
        "GLine" => {
            let (Some(o), Some(d)) = (v3(c.fs("m_origin")), v3(c.fs("m_dirVec"))) else { return vec![] };
            vec![add(o, mul(d, t0)), add(o, mul(d, t1))]
        }
        "GArc" | "GEllipse" => {
            let (Some(ce), Some(x), Some(y)) = (v3(c.fs("m_center")), v3(c.fs("m_xVec")), v3(c.fs("m_yVec"))) else { return vec![] };
            let (rx, ry) = match s.name(c.tag) {
                "GArc" => {
                    let r = match c.get("m_radius") {
                        Some(G::F(r)) => *r,
                        _ => return vec![],
                    };
                    (r, r)
                }
                _ => match (c.get("m_xLen"), c.get("m_yLen")) {
                    (Some(G::F(a)), Some(G::F(b))) => (*a, *b),
                    _ => return vec![],
                },
            };
            let span = t1 - t0;
            if !(span.abs() < 64.0) {
                return vec![];
            }
            // 24 segments a turn, finer on large radii (chord within about 1 cm)
            let r = rx.abs().max(ry.abs());
            let step = if r > 0.03 { (std::f64::consts::PI / 12.0).min(2.0 * (1.0 - 0.03 / r).acos()) } else { std::f64::consts::PI / 12.0 };
            let n = ((span.abs() / step.max(std::f64::consts::PI / 96.0)).ceil() as usize).clamp(2, 192);
            (0..=n)
                .map(|i| {
                    let t = t0 + span * i as f64 / n as f64;
                    add(ce, add(mul(x, rx * t.cos()), mul(y, ry * t.sin())))
                })
                .collect()
        }
        "GNurbSpline" => c
            .list("m_ctrlPoints")
            .iter()
            .filter_map(|p| {
                let q = v3(p.fs("m_Point"))?;
                Some(q)
            })
            .collect(),
        _ => vec![],
    }
}

/// The curves of a `CurveLoop` chained into one closed polyline (feet).
fn loop_points(lp: &Obj, tr: &Tree, s: &Schema) -> Vec<V3> {
    let curves: Vec<Vec<V3>> = lp.ptrs("m_curves").iter().filter_map(|&i| tr.objs.get(i)).map(|c| curve_points(c, s)).filter(|p| p.len() >= 2).collect();
    chain(curves)
}

fn chain(mut curves: Vec<Vec<V3>>) -> Vec<V3> {
    if curves.is_empty() {
        return vec![];
    }
    let d2 = |a: V3, b: V3| dot(sub(a, b), sub(a, b));
    let mut out = curves.remove(0);
    // orient the first curve towards the next one
    if let Some(nx) = curves.first() {
        let (a, b) = (out[0], *out.last().unwrap());
        let m = |p: V3| d2(p, nx[0]).min(d2(p, *nx.last().unwrap()));
        if m(a) < m(b) {
            out.reverse();
        }
    }
    while !curves.is_empty() {
        let tail = *out.last().unwrap();
        // nearest next curve (normally the next one in order)
        let (k, rev) = curves
            .iter()
            .enumerate()
            .map(|(k, c)| {
                let (ds, de) = (d2(tail, c[0]), d2(tail, *c.last().unwrap()));
                (k, de < ds, ds.min(de))
            })
            .min_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(&b.0)))
            .map(|(k, r, _)| (k, r))
            .unwrap();
        let mut c = curves.remove(k);
        if rev {
            c.reverse();
        }
        out.extend(c.into_iter().skip(1));
    }
    if out.len() > 1 && d2(out[0], *out.last().unwrap()) < 1e-8 {
        out.pop();
    }
    out
}

// ------------------------------------------------------------------ region tessellation

/// `y = k·u + m` on a strip.
#[derive(Clone, Copy, Debug)]
struct Lin {
    k: f64,
    m: f64,
}
impl Lin {
    fn at(&self, u: f64) -> f64 {
        self.k * u + self.m
    }
}

const MAX_STRIPS: usize = 20_000;

/// Tessellates the region inside `loops` (even-odd) minus the rectangles `cuts`
/// (`[u0, v0, u1, v1]`), extruded from `w0` to `w1`: returns (u, v, w) points and triangles
/// oriented outwards (for a right-handed frame U, V, N).
fn extrude_region(loops: &[Vec<[f64; 2]>], cuts: &[[f64; 4]], w0: f64, w1: f64) -> (Vec<V3>, Vec<[u32; 3]>) {
    let (w0, w1) = if w1 < w0 { (w1, w0) } else { (w0, w1) };
    let mut pts: Vec<V3> = Vec::new();
    let mut tri: Vec<[u32; 3]> = Vec::new();
    let mut us: Vec<f64> = loops.iter().flatten().map(|p| p[0]).filter(|x| x.is_finite()).collect();
    for c in cuts {
        us.push(c[0]);
        us.push(c[2]);
    }
    us.sort_by(f64::total_cmp);
    us.dedup_by(|a, b| (*a - *b).abs() < 1e-7);
    if us.len() < 2 || us.len() > MAX_STRIPS {
        return (pts, tri);
    }
    let edges: Vec<([f64; 2], [f64; 2])> = loops
        .iter()
        .filter(|l| l.len() >= 3)
        .flat_map(|l| (0..l.len()).map(move |i| (l[i], l[(i + 1) % l.len()])))
        .filter(|(a, b)| (a[0] - b[0]).abs() > 1e-9)
        .map(|(a, b)| if a[0] < b[0] { (a, b) } else { (b, a) })
        .collect();
    let quad = |p: [V3; 4], pts: &mut Vec<V3>, tri: &mut Vec<[u32; 3]>| {
        let b = pts.len() as u32;
        pts.extend_from_slice(&p);
        tri.push([b, b + 1, b + 2]);
        tri.push([b, b + 2, b + 3]);
    };
    let mut prev: Vec<(Lin, Lin)> = Vec::new();
    let mut prev_u1 = us[0];
    for s in 0..us.len() {
        let (u0, u1) = (us[s], *us.get(s + 1).unwrap_or(&us[s]));
        let mut cur: Vec<(Lin, Lin)> = Vec::new();
        if s + 1 < us.len() {
            let um = (u0 + u1) * 0.5;
            let mut xs: Vec<Lin> = edges
                .iter()
                .filter(|(a, b)| a[0] < um && b[0] > um)
                .map(|(a, b)| {
                    let k = (b[1] - a[1]) / (b[0] - a[0]);
                    Lin { k, m: a[1] - k * a[0] }
                })
                .collect();
            xs.sort_by(|a, b| a.at(um).total_cmp(&b.at(um)));
            let mut iv: Vec<(Lin, Lin)> = xs.chunks_exact(2).map(|c| (c[0], c[1])).collect();
            for c in cuts.iter().filter(|c| c[0] <= um && c[2] >= um) {
                let (lo, hi) = (Lin { k: 0.0, m: c[1] }, Lin { k: 0.0, m: c[3] });
                let mut next = Vec::new();
                for (a, b) in iv {
                    let (ya, yb) = (a.at(um), b.at(um));
                    if c[3] <= ya || c[1] >= yb {
                        next.push((a, b));
                        continue;
                    }
                    if c[1] > ya {
                        next.push((a, lo));
                    }
                    if c[3] < yb {
                        next.push((hi, b));
                    }
                }
                iv = next;
            }
            cur = iv;
        }
        // vertical boundary at u0 between the previous strip and this one
        let ul = u0;
        let left: Vec<(f64, f64)> = if (prev_u1 - ul).abs() < 1e-9 { prev.iter().map(|(a, b)| (a.at(ul), b.at(ul))).collect() } else { vec![] };
        let right: Vec<(f64, f64)> = cur.iter().map(|(a, b)| (a.at(ul), b.at(ul))).collect();
        for (a, b) in diff(&left, &right) {
            // covered on the left only: outward +U, edge direction +V
            quad([[ul, a, w0], [ul, b, w0], [ul, b, w1], [ul, a, w1]], &mut pts, &mut tri);
        }
        for (a, b) in diff(&right, &left) {
            quad([[ul, b, w0], [ul, a, w0], [ul, a, w1], [ul, b, w1]], &mut pts, &mut tri);
        }
        if s + 1 >= us.len() {
            break;
        }
        for (lo, hi) in &cur {
            let (a0, a1, b0, b1) = (lo.at(u0), lo.at(u1), hi.at(u0), hi.at(u1));
            // caps
            quad([[u0, a0, w1], [u1, a1, w1], [u1, b1, w1], [u0, b0, w1]], &mut pts, &mut tri);
            quad([[u0, a0, w0], [u0, b0, w0], [u1, b1, w0], [u1, a1, w0]], &mut pts, &mut tri);
            // bottom edge (outward -V) and top edge (outward +V)
            quad([[u0, a0, w0], [u1, a1, w0], [u1, a1, w1], [u0, a0, w1]], &mut pts, &mut tri);
            quad([[u1, b1, w0], [u0, b0, w0], [u0, b0, w1], [u1, b1, w1]], &mut pts, &mut tri);
        }
        prev = cur;
        prev_u1 = u1;
    }
    // drop degenerate triangles (zero-height trapezoid ends)
    tri.retain(|t| {
        let (a, b, c) = (pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]);
        dot(cross(sub(b, a), sub(c, a)), cross(sub(b, a), sub(c, a))) > 1e-18
    });
    (pts, tri)
}

/// Parts of the union of intervals `a` not covered by the union of `b`.
fn diff(a: &[(f64, f64)], b: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for &(lo, hi) in a {
        let mut segs = vec![(lo.min(hi), lo.max(hi))];
        for &(c, d) in b {
            let (c, d) = (c.min(d), c.max(d));
            segs = segs
                .into_iter()
                .flat_map(|(x, y)| {
                    let mut r = Vec::new();
                    if d <= x || c >= y {
                        r.push((x, y));
                    } else {
                        if c > x {
                            r.push((x, c));
                        }
                        if d < y {
                            r.push((d, y));
                        }
                    }
                    r
                })
                .collect();
        }
        out.extend(segs.into_iter().filter(|(x, y)| y - x > 1e-7));
    }
    out
}

/// Region in frame `f` extruded between `w0` and `w1` (feet) as a mesh in metres.
fn solid(f: &Frame, loops: &[Vec<[f64; 2]>], cuts: &[[f64; 4]], w0: f64, w1: f64, trf: Option<&Trf>) -> Mesh {
    let (pts, tri) = extrude_region(loops, cuts, w0, w1);
    let mut m = Mesh { pos: Vec::with_capacity(pts.len()), tri };
    let flip = trf.is_some_and(|t| t.det() < 0.0);
    for p in pts {
        let mut q = f.at(p[0], p[1], p[2]);
        if let Some(t) = trf {
            q = t.apply(q);
        }
        m.pos.push([(q[0] * FEET) as f32, (q[1] * FEET) as f32, (q[2] * FEET) as f32]);
    }
    if flip {
        for t in &mut m.tri {
            t.swap(1, 2);
        }
    }
    m
}

// ------------------------------------------------------------------ swept solids

/// Closed loops and open chains formed by curves (feet) whose ends meet.
fn loops_from(curves: &[Vec<V3>]) -> (Vec<Vec<V3>>, Vec<Vec<V3>>) {
    let d2 = |a: V3, b: V3| dot(sub(a, b), sub(a, b));
    let tol = 1e-8;
    let mut left: Vec<Vec<V3>> = curves.iter().filter(|c| c.len() >= 2).cloned().collect();
    let (mut closed, mut open) = (Vec::new(), Vec::new());
    while !left.is_empty() {
        let mut cur = left.remove(0);
        loop {
            if cur.len() > 2 && d2(cur[0], *cur.last().unwrap()) < tol {
                break;
            }
            let tail = *cur.last().unwrap();
            let head = cur[0];
            if let Some(k) = left.iter().position(|c| d2(c[0], tail) < tol || d2(*c.last().unwrap(), tail) < tol) {
                let mut c = left.remove(k);
                if d2(c[0], tail) >= tol {
                    c.reverse();
                }
                cur.extend(c.into_iter().skip(1));
            } else if let Some(k) = left.iter().position(|c| d2(c[0], head) < tol || d2(*c.last().unwrap(), head) < tol) {
                let mut c = left.remove(k);
                if d2(*c.last().unwrap(), head) >= tol {
                    c.reverse();
                }
                c.pop();
                c.extend(cur);
                cur = c;
            } else {
                break;
            }
        }
        if cur.len() > 3 && d2(cur[0], *cur.last().unwrap()) < tol {
            cur.pop();
            closed.push(cur);
        } else {
            open.push(cur);
        }
    }
    (closed, open)
}

fn area2(l: &[[f64; 2]]) -> f64 {
    (0..l.len()).map(|i| {
        let (a, b) = (l[i], l[(i + 1) % l.len()]);
        a[0] * b[1] - b[0] * a[1]
    }).sum::<f64>() * 0.5
}

fn inside(p: [f64; 2], l: &[[f64; 2]]) -> bool {
    let mut c = false;
    for i in 0..l.len() {
        let (a, b) = (l[i], l[(i + 1) % l.len()]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]) {
            c = !c;
        }
    }
    c
}

/// Orients outer loops counter-clockwise and holes clockwise (even-odd nesting).
fn orient(mut loops: Vec<Vec<[f64; 2]>>) -> Vec<Vec<[f64; 2]>> {
    let depth: Vec<usize> = (0..loops.len()).map(|i| (0..loops.len()).filter(|&j| j != i && inside(loops[i][0], &loops[j])).count()).collect();
    for (l, d) in loops.iter_mut().zip(depth) {
        if (area2(l) > 0.0) != (d % 2 == 0) {
            l.reverse();
        }
    }
    loops
}

/// Region inside `loops` as triangles facing +N (counter-clockwise in u, v).
fn cap2d(loops: &[Vec<[f64; 2]>]) -> (Vec<[f64; 2]>, Vec<[u32; 3]>) {
    let (pts, tri) = extrude_region(loops, &[], 0.0, 1.0);
    let mut out = Vec::new();
    let mut ix: HashMap<usize, u32> = HashMap::new();
    let mut t2 = Vec::new();
    for t in tri {
        if t.iter().all(|&k| (pts[k as usize][2] - 1.0).abs() < 1e-12) {
            let mut q = [0u32; 3];
            for (n, &k) in t.iter().enumerate() {
                q[n] = *ix.entry(k as usize).or_insert_with(|| {
                    out.push([pts[k as usize][0], pts[k as usize][1]]);
                    (out.len() - 1) as u32
                });
            }
            t2.push(q);
        }
    }
    (out, t2)
}

/// Mesh (metres) from rings of loops (feet, one ring per station, loops with matching point
/// counts, counter-clockwise about the advance direction) and the end caps (`cap0` facing
/// backwards, `cap1` forwards). Turned inside out when its signed volume is negative.
fn assemble(rings: &[Vec<Vec<V3>>], wrap: bool, caps: [Option<(Vec<V3>, Vec<[u32; 3]>)>; 2]) -> Option<Mesh> {
    let mut pts: Vec<V3> = Vec::new();
    let mut tri: Vec<[u32; 3]> = Vec::new();
    let n = rings.len();
    let steps = if wrap { n } else { n.saturating_sub(1) };
    for i in 0..steps {
        let (a, b) = (&rings[i], &rings[(i + 1) % n]);
        for (la, lb) in a.iter().zip(b) {
            let m = la.len();
            if m != lb.len() {
                return None;
            }
            for j in 0..m {
                let k = (j + 1) % m;
                let base = pts.len() as u32;
                pts.extend_from_slice(&[la[j], la[k], lb[k], lb[j]]);
                tri.push([base, base + 1, base + 2]);
                tri.push([base, base + 2, base + 3]);
            }
        }
    }
    for (e, c) in caps.into_iter().enumerate() {
        if let Some((p, t)) = c {
            let base = pts.len() as u32;
            pts.extend(p);
            tri.extend(t.into_iter().map(|t| if e == 0 { [t[0] + base, t[2] + base, t[1] + base] } else { [t[0] + base, t[1] + base, t[2] + base] }));
        }
    }
    tri.retain(|t| {
        let (a, b, c) = (pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]);
        let x = cross(sub(b, a), sub(c, a));
        dot(x, x) > 1e-18
    });
    if tri.is_empty() || pts.iter().any(|p| !p.iter().all(|x| x.is_finite())) {
        return None;
    }
    let vol: f64 = tri.iter().map(|t| dot(pts[t[0] as usize], cross(pts[t[1] as usize], pts[t[2] as usize]))).sum();
    if vol < 0.0 {
        for t in &mut tri {
            t.swap(1, 2);
        }
    }
    Some(Mesh { pos: pts.iter().map(|q| [(q[0] * FEET) as f32, (q[1] * FEET) as f32, (q[2] * FEET) as f32]).collect(), tri })
}

/// Frame of a sketch: its plane, or the plane of its loops.
fn sketch_frame(plane: Option<(V3, V3, V3)>, loops: &[Vec<V3>]) -> Option<Frame> {
    let (o, x, y) = match plane {
        Some(p) => p,
        None => {
            let l = loops.first()?;
            let mut nn = [0.0; 3];
            for i in 0..l.len() {
                nn = add(nn, cross(l[i], l[(i + 1) % l.len()]));
            }
            let nz = norm(nn)?;
            let x = norm(sub(l[1], l[0]))?;
            (l[0], x, cross(nz, x))
        }
    };
    let u = norm(x)?;
    let n = norm(cross(x, y))?;
    let f = Frame { o, u, v: cross(n, u), n };
    // the loops must lie in it
    if loops.iter().flatten().any(|&p| dot(sub(p, o), n).abs() > 1e-3 * (1.0 + dot(sub(p, o), sub(p, o)).sqrt())) {
        return None;
    }
    Some(f)
}

/// Rotation of `p` by `a` radians about the line through `o` along the unit `d`.
fn rotate(p: V3, o: V3, d: V3, a: f64) -> V3 {
    let v = sub(p, o);
    let (c, s) = (a.cos(), a.sin());
    add(o, add(add(mul(v, c), mul(cross(d, v), s)), mul(d, dot(d, v) * (1.0 - c))))
}

/// Vector `v` turned by the rotation taking unit `a` to unit `b`.
fn turn(v: V3, a: V3, b: V3) -> V3 {
    let ax = cross(a, b);
    let s = dot(ax, ax).sqrt();
    let c = dot(a, b);
    if s < 1e-12 {
        return if c > 0.0 { v } else { mul(v, -1.0) };
    }
    rotate(v, [0.0; 3], mul(ax, 1.0 / s), s.atan2(c))
}

fn revolution(sk: &SketchData, a0: f64, a1: f64) -> Option<Mesh> {
    let (closed, open) = loops_from(&sk.curves);
    if closed.is_empty() {
        return None;
    }
    let axis = open.iter().find(|c| c.len() == 2)?;
    let (ao, ad) = (axis[0], norm(sub(axis[1], axis[0]))?);
    let f = sketch_frame(sk.plane, &closed)?;
    let prof = orient(closed.iter().map(|l| l.iter().map(|&p| f.uv(p)).collect()).collect());
    let span = a1 - a0;
    if !(span.abs() > 1e-4 && span.abs() <= std::f64::consts::TAU + 1e-6) {
        return None;
    }
    let full = span.abs() > std::f64::consts::TAU - 1e-4;
    let n = ((span.abs() / (std::f64::consts::PI / 12.0)).ceil() as usize).clamp(3, 32);
    let at = |i: usize, q: [f64; 2]| rotate(f.at(q[0], q[1], 0.0), ao, ad, a0 + span * i as f64 / n as f64);
    let count = if full { n } else { n + 1 };
    let rings: Vec<Vec<Vec<V3>>> = (0..count).map(|i| prof.iter().map(|l| l.iter().map(|&q| at(i, q)).collect()).collect()).collect();
    let caps = if full {
        [None, None]
    } else {
        let (cp, ct) = cap2d(&prof);
        [Some((cp.iter().map(|&q| at(0, q)).collect(), ct.clone())), Some((cp.iter().map(|&q| at(n, q)).collect(), ct))]
    };
    assemble(&rings, full, caps)
}

fn sweep(profile: &SketchData, path: &SketchData) -> Option<Mesh> {
    let (closed, _) = loops_from(&profile.curves);
    if closed.is_empty() {
        return None;
    }
    let f = sketch_frame(profile.plane, &closed)?;
    let (pc, po) = loops_from(&path.curves);
    let mut p: Vec<V3> = match (pc.first(), po.first()) {
        (_, Some(o)) if pc.is_empty() && po.len() == 1 => o.clone(),
        (Some(c), _) if po.is_empty() && pc.len() == 1 => {
            let mut c = c.clone();
            c.push(c[0]);
            c
        }
        _ => return None,
    };
    p.dedup_by(|a, b| dot(sub(*a, *b), sub(*a, *b)) < 1e-14);
    if p.len() < 2 || p.len() > 4096 {
        return None;
    }
    let seg: Vec<V3> = p.windows(2).map(|w| norm(sub(w[1], w[0]))).collect::<Option<_>>()?;
    // where the profile plane meets the path
    let side = |q: V3| dot(sub(q, f.o), f.n);
    let k = (0..seg.len()).find(|&k| side(p[k]) * side(p[k + 1]) <= 0.0).or_else(|| (0..seg.len()).min_by(|&a, &b| side(p[a]).abs().total_cmp(&side(p[b]).abs())))?;
    let (sa, sb) = (side(p[k]), side(p[k + 1]));
    let q = if (sa - sb).abs() > 1e-12 { add(p[k], mul(sub(p[k + 1], p[k]), sa / (sa - sb))) } else { p[k] };
    if dot(f.n, seg[k]).abs() < 0.9 {
        return None; // the profile is not across the path
    }
    let prof = orient(closed.iter().map(|l| l.iter().map(|&x| {
        let d = sub(x, q);
        [dot(d, f.u), dot(d, f.v)]
    }).collect()).collect());
    // profile axes on each segment, carried along by the turns of the path
    let mut ax = vec![(f.u, f.v); seg.len()];
    for j in k + 1..seg.len() {
        ax[j] = (turn(ax[j - 1].0, seg[j - 1], seg[j]), turn(ax[j - 1].1, seg[j - 1], seg[j]));
    }
    for j in (0..k).rev() {
        ax[j] = (turn(ax[j + 1].0, seg[j + 1], seg[j]), turn(ax[j + 1].1, seg[j + 1], seg[j]));
    }
    let n = p.len();
    let at = |i: usize, c: [f64; 2]| -> V3 {
        let s = if i == 0 { 0 } else { i - 1 };
        let (u, v) = ax[s];
        let x = add(p[i], add(mul(u, c[0]), mul(v, c[1])));
        if i > 0 && i + 1 < n {
            // mitre joint: onto the plane halfway between the two segments
            if let Some(t) = norm(add(seg[i - 1], seg[i])) {
                let c = dot(seg[i - 1], t);
                if c > 0.2 {
                    return sub(x, mul(seg[i - 1], dot(sub(x, p[i]), t) / c));
                }
            }
        }
        x
    };
    let rings: Vec<Vec<Vec<V3>>> = (0..n).map(|i| prof.iter().map(|l| l.iter().map(|&c| at(i, c)).collect()).collect()).collect();
    let (cp, ct) = cap2d(&prof);
    let caps = [Some((cp.iter().map(|&c| at(0, c)).collect(), ct.clone())), Some((cp.iter().map(|&c| at(n - 1, c)).collect(), ct))];
    assemble(&rings, false, caps)
}

/// Loop resampled to `n` points evenly along its length.
fn resample(l: &[V3], n: usize) -> Vec<V3> {
    let m = l.len();
    let len: Vec<f64> = (0..m).map(|i| dot(sub(l[(i + 1) % m], l[i]), sub(l[(i + 1) % m], l[i])).sqrt()).collect();
    let total: f64 = len.iter().sum();
    let mut out = Vec::with_capacity(n);
    let (mut i, mut acc) = (0usize, 0.0);
    for k in 0..n {
        let t = total * k as f64 / n as f64;
        while i < m - 1 && acc + len[i] < t {
            acc += len[i];
            i += 1;
        }
        let r = if len[i] > 0.0 { ((t - acc) / len[i]).clamp(0.0, 1.0) } else { 0.0 };
        out.push(add(l[i], mul(sub(l[(i + 1) % m], l[i]), r)));
    }
    out
}

fn blend(a: &SketchData, b: &SketchData, first: f64, second: f64) -> Option<Mesh> {
    let (la, _) = loops_from(&a.curves);
    let (lb, _) = loops_from(&b.curves);
    if la.len() != 1 || lb.len() != 1 {
        return None;
    }
    let f = sketch_frame(a.plane, &la)?;
    let fb = sketch_frame(b.plane, &lb)?;
    if dot(f.n, fb.n).abs() < 0.999 {
        return None;
    }
    let ccw = |l: &Vec<V3>| -> Vec<V3> {
        let mut l = l.clone();
        if area2(&l.iter().map(|&p| f.uv(p)).collect::<Vec<_>>()) < 0.0 {
            l.reverse();
        }
        l
    };
    let n = la[0].len().max(lb[0].len()).clamp(12, 96);
    let ra: Vec<V3> = resample(&ccw(&la[0]), n).into_iter().map(|p| add(p, mul(f.n, first))).collect();
    let rb0: Vec<V3> = resample(&ccw(&lb[0]), n).into_iter().map(|p| add(p, mul(f.n, second))).collect();
    // start of the second loop: the rotation closest to the first loop
    let ua: Vec<[f64; 2]> = ra.iter().map(|&p| f.uv(p)).collect();
    let ub: Vec<[f64; 2]> = rb0.iter().map(|&p| f.uv(p)).collect();
    let cost = |k: usize| (0..n).map(|j| {
        let (p, q) = (ua[j], ub[(j + k) % n]);
        (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)
    }).sum::<f64>();
    let k = (0..n).min_by(|&x, &y| cost(x).total_cmp(&cost(y)))?;
    let rb: Vec<V3> = (0..n).map(|j| rb0[(j + k) % n]).collect();
    let cap = |l: &[V3]| {
        let uv: Vec<[f64; 2]> = l.iter().map(|&p| f.uv(p)).collect();
        let w = dot(sub(l[0], f.o), f.n);
        let (cp, ct) = cap2d(&[uv]);
        (cp.iter().map(|c| f.at(c[0], c[1], w)).collect::<Vec<V3>>(), ct)
    };
    let (c0, c1) = (cap(&ra), cap(&rb));
    assemble(&[vec![ra], vec![rb]], false, [Some(c0), Some(c1)])
}

// ------------------------------------------------------------------ transforms

/// Affine transform: `p' = M·p + or`, the columns of M being the images of X, Y, Z.
#[derive(Clone, Copy, Debug)]
pub struct Trf {
    m: [f64; 9],
    or: V3,
}

impl Trf {
    /// Rotation-scale part (columns: images of x, y, z) and origin.
    pub(crate) fn new(m: [f64; 9], or: V3) -> Trf {
        Trf { m, or }
    }
    pub(crate) fn from(o: &Obj) -> Option<Trf> {
        let m = o.fs("m_3x3")?;
        let or = v3(o.fs("m_or"))?;
        (m.len() == 9 && m.iter().all(|x| x.is_finite())).then(|| Trf { m: m.try_into().unwrap(), or })
    }
    pub(crate) fn apply(&self, p: V3) -> V3 {
        let m = &self.m;
        [m[0] * p[0] + m[3] * p[1] + m[6] * p[2] + self.or[0], m[1] * p[0] + m[4] * p[1] + m[7] * p[2] + self.or[1], m[2] * p[0] + m[5] * p[1] + m[8] * p[2] + self.or[2]]
    }
    pub(crate) fn lin(&self, p: V3) -> V3 {
        let m = &self.m;
        [m[0] * p[0] + m[3] * p[1] + m[6] * p[2], m[1] * p[0] + m[4] * p[1] + m[7] * p[2], m[2] * p[0] + m[5] * p[1] + m[8] * p[2]]
    }
    /// Rotation part made of 0 and ±1 only (axis permutations and flips).
    fn axis_aligned(&self) -> bool {
        self.m.iter().all(|x| x.abs() < 1e-6 || (x.abs() - 1.0).abs() < 1e-6)
    }
    fn det(&self) -> f64 {
        let m = &self.m;
        dot([m[0], m[1], m[2]], cross([m[3], m[4], m[5]], [m[6], m[7], m[8]]))
    }
}

// ------------------------------------------------------------------ stored BRep

/// Mesh (metres) of the boundary representation an element stores (`Geometry`:
/// `Face` → `EdgeLoop` ring of `Edge`s, each edge holding its end points in the face's
/// surface parameters, per side): the planar faces, triangulated in their plane.
/// Non-planar faces are left out.
pub(crate) fn stored_brep(s: &Schema, tr: &Tree) -> Option<Mesh> {
    brep_faces(s, tr, &mut (0..tr.objs.len()))
}

/// [`stored_brep`] of the faces of one `Geometry` object (tree index `g`).
fn brep_of(s: &Schema, tr: &Tree, g: usize) -> Option<Mesh> {
    brep_faces(s, tr, &mut tr.objs.get(g)?.ptrs("m_pFaces").into_iter())
}

fn brep_faces(s: &Schema, tr: &Tree, faces: &mut dyn Iterator<Item = usize>) -> Option<Mesh> {
    let lid: HashMap<usize, u32> = tr.local.iter().map(|(&l, &i)| (i, l)).collect();
    let by_local = |l: u32| tr.local.get(&l).and_then(|&i| tr.objs.get(i));
    let rref = |o: &Obj, k: &str| match o.get(k) {
        Some(G::R(v)) => Some(*v),
        _ => None,
    };
    let ints = |o: &Obj, k: &str| match o.get(k) {
        Some(G::Is(v)) => v.clone(),
        _ => vec![],
    };
    let mut m = Mesh::default();
    for fi in faces {
        let Some(f) = tr.objs.get(fi) else { continue };
        if s.name(f.tag) != "Face" {
            continue;
        }
        let Some(fl) = lid.get(&fi).copied() else { continue };
        let Some(sf) = f.ptr("m_pSurf").and_then(|k| tr.objs.get(k)) else { continue };
        if s.name(sf.tag) != "Plane" {
            continue;
        }
        let (Some(o), Some(x), Some(y)) = (v3(sf.fs("m_origin")), v3(sf.fs("m_xVec")), v3(sf.fs("m_yVec"))) else { continue };
        let mut loops: Vec<Vec<[f64; 2]>> = Vec::new();
        let mut lp = f.ptr("m_pFirstLoop");
        let mut nl = 0;
        while let Some(li) = lp {
            nl += 1;
            if nl > 256 {
                break;
            }
            let Some(l) = tr.objs.get(li) else { break };
            lp = l.ptr("m_nextLoop");
            let Some(ll) = lid.get(&li).copied() else { continue };
            let mut ring: Vec<[f64; 2]> = Vec::new();
            let mut e = rref(l, "m_next");
            let mut n = 0;
            while let Some(el) = e {
                if el == ll || n > 4096 {
                    break;
                }
                n += 1;
                let Some(ed) = by_local(el) else { break };
                if s.name(ed.tag) != "Edge" {
                    break;
                }
                let side = ints(ed, "m_pFace").iter().position(|&v| v == fl as i64).unwrap_or(0);
                let ends = ed.list("m_firstAndLastEdgePnts");
                let uv = |p: &Obj| p.fs("uv").filter(|u| u.len() >= 2 * side + 2).map(|u| [u[2 * side], u[2 * side + 1]]);
                if let Some(a) = ends.first().and_then(uv) {
                    ring.push(a);
                }
                for p in ed.list("m_interiorEdgePnts") {
                    if let Some(a) = uv(p) {
                        ring.push(a);
                    }
                }
                e = ints(ed, "m_next").get(side).map(|&v| v as u32);
            }
            ring.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9);
            if ring.len() >= 3 && ring.iter().all(|p| p[0].is_finite() && p[1].is_finite()) {
                loops.push(ring);
            }
        }
        if loops.is_empty() {
            continue;
        }
        let (pts, tri) = cap2d(&orient(loops));
        let base = m.pos.len() as u32;
        for p in &pts {
            let q = add(o, add(mul(x, p[0]), mul(y, p[1])));
            m.pos.push([(q[0] * FEET) as f32, (q[1] * FEET) as f32, (q[2] * FEET) as f32]);
        }
        m.tri.extend(tri.iter().map(|t| [t[0] + base, t[1] + base, t[2] + base]));
    }
    (!m.tri.is_empty()).then_some(m)
}

// ------------------------------------------------------------------ collection

struct Wall {
    id: i64,
    o: V3,
    d: V3,
    /// Location line parameters (along `d` from `o`).
    ends: (f64, f64),
    /// Edited elevation profile (none: a rectangle between the bottom and top faces).
    loops: Vec<Vec<V3>>,
    planes: Vec<(V3, V3, V3, [f64; 4])>,
    cutters: Vec<i64>,
    #[allow(dead_code)]
    curtain: bool,
}

struct Slab {
    id: i64,
    loops: Vec<Vec<V3>>,
    range: Option<(f64, f64)>,
    planes: Vec<(V3, V3, V3, [f64; 4])>,
}

/// `m_famElemVisibility.m_flags` of a family form (-1: none)
fn vis_flags(tr: &Tree) -> i64 {
    tr.root.obj("m_famElemVisibility").and_then(|o| o.int("m_flags")).unwrap_or(-1)
}

/// A family form not part of the loaded family: hidden by its `Visible` parameter
/// (-1006205 = 0 for the type stored), or the sample host of a hosted family template
/// (the extrusion that the family's voids cut in the family editor).
fn not_in_family(tr: &Tree, s: &Schema) -> bool {
    let hidden = tr.root.ptr("m_pParamValueSetInt").and_then(|i| tr.objs.get(i)).is_some_and(|set| {
        set.list("m_paramSet").iter().any(|p| p.int("m_paramId") == Some(-1006205) && p.int("m_value") == Some(0))
    });
    hidden || tr.objs.iter().any(|o| s.name(o.tag) == "AddGeomToHostInFamEnvGStep")
}

struct Form {
    id: i64,
    fam: i64,
    vis: i64,
    sketch: i64,
    loops: Vec<Vec<V3>>,
    start: f64,
    end: f64,
}

/// The other family solids: their sketches are read from the sketch elements (`m_userId`).
enum Kind {
    /// Profile sketch revolved about the line of the sketch not in a closed loop.
    Revolution { sketch: i64, a0: f64, a1: f64 },
    /// Profile sketch swept along the path sketch.
    Sweep { profile: i64, path: i64 },
    /// Two sketches of one form, offset along their normal, joined by ruled faces.
    Blend { first: f64, second: f64 },
}

struct Form2 {
    id: i64,
    fam: i64,
    vis: i64,
    kind: Kind,
}

/// A sketch: its plane and its curves (feet).
struct SketchData {
    user: i64,
    plane: Option<(V3, V3, V3)>,
    curves: Vec<Vec<V3>>,
}

/// Collects the data of the elements with a rebuilt shape, partition by partition.
#[derive(Default)]
pub struct Shapes {
    walls: Vec<Wall>,
    slabs: Vec<Slab>,
    forms: Vec<Form>,
    sketch_planes: HashMap<i64, (V3, V3, V3)>,
    sketches: HashMap<i64, SketchData>,
    forms2: Vec<Form2>,
    /// project or nested `Family` element → content document GUID (`m_oFamDoc`)
    fam_guid: HashMap<i64, [u8; 16]>,
    /// content document GUID → id of its root element (the `m_famId` of its elements)
    content: HashMap<[u8; 16], i64>,
    /// project `Family` element → ids of its family document's elements (`m_big2SmallMap2`)
    fam_small: HashMap<i64, Vec<i64>>,
    /// instance id → (symbol id, own family doc id if nested, transform)
    instances: Vec<(i64, i64, i64, Trf)>,
    /// symbol id → (m_famId, m_familyId)
    symbols: HashMap<i64, (i64, i64)>,
    /// Type width and height (feet) of the family symbols that have them (doors, windows).
    sym_size: HashMap<i64, (f64, f64)>,
    /// Meshes of the boundary representations stored by stairs, landings, parts and roofs.
    breps: Vec<(i64, Mesh)>,
    /// Curved walls: id, location curve points (feet), half thickness, reference planes.
    curved_walls: Vec<(i64, Vec<V3>, f64, Vec<(V3, V3, V3, [f64; 4])>)>,
    mep: Vec<Mep>,
    /// curve-driven instance (beam, brace) → its location line (start, end), feet
    framing: Vec<(i64, V3, V3)>,
    /// curtain mullions and panels → their placement, for a box in their own axes
    curtain: Vec<(i64, Trf)>,
}

/// A straight pipe, duct, cable tray or conduit: a section swept along its centre line.
struct Mep {
    id: i64,
    a: V3,
    b: V3,
    /// Width (or diameter) and height, feet.
    w: f64,
    h: f64,
    /// Direction of the height.
    up: V3,
    round: bool,
}

fn planes_of(tr: &Tree, field: &str) -> Vec<(V3, V3, V3, [f64; 4])> {
    tr.root
        .ptrs(field)
        .iter()
        .filter_map(|&i| tr.objs.get(i))
        .filter_map(|f| f.ptr("m_pSurf").and_then(|i| tr.objs.get(i)))
        .filter_map(|p| {
            let env = p.obj("m_Envelope").and_then(|e| e.fs("m_corners")).unwrap_or(&[]);
            let env: [f64; 4] = if env.len() == 4 { env.try_into().unwrap() } else { [f64::NAN; 4] };
            Some((v3(p.fs("m_origin"))?, v3(p.fs("m_xVec"))?, v3(p.fs("m_yVec"))?, env))
        })
        .collect()
}

fn helper_loops(tr: &Tree, s: &Schema, helper: &str) -> Vec<Vec<V3>> {
    let Some(h) = tr.objs.iter().find(|o| s.name(o.tag).ends_with(helper)) else { return vec![] };
    h.ptrs("m_pCurveLoops").iter().filter_map(|&i| tr.objs.get(i)).map(|lp| loop_points(lp, tr, s)).filter(|l| l.len() >= 3).collect()
}

/// Pipe or duct solid: its section (a circle of 16 sides, or a rectangle) along the centre line.
fn mep(m: &Mep) -> Option<Mesh> {
    let d = sub(m.b, m.a);
    let len = dot(d, d).sqrt();
    let n = norm(d)?;
    if len < 1e-4 {
        return None;
    }
    // height direction square to the line
    let up = norm(sub(m.up, [n[0] * dot(m.up, n), n[1] * dot(m.up, n), n[2] * dot(m.up, n)])).or_else(|| norm(cross(n, if n[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] })))?;
    let side = norm(cross(up, n))?;
    let f = Frame { o: m.a, u: side, v: up, n };
    let sec: Vec<[f64; 2]> = if m.round {
        (0..16).map(|k| {
            let a = k as f64 * std::f64::consts::TAU / 16.0;
            [m.w * 0.5 * a.cos(), m.w * 0.5 * a.sin()]
        }).collect()
    } else {
        let (a, b) = (m.w * 0.5, m.h * 0.5);
        vec![[-a, -b], [a, -b], [a, b], [-a, b]]
    };
    let out = solid(&f, &[sec], &[], 0.0, len, None);
    (!out.tri.is_empty()).then_some(out)
}

/// A box along a beam's location line `a`→`b` (feet), its section solved from the element's
/// world box (`lo`, `hi`, metres): each world extent is the sum of the box edges projected on it.
fn framing_box(a: V3, b: V3, lo: [f64; 3], hi: [f64; 3]) -> Option<Mesh> {
    let lo = mul(lo, 1.0 / FEET);
    let hi = mul(hi, 1.0 / FEET);
    let d = sub(b, a);
    let len = dot(d, d).sqrt();
    let n = norm(d)?;
    let y = norm(cross([0.0, 0.0, 1.0], n)).unwrap_or([1.0, 0.0, 0.0]);
    let z = norm(cross(n, y))?;
    let e = sub(hi, lo);
    // least squares for (w, h): e_i - len |n_i| = w |y_i| + h |z_i|
    let r: Vec<f64> = (0..3).map(|i| e[i] - len * n[i].abs()).collect();
    let (ya, za) = ([y[0].abs(), y[1].abs(), y[2].abs()], [z[0].abs(), z[1].abs(), z[2].abs()]);
    let (s11, s12, s22) = (dot(ya, ya), dot(ya, za), dot(za, za));
    let (b1, b2) = ((0..3).map(|i| ya[i] * r[i]).sum::<f64>(), (0..3).map(|i| za[i] * r[i]).sum::<f64>());
    let det = s11 * s22 - s12 * s12;
    if det.abs() < 1e-9 {
        return None;
    }
    let w = (b1 * s22 - b2 * s12) / det;
    let h = (s11 * b2 - s12 * b1) / det;
    if !(w > 0.01 && h > 0.01) || w > 10.0 || h > 10.0 {
        return None;
    }
    // the section is centred on the world box (justification offsets), the length on the line
    let c = mul(add(lo, hi), 0.5);
    let mid = mul(add(a, b), 0.5);
    let off = sub(c, mid);
    let off = sub(off, mul(n, dot(off, n)));
    let m = Mep { id: 0, a: add(a, off), b: add(b, off), w, h, up: z, round: false };
    mep(&m)
}

/// A box in the axes of the placement `t` whose world box is (`lo`, `hi`, metres): its edges
/// solve `e_i = sum_j |u_j[i]| l_j`. For mullions and curtain panels, whose solids are boxes.
fn oriented_box(t: &Trf, lo: [f64; 3], hi: [f64; 3]) -> Option<Mesh> {
    let e = mul(sub(hi, lo), 1.0 / FEET);
    let u = [norm(t.lin([1.0, 0.0, 0.0]))?, norm(t.lin([0.0, 1.0, 0.0]))?, norm(t.lin([0.0, 0.0, 1.0]))?];
    let a = |i: usize, j: usize| u[j][i].abs();
    let det3 = |c: [V3; 3]| dot(c[0], cross(c[1], c[2]));
    let cols = [[a(0, 0), a(1, 0), a(2, 0)], [a(0, 1), a(1, 1), a(2, 1)], [a(0, 2), a(1, 2), a(2, 2)]];
    let d = det3(cols);
    if d.abs() < 1e-3 {
        return None;
    }
    let mut l = [0.0; 3];
    for j in 0..3 {
        let mut c = cols;
        c[j] = e;
        l[j] = det3(c) / d;
        if !(l[j] > 1e-3) {
            return None;
        }
    }
    let c = mul(add(lo, hi), 0.5 / FEET);
    let m = Mep { id: 0, a: sub(c, mul(u[0], l[0] * 0.5)), b: add(c, mul(u[0], l[0] * 0.5)), w: l[1], h: l[2], up: u[2], round: false };
    mep(&m)
}

/// Appends `m` (metres, family frame) placed by `t` (feet).
fn append(out: &mut Mesh, m: &Mesh, t: &Trf) {
    if m.tri.is_empty() {
        return;
    }
    let flip = t.det() < 0.0;
    let base = out.pos.len() as u32;
    for p in &m.pos {
        let q = t.apply([p[0] as f64 / FEET, p[1] as f64 / FEET, p[2] as f64 / FEET]);
        out.pos.push([(q[0] * FEET) as f32, (q[1] * FEET) as f32, (q[2] * FEET) as f32]);
    }
    out.tri.extend(m.tri.iter().map(|tr| if flip { [tr[0] + base, tr[2] + base, tr[1] + base] } else { [tr[0] + base, tr[1] + base, tr[2] + base] }));
}

/// A wall along the polyline `path` (feet, its z ignored): the band `±half` about it in
/// plan, from `z0` to `z1` (feet). Mesh in metres.
fn swept_wall(path: &[V3], half: f64, z0: f64, z1: f64) -> Option<Mesh> {
    let n = path.len();
    if n < 2 || !(z1 > z0) {
        return None;
    }
    // offset directions at the vertices (mitred)
    let mut sides: Vec<([f64; 2], [f64; 2])> = Vec::with_capacity(n);
    for i in 0..n {
        let a = path[i.saturating_sub(1)];
        let b = path[(i + 1).min(n - 1)];
        let t = norm([b[0] - a[0], b[1] - a[1], 0.0])?;
        let nn = [-t[1], t[0]];
        let p = [path[i][0], path[i][1]];
        sides.push(([p[0] - nn[0] * half, p[1] - nn[1] * half], [p[0] + nn[0] * half, p[1] + nn[1] * half]));
    }
    let mut m = Mesh::default();
    let mut push = |q: [f64; 2], z: f64| -> u32 {
        m.pos.push([(q[0] * FEET) as f32, (q[1] * FEET) as f32, (z * FEET) as f32]);
        (m.pos.len() - 1) as u32
    };
    // per vertex: inner bottom, inner top, outer bottom, outer top
    let ids: Vec<[u32; 4]> = sides.iter().map(|&(a, b)| [push(a, z0), push(a, z1), push(b, z0), push(b, z1)]).collect();
    for i in 0..n - 1 {
        let (p, q) = (ids[i], ids[i + 1]);
        for (a, b) in [(0, 1), (2, 3), (0, 2), (1, 3)] {
            m.tri.push([p[a], q[a], q[b]]);
            m.tri.push([p[a], q[b], p[b]]);
        }
    }
    for e in [ids[0], ids[n - 1]] {
        m.tri.push([e[0], e[1], e[3]]);
        m.tri.push([e[0], e[3], e[2]]);
    }
    Some(m)
}

/// The family solids stretched along the family x (about the middle of the solids) to
/// width `w` and along z (from their bottom) to height `h` (metres), placed by `t`, when
/// the result lies within the instance box `b` (± `tol`).
fn sized(parts: &[Mesh], pb: &[Option<([f32; 3], [f32; 3])>], w: f64, h: f64, t: &Trf, b: ([f64; 3], [f64; 3]), tol: f64) -> Option<Mesh> {
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for (l, u) in pb.iter().flatten() {
        for a in 0..3 {
            lo[a] = lo[a].min(l[a]);
            hi[a] = hi[a].max(u[a]);
        }
    }
    let (ex, ez) = ((hi[0] - lo[0]) as f64, (hi[2] - lo[2]) as f64);
    if ex < 0.05 || ez < 0.05 {
        return None;
    }
    let (sx, sz) = (w / ex, h / ez);
    if !(0.25..=4.0).contains(&sx) || !(0.25..=4.0).contains(&sz) {
        return None;
    }
    let cx = (lo[0] + hi[0]) * 0.5;
    let mut m = Mesh::default();
    for p in parts {
        let mut q = p.clone();
        for v in &mut q.pos {
            v[0] = cx + (v[0] - cx) * sx as f32;
            v[2] = lo[2] + (v[2] - lo[2]) * sz as f32;
        }
        append(&mut m, &q, t);
    }
    let (ml, mh) = m.bbox()?;
    let inside = (0..3).all(|a| ml[a] as f64 >= b.0[a] - tol && mh[a] as f64 <= b.1[a] + tol);
    inside.then_some(m)
}

fn param(tr: &Tree, id: i64) -> Option<f64> {
    let set = tr.objs.get(tr.root.ptr("m_pParamValueSetDouble")?)?;
    set.list("m_paramSet").iter().find(|p| p.int("m_paramId") == Some(id)).and_then(|p| match p.get("m_value") {
        Some(G::F(v)) if v.is_finite() => Some(*v),
        _ => None,
    })
}

impl Shapes {
    /// Element classes read.
    const CLASSES: [&'static str; 21] = [
        "StairsRun", "StairsLanding", "PartMaker", "ProfileRoof", "SWall", "Floor", "Ceiling", "BuildingPad", "RoofSoffit", "Toposolid", "ExtrusionElem", "RevolutionElem", "SweepElem", "BlendElem", "Family", "FamilyInstance", "FamilySymbol", "VarSketch", "Sketch", "SketchPlane", "Path3d",
    ];

    /// Element classes read, by tag.
    pub fn wants(s: &Schema) -> Vec<bool> {
        let mut want = vec![false; 65536];
        for c in Self::CLASSES {
            if let Some(t) = s.tag_of(c) {
                want[t as usize] = true;
            }
        }
        for &t in s.classes.keys() {
            if s.isa(t, "RbsSingleCurve") {
                want[t as usize] = true;
            }
        }
        want
    }

    /// One decoded element (of a class in [`Self::wants`]).
    pub fn take(&mut self, s: &Schema, t: u16, tr: &Tree) {
        if s.isa(t, "RbsSingleCurve") {
            self.mep_curve(s, s.name(t), tr);
        } else {
            self.element(s, s.name(t), tr)
        }
    }

    pub fn partition(&mut self, s: &Schema, d: &[u8], w: usize) {
        let want = Self::wants(s);
        elements(s, d, w, &|t| want[t as usize], |_, t, _, tr| self.take(s, t, &tr));
    }

    /// `/Global/ContentDocuments`: one segment per family document stored in the project
    /// (`ContentMarker`, `ContentKey` GUID, size), whose root element id (the `m_famId` of the
    /// document's elements) follows the `NOBLE_SecondaryDataStorage` tag:
    /// `[tag] [root id] [-1] 01 00 00 00`.
    pub fn content_docs(&mut self, s: &Schema, d: &[u8]) {
        let (Some(cm), Some(ck), Some(ns)) = (s.tag_of("ContentMarker"), s.tag_of("ContentKey"), s.tag_of("NOBLE_SecondaryDataStorage")) else { return };
        let mut head = cm.to_le_bytes().to_vec();
        head.extend([0xff; 4]);
        head.extend(ck.to_le_bytes());
        head.extend([0xff; 4]);
        let starts: Vec<usize> = d.windows(head.len()).enumerate().filter(|(_, w)| *w == &head[..]).map(|(i, _)| i).collect();
        let tag = ns.to_le_bytes();
        for (n, &p) in starts.iter().enumerate() {
            let end = starts.get(n + 1).copied().unwrap_or(d.len());
            if p + 28 > end {
                continue;
            }
            let guid: [u8; 16] = d[p + 12..p + 28].try_into().unwrap();
            let seg = &d[p + 28..end];
            let root = seg.windows(2).enumerate().filter(|(_, w)| *w == tag).find_map(|(k, _)| {
                [8usize, 4].into_iter().find_map(|w| {
                    let a = k + 2;
                    let b = seg.get(a..a + 2 * w + 4)?;
                    if !b[w..2 * w].iter().all(|&x| x == 0xff) || b[2 * w..] != [1, 0, 0, 0] {
                        return None;
                    }
                    let mut v = [0u8; 8];
                    v[..w].copy_from_slice(&b[..w]);
                    let id = i64::from_le_bytes(v);
                    (id > 0 && id < 1 << 40).then_some(id)
                })
            });
            if let Some(r) = root {
                self.content.insert(guid, r);
            }
        }
    }

    fn mep_curve(&mut self, s: &Schema, cls: &str, tr: &Tree) {
        let r = &tr.root;
        let num = |k: &str| match r.get(k) {
            Some(G::F(v)) if v.is_finite() && *v > 1e-4 && *v < 100.0 => Some(*v),
            _ => None,
        };
        let Some(w) = num("m_dWidthOrDiameter") else { return };
        let h = num("m_dHeight").unwrap_or(w);
        let Some(c) = r.ptr("m_pCurveDriver").and_then(|i| tr.objs.get(i)).and_then(|d| d.ptr("m_pCrv")).and_then(|i| tr.objs.get(i)) else { return };
        if s.name(c.tag) != "GLine" {
            return;
        }
        let (Some(o), Some(d)) = (v3(c.fs("m_origin")), v3(c.fs("m_dirVec"))) else { return };
        let Some([t0, t1, ..]) = c.fs("m_endParams") else { return };
        let Some(d) = norm(d) else { return };
        let up = v3(r.fs("m_vNormal")).and_then(norm).unwrap_or([0.0, 0.0, 1.0]);
        // round: pipes and conduits; ducts when their size reads as a diameter
        let size = tr.objs.iter().find(|o| s.name(o.tag).ends_with("DesignPropertyManager")).and_then(|o| match o.get("m_sOverallSize") {
            Some(G::S(t)) => Some(t.clone()),
            _ => None,
        });
        let round = cls.contains("Pipe") || (cls.contains("Conduit") && !cls.contains("CableTray")) || size.is_some_and(|t| t.contains(['ø', 'Ø', '⌀']));
        let at = |t: f64| [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t];
        self.mep.push(Mep { id: tr.id, a: at(*t0), b: at(*t1), w, h, up, round });
    }

    fn element(&mut self, s: &Schema, cls: &str, tr: &Tree) {
        let id = tr.id;
        match cls {
            "PartMaker" => {
                // one part per DPartData: its id and its stored geometry
                for d in tr.objs.iter().filter(|o| s.name(o.tag) == "DPartData") {
                    if let (Some(pid), Some(g)) = (d.int("m_dPartId"), d.ptr("m_oGeometry")) {
                        if let Some(m) = brep_of(s, tr, g) {
                            self.breps.push((pid, m));
                        }
                    }
                }
            }
            "StairsRun" | "StairsLanding" | "ProfileRoof" => {
                if let Some(m) = stored_brep(s, tr) {
                    self.breps.push((id, m));
                }
            }
            "SWall" => {
                let Some(dr) = tr.objs.iter().find(|o| s.name(o.tag) == "VWallDriver") else { return };
                let Some(c) = dr.ptr("m_pCrv").and_then(|i| tr.objs.get(i)) else { return };
                if s.name(c.tag) != "GLine" {
                    // curved wall: its location curve swept by the wall's thickness (twice the
                    // key reference offset, about the curve) between its bottom and top faces
                    let path = curve_points(c, s);
                    let half = match tr.root.get("m_keyRefOffset") {
                        Some(G::F(k)) if k.is_finite() && *k > 1e-3 && *k < 10.0 => *k,
                        _ => 0.25,
                    };
                    if path.len() >= 2 {
                        self.curved_walls.push((id, path, half, planes_of(&tr, "m_pRefFaces")));
                    }
                    return;
                }
                let (Some(o), Some(dv)) = (v3(c.fs("m_origin")), v3(c.fs("m_dirVec"))) else { return };
                let ends = match c.fs("m_endParams") {
                    Some([a, b, ..]) if a.is_finite() && b.is_finite() => (*a, *b),
                    _ => return,
                };
                let loops = helper_loops(&tr, s, "VWallExtrusionHelper");
                let cutters = tr
                    .objs
                    .iter()
                    .filter(|o| matches!(s.name(o.tag), "WallCutoutGStep" | "WallEmbedWallGStep"))
                    .filter_map(|o| o.int("m_instId"))
                    .filter(|&i| i > 0)
                    .collect();
                let curtain = tr.objs.iter().any(|o| s.name(o.tag).contains("Curtain") || s.name(o.tag).contains("Mullion"));
                self.walls.push(Wall { id, o, d: dv, ends, loops, planes: planes_of(&tr, "m_pRefFaces"), cutters, curtain });
            }
            "Floor" | "Ceiling" | "BuildingPad" | "RoofSoffit" | "Toposolid" => {
                let loops = helper_loops(&tr, s, "ExtrusionHelper");
                let range = match (tr.root.get("m_bottom"), tr.root.get("m_top")) {
                    (Some(G::F(b)), Some(G::F(t))) if b.is_finite() && t.is_finite() && t - b > 1e-6 && (t - b) < 1e3 => Some((*b, *t)),
                    _ => None,
                };
                if !loops.is_empty() {
                    self.slabs.push(Slab { id, loops, range, planes: planes_of(&tr, "m_pRefFaces") });
                }
            }
            "ExtrusionElem" => {
                if tr.root.int("m_cutting").unwrap_or(0) != 0 || not_in_family(tr, s) {
                    return;
                }
                let Some(h) = tr.objs.iter().find(|o| s.name(o.tag) == "ExtrusionElemExtrusionHelper") else { return };
                let loops = helper_loops(&tr, s, "ExtrusionElemExtrusionHelper");
                if loops.is_empty() {
                    return;
                }
                let start = param(&tr, -1001800).unwrap_or(0.0);
                let end = param(&tr, -1001801).unwrap_or(1.0);
                self.forms.push(Form { id, fam: tr.root.int("m_famId").unwrap_or(-1), vis: vis_flags(tr), sketch: h.int("m_sketchId").unwrap_or(-1), loops, start, end });
            }
            "VarSketch" | "Sketch" => {
                let mut plane = None;
                if let Some(p) = tr.root.ptr("m_pPlane").and_then(|i| tr.objs.get(i)) {
                    if let (Some(o), Some(x), Some(y)) = (v3(p.fs("m_origin")), v3(p.fs("m_xVec")), v3(p.fs("m_yVec"))) {
                        self.sketch_planes.insert(id, (o, x, y));
                        plane = Some((o, x, y));
                    }
                }
                let user = tr.root.int("m_userId").unwrap_or(-1);
                if user > 0 {
                    let curves = tr.root.ptrs("m_absorbedCurves").iter().filter_map(|&i| tr.objs.get(i)).map(|c| curve_points(c, s)).filter(|p| p.len() >= 2).collect();
                    self.sketches.insert(id, SketchData { user, plane, curves });
                }
            }
            "Path3d" => {
                // 3D sweep path: open or closed curve loops, kept like a path sketch
                let user = tr.root.int("m_userId").unwrap_or(-1);
                let curves: Vec<Vec<V3>> = tr
                    .root
                    .ptrs("m_curveLoops")
                    .iter()
                    .filter_map(|&i| tr.objs.get(i))
                    .flat_map(|l| l.ptrs("m_curves"))
                    .filter_map(|i| tr.objs.get(i))
                    .map(|c| curve_points(c, s))
                    .filter(|p| p.len() >= 2)
                    .collect();
                if user > 0 && !curves.is_empty() {
                    self.sketches.insert(id, SketchData { user, plane: None, curves });
                }
            }
            "RevolutionElem" | "SweepElem" | "BlendElem" => {
                if tr.root.int("m_cutting").unwrap_or(0) != 0 || not_in_family(tr, s) {
                    return;
                }
                let fam = tr.root.int("m_famId").unwrap_or(-1);
                let kind = match cls {
                    "RevolutionElem" => Kind::Revolution { sketch: tr.root.int("m_sketchId").unwrap_or(-1), a0: param(tr, -1001802).unwrap_or(0.0), a1: param(tr, -1001803).unwrap_or(std::f64::consts::TAU) },
                    "SweepElem" => {
                        let Some(pd) = tr.root.obj("m_profileData").or_else(|| tr.root.ptr("m_profileData").and_then(|i| tr.objs.get(i))) else { return };
                        Kind::Sweep { profile: pd.int("m_profileSketchId").unwrap_or(-1), path: tr.root.int("m_pathSketchId").filter(|&v| v > 0).or_else(|| tr.root.int("m_path3dId")).unwrap_or(-1) }
                    }
                    _ => Kind::Blend { first: param(tr, -1001804).unwrap_or(0.0), second: param(tr, -1001805).unwrap_or(1.0) },
                };
                self.forms2.push(Form2 { id, fam, vis: vis_flags(tr), kind });
            }
            "Family" => {
                let guid = tr.root.ptr("m_oFamDoc").and_then(|i| tr.objs.get(i)).and_then(|o| {
                    let g = o.get("m_contentDocGUID")?;
                    let g = match g {
                        G::O(x) => x.get("m_guid")?,
                        _ => g,
                    };
                    match g {
                        G::Raw(b) if b.len() == 16 => Some(<[u8; 16]>::try_from(&b[..]).unwrap()),
                        _ => None,
                    }
                });
                if let Some(g) = guid {
                    self.fam_guid.insert(id, g);
                }
                // project id → family document id pairs: the document's elements
                if let Some(o) = tr.root.ptr("m_oFamDoc").and_then(|i| tr.objs.get(i)) {
                    let small: Vec<i64> = o
                        .list("m_big2SmallMap2")
                        .iter()
                        .filter_map(|p| match p.get("first")? {
                            G::I(v) => Some(*v),
                            G::O(x) => x.fields.iter().find_map(|(_, g)| if let G::I(v) = g { Some(*v) } else { None }),
                            _ => None,
                        })
                        .filter(|&v| v > 0)
                        .take(256)
                        .collect();
                    if !small.is_empty() {
                        self.fam_small.insert(id, small);
                    }
                }
            }
            "FamilyInstance" => {
                let Some(ii) = tr.root.ptr("m_pInstanceInfo").and_then(|i| tr.objs.get(i)) else { return };
                let Some(t) = ii.obj("m_Trf").and_then(Trf::from) else { return };
                let sym = ii.int("m_symbolId").unwrap_or(-1);
                self.instances.push((id, sym, tr.root.int("m_famId").unwrap_or(-1), t));
                if tr.objs.iter().any(|o| matches!(s.name(o.tag), "MullionFamRef" | "CurtainCellFamRef")) {
                    self.curtain.push((id, t.clone()));
                }
                // beams and braces: the location line, for a box along it when the family solids are not found
                let crv = tr.root.ptr("m_pCurveDriver").and_then(|i| tr.objs.get(i)).and_then(|d| {
                    let sb = d.fs("m_setbacks").map(|s| s.to_vec()).unwrap_or_default();
                    d.ptr("m_pCrv").and_then(|i| tr.objs.get(i)).map(|c| (c, sb))
                });
                if let Some((c, sb)) = crv {
                    if let (Some(o), Some(dv), Some([t0, t1, ..])) = (v3(c.fs("m_origin")), v3(c.fs("m_dirVec")), c.fs("m_endParams")) {
                        let (s0, s1) = (sb.first().copied().unwrap_or(0.0).max(0.0), sb.get(1).copied().unwrap_or(0.0).max(0.0));
                        let (a, b) = (t0 + s0, t1 - s1);
                        if b - a > 1e-3 {
                            self.framing.push((id, add(o, mul(dv, a)), add(o, mul(dv, b))));
                        }
                    }
                }
            }
            "FamilySymbol" => {
                self.symbols.insert(id, (tr.root.int("m_famId").unwrap_or(-1), tr.root.int("m_familyId").unwrap_or(-1)));
                if let Some(ps) = tr.root.ptr("m_pParams").and_then(|k| tr.objs.get(k)) {
                    let get = |pid: i64| {
                        ps.list("m_params").iter().find(|p| p.int("m_paramId") == Some(pid)).and_then(|p| match p.get("m_value") {
                            Some(G::F(v)) if v.is_finite() && *v > 0.05 => Some(*v),
                            _ => None,
                        })
                    };
                    // FAMILY_WIDTH_PARAM, FAMILY_HEIGHT_PARAM
                    if let (Some(w), Some(h)) = (get(-1001301), get(-1001300)) {
                        self.sym_size.insert(id, (w, h));
                    }
                }
            }
            _ => {}
        }
    }

    /// Builds the meshes, by element id. `boxes`: the element boxes (metres) by id, for the
    /// openings cut in the walls and to check the placed family solids.
    pub fn finish(self, boxes: &HashMap<i64, ([f64; 3], [f64; 3])>, family_doc: bool) -> HashMap<i64, Mesh> {
        let mut out: HashMap<i64, Mesh> = HashMap::new();
        let t0 = std::time::Instant::now();
        if std::env::var_os("RVT_SHAPE_DEBUG").is_some() {
            eprintln!("walls {} slabs {} forms {} sketches {} instances {} symbols {}", self.walls.len(), self.slabs.len(), self.forms.len(), self.sketch_planes.len(), self.instances.len(), self.symbols.len());
        }
        for (id, m) in &self.breps {
            out.insert(*id, m.clone());
        }
        for m in &self.mep {
            if let Some(x) = mep(m) {
                out.insert(m.id, x);
            }
        }
        for w in &self.walls {
            if let Some(m) = self.wall(w, boxes) {
                out.insert(w.id, m);
            }
        }
        for (id, path, half, planes) in &self.curved_walls {
            // bottom and top: the horizontal reference faces, else the element box
            let zs: Vec<f64> = planes.iter().filter(|(_, x, y, _)| norm(cross(*x, *y)).is_some_and(|n| n[2].abs() > 0.99)).map(|(o, ..)| o[2]).collect();
            let (mut z0, mut z1) = (zs.iter().copied().fold(f64::INFINITY, f64::min), zs.iter().copied().fold(f64::NEG_INFINITY, f64::max));
            if !(z1 - z0 > 1e-3) {
                let Some((lo, hi)) = boxes.get(id) else { continue };
                (z0, z1) = (lo[2] / FEET, hi[2] / FEET);
            }
            if let Some(m) = swept_wall(path, *half, z0, z1) {
                out.insert(*id, m);
            }
        }
        for sl in &self.slabs {
            if let Some(m) = slab(sl) {
                out.insert(sl.id, m);
            }
        }
        if std::env::var_os("RVT_SHAPE_DEBUG").is_some() {
            let mut h: HashMap<i64, usize> = HashMap::new();
            for v in self.forms.iter().map(|f| f.vis).chain(self.forms2.iter().map(|f| f.vis)) {
                *h.entry(v).or_default() += 1;
            }
            let mut h: Vec<_> = h.into_iter().collect();
            h.sort_by_key(|x| std::cmp::Reverse(x.1));
            eprintln!("form visibility flags {:?}", &h[..h.len().min(16)].iter().map(|(v, n)| format!("{v:#x}:{n}")).collect::<Vec<_>>());
        }
        // family solids, in their family's coordinates
        let mut by_fam: HashMap<i64, Vec<Mesh>> = HashMap::new();
        for f in self.forms.iter() {
            if let Some(m) = self.form(f) {
                if family_doc {
                    // the family file itself: its solids as they are
                    out.insert(f.id, m);
                } else {
                    by_fam.entry(f.fam).or_default().push(m);
                }
            }
        }
        for f in self.forms2.iter() {
            let m = match f.kind {
                Kind::Revolution { sketch, a0, a1 } => self.sketches.get(&sketch).and_then(|sk| revolution(sk, a0, a1)),
                Kind::Sweep { profile, path } => match (self.sketches.get(&profile), self.sketches.get(&path)) {
                    (Some(a), Some(b)) => sweep(a, b),
                    _ => None,
                },
                Kind::Blend { first, second } => {
                    let mut sk: Vec<(&i64, &SketchData)> = self.sketches.iter().filter(|(_, d)| d.user == f.id).collect();
                    sk.sort_by_key(|(id, _)| **id);
                    match sk.as_slice() {
                        [(_, a), (_, b)] => blend(a, b, first, second),
                        _ => None,
                    }
                }
            };
            if let Some(m) = m {
                if family_doc {
                    out.insert(f.id, m);
                } else {
                    by_fam.entry(f.fam).or_default().push(m);
                }
            }
        }
        if family_doc {
            return out;
        }
        if std::env::var_os("RVT_SHAPE_DEBUG").is_some() {
            eprintln!("forms built in {:?}", t0.elapsed());
        }
        // a `Family` element → the root of its family document (ContentDocuments GUID)
        // (the family document's elements have the root as their `m_famId`)
        let mut owner: HashMap<i64, i64> = HashMap::new();
        for (id, (f, _)) in &self.symbols {
            owner.insert(*id, *f);
        }
        for f in &self.forms {
            owner.insert(f.id, f.fam);
        }
        for f in &self.forms2 {
            owner.insert(f.id, f.fam);
        }
        for i in &self.instances {
            owner.insert(i.0, i.2);
        }
        let root_of = |fam: i64| {
            self.fam_small
                .get(&fam)
                .and_then(|v| v.iter().find_map(|s| owner.get(s).copied().filter(|&r| r > 0)))
                .or_else(|| self.fam_guid.get(&fam).and_then(|g| self.content.get(g)).copied())
        };
        if std::env::var_os("RVT_SHAPE_DEBUG").is_some() {
            let roots = self.fam_guid.keys().filter(|f| root_of(**f).is_some()).count();
            eprintln!("families {} content docs {} resolved {} solids by doc {} forms2 {}", self.fam_guid.len(), self.content.len(), roots, by_fam.len(), self.forms2.len());
            let hex = |g: &[u8; 16]| g.iter().map(|b| format!("{b:02x}")).collect::<String>();
            for (f, g) in self.fam_guid.iter().take(6) {
                eprintln!("  family {f} guid {} root {:?} solids {}", hex(g), root_of(*f), root_of(*f).is_some_and(|r| by_fam.contains_key(&r)));
            }
            for (g, r) in self.content.iter().take(4) {
                eprintln!("  content {} → {r}", hex(g));
            }
            eprintln!("  solids keys {:?}", by_fam.keys().take(12).collect::<Vec<_>>());
        }
        // instances: family doc of the symbol, nested instances inside family docs
        let hosts: std::collections::HashSet<i64> = self.instances.iter().map(|i| i.2).filter(|&o| o > 0).collect();
        let fam_of = |sym: i64| -> Option<i64> {
            let &(_, b) = self.symbols.get(&sym)?;
            [root_of(b), Some(b)].into_iter().flatten().find(|f| *f > 0 && (by_fam.contains_key(f) || hosts.contains(f)))
        };
        let mut nested: HashMap<i64, Vec<(i64, Trf)>> = HashMap::new();
        for &(_, sym, owner, t) in &self.instances {
            if owner > 0 {
                if let Some(f) = fam_of(sym) {
                    nested.entry(owner).or_default().push((f, t));
                }
            }
        }
        // a family's solids with its nested families, in the family frame, solid by solid
        // (memoized: nested families repeat; the size is capped against runaway nesting)
        const CAP: usize = 400_000;
        type Parts = std::rc::Rc<Vec<Mesh>>;
        fn flat(fam: i64, by_fam: &HashMap<i64, Vec<Mesh>>, nested: &HashMap<i64, Vec<(i64, Trf)>>, depth: u32, memo: &mut HashMap<i64, Parts>) -> Parts {
            if let Some(m) = memo.get(&fam) {
                return m.clone();
            }
            let mut out: Vec<Mesh> = by_fam.get(&fam).cloned().unwrap_or_default();
            let mut size: usize = out.iter().map(|m| m.pos.len()).sum();
            if depth < 4 {
                for (f, ti) in nested.get(&fam).map(|v| v.as_slice()).unwrap_or(&[]) {
                    if *f == fam || size > CAP {
                        continue;
                    }
                    for m in flat(*f, by_fam, nested, depth + 1, memo).iter() {
                        let mut q = Mesh::default();
                        append(&mut q, m, ti);
                        size += q.pos.len();
                        out.push(q);
                    }
                }
            }
            let r = std::rc::Rc::new(out);
            memo.insert(fam, r.clone());
            r
        }
        /// World box (metres) of a part's box (metres, family frame) placed by `t` (feet).
        fn placed_box(b: ([f32; 3], [f32; 3]), t: &Trf) -> ([f64; 3], [f64; 3]) {
            let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
            for k in 0..8 {
                let c = [if k & 1 == 0 { b.0[0] } else { b.1[0] }, if k & 2 == 0 { b.0[1] } else { b.1[1] }, if k & 4 == 0 { b.0[2] } else { b.1[2] }];
                let q = t.apply([c[0] as f64 / FEET, c[1] as f64 / FEET, c[2] as f64 / FEET]);
                for a in 0..3 {
                    lo[a] = lo[a].min(q[a] * FEET);
                    hi[a] = hi[a].max(q[a] * FEET);
                }
            }
            (lo, hi)
        }
        let cover = 0.6;
        let mut memo: HashMap<i64, Parts> = HashMap::new();
        let mut part_boxes: HashMap<i64, std::rc::Rc<Vec<Option<([f32; 3], [f32; 3])>>>> = HashMap::new();
        let dbg = std::env::var_os("RVT_SHAPE_DEBUG").is_some();
        let mut st = [0usize; 6];
        for &(id, sym, owner, t) in &self.instances {
            if owner > 0 {
                continue;
            }
            st[0] += 1;
            let Some(f) = fam_of(sym) else {
                if dbg {
                    let sy = self.symbols.get(&sym);
                    let r = sy.and_then(|&(_, b)| root_of(b));
                    eprintln!("NOFAM {id} sym {sym} {:?} root {:?} guid {} content-root-known {}", sy, r, sy.is_some_and(|&(_, b)| self.fam_guid.contains_key(&b)), r.is_some_and(|r| by_fam.contains_key(&r) || hosts.contains(&r)));
                }
                continue;
            };
            st[1] += 1;
            let parts = flat(f, &by_fam, &nested, 0, &mut memo);
            if parts.iter().all(|m| m.tri.is_empty()) {
                continue;
            }
            st[2] += 1;
            let pb = part_boxes.entry(f).or_insert_with(|| std::rc::Rc::new(parts.iter().map(|m| m.bbox()).collect())).clone();
            let Some(&(blo, bhi)) = boxes.get(&id) else {
                let mut m = Mesh::default();
                for p in parts.iter() {
                    append(&mut m, p, &t);
                }
                out.insert(id, m);
                continue;
            };
            // the solids must lie within the instance's box: the family document holds one
            // type (its default sizes) and may hold solids hidden in the project (the sample
            // host of a hosted template, solids hidden by a type parameter): the solids out of
            // the box are left out, and the instance keeps its box when what remains does not
            // fill it
            let tol = 0.05 + 0.1 * (0..3).map(|a| bhi[a] - blo[a]).fold(0.0, f64::max);
            let inside = |(lo, hi): ([f64; 3], [f64; 3])| (0..3).all(|a| lo[a] >= blo[a] - tol && hi[a] <= bhi[a] + tol);
            let placed: Vec<Option<([f64; 3], [f64; 3])>> = pb.iter().map(|b| b.map(|b| placed_box(b, &t))).collect();
            let keep: Vec<usize> = (0..parts.len()).filter(|&k| placed[k].is_some_and(inside)).collect();
            let (mut ulo, mut uhi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
            for &k in &keep {
                let (lo, hi) = placed[k].unwrap();
                for a in 0..3 {
                    ulo[a] = ulo[a].min(lo[a]);
                    uhi[a] = uhi[a].max(hi[a]);
                }
            }
            let covered = (0..3)
                .filter(|&a| {
                    let e = bhi[a] - blo[a];
                    e < 0.1 || uhi[a] - ulo[a] >= cover * e
                })
                .count();
            // all three axes, or two when the solids lie inside the box: a door or window
            // whose box also holds its swing, frame depth or opening (the panel and trims
            // span the width and height but not the whole depth)
            let covers = !keep.is_empty() && (covered == 3 || (covered == 2 && keep.len() == parts.len()));
            if covers {
                let mut m = Mesh::default();
                for &k in &keep {
                    append(&mut m, &parts[k], &t);
                }
                if keep.len() < parts.len() {
                    st[3] += 1;
                }
                out.insert(id, m);
                continue;
            }
            if dbg {
                eprintln!("OFFBOX {id} sym {sym} fam {f} aligned {} parts {} in box {} box {blo:?} {bhi:?}", t.axis_aligned(), parts.len(), keep.len());
                if std::env::var_os("RVT_SHAPE_DEBUG2").is_some() { for b in placed.iter().flatten() { eprintln!("   part {:?} {:?}", b.0, b.1); } }
            }
            st[4] += 1;
            // a door or window of another type: the solids of the family's default type
            // stretched to the type's width (family x) and height (family z, from the sill)
            if let Some(&(w, h)) = self.sym_size.get(&sym) {
                if let Some(m) = sized(&parts, &pb, w * FEET, h * FEET, &t, (blo, bhi), tol) {
                    st[5] += 1;
                    out.insert(id, m);
                    continue;
                }
            }
            // another type (other sizes): MEP fittings and most parametric families scale
            // with their nominal size: the solids scaled about the insertion point so that
            // their box is the instance's
            if let Some(m) = uniform_fit(&parts, &placed, &t, (blo, bhi)) {
                st[5] += 1;
                out.insert(id, m);
                continue;
            }
            // else the solids in the box (else all) stretched onto it, axis by axis, when
            // the placement keeps the axes
            if !t.axis_aligned() {
                continue;
            }
            let mut m = Mesh::default();
            for k in 0..parts.len() {
                if keep.is_empty() || keep.contains(&k) {
                    append(&mut m, &parts[k], &t);
                }
            }
            if let Some(b) = m.bbox() {
                if fit(&mut m, b, (blo, bhi)) {
                    st[5] += 1;
                    out.insert(id, m);
                }
            }
        }
        if dbg {
            eprintln!("instances placed in {:?}", t0.elapsed());
            eprintln!("instances top {} family found {} solids {} trimmed {} off box {} fitted {}", st[0], st[1], st[2], st[3], st[4], st[5]);
        }
        let mut nf = 0;
        for (id, a, b) in &self.framing {
            if out.contains_key(id) {
                continue;
            }
            let Some(&(lo, hi)) = boxes.get(id) else { continue };
            if let Some(m) = framing_box(*a, *b, lo, hi) {
                nf += 1;
                out.insert(*id, m);
            }
        }
        let mut nc = 0;
        for (id, t) in &self.curtain {
            if out.contains_key(id) {
                continue;
            }
            let Some(&(lo, hi)) = boxes.get(id) else { continue };
            if let Some(m) = oriented_box(t, lo, hi) {
                nc += 1;
                out.insert(*id, m);
            }
        }
        if dbg {
            eprintln!("framing boxes {nf} of {}, curtain boxes {nc} of {}", self.framing.len(), self.curtain.len());
        }
        out
    }

    fn wall(&self, w: &Wall, boxes: &HashMap<i64, ([f64; 3], [f64; 3])>) -> Option<Mesh> {
        let z = [0.0, 0.0, 1.0];
        let d = norm([w.d[0], w.d[1], 0.0])?;
        let n = norm(cross(d, z))?;
        let f = Frame { o: w.o, u: d, v: z, n };
        // thickness: side faces (normal ∥ n) and the faces across the wall (ends, top, bottom)
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for &(o, x, y, env) in &w.planes {
            let Some(pn) = norm(cross(x, y)) else { continue };
            let off = dot(sub(o, w.o), n);
            if dot(pn, n).abs() > 0.99 {
                lo = lo.min(off);
                hi = hi.max(off);
            }
            for (axis, (a, b)) in [(x, (env[0], env[2])), (y, (env[1], env[3]))] {
                let k = dot(axis, n);
                if k.abs() > 0.99 && a.is_finite() && b.is_finite() && (b - a).abs() < 50.0 {
                    lo = lo.min(off + k * a).min(off + k * b);
                    hi = hi.max(off + k * a).max(off + k * b);
                }
            }
        }
        if !(hi - lo > 1e-4 && hi - lo < 50.0) {
            return None;
        }
        // ends (faces across the location line: joins may move them) and bottom / top faces
        let (mut e0, mut e1) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut z0, mut z1) = (f64::INFINITY, f64::NEG_INFINITY);
        for &(o, x, y, env) in &w.planes {
            let Some(pn) = norm(cross(x, y)) else { continue };
            for (axis, (a, b)) in [(x, (env[0], env[2])), (y, (env[1], env[3]))] {
                if !(a.is_finite() && b.is_finite() && (b - a).abs() < 3e3) {
                    continue;
                }
                let k = dot(axis, d);
                if k.abs() > 0.99 {
                    let e = dot(sub(o, w.o), d);
                    e0 = e0.min(e + k * a).min(e + k * b);
                    e1 = e1.max(e + k * a).max(e + k * b);
                }
                if axis[2].abs() > 0.99 {
                    let e = o[2] - w.o[2];
                    z0 = z0.min(e + axis[2] * a).min(e + axis[2] * b);
                    z1 = z1.max(e + axis[2] * a).max(e + axis[2] * b);
                }
            }
            if dot(pn, d).abs() > 0.99 {
                let e = dot(sub(o, w.o), d);
                e0 = e0.min(e);
                e1 = e1.max(e);
            }
            if pn[2].abs() > 0.99 {
                z0 = z0.min(o[2] - w.o[2]);
                z1 = z1.max(o[2] - w.o[2]);
            }
        }
        let (t0, t1) = (w.ends.0.min(w.ends.1), w.ends.0.max(w.ends.1));
        if std::env::var_os("RVT_SHAPE_DEBUG").is_some() {
            eprintln!("wall {} planes {} thick {lo}..{hi} ends {t0}..{t1} faces {e0}..{e1} z {z0}..{z1} loops {}", w.id, w.planes.len(), w.loops.len());
        }
        let mut loops: Vec<Vec<[f64; 2]>> = w.loops.iter().map(|l| l.iter().map(|&p| f.uv(p)).collect()).collect();
        if loops.is_empty() {
            if !(z1 - z0 > 1e-3 && z1 - z0 < 3e3) {
                return None;
            }
            loops.push(vec![[t0, z0], [t1, z0], [t1, z1], [t0, z1]]);
        }
        // move the profile's end points to the end faces when those lie a little beyond (joins)
        let umin = loops.iter().flatten().map(|p| p[0]).fold(f64::INFINITY, f64::min);
        let umax = loops.iter().flatten().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max);
        let reach = (hi - lo) * 3.0 + 0.3;
        // joins are not stored with the wall; along a grid axis the element box shows them
        if let Some((bmin, bmax)) = boxes.get(&w.id) {
            if let Some(k) = (0..2).find(|&k| d[k].abs() > 0.9999) {
                let a = (bmin[k] / FEET - w.o[k]) * d[k].signum();
                let b = (bmax[k] / FEET - w.o[k]) * d[k].signum();
                e0 = e0.min(a.min(b));
                e1 = e1.max(a.max(b));
            }
        }
        let lo_end = if e0 < umin && umin - e0 < reach { e0 } else { umin };
        let hi_end = if e1 > umax && e1 - umax < reach { e1 } else { umax };
        for p in loops.iter_mut().flatten() {
            if (p[0] - umin).abs() < 1e-6 {
                p[0] = lo_end;
            } else if (p[0] - umax).abs() < 1e-6 {
                p[0] = hi_end;
            }
        }
        // the loops must lie in the wall's plane
        if w.loops.iter().flatten().any(|&p| dot(sub(p, w.o), n).abs() > (hi - lo).abs() + 1.0) {
            return None;
        }
        let mut cuts = Vec::new();
        for c in &w.cutters {
            let Some((blo, bhi)) = boxes.get(c) else { continue };
            let mut r = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
            let mut wr = (f64::INFINITY, f64::NEG_INFINITY);
            for k in 0..8 {
                let p = [if k & 1 == 0 { blo[0] } else { bhi[0] }, if k & 2 == 0 { blo[1] } else { bhi[1] }, if k & 4 == 0 { blo[2] } else { bhi[2] }];
                let p = mul(p, 1.0 / FEET);
                let q = f.uv(p);
                r = [r[0].min(q[0]), r[1].min(q[1]), r[2].max(q[0]), r[3].max(q[1])];
                let o = dot(sub(p, w.o), n);
                wr = (wr.0.min(o), wr.1.max(o));
            }
            // only an insert across the wall (its box spans the wall's thickness)
            if wr.0 <= lo + 0.02 && wr.1 >= hi - 0.02 && r[2] - r[0] > 0.05 && r[3] - r[1] > 0.05 {
                cuts.push(r);
            }
        }
        let m = solid(&f, &loops, &cuts, lo, hi, None);
        (!m.tri.is_empty()).then_some(m)
    }

    fn form(&self, fm: &Form) -> Option<Mesh> {
        let (o, x, y) = match self.sketch_planes.get(&fm.sketch) {
            Some(&p) => p,
            None => {
                // plane of the loops (Newell), origin at the first point
                let l = &fm.loops[0];
                let mut nn = [0.0; 3];
                for i in 0..l.len() {
                    let (a, b) = (l[i], l[(i + 1) % l.len()]);
                    nn = add(nn, cross(a, b));
                }
                let nz = norm(nn)?;
                let x = norm(sub(l[1], l[0]))?;
                (l[0], x, cross(nz, x))
            }
        };
        let u = norm(x)?;
        let nz = norm(cross(x, y))?;
        let v = cross(nz, u);
        let f = Frame { o, u, v, n: nz };
        if fm.loops.iter().flatten().any(|&p| dot(sub(p, o), nz).abs() > 1e-3 * (1.0 + dot(sub(p, o), sub(p, o)).sqrt())) {
            return None;
        }
        let loops: Vec<Vec<[f64; 2]>> = fm.loops.iter().map(|l| l.iter().map(|&p| f.uv(p)).collect()).collect();
        if !((fm.end - fm.start).abs() > 1e-6 && (fm.end - fm.start).abs() < 1e4) {
            return None;
        }
        let m = solid(&f, &loops, &[], fm.start, fm.end, None);
        (!m.tri.is_empty()).then_some(m)
    }
}

/// The parts (family frame, metres) placed by `t` and scaled about the insertion point by the
/// factor that best matches their world box (`placed`: the unscaled one per part) to `to`;
/// `None` when no single factor gives that box (within 2 cm + 8 % of its size).
fn uniform_fit(parts: &[Mesh], placed: &[Option<([f64; 3], [f64; 3])>], t: &Trf, to: ([f64; 3], [f64; 3])) -> Option<Mesh> {
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for (l, h) in placed.iter().flatten() {
        for a in 0..3 {
            lo[a] = lo[a].min(l[a]);
            hi[a] = hi[a].max(h[a]);
        }
    }
    let o = [t.or[0] * FEET, t.or[1] * FEET, t.or[2] * FEET];
    // factor per axis from the box size, on the axes with some extent
    let mut ks: Vec<f64> = (0..3).filter(|&a| hi[a] - lo[a] > 0.01 && to.1[a] - to.0[a] > 0.01).map(|a| (to.1[a] - to.0[a]) / (hi[a] - lo[a])).collect();
    if ks.is_empty() {
        return None;
    }
    ks.sort_by(f64::total_cmp);
    let k = ks[ks.len() / 2];
    if !(0.1..=10.0).contains(&k) || (k - 1.0).abs() < 1e-3 {
        return None;
    }
    let size = (0..3).map(|a| to.1[a] - to.0[a]).fold(0.0, f64::max);
    let tol = 0.02 + 0.08 * size;
    for a in 0..3 {
        let (l, h) = (o[a] + k * (lo[a] - o[a]), o[a] + k * (hi[a] - o[a]));
        if (l - to.0[a]).abs() > tol || (h - to.1[a]).abs() > tol {
            return None;
        }
    }
    let flip = t.det() < 0.0;
    let mut m = Mesh::default();
    for p in parts {
        let base = m.pos.len() as u32;
        for q in &p.pos {
            let w = t.apply([q[0] as f64 * k / FEET, q[1] as f64 * k / FEET, q[2] as f64 * k / FEET]);
            m.pos.push([(w[0] * FEET) as f32, (w[1] * FEET) as f32, (w[2] * FEET) as f32]);
        }
        m.tri.extend(p.tri.iter().map(|tr| if flip { [tr[0] + base, tr[2] + base, tr[1] + base] } else { [tr[0] + base, tr[1] + base, tr[2] + base] }));
    }
    Some(m)
}

/// Maps the box `from` onto `to`, axis by axis, when the scales stay moderate.
fn fit(m: &mut Mesh, from: ([f32; 3], [f32; 3]), to: ([f64; 3], [f64; 3])) -> bool {
    let mut k = [1.0f64; 3];
    for a in 0..3 {
        let (s, d) = ((from.1[a] - from.0[a]) as f64, to.1[a] - to.0[a]);
        if s < 1e-4 {
            if d > 0.05 {
                return false;
            }
            k[a] = 1.0;
        } else {
            k[a] = d / s;
            if !(0.33..=3.0).contains(&k[a]) {
                return false;
            }
        }
    }
    for p in &mut m.pos {
        for a in 0..3 {
            let s = (from.1[a] - from.0[a]) as f64;
            p[a] = if s < 1e-4 { ((to.0[a] + to.1[a]) * 0.5) as f32 } else { (to.0[a] + (p[a] - from.0[a]) as f64 * k[a]) as f32 };
        }
    }
    true
}

fn slab(sl: &Slab) -> Option<Mesh> {
    let (b, t) = match sl.range {
        Some(r) => r,
        None => {
            // horizontal faces
            let zs: Vec<f64> = sl.planes.iter().filter(|(_, x, y, _)| norm(cross(*x, *y)).is_some_and(|n| n[2].abs() > 0.99)).map(|(o, _, _, _)| o[2]).collect();
            let lo = zs.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = zs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            if hi - lo > 1e-4 {
                (lo, hi)
            } else {
                // a single face (ceilings without thickness): a thin slab under the sketch
                let z = sl.loops[0][0][2];
                (z - 0.03, z)
            }
        }
    };
    // horizontal loops only (sloped slabs keep their box)
    let z0 = sl.loops[0][0][2];
    if sl.loops.iter().flatten().any(|p| (p[2] - z0).abs() > 1e-3) {
        return None;
    }
    let f = Frame { o: [0.0; 3], u: [1.0, 0.0, 0.0], v: [0.0, 1.0, 0.0], n: [0.0, 0.0, 1.0] };
    let loops: Vec<Vec<[f64; 2]>> = sl.loops.iter().map(|l| l.iter().map(|p| [p[0], p[1]]).collect()).collect();
    let m = solid(&f, &loops, &[], b, t, None);
    (!m.tri.is_empty()).then_some(m)
}

/// Names of the classes read (for probes).
pub fn classes() -> &'static [&'static str] {
    &Shapes::CLASSES
}

#[cfg(test)]
mod tests {
    #[test]
    fn curved_wall_band() {
        // quarter circle of radius 10 ft, 1 ft thick, 10 ft high
        let path: Vec<[f64; 3]> = (0..=16).map(|k| {
            let a = std::f64::consts::FRAC_PI_2 * k as f64 / 16.0;
            [10.0 * a.cos(), 10.0 * a.sin(), 0.0]
        }).collect();
        let m = super::swept_wall(&path, 0.5, 0.0, 10.0).unwrap();
        let (lo, hi) = m.bbox().unwrap();
        assert!((hi[2] - 3.048).abs() < 1e-4 && lo[2].abs() < 1e-6);
        assert!((hi[0] - 10.5 * 0.3048).abs() < 1e-3, "{hi:?}");
        assert_eq!(m.tri.len(), 16 * 8 + 4);
    }

    use super::*;

    fn volume(pts: &[V3], tri: &[[u32; 3]]) -> f64 {
        tri.iter().map(|t| dot(pts[t[0] as usize], cross(pts[t[1] as usize], pts[t[2] as usize])) / 6.0).sum()
    }

    #[test]
    fn region_square_with_hole_and_cut() {
        let outer = vec![[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [0.0, 3.0]];
        let hole = vec![[1.0, 1.0], [1.0, 2.0], [2.0, 2.0], [2.0, 1.0]];
        let (p, t) = extrude_region(&[outer.clone()], &[], 0.0, 1.0);
        assert!((volume(&p, &t) - 12.0).abs() < 1e-9, "{}", volume(&p, &t));
        let (p, t) = extrude_region(&[outer.clone(), hole], &[], 0.0, 2.0);
        assert!((volume(&p, &t) - 22.0).abs() < 1e-9, "{}", volume(&p, &t));
        // a door cut from the bottom edge
        let (p, t) = extrude_region(&[outer], &[[3.0, -1.0, 3.5, 2.0]], 0.0, 1.0);
        assert!((volume(&p, &t) - 11.0).abs() < 1e-9, "{}", volume(&p, &t));
    }

    #[test]
    fn region_triangle_reversed_loop() {
        let tri = vec![[0.0, 0.0], [0.0, 2.0], [2.0, 0.0]];
        let (p, t) = extrude_region(&[tri], &[], 1.0, 0.0);
        assert!((volume(&p, &t) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn chain_reorients() {
        let c = vec![vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]], vec![[1.0, 1.0, 0.0], [1.0, 0.0, 0.0]], vec![[1.0, 1.0, 0.0], [0.0, 0.0, 0.0]]];
        let l = chain(c);
        assert_eq!(l.len(), 3);
        assert_eq!(l[2], [1.0, 1.0, 0.0]);
    }
}
