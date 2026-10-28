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
    fn append(&mut self, o: &Mesh) {
        let base = self.pos.len() as u32;
        self.pos.extend_from_slice(&o.pos);
        self.tri.extend(o.tri.iter().map(|t| [t[0] + base, t[1] + base, t[2] + base]));
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
fn curve_points(c: &Obj, s: &Schema) -> Vec<V3> {
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
            let n = ((span.abs() / (std::f64::consts::PI / 24.0)).ceil() as usize).clamp(2, 192);
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

// ------------------------------------------------------------------ transforms

/// Affine transform: `p' = M·p + or`, the columns of M being the images of X, Y, Z.
#[derive(Clone, Copy, Debug)]
pub struct Trf {
    m: [f64; 9],
    or: V3,
}

impl Trf {
    fn from(o: &Obj) -> Option<Trf> {
        let m = o.fs("m_3x3")?;
        let or = v3(o.fs("m_or"))?;
        (m.len() == 9 && m.iter().all(|x| x.is_finite())).then(|| Trf { m: m.try_into().unwrap(), or })
    }
    fn apply(&self, p: V3) -> V3 {
        let m = &self.m;
        [m[0] * p[0] + m[3] * p[1] + m[6] * p[2] + self.or[0], m[1] * p[0] + m[4] * p[1] + m[7] * p[2] + self.or[1], m[2] * p[0] + m[5] * p[1] + m[8] * p[2] + self.or[2]]
    }
    fn lin(&self, p: V3) -> V3 {
        let m = &self.m;
        [m[0] * p[0] + m[3] * p[1] + m[6] * p[2], m[1] * p[0] + m[4] * p[1] + m[7] * p[2], m[2] * p[0] + m[5] * p[1] + m[8] * p[2]]
    }
    /// `self ∘ o`
    fn then_inner(&self, o: &Trf) -> Trf {
        let c = |k: usize| self.lin([o.m[3 * k], o.m[3 * k + 1], o.m[3 * k + 2]]);
        let (a, b, cc) = (c(0), c(1), c(2));
        Trf { m: [a[0], a[1], a[2], b[0], b[1], b[2], cc[0], cc[1], cc[2]], or: self.apply(o.or) }
    }
    fn det(&self) -> f64 {
        let m = &self.m;
        dot([m[0], m[1], m[2]], cross([m[3], m[4], m[5]], [m[6], m[7], m[8]]))
    }
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

struct Form {
    id: i64,
    fam: i64,
    sketch: i64,
    loops: Vec<Vec<V3>>,
    start: f64,
    end: f64,
}

/// Collects the data of the elements with a rebuilt shape, partition by partition.
#[derive(Default)]
pub struct Shapes {
    walls: Vec<Wall>,
    slabs: Vec<Slab>,
    forms: Vec<Form>,
    sketch_planes: HashMap<i64, (V3, V3, V3)>,
    /// instance id → (symbol id, own family doc id if nested, transform)
    instances: Vec<(i64, i64, i64, Trf)>,
    /// symbol id → (m_famId, m_familyId)
    symbols: HashMap<i64, (i64, i64)>,
    mep: Vec<Mep>,
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

fn param(tr: &Tree, id: i64) -> Option<f64> {
    let set = tr.objs.get(tr.root.ptr("m_pParamValueSetDouble")?)?;
    set.list("m_paramSet").iter().find(|p| p.int("m_paramId") == Some(id)).and_then(|p| match p.get("m_value") {
        Some(G::F(v)) if v.is_finite() => Some(*v),
        _ => None,
    })
}

impl Shapes {
    /// Element classes read.
    const CLASSES: [&'static str; 12] = ["SWall", "Floor", "Ceiling", "BuildingPad", "RoofSoffit", "Toposolid", "ExtrusionElem", "FamilyInstance", "FamilySymbol", "VarSketch", "Sketch", "SketchPlane"];

    pub fn partition(&mut self, s: &Schema, d: &[u8], w: usize) {
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
        elements(s, d, w, &|t| want[t as usize], |_, t, _, tr| {
            if s.isa(t, "RbsSingleCurve") {
                self.mep_curve(s, s.name(t), tr);
            } else {
                self.element(s, s.name(t), tr)
            }
        });
    }

    fn mep_curve(&mut self, s: &Schema, cls: &str, tr: Tree) {
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

    fn element(&mut self, s: &Schema, cls: &str, tr: Tree) {
        let id = tr.id;
        match cls {
            "SWall" => {
                let Some(dr) = tr.objs.iter().find(|o| s.name(o.tag) == "VWallDriver") else { return };
                let Some(c) = dr.ptr("m_pCrv").and_then(|i| tr.objs.get(i)) else { return };
                if s.name(c.tag) != "GLine" {
                    return; // curved walls keep their box
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
                if tr.root.int("m_cutting").unwrap_or(0) != 0 {
                    return;
                }
                let Some(h) = tr.objs.iter().find(|o| s.name(o.tag) == "ExtrusionElemExtrusionHelper") else { return };
                let loops = helper_loops(&tr, s, "ExtrusionElemExtrusionHelper");
                if loops.is_empty() {
                    return;
                }
                let start = param(&tr, -1001800).unwrap_or(0.0);
                let end = param(&tr, -1001801).unwrap_or(1.0);
                self.forms.push(Form { id, fam: tr.root.int("m_famId").unwrap_or(-1), sketch: h.int("m_sketchId").unwrap_or(-1), loops, start, end });
            }
            "VarSketch" | "Sketch" => {
                if let Some(p) = tr.root.ptr("m_pPlane").and_then(|i| tr.objs.get(i)) {
                    if let (Some(o), Some(x), Some(y)) = (v3(p.fs("m_origin")), v3(p.fs("m_xVec")), v3(p.fs("m_yVec"))) {
                        self.sketch_planes.insert(id, (o, x, y));
                    }
                }
            }
            "FamilyInstance" => {
                let Some(ii) = tr.root.ptr("m_pInstanceInfo").and_then(|i| tr.objs.get(i)) else { return };
                let Some(t) = ii.obj("m_Trf").and_then(Trf::from) else { return };
                let sym = ii.int("m_symbolId").unwrap_or(-1);
                self.instances.push((id, sym, tr.root.int("m_famId").unwrap_or(-1), t));
            }
            "FamilySymbol" => {
                self.symbols.insert(id, (tr.root.int("m_famId").unwrap_or(-1), tr.root.int("m_familyId").unwrap_or(-1)));
            }
            _ => {}
        }
    }

    /// Builds the meshes, by element id. `boxes`: the element boxes (metres) by id, for the
    /// openings cut in the walls and to check the placed family solids.
    pub fn finish(self, boxes: &HashMap<i64, ([f64; 3], [f64; 3])>, family_doc: bool) -> HashMap<i64, Mesh> {
        let mut out: HashMap<i64, Mesh> = HashMap::new();
        if std::env::var_os("RVT_SHAPE_DEBUG").is_some() {
            eprintln!("walls {} slabs {} forms {} sketches {} instances {} symbols {}", self.walls.len(), self.slabs.len(), self.forms.len(), self.sketch_planes.len(), self.instances.len(), self.symbols.len());
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
        for sl in &self.slabs {
            if let Some(m) = slab(sl) {
                out.insert(sl.id, m);
            }
        }
        // family solids, in their family's coordinates
        let mut by_fam: HashMap<i64, Mesh> = HashMap::new();
        for f in &self.forms {
            if let Some(m) = self.form(f) {
                if family_doc {
                    // the family file itself: its solids as they are
                    out.insert(f.id, m);
                } else {
                    by_fam.entry(f.fam).or_default().append(&m);
                }
            }
        }
        if family_doc {
            return out;
        }
        // instances: family doc of the symbol, nested instances inside family docs
        let fam_of = |sym: i64| -> Option<i64> {
            let &(a, b) = self.symbols.get(&sym)?;
            [a, b].into_iter().find(|f| *f > 0 && (by_fam.contains_key(f) || self.instances.iter().any(|i| i.2 == *f)))
        };
        let mut nested: HashMap<i64, Vec<(i64, Trf)>> = HashMap::new();
        for &(_, sym, owner, t) in &self.instances {
            if owner > 0 {
                if let Some(f) = fam_of(sym) {
                    nested.entry(owner).or_default().push((f, t));
                }
            }
        }
        fn place(fam: i64, t: &Trf, by_fam: &HashMap<i64, Mesh>, nested: &HashMap<i64, Vec<(i64, Trf)>>, depth: u32, out: &mut Mesh) {
            if depth > 4 {
                return;
            }
            if let Some(m) = by_fam.get(&fam) {
                let flip = t.det() < 0.0;
                let base = out.pos.len() as u32;
                for p in &m.pos {
                    let q = t.apply([p[0] as f64 / FEET, p[1] as f64 / FEET, p[2] as f64 / FEET]);
                    out.pos.push([(q[0] * FEET) as f32, (q[1] * FEET) as f32, (q[2] * FEET) as f32]);
                }
                out.tri.extend(m.tri.iter().map(|tr| if flip { [tr[0] + base, tr[2] + base, tr[1] + base] } else { [tr[0] + base, tr[1] + base, tr[2] + base] }));
            }
            for (f, ti) in nested.get(&fam).map(|v| v.as_slice()).unwrap_or(&[]) {
                place(*f, &t.then_inner(ti), by_fam, nested, depth + 1, out);
            }
        }
        for &(id, sym, owner, t) in &self.instances {
            if owner > 0 {
                continue;
            }
            let Some(f) = fam_of(sym) else { continue };
            let mut m = Mesh::default();
            place(f, &t, &by_fam, &nested, 0, &mut m);
            if m.tri.is_empty() {
                continue;
            }
            // sanity: the solids must lie within the instance's box (else the placement is
            // not understood: keep the box)
            if let (Some((lo, hi)), Some((blo, bhi))) = (m.bbox(), boxes.get(&id)) {
                let tol = 0.05 + 0.1 * (0..3).map(|a| bhi[a] - blo[a]).fold(0.0, f64::max);
                if (0..3).any(|a| (lo[a] as f64) < blo[a] - tol || (hi[a] as f64) > bhi[a] + tol) {
                    continue;
                }
            }
            out.insert(id, m);
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
