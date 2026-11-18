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
use crate::det::HashMap;

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
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
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
    /// Material of its triangles from each start index on (empty: none known).
    pub runs: Vec<(u32, Mat)>,
    /// One of its materials has a surface pattern (drawn hatched in plan).
    pub patterned: bool,
    /// One of its materials has a cut pattern: its sections are hatched.
    pub cut_hatched: bool,
    /// Family element visibility (`m_famElemVisibility`) of its triangles from each start
    /// index on (empty: shown everywhere).
    pub vis: Vec<(u32, i64)>,
    /// Subcategory of its triangles from each start index on (`rvt_annot::scoped` family
    /// subcategory, -1: its category's).
    pub cats: Vec<(u32, i64)>,
}

/// A family's geometry fitted to one of its types (or to an instance with its own sizes): the
/// family document holds the geometry of one type, and each type keeps its reference planes as
/// Revit regenerated them for its sizes (`m_refFaces`). Along each axis of the family, the
/// positions of the reference planes in the document and in the type, sorted: a point between
/// two planes moves in proportion, one beyond the last plane moves with it (feet).
#[derive(Debug, Clone, Default)]
pub struct Stretch(pub [Vec<(f64, f64)>; 3]);

impl Stretch {
    /// Where coordinate `x` (feet) along axis `a` goes.
    pub fn map(&self, a: usize, x: f64) -> f64 {
        let k = &self.0[a];
        let (Some(first), Some(last)) = (k.first(), k.last()) else { return x };
        if x <= first.0 {
            return x + first.1 - first.0;
        }
        if x >= last.0 {
            return x + last.1 - last.0;
        }
        let i = k.partition_point(|p| p.0 <= x).clamp(1, k.len() - 1);
        let (a0, a1) = (k[i - 1], k[i]);
        a0.1 + (x - a0.0) * (a1.1 - a0.1) / (a1.0 - a0.0)
    }

    /// Where point `p` (feet, family frame) goes.
    pub fn apply(&self, p: V3) -> V3 {
        [self.map(0, p[0]), self.map(1, p[1]), self.map(2, p[2])]
    }

    /// A mesh (metres, family frame) moved by the stretch.
    fn mesh(&self, m: &Mesh) -> Mesh {
        let mut m = m.clone();
        for q in m.pos.iter_mut() {
            let r = self.apply([q[0] as f64 / FEET, q[1] as f64 / FEET, q[2] as f64 / FEET]);
            *q = [(r[0] * FEET) as f32, (r[1] * FEET) as f32, (r[2] * FEET) as f32];
        }
        m
    }

    /// Built from (document, type) positions along each axis: sorted, a position given twice
    /// kept once; None when the planes would cross (not a stretch).
    fn new(mut axes: [Vec<(f64, f64)>; 3]) -> Option<Stretch> {
        for k in axes.iter_mut() {
            k.sort_by(|a, b| a.0.total_cmp(&b.0));
            k.dedup_by(|b, a| (b.0 - a.0).abs() < 1e-6);
            if k.windows(2).any(|w| w[1].1 < w[0].1 - 1e-6) {
                return None;
            }
        }
        Some(Stretch(axes))
    }

    /// Total distance the planes move (feet).
    fn amount(&self) -> f64 {
        self.0.iter().flatten().map(|(d, n)| (d - n).abs()).sum()
    }
}

/// The mirror a family instance's flips (hand, facing) put on its family's x and y: one flip
/// mirrors that axis; both are a half turn, already in the instance's stored placement.
pub(crate) fn flip_signs(hand: bool, facing: bool) -> (f64, f64) {
    if hand && facing {
        return (1.0, 1.0);
    }
    (if hand { -1.0 } else { 1.0 }, if facing { -1.0 } else { 1.0 })
}

/// Origin and normal of a `Plane` surface.
fn plane_of(o: &Obj) -> Option<(V3, V3)> {
    let (p, x, y) = (v3(o.fs("m_origin"))?, v3(o.fs("m_xVec"))?, v3(o.fs("m_yVec"))?);
    Some((p, cross(x, y)))
}

/// Material of a family solid: none, a material element, the family parameter giving it, or
/// (once resolved) its colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mat {
    None,
    Elem(i64),
    Param(i64),
    Color([u8; 3]),
}

/// The "Material" parameter of family solids.
const MATERIAL_PARAM: i64 = -1002107;

/// A solid's material: its family parameter, else its own value.
fn mat_of(tr: &Tree, s: &Schema) -> Mat {
    let param = tr.objs.iter().filter(|o| s.name(o.tag) == "FamilyParametrizedElemParamsCell").flat_map(|o| o.list("m_paramDrivenData").iter()).find(|d| d.int("m_elemPropId") == Some(MATERIAL_PARAM)).and_then(|d| d.int("m_famParamId"));
    if let Some(p) = param {
        return Mat::Param(p);
    }
    if let Some(m) = tr.root.int("m_materialId").filter(|v| *v > 0) {
        return Mat::Elem(m);
    }
    let own = tr.root.ptr("m_pParamValueSetElementId").and_then(|i| tr.objs.get(i)).and_then(|o| o.list("m_paramSet").iter().find(|p| p.int("m_paramId") == Some(MATERIAL_PARAM)).and_then(|p| p.int("m_value")));
    own.filter(|v| *v > 0).map_or(Mat::None, Mat::Elem)
}

/// Element-id values of the family parameters an element holds (`FamilyParams.m_params`).
fn family_elem_values(tr: &Tree, s: &Schema) -> HashMap<i64, i64> {
    tr.objs.iter().filter(|o| s.name(o.tag) == "FamilyParams").flat_map(|o| o.list("m_params").iter()).filter_map(|p| Some((p.int("m_paramId")?, p.int("m_elemId").filter(|v| *v > 0)?))).collect()
}

impl Mesh {
    /// Its triangles grouped by resolved colour (None: no material colour), in order.
    pub fn by_color(&self) -> Vec<(Option<[u8; 3]>, Vec<[u32; 3]>)> {
        let mut out: Vec<(Option<[u8; 3]>, Vec<[u32; 3]>)> = Vec::new();
        let mut r = 0;
        for (k, t) in self.tri.iter().enumerate() {
            while r + 1 < self.runs.len() && self.runs[r + 1].0 as usize <= k {
                r += 1;
            }
            let c = match self.runs.get(r) {
                Some(&(start, Mat::Color(c))) if start as usize <= k => Some(c),
                _ => None,
            };
            match out.iter_mut().find(|g| g.0 == c) {
                Some(g) => g.1.push(*t),
                None => out.push((c, vec![*t])),
            }
        }
        out
    }
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
        add(
            add(add(self.o, mul(self.u, u)), mul(self.v, v)),
            mul(self.n, w),
        )
    }
    fn uv(&self, p: V3) -> [f64; 2] {
        let d = sub(p, self.o);
        [dot(d, self.u), dot(d, self.v)]
    }
}

// ------------------------------------------------------------------ curves and loops

/// Points along a curve object (feet), start to end.
pub(crate) fn curve_points(c: &Obj, s: &Schema) -> Vec<V3> {
    curve_points_within(c, s, 0.03)
}

/// `curve_points` with chords within `tol` (feet) on large arcs.
pub(crate) fn curve_points_within(c: &Obj, s: &Schema, tol: f64) -> Vec<V3> {
    let ep = c.fs("m_endParams").unwrap_or(&[]);
    let (t0, t1) = match ep {
        [a, b, ..] if a.is_finite() && b.is_finite() => (*a, *b),
        _ => return vec![],
    };
    match s.name(c.tag) {
        "GLine" => {
            let (Some(o), Some(d)) = (v3(c.fs("m_origin")), v3(c.fs("m_dirVec"))) else {
                return vec![];
            };
            vec![add(o, mul(d, t0)), add(o, mul(d, t1))]
        }
        "GArc" | "GEllipse" => {
            let (Some(ce), Some(x), Some(y)) =
                (v3(c.fs("m_center")), v3(c.fs("m_xVec")), v3(c.fs("m_yVec")))
            else {
                return vec![];
            };
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
            // equal bounds: the whole circle
            let span = if t1 == t0 { std::f64::consts::TAU } else { t1 - t0 };
            if !(span.abs() < 64.0) {
                return vec![];
            }
            // 24 segments a turn, finer on large radii (chord within about 1 cm)
            let r = rx.abs().max(ry.abs());
            let step = if r > tol {
                (std::f64::consts::PI / 12.0).min(2.0 * (1.0 - tol / r).acos())
            } else {
                std::f64::consts::PI / 12.0
            };
            let n = ((span.abs() / step.max(std::f64::consts::PI / 96.0)).ceil() as usize)
                .clamp(2, 192);
            (0..=n)
                .map(|i| {
                    let t = t0 + span * i as f64 / n as f64;
                    add(ce, add(mul(x, rx * t.cos()), mul(y, ry * t.sin())))
                })
                .collect()
        }
        "GNurbSpline" => nurbs_points(c, t0, t1),
        "GHermiteSpline" => hermite_points(c),
        _ => vec![],
    }
}

/// Points along a Hermite spline (`m_NodeArray`: parameter, point and tangent of each node).
fn hermite_points(c: &Obj) -> Vec<V3> {
    let nodes: Vec<(f64, V3, V3)> = c
        .list("m_NodeArray")
        .iter()
        .filter_map(|n| {
            let t = match n.get("m_iParametr")? {
                G::F(t) => *t,
                _ => return None,
            };
            Some((t, v3(n.fs("m_iPoint"))?, v3(n.fs("m_iTangent"))?))
        })
        .collect();
    let mut out = Vec::new();
    for w in nodes.windows(2) {
        let ((t0, p0, m0), (t1, p1, m1)) = (w[0], w[1]);
        let d = t1 - t0;
        for k in 0..16 {
            let t = k as f64 / 16.0;
            let (t2, t3) = (t * t, t * t * t);
            let (h00, h10, h01, h11) = (2.0 * t3 - 3.0 * t2 + 1.0, t3 - 2.0 * t2 + t, -2.0 * t3 + 3.0 * t2, t3 - t2);
            out.push(std::array::from_fn(|a| h00 * p0[a] + h10 * d * m0[a] + h01 * p1[a] + h11 * d * m1[a]));
        }
    }
    if let Some(n) = nodes.last() {
        out.push(n.1);
    }
    out
}

/// Points along a NURBS curve between parameters `t0` and `t1` (de Boor); its control polygon
/// when its knots do not read.
fn nurbs_points(c: &Obj, t0: f64, t1: f64) -> Vec<V3> {
    let ctrl: Vec<(V3, f64)> = c
        .list("m_ctrlPoints")
        .iter()
        .filter_map(|p| Some((v3(p.fs("m_Point"))?, p.fs("m_Weight").and_then(|w| w.first().copied()).or(match p.get("m_Weight") {
            Some(G::F(w)) => Some(*w),
            _ => None,
        }).unwrap_or(1.0))))
        .collect();
    let polygon = || ctrl.iter().map(|c| c.0).collect::<Vec<_>>();
    let p = c.int("m_Degree").unwrap_or(0).max(0) as usize;
    let knots = c.fs("m_Knots").unwrap_or(&[]);
    if p == 0 || ctrl.len() <= p || knots.len() != ctrl.len() + p + 1 || !(t1 > t0) {
        return polygon();
    }
    let n = (ctrl.len() * 8).clamp(16, 192);
    (0..=n)
        .map(|i| {
            let t = (t0 + (t1 - t0) * i as f64 / n as f64).clamp(knots[p], knots[ctrl.len()]);
            // knot span
            let mut k = p;
            while k + 1 < ctrl.len() && t >= knots[k + 1] {
                k += 1;
            }
            let mut d: Vec<[f64; 4]> = (0..=p)
                .map(|j| {
                    let (q, w) = ctrl[j + k - p];
                    [q[0] * w, q[1] * w, q[2] * w, w]
                })
                .collect();
            for r in 1..=p {
                for j in (r..=p).rev() {
                    let (a, b) = (knots[j + k - p], knots[j + 1 + k - r]);
                    let f = if b > a { (t - a) / (b - a) } else { 0.0 };
                    let prev = d[j - 1];
                    for e in 0..4 {
                        d[j][e] = prev[e] * (1.0 - f) + d[j][e] * f;
                    }
                }
            }
            let w = if d[p][3].abs() > 1e-12 { d[p][3] } else { 1.0 };
            [d[p][0] / w, d[p][1] / w, d[p][2] / w]
        })
        .collect()
}

/// The curves of a `CurveLoop` chained into one closed polyline (feet).
fn loop_points(lp: &Obj, tr: &Tree, s: &Schema) -> Vec<V3> {
    let curves: Vec<Vec<V3>> = lp
        .ptrs("m_curves")
        .iter()
        .filter_map(|&i| tr.objs.get(i))
        .map(|c| curve_points(c, s))
        .filter(|p| p.len() >= 2)
        .collect();
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
fn extrude_region(
    loops: &[Vec<[f64; 2]>],
    cuts: &[[f64; 4]],
    w0: f64,
    w1: f64,
) -> (Vec<V3>, Vec<[u32; 3]>) {
    let (w0, w1) = if w1 < w0 { (w1, w0) } else { (w0, w1) };
    let mut pts: Vec<V3> = Vec::new();
    let mut tri: Vec<[u32; 3]> = Vec::new();
    let mut us: Vec<f64> = loops
        .iter()
        .flatten()
        .map(|p| p[0])
        .filter(|x| x.is_finite())
        .collect();
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
                    Lin {
                        k,
                        m: a[1] - k * a[0],
                    }
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
        let left: Vec<(f64, f64)> = if (prev_u1 - ul).abs() < 1e-9 {
            prev.iter().map(|(a, b)| (a.at(ul), b.at(ul))).collect()
        } else {
            vec![]
        };
        let right: Vec<(f64, f64)> = cur.iter().map(|(a, b)| (a.at(ul), b.at(ul))).collect();
        for (a, b) in diff(&left, &right) {
            // covered on the left only: outward +U, edge direction +V
            quad(
                [[ul, a, w0], [ul, b, w0], [ul, b, w1], [ul, a, w1]],
                &mut pts,
                &mut tri,
            );
        }
        for (a, b) in diff(&right, &left) {
            quad(
                [[ul, b, w0], [ul, a, w0], [ul, a, w1], [ul, b, w1]],
                &mut pts,
                &mut tri,
            );
        }
        if s + 1 >= us.len() {
            break;
        }
        for (lo, hi) in &cur {
            let (a0, a1, b0, b1) = (lo.at(u0), lo.at(u1), hi.at(u0), hi.at(u1));
            // caps
            quad(
                [[u0, a0, w1], [u1, a1, w1], [u1, b1, w1], [u0, b0, w1]],
                &mut pts,
                &mut tri,
            );
            quad(
                [[u0, a0, w0], [u0, b0, w0], [u1, b1, w0], [u1, a1, w0]],
                &mut pts,
                &mut tri,
            );
            // bottom edge (outward -V) and top edge (outward +V)
            quad(
                [[u0, a0, w0], [u1, a1, w0], [u1, a1, w1], [u0, a0, w1]],
                &mut pts,
                &mut tri,
            );
            quad(
                [[u1, b1, w0], [u0, b0, w0], [u0, b0, w1], [u1, b1, w1]],
                &mut pts,
                &mut tri,
            );
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
fn solid(
    f: &Frame,
    loops: &[Vec<[f64; 2]>],
    cuts: &[[f64; 4]],
    w0: f64,
    w1: f64,
    trf: Option<&Trf>,
) -> Mesh {
    let (pts, tri) = extrude_region(loops, cuts, w0, w1);
    let mut m = Mesh {
        pos: Vec::with_capacity(pts.len()),
        tri,
        ..Default::default()
    };
    let flip = trf.is_some_and(|t| t.det() < 0.0);
    for p in pts {
        let mut q = f.at(p[0], p[1], p[2]);
        if let Some(t) = trf {
            q = t.apply(q);
        }
        m.pos.push([
            (q[0] * FEET) as f32,
            (q[1] * FEET) as f32,
            (q[2] * FEET) as f32,
        ]);
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
            if let Some(k) = left
                .iter()
                .position(|c| d2(c[0], tail) < tol || d2(*c.last().unwrap(), tail) < tol)
            {
                let mut c = left.remove(k);
                if d2(c[0], tail) >= tol {
                    c.reverse();
                }
                cur.extend(c.into_iter().skip(1));
            } else if let Some(k) = left
                .iter()
                .position(|c| d2(c[0], head) < tol || d2(*c.last().unwrap(), head) < tol)
            {
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
    (0..l.len())
        .map(|i| {
            let (a, b) = (l[i], l[(i + 1) % l.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        * 0.5
}

fn inside(p: [f64; 2], l: &[[f64; 2]]) -> bool {
    let mut c = false;
    for i in 0..l.len() {
        let (a, b) = (l[i], l[(i + 1) % l.len()]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0])
        {
            c = !c;
        }
    }
    c
}

/// Orients outer loops counter-clockwise and holes clockwise (even-odd nesting).
fn orient(mut loops: Vec<Vec<[f64; 2]>>) -> Vec<Vec<[f64; 2]>> {
    let depth: Vec<usize> = (0..loops.len())
        .map(|i| {
            (0..loops.len())
                .filter(|&j| j != i && inside(loops[i][0], &loops[j]))
                .count()
        })
        .collect();
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
    let mut ix: HashMap<usize, u32> = HashMap::default();
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
fn assemble(
    rings: &[Vec<Vec<V3>>],
    wrap: bool,
    caps: [Option<(Vec<V3>, Vec<[u32; 3]>)>; 2],
) -> Option<Mesh> {
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
            tri.extend(t.into_iter().map(|t| {
                if e == 0 {
                    [t[0] + base, t[2] + base, t[1] + base]
                } else {
                    [t[0] + base, t[1] + base, t[2] + base]
                }
            }));
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
    let vol: f64 = tri
        .iter()
        .map(|t| {
            dot(
                pts[t[0] as usize],
                cross(pts[t[1] as usize], pts[t[2] as usize]),
            )
        })
        .sum();
    if vol < 0.0 {
        for t in &mut tri {
            t.swap(1, 2);
        }
    }
    Some(Mesh {
        pos: pts
            .iter()
            .map(|q| {
                [
                    (q[0] * FEET) as f32,
                    (q[1] * FEET) as f32,
                    (q[2] * FEET) as f32,
                ]
            })
            .collect(),
        tri,
        ..Default::default()
    })
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
    let f = Frame {
        o,
        u,
        v: cross(n, u),
        n,
    };
    // the loops must lie in it
    if loops
        .iter()
        .flatten()
        .any(|&p| dot(sub(p, o), n).abs() > 1e-3 * (1.0 + dot(sub(p, o), sub(p, o)).sqrt()))
    {
        return None;
    }
    Some(f)
}

/// Rotation of `p` by `a` radians about the line through `o` along the unit `d`.
fn rotate(p: V3, o: V3, d: V3, a: f64) -> V3 {
    let v = sub(p, o);
    let (c, s) = (a.cos(), a.sin());
    add(
        o,
        add(
            add(mul(v, c), mul(cross(d, v), s)),
            mul(d, dot(d, v) * (1.0 - c)),
        ),
    )
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
    let (mut closed, open) = loops_from(&sk.curves);
    let mut axis = open.iter().find(|c| c.len() == 2).cloned();
    if closed.is_empty() || !open.iter().all(|c| c.len() == 2) {
        // the axis line touches the profile (an edge of the profile along the axis): the
        // axis is the straight curve without which the others close
        let lines: Vec<usize> = (0..sk.curves.len())
            .filter(|&k| sk.curves[k].len() == 2)
            .collect();
        let found = lines.into_iter().find_map(|k| {
            let rest: Vec<Vec<V3>> = sk
                .curves
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != k)
                .map(|(_, c)| c.clone())
                .collect();
            let (c, o) = loops_from(&rest);
            (!c.is_empty() && o.is_empty()).then(|| (c, sk.curves[k].clone()))
        });
        if let Some((c, a)) = found {
            closed = c;
            axis = Some(a);
        }
    }
    if closed.is_empty() {
        return None;
    }
    let axis = axis?;
    let (ao, ad) = (axis[0], norm(sub(axis[1], axis[0]))?);
    let f = sketch_frame(sk.plane, &closed)?;
    let prof = orient(
        closed
            .iter()
            .map(|l| l.iter().map(|&p| f.uv(p)).collect())
            .collect(),
    );
    let span = a1 - a0;
    if !(span.abs() > 1e-4 && span.abs() <= std::f64::consts::TAU + 1e-6) {
        return None;
    }
    let full = span.abs() > std::f64::consts::TAU - 1e-4;
    let n = ((span.abs() / (std::f64::consts::PI / 12.0)).ceil() as usize).clamp(3, 32);
    let at = |i: usize, q: [f64; 2]| {
        rotate(
            f.at(q[0], q[1], 0.0),
            ao,
            ad,
            a0 + span * i as f64 / n as f64,
        )
    };
    let count = if full { n } else { n + 1 };
    let rings: Vec<Vec<Vec<V3>>> = (0..count)
        .map(|i| {
            prof.iter()
                .map(|l| l.iter().map(|&q| at(i, q)).collect())
                .collect()
        })
        .collect();
    let caps = if full {
        [None, None]
    } else {
        let (cp, ct) = cap2d(&prof);
        [
            Some((cp.iter().map(|&q| at(0, q)).collect(), ct.clone())),
            Some((cp.iter().map(|&q| at(n, q)).collect(), ct)),
        ]
    };
    assemble(&rings, full, caps)
}

/// A profile family's loops (its XY plane) set across the start of a sweep path: the
/// profile's X along the horizontal square to the path, its Y up (Revit's default).
/// Direction of a path at its start `p[0]`: its first chord, turned back by half the turn to
/// the next one where the path follows a curve (chords turning by less than 20 degrees).
fn start_tangent(p: &[V3]) -> Option<V3> {
    let s0 = norm(sub(*p.get(1)?, p[0]))?;
    let Some(s1) = p.get(2).and_then(|q| norm(sub(*q, p[1]))) else { return Some(s0) };
    let c = dot(s0, s1);
    if c < 0.94 || c > 1.0 - 1e-12 {
        return Some(s0);
    }
    // the next chord mirrored across the first turns back a whole step: half of it
    let back = sub(mul(s0, 2.0 * c), s1);
    norm(add(s0, back)).or(Some(s0))
}

fn profile_on_path(
    loops: &[Vec<V3>],
    off: [f64; 3],
    flip: bool,
    path: &SketchData,
) -> Option<SketchData> {
    let (pc, po) = loops_from(&path.curves);
    let p = po.first().or(pc.first())?;
    if p.len() < 2 {
        return None;
    }
    // across the path's direction at its start (not its first chord, on an arc)
    profile_at(loops, off, flip, p[0], start_tangent(p)?)
}

/// A profile family's loops placed at `o`, across direction `t` (its x level, its y up),
/// moved by `off` (x, y, angle) and flipped.
fn profile_at(loops: &[Vec<V3>], off: [f64; 3], flip: bool, o: V3, t: V3) -> Option<SketchData> {
    let x = norm(cross([0.0, 0.0, 1.0], t)).or_else(|| norm(cross([0.0, 1.0, 0.0], t)))?;
    let y = cross(t, x);
    let (c, sn) = (off[2].cos(), off[2].sin());
    let curves = loops
        .iter()
        .map(|l| {
            let mut l: Vec<V3> = l
                .iter()
                .map(|q| {
                    let qx = if flip { -q[0] } else { q[0] };
                    let (a, b) = (qx * c - q[1] * sn + off[0], qx * sn + q[1] * c + off[1]);
                    add(o, add(mul(x, a), mul(y, b)))
                })
                .collect();
            l.push(l[0]);
            l
        })
        .collect();
    Some(SketchData {
        user: -1,
        plane: Some((o, x, y)),
        curves,
        exact: Vec::new(),
    })
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
    let seg: Vec<V3> = p
        .windows(2)
        .map(|w| norm(sub(w[1], w[0])))
        .collect::<Option<_>>()?;
    // where the profile plane meets the path
    let side = |q: V3| dot(sub(q, f.o), f.n);
    let k = (0..seg.len())
        .find(|&k| side(p[k]) * side(p[k + 1]) <= 0.0)
        .or_else(|| {
            (0..seg.len()).min_by(|&a, &b| side(p[a]).abs().total_cmp(&side(p[b]).abs()))
        })?;
    let (sa, sb) = (side(p[k]), side(p[k + 1]));
    let q = if (sa - sb).abs() > 1e-12 {
        add(p[k], mul(sub(p[k + 1], p[k]), sa / (sa - sb)))
    } else {
        p[k]
    };
    if dot(f.n, seg[k]).abs() < 0.9 {
        return None; // the profile is not across the path
    }
    let prof = orient(
        closed
            .iter()
            .map(|l| {
                l.iter()
                    .map(|&x| {
                        let d = sub(x, q);
                        [dot(d, f.u), dot(d, f.v)]
                    })
                    .collect()
            })
            .collect(),
    );
    // profile axes on each segment, carried along by the turns of the path
    let mut ax = vec![(f.u, f.v); seg.len()];
    for j in k + 1..seg.len() {
        ax[j] = (
            turn(ax[j - 1].0, seg[j - 1], seg[j]),
            turn(ax[j - 1].1, seg[j - 1], seg[j]),
        );
    }
    for j in (0..k).rev() {
        ax[j] = (
            turn(ax[j + 1].0, seg[j + 1], seg[j]),
            turn(ax[j + 1].1, seg[j + 1], seg[j]),
        );
    }
    let n = p.len();
    // the ends across the path's direction there (an arc's, not its chord's)
    let rev: Vec<V3> = p.iter().rev().copied().collect();
    let ends = [start_tangent(&p), start_tangent(&rev).map(|t| mul(t, -1.0))];
    let at = |i: usize, c: [f64; 2]| -> V3 {
        let s = if i == 0 { 0 } else { i - 1 };
        let (u, v) = ax[s];
        let x = add(p[i], add(mul(u, c[0]), mul(v, c[1])));
        if i == 0 || i + 1 == n {
            let (Some(t), d) = (ends[usize::from(i != 0)], seg[s]) else { return x };
            let c = dot(d, t);
            return if c > 0.2 { sub(x, mul(d, dot(sub(x, p[i]), t) / c)) } else { x };
        }
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
    let rings: Vec<Vec<Vec<V3>>> = (0..n)
        .map(|i| {
            prof.iter()
                .map(|l| l.iter().map(|&c| at(i, c)).collect())
                .collect()
        })
        .collect();
    let (cp, ct) = cap2d(&prof);
    let caps = [
        Some((cp.iter().map(|&c| at(0, c)).collect(), ct.clone())),
        Some((cp.iter().map(|&c| at(n - 1, c)).collect(), ct)),
    ];
    assemble(&rings, false, caps)
}

/// Loop resampled to `n` points evenly along its length.
fn resample(l: &[V3], n: usize) -> Vec<V3> {
    let m = l.len();
    let len: Vec<f64> = (0..m)
        .map(|i| dot(sub(l[(i + 1) % m], l[i]), sub(l[(i + 1) % m], l[i])).sqrt())
        .collect();
    let total: f64 = len.iter().sum();
    let mut out = Vec::with_capacity(n);
    let (mut i, mut acc) = (0usize, 0.0);
    for k in 0..n {
        let t = total * k as f64 / n as f64;
        while i < m - 1 && acc + len[i] < t {
            acc += len[i];
            i += 1;
        }
        let r = if len[i] > 0.0 {
            ((t - acc) / len[i]).clamp(0.0, 1.0)
        } else {
            0.0
        };
        out.push(add(l[i], mul(sub(l[(i + 1) % m], l[i]), r)));
    }
    out
}

/// The corners of a closed loop: its points less the closing one and those on a straight run.
fn corners(l: &[V3]) -> Vec<V3> {
    let mut v: Vec<V3> = l.to_vec();
    v.dedup_by(|a, b| dot(sub(*a, *b), sub(*a, *b)) < 1e-14);
    if v.len() > 1 && dot(sub(v[0], v[v.len() - 1]), sub(v[0], v[v.len() - 1])) < 1e-14 {
        v.pop();
    }
    let m = v.len();
    if m < 3 {
        return v;
    }
    (0..m)
        .filter(|&i| {
            let (p, q, r) = (v[(i + m - 1) % m], v[i], v[(i + 1) % m]);
            match (norm(sub(q, p)), norm(sub(r, q))) {
                (Some(d0), Some(d1)) => dot(d0, d1) < 1.0 - 1e-6,
                _ => false,
            }
        })
        .map(|i| v[i])
        .collect()
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
    // two polygons of as many corners: corner to corner (flat sides), else both resampled
    let (ca, cb) = (corners(&ccw(&la[0])), corners(&ccw(&lb[0])));
    let (ra, rb0) = if ca.len() == cb.len() && (3..=96).contains(&ca.len()) {
        (ca, cb)
    } else {
        let n = la[0].len().max(lb[0].len()).clamp(12, 96);
        (resample(&ccw(&la[0]), n), resample(&ccw(&lb[0]), n))
    };
    let n = ra.len();
    let ra: Vec<V3> = ra.into_iter().map(|p| add(p, mul(f.n, first))).collect();
    let rb0: Vec<V3> = rb0.into_iter().map(|p| add(p, mul(f.n, second))).collect();
    // start of the second loop: the rotation closest to the first loop
    let ua: Vec<[f64; 2]> = ra.iter().map(|&p| f.uv(p)).collect();
    let ub: Vec<[f64; 2]> = rb0.iter().map(|&p| f.uv(p)).collect();
    let cost = |k: usize| {
        (0..n)
            .map(|j| {
                let (p, q) = (ua[j], ub[(j + k) % n]);
                (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)
            })
            .sum::<f64>()
    };
    let k = (0..n).min_by(|&x, &y| cost(x).total_cmp(&cost(y)))?;
    let rb: Vec<V3> = (0..n).map(|j| rb0[(j + k) % n]).collect();
    let cap = |l: &[V3]| {
        let uv: Vec<[f64; 2]> = l.iter().map(|&p| f.uv(p)).collect();
        let w = dot(sub(l[0], f.o), f.n);
        let (cp, ct) = cap2d(&[uv]);
        (
            cp.iter().map(|c| f.at(c[0], c[1], w)).collect::<Vec<V3>>(),
            ct,
        )
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
    /// `self` applied after `inner`.
    pub(crate) fn after(&self, inner: &Trf) -> Trf {
        let (x, y, z) = (self.lin(inner.lin([1.0, 0.0, 0.0])), self.lin(inner.lin([0.0, 1.0, 0.0])), self.lin(inner.lin([0.0, 0.0, 1.0])));
        let m = [x[0], x[1], x[2], y[0], y[1], y[2], z[0], z[1], z[2]];
        Trf { m, or: self.apply(inner.apply([0.0; 3])) }
    }
    /// A stored placement (`m_3x3`, `m_or`): the matrix is stored row by row (the image of x
    /// is its first column), as Revit's `GetTransform` gives it.
    pub(crate) fn from(o: &Obj) -> Option<Trf> {
        let m = o.fs("m_3x3")?;
        let or = v3(o.fs("m_or"))?;
        (m.len() == 9 && m.iter().all(|x| x.is_finite())).then(|| Trf {
            m: [m[0], m[3], m[6], m[1], m[4], m[7], m[2], m[5], m[8]],
            or,
        })
    }
    pub(crate) fn apply(&self, p: V3) -> V3 {
        let m = &self.m;
        [
            m[0] * p[0] + m[3] * p[1] + m[6] * p[2] + self.or[0],
            m[1] * p[0] + m[4] * p[1] + m[7] * p[2] + self.or[1],
            m[2] * p[0] + m[5] * p[1] + m[8] * p[2] + self.or[2],
        ]
    }
    pub(crate) fn lin(&self, p: V3) -> V3 {
        let m = &self.m;
        [
            m[0] * p[0] + m[3] * p[1] + m[6] * p[2],
            m[1] * p[0] + m[4] * p[1] + m[7] * p[2],
            m[2] * p[0] + m[5] * p[1] + m[8] * p[2],
        ]
    }
    /// Rotation part made of 0 and ±1 only (axis permutations and flips).
    fn axis_aligned(&self) -> bool {
        self.m
            .iter()
            .all(|x| x.abs() < 1e-6 || (x.abs() - 1.0).abs() < 1e-6)
    }
    fn det(&self) -> f64 {
        let m = &self.m;
        dot(
            [m[0], m[1], m[2]],
            cross([m[3], m[4], m[5]], [m[6], m[7], m[8]]),
        )
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
        let Some(fl) = lid.get(&fi).copied() else {
            continue;
        };
        let Some(sf) = f.ptr("m_pSurf").and_then(|k| tr.objs.get(k)) else {
            continue;
        };
        if s.name(sf.tag) != "Plane" {
            continue;
        }
        let (Some(o), Some(x), Some(y)) = (
            v3(sf.fs("m_origin")),
            v3(sf.fs("m_xVec")),
            v3(sf.fs("m_yVec")),
        ) else {
            continue;
        };
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
            let Some(ll) = lid.get(&li).copied() else {
                continue;
            };
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
                let side = ints(ed, "m_pFace")
                    .iter()
                    .position(|&v| v == fl as i64)
                    .unwrap_or(0);
                let ends = ed.list("m_firstAndLastEdgePnts");
                let uv = |p: &Obj| {
                    p.fs("uv")
                        .filter(|u| u.len() >= 2 * side + 2)
                        .map(|u| [u[2 * side], u[2 * side + 1]])
                };
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
            m.pos.push([
                (q[0] * FEET) as f32,
                (q[1] * FEET) as f32,
                (q[2] * FEET) as f32,
            ]);
        }
        m.tri
            .extend(tri.iter().map(|t| [t[0] + base, t[1] + base, t[2] + base]));
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
    tr.root
        .obj("m_famElemVisibility")
        .and_then(|o| o.int("m_flags"))
        .unwrap_or(-1)
}

/// A family form not part of the loaded family: hidden by its `Visible` parameter
/// (-1006205 = 0 for the type stored), or the sample host of a hosted family template
/// (the extrusion that the family's voids cut in the family editor).
/// Whether a family document's form shows: its "Visible" value and the family parameter
/// driving it (-1: none). None: never (the sample host of a hosted family's template).
fn family_shown(tr: &Tree, s: &Schema) -> Option<(bool, i64)> {
    if tr.objs.iter().any(|o| s.name(o.tag) == "AddGeomToHostInFamEnvGStep") {
        return None;
    }
    let hidden = tr
        .root
        .ptr("m_pParamValueSetInt")
        .and_then(|i| tr.objs.get(i))
        .is_some_and(|set| {
            set.list("m_paramSet")
                .iter()
                .any(|p| p.int("m_paramId") == Some(-1006205) && p.int("m_value") == Some(0))
        });
    let param = tr
        .objs
        .iter()
        .filter(|o| s.name(o.tag) == "FamilyParametrizedElemParamsCell")
        .flat_map(|o| o.list("m_paramDrivenData").iter())
        .find(|d| d.int("m_elemPropId") == Some(-1006205))
        .and_then(|d| d.int("m_famParamId"))
        .unwrap_or(-1);
    Some((!hidden, param))
}

struct Form {
    id: i64,
    fam: i64,
    /// its subcategory in its family (`m_categoryId`, -1: the family's category)
    cat: i64,
    mat: Mat,
    /// A void: it cuts the family's solids.
    void: bool,
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
    Sweep {
        profile: i64,
        path: i64,
        sym: i64,
        off: [f64; 3],
        flip: bool,
    },
    /// Two sketches of one form, offset along their normal, joined by ruled faces.
    Blend { first: f64, second: f64 },
    /// Two profiles at the ends of a path joined by ruled faces: each a sketch or a profile
    /// family (sketch, profile type, offset x y angle, flipped).
    SweptBlend { path: i64, ends: [(i64, i64, [f64; 3], bool); 2] },
}

struct Form2 {
    id: i64,
    fam: i64,
    cat: i64,
    mat: Mat,
    void: bool,
    vis: i64,
    kind: Kind,
}

/// A sketch: its plane and its curves (feet).
struct SketchData {
    user: i64,
    plane: Option<(V3, V3, V3)>,
    curves: Vec<Vec<V3>>,
    /// A sweep path's curves as defined (to rebuild it for other sizes), when all are known.
    exact: Vec<Exact>,
}

/// A path curve as defined: a line (its ends), an arc (centre, axes, radius, angles).
#[derive(Debug, Clone, Copy)]
enum Exact {
    Line(V3, V3),
    Arc { c: V3, x: V3, y: V3, r: f64, t0: f64, t1: f64 },
}

impl Exact {
    fn of(c: &Obj, s: &Schema) -> Option<Exact> {
        let ep = c.fs("m_endParams")?;
        let (t0, t1) = (*ep.first()?, *ep.get(1)?);
        match s.name(c.tag) {
            "GLine" => {
                let (o, d) = (v3(c.fs("m_origin"))?, v3(c.fs("m_dirVec"))?);
                Some(Exact::Line(add(o, mul(d, t0)), add(o, mul(d, t1))))
            }
            "GArc" => {
                let r = match c.get("m_radius") {
                    Some(G::F(r)) => *r,
                    _ => return None,
                };
                let span = if t1 == t0 { std::f64::consts::TAU } else { t1 - t0 };
                Some(Exact::Arc { c: v3(c.fs("m_center"))?, x: v3(c.fs("m_xVec"))?, y: v3(c.fs("m_yVec"))?, r, t0, t1: t0 + span })
            }
            _ => None,
        }
    }

    /// The curve moved by `st`: a line by its ends, an arc by its centre and ends (it keeps
    /// its plane and angle, its radius from the centre to its ends); as points, fine.
    /// `angle`: an arc's angle for its own (one an angle parameter drives), if it changes:
    /// it then keeps its start, its radius from the centre to it.
    fn stretched(&self, st: &Stretch, angle: &dyn Fn(f64) -> Option<f64>) -> Vec<V3> {
        match *self {
            Exact::Line(a, b) => vec![st.apply(a), st.apply(b)],
            Exact::Arc { c, x, y, r, t0, t1 } => {
                let at = |t: f64| add(c, add(mul(x, r * t.cos()), mul(y, r * t.sin())));
                let (c2, a2, b2) = (st.apply(c), st.apply(at(t0)), st.apply(at(t1)));
                let len = |v: V3| dot(v, v).sqrt();
                let turned = angle((t1 - t0).abs()).filter(|a| *a > 1e-6 && *a <= std::f64::consts::TAU + 1e-9);
                let r2 = match turned {
                    Some(_) => len(sub(a2, c2)),
                    None => 0.5 * (len(sub(a2, c2)) + len(sub(b2, c2))),
                };
                let Some(u) = norm(sub(a2, c2)) else { return vec![a2, b2] };
                let v = cross(cross(x, y), u);
                let span = turned.map_or(t1 - t0, |a| a.copysign(t1 - t0));
                let n = ((span.abs() / (std::f64::consts::PI / 72.0)).ceil() as usize).clamp(2, 288);
                (0..=n)
                    .map(|i| {
                        let t = span * i as f64 / n as f64;
                        add(c2, add(mul(u, r2 * t.cos()), mul(v, r2 * t.sin())))
                    })
                    .collect()
            }
        }
    }
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
    /// material element -> its colour; element -> its family parameter element values
    mat_colors: HashMap<i64, [u8; 3]>,
    mat_patterned: crate::det::HashSet<i64>,
    mat_cut: crate::det::HashSet<i64>,
    /// family parameter -> its name
    param_names: HashMap<i64, String>,
    elem_values: HashMap<i64, HashMap<i64, i64>>,
    /// Type width and height (feet) of the family symbols that have them (doors, windows).
    sym_size: HashMap<i64, (f64, f64)>,
    /// The reference planes of each family document (by root): id, kind (`m_refName`: 0-8
    /// left, centre, right, front, centre, back, bottom, centre, top), origin, normal (feet).
    ref_planes: HashMap<i64, Vec<(i64, i64, V3, V3)>>,
    /// The reference faces of each family type as Revit regenerated them for its sizes
    /// (`m_refFaces`): first its left … top references (kinds 0-8: such a plane, else its
    /// extents), then its other reference planes by id (origin, normal, feet).
    sym_faces: HashMap<i64, Vec<Option<(V3, V3)>>>,
    /// Flipped instances having their own type (sizes of their own): its faces are flipped
    /// with it (the signs of the family axes).
    inst_signs: HashMap<i64, [f64; 3]>,
    /// Length values of each family type's parameters (feet).
    sym_lengths: HashMap<i64, HashMap<i64, f64>>,
    /// A nested family type's parameters driven by its host family's: (host, its own).
    sym_driven: HashMap<i64, Vec<(i64, i64)>>,
    /// Integer values (yes/no…) of each family type's parameters.
    sym_ints: HashMap<i64, HashMap<i64, i64>>,
    /// The angles of each family document that a parameter labels: (parameter, value).
    labelled_angles: HashMap<i64, Vec<(i64, f64)>>,
    /// Family forms whose visibility a family parameter drives: (shown by default, parameter).
    form_shown: HashMap<i64, (bool, i64)>,
    /// Meshes of the boundary representations stored by stairs, landings, parts and roofs.
    breps: Vec<(i64, Mesh)>,
    /// Rebar: id, bar type, bars (centre lines, feet).
    rebar: Vec<(i64, i64, Vec<Vec<V3>>)>,
    /// Bars of rebar systems: (system id, layer, index) → (bar type, centre line).
    system_bars: HashMap<(i64, i64, i64), (i64, Vec<Vec<V3>>)>,
    /// Rebar sets with no cached centre lines (older files): id, bar type, bar count, spacing (feet).
    rebar_sets: Vec<(i64, i64, i64, f64)>,
    /// The same for the bar sets of rebar systems: (system id, layer, index) → (bar type, count, spacing).
    system_sets: HashMap<(i64, i64, i64), (i64, i64, f64)>,
    /// Filled regions of family documents: id, family document, sketch.
    regions: Vec<(i64, i64, i64)>,
    /// Stair stringers and carriages rebuilt from their path curves: id, mesh (metres).
    stringers: Vec<(i64, Mesh)>,
    /// Bars in systems: id, system id, layer, index.
    bars_in_system: Vec<(i64, i64, i64, i64)>,
    /// Wall footing segments: id, profile loop and straight path (feet).
    footings: Vec<(i64, Vec<V3>, Vec<V3>)>,
    /// Isolated footing types: the outline (feet, family frame) of a rectangular footing.
    footing_outline: HashMap<i64, (V3, V3)>,
    /// Instances' own stored geometry (masses, in-place copies), by instance id.
    own_geom: Vec<(i64, Mesh)>,
    /// Bar diameters (feet) by bar type.
    bar_diam: HashMap<i64, f64>,
    /// Stair runs and landings: id, id of their stairs.
    stair_parts: Vec<(i64, i64)>,
    /// Curved walls: id, location curve points (feet), half thickness, reference planes.
    /// Rails (top rails, handrails): id, `Path3d` id of their top line.
    rails: Vec<(i64, i64)>,
    /// Railings: id, path sketch id, ids of their continuous rails.
    railings: Vec<(i64, i64, Vec<i64>)>,
    curved_walls: Vec<(i64, Vec<V3>, f64, Vec<(V3, V3, V3, [f64; 4])>)>,
    mep: Vec<Mep>,
    /// curve-driven instance (beam, brace) → its location line (start, end), feet
    framing: Vec<(i64, V3, V3)>,
    /// curtain mullions and panels → their placement, for a box in their own axes
    curtain: Vec<(i64, Trf)>,
    /// System mullion types (one per mullion): centre line (feet, symbol frame), depth
    /// direction, widths on either side of the line and depth (feet), rectangular or round.
    mullion_syms: HashMap<i64, MullionSym>,
    /// Profile family symbols: their profile loops (feet, profile family XY plane).
    sym_profile: HashMap<i64, Vec<Vec<V3>>>,
}

/// A system mullion type: its centre line and section.
#[derive(Clone)]
struct MullionSym {
    line: Vec<V3>,
    depth_dir: V3,
    w1: f64,
    w2: f64,
    depth: f64,
    offset: f64,
    round: bool,
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
            let env = p
                .obj("m_Envelope")
                .and_then(|e| e.fs("m_corners"))
                .unwrap_or(&[]);
            let env: [f64; 4] = if env.len() == 4 {
                env.try_into().unwrap()
            } else {
                [f64::NAN; 4]
            };
            Some((
                v3(p.fs("m_origin"))?,
                v3(p.fs("m_xVec"))?,
                v3(p.fs("m_yVec"))?,
                env,
            ))
        })
        .collect()
}

fn helper_loops(tr: &Tree, s: &Schema, helper: &str) -> Vec<Vec<V3>> {
    let Some(h) = tr.objs.iter().find(|o| s.name(o.tag).ends_with(helper)) else {
        return vec![];
    };
    h.ptrs("m_pCurveLoops")
        .iter()
        .filter_map(|&i| tr.objs.get(i))
        .map(|lp| loop_points(lp, tr, s))
        .filter(|l| l.len() >= 3)
        .collect()
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
    let up = norm(sub(
        m.up,
        [
            n[0] * dot(m.up, n),
            n[1] * dot(m.up, n),
            n[2] * dot(m.up, n),
        ],
    ))
    .or_else(|| {
        norm(cross(
            n,
            if n[2].abs() < 0.9 {
                [0.0, 0.0, 1.0]
            } else {
                [1.0, 0.0, 0.0]
            },
        ))
    })?;
    let side = norm(cross(up, n))?;
    let f = Frame {
        o: m.a,
        u: side,
        v: up,
        n,
    };
    let sec: Vec<[f64; 2]> = if m.round {
        (0..16)
            .map(|k| {
                let a = k as f64 * std::f64::consts::TAU / 16.0;
                [m.w * 0.5 * a.cos(), m.w * 0.5 * a.sin()]
            })
            .collect()
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
    let (ya, za) = (
        [y[0].abs(), y[1].abs(), y[2].abs()],
        [z[0].abs(), z[1].abs(), z[2].abs()],
    );
    let (s11, s12, s22) = (dot(ya, ya), dot(ya, za), dot(za, za));
    let (b1, b2) = (
        (0..3).map(|i| ya[i] * r[i]).sum::<f64>(),
        (0..3).map(|i| za[i] * r[i]).sum::<f64>(),
    );
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
    let m = Mep {
        id: 0,
        a: add(a, off),
        b: add(b, off),
        w,
        h,
        up: z,
        round: false,
    };
    mep(&m)
}

/// A system mullion (metres, its type's frame): its section swept along its centre line,
/// the depth along the type's path normal (shifted by the offset), the widths either side.
fn mullion(ms: &MullionSym) -> Option<Mesh> {
    let mut out = Mesh::default();
    for w in ms.line.windows(2) {
        let n = norm(sub(w[1], w[0]))?;
        let up = norm(sub(ms.depth_dir, mul(n, dot(ms.depth_dir, n))))?;
        let side = norm(cross(up, n))?;
        let shift = add(mul(side, (ms.w2 - ms.w1) * 0.5), mul(up, ms.offset));
        let m = Mep {
            id: 0,
            a: add(w[0], shift),
            b: add(w[1], shift),
            w: ms.w1 + ms.w2,
            h: ms.depth,
            up,
            round: ms.round,
        };
        let m = mep(&m)?;
        let b = out.pos.len() as u32;
        out.pos.extend(m.pos);
        out.tri
            .extend(m.tri.iter().map(|t| [t[0] + b, t[1] + b, t[2] + b]));
    }
    (!out.tri.is_empty()).then_some(out)
}

/// A box in the axes of the placement `t` whose world box is (`lo`, `hi`, metres): its edges
/// solve `e_i = sum_j |u_j[i]| l_j`. For mullions and curtain panels, whose solids are boxes.
fn oriented_box(t: &Trf, lo: [f64; 3], hi: [f64; 3]) -> Option<Mesh> {
    let e = mul(sub(hi, lo), 1.0 / FEET);
    let u = [
        norm(t.lin([1.0, 0.0, 0.0]))?,
        norm(t.lin([0.0, 1.0, 0.0]))?,
        norm(t.lin([0.0, 0.0, 1.0]))?,
    ];
    let a = |i: usize, j: usize| u[j][i].abs();
    let det3 = |c: [V3; 3]| dot(c[0], cross(c[1], c[2]));
    let cols = [
        [a(0, 0), a(1, 0), a(2, 0)],
        [a(0, 1), a(1, 1), a(2, 1)],
        [a(0, 2), a(1, 2), a(2, 2)],
    ];
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
    let m = Mep {
        id: 0,
        a: sub(c, mul(u[0], l[0] * 0.5)),
        b: add(c, mul(u[0], l[0] * 0.5)),
        w: l[1],
        h: l[2],
        up: u[2],
        round: false,
    };
    mep(&m)
}

/// Appends `m` (metres, family frame) placed by `t` (feet).
fn append(out: &mut Mesh, m: &Mesh, t: &Trf) {
    if m.tri.is_empty() {
        return;
    }
    let flip = t.det() < 0.0;
    let base = out.pos.len() as u32;
    let first = out.tri.len() as u32;
    if !m.runs.is_empty() || !out.runs.is_empty() {
        if m.runs.first().map_or(true, |r| r.0 > 0) {
            out.runs.push((first, Mat::None));
        }
        out.runs.extend(m.runs.iter().map(|&(k, mat)| (k + first, mat)));
    }
    if !m.vis.is_empty() || !out.vis.is_empty() {
        if m.vis.first().map_or(true, |r| r.0 > 0) {
            out.vis.push((first, -1));
        }
        out.vis.extend(m.vis.iter().map(|&(k, v)| (k + first, v)));
    }
    if !m.cats.is_empty() || !out.cats.is_empty() {
        if m.cats.first().map_or(true, |r| r.0 > 0) {
            out.cats.push((first, -1));
        }
        out.cats.extend(m.cats.iter().map(|&(k, c)| (k + first, c)));
    }
    for p in &m.pos {
        let q = t.apply([p[0] as f64 / FEET, p[1] as f64 / FEET, p[2] as f64 / FEET]);
        out.pos.push([
            (q[0] * FEET) as f32,
            (q[1] * FEET) as f32,
            (q[2] * FEET) as f32,
        ]);
    }
    out.tri.extend(m.tri.iter().map(|tr| {
        if flip {
            [tr[0] + base, tr[2] + base, tr[1] + base]
        } else {
            [tr[0] + base, tr[1] + base, tr[2] + base]
        }
    }));
}

/// A horizontal sheet made a slab down to `z0`: a copy at `z0` and the walls along the
/// sheet's boundary (the edges of a single triangle).
fn thicken_down(m: &mut Mesh, z0: f32) {
    let n = m.pos.len() as u32;
    let mut count: HashMap<(u32, u32), i32> = HashMap::default();
    for t in &m.tri {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *count.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let tris = m.tri.clone();
    for i in 0..n as usize {
        let p = m.pos[i];
        m.pos.push([p[0], p[1], z0]);
    }
    for t in &tris {
        m.tri.push([t[0] + n, t[2] + n, t[1] + n]);
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if count[&(a.min(b), a.max(b))] == 1 {
                m.tri.push([a, b + n, b]);
                m.tri.push([a, a + n, b + n]);
            }
        }
    }
}

/// Closed planar profile (feet) swept along a straight path: a prism (metres).
fn prism(prof: &[V3], path: &[V3]) -> Option<Mesh> {
    let [a, b] = path else { return None };
    let d = sub(*b, *a);
    let mut prof = prof.to_vec();
    if prof.len() > 3 && dot(sub(prof[0], *prof.last()?), sub(prof[0], *prof.last()?)) < 1e-10 {
        prof.pop();
    }
    if prof.len() < 3 {
        return None;
    }
    let f = sketch_frame(None, std::slice::from_ref(&prof))?;
    if dot(f.n, d).abs() < 1e-6 {
        return None;
    }
    let mut l2: Vec<[f64; 2]> = prof.iter().map(|&p| f.uv(p)).collect();
    if (area2(&l2) > 0.0) != (dot(f.n, d) > 0.0) {
        prof.reverse();
        l2.reverse();
    }
    let (c2, mut ct) = cap2d(&[l2]);
    // caps counter-clockwise about the advance direction
    let ccw = |t: &[u32; 3]| {
        let (a, b, c) = (c2[t[0] as usize], c2[t[1] as usize], c2[t[2] as usize]);
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) > 0.0
    };
    let fwd = dot(f.n, d) > 0.0;
    for t in &mut ct {
        if ccw(t) != fwd {
            t.swap(1, 2);
        }
    }
    let c0: Vec<V3> = c2.iter().map(|q| f.at(q[0], q[1], 0.0)).collect();
    let c1: Vec<V3> = c0.iter().map(|&p| add(p, d)).collect();
    let far: Vec<V3> = prof.iter().map(|&p| add(p, d)).collect();
    assemble(
        &[vec![prof], vec![far]],
        false,
        [Some((c0, ct.clone())), Some((c1, ct))],
    )
}

/// A stair stringer or carriage with no stored solid, rebuilt from the curves of its path
/// step: the longest line is its top edge, the shortest horizontal one its width, and the
/// largest face envelope shorter than the path its vertical depth.
fn stringer(s: &Schema, tr: &Tree) -> Option<Mesh> {
    let lines: Vec<(V3, V3)> = tr
        .objs
        .iter()
        .filter(|o| s.name(o.tag) == "GLine")
        .filter_map(|o| {
            let p = curve_points(o, s);
            (p.len() == 2).then(|| (p[0], p[1]))
        })
        .collect();
    let len = |l: &(V3, V3)| dot(sub(l.1, l.0), sub(l.1, l.0)).sqrt();
    let path = *lines
        .iter()
        .max_by(|a, b| len(a).partial_cmp(&len(b)).unwrap())?;
    let pl = len(&path);
    let d = sub(path.1, path.0);
    let w = lines
        .iter()
        .filter(|l| {
            let v = sub(l.1, l.0);
            v[2].abs() < 1e-6
                && len(l) > 1e-3
                && len(l) < pl * 0.5
                && dot(v, d).abs() < 1e-6 * len(l) * pl + 1e-9
        })
        .min_by(|a, b| len(a).partial_cmp(&len(b)).unwrap())?;
    let mut wv = sub(w.1, w.0);
    // the width on the side where the other edge lines start
    let pts: Vec<V3> = lines.iter().flat_map(|l| [l.0, l.1]).collect();
    let near = |q: V3| pts.iter().any(|&p| dot(sub(p, q), sub(p, q)) < 1e-6);
    if !near(add(path.0, wv)) && near(sub(path.0, wv)) {
        wv = mul(wv, -1.0);
    }
    let h = tr
        .objs
        .iter()
        .filter(|o| s.name(o.tag) == "Plane")
        .filter_map(|o| {
            o.obj("m_Envelope")
                .map(|e| e.fs("m_corners").map(|v| v.to_vec()))
                .flatten()
        })
        .flatten()
        .map(f64::abs)
        .filter(|&v| v < pl * 0.95 && v > len(w) * 1.5)
        .fold(0.0f64, f64::max);
    let h = if h > 0.0 { h } else { 1.0 };
    let z = [0.0, 0.0, h];
    let p0 = path.0;
    let prof = [p0, add(p0, wv), sub(add(p0, wv), z), sub(p0, z)];
    prism(&prof, &[path.0, path.1])
}

/// Axis-aligned cuboid (metres) between `lo` and `hi` (feet).
fn cuboid(lo: V3, hi: V3) -> Mesh {
    let mut pos = Vec::new();
    for k in 0..8 {
        let p = [
            if k & 1 == 0 { lo[0] } else { hi[0] },
            if k & 2 == 0 { lo[1] } else { hi[1] },
            if k & 4 == 0 { lo[2] } else { hi[2] },
        ];
        pos.push([
            (p[0] * FEET) as f32,
            (p[1] * FEET) as f32,
            (p[2] * FEET) as f32,
        ]);
    }
    let q = |a: u32, b: u32, c: u32, d: u32| [[a, b, c], [a, c, d]];
    let tri = [
        q(0, 2, 3, 1),
        q(4, 5, 7, 6),
        q(0, 1, 5, 4),
        q(2, 6, 7, 3),
        q(0, 4, 6, 2),
        q(1, 3, 7, 5),
    ]
    .concat();
    Mesh { pos, tri, ..Default::default() }
}

/// Whether the box of `m` is within `tol` metres of `b`.
fn near_box(m: &Mesh, b: Option<&([f64; 3], [f64; 3])>, tol: f64) -> bool {
    let (Some((lo, hi)), Some((blo, bhi))) = (m.bbox(), b) else {
        return b.is_none();
    };
    (0..3).all(|k| (lo[k] as f64 - blo[k]).abs() <= tol && (hi[k] as f64 - bhi[k]).abs() <= tol)
}

/// Appends the points of a curve to a polyline, skipping the shared end point and
/// reversing the curve when its far end is the one that touches.
fn chain_onto(line: &mut Vec<V3>, mut pts: Vec<V3>) {
    let d2 = |a: V3, b: V3| dot(sub(a, b), sub(a, b));
    if let (Some(&last), Some(&first), Some(&end)) = (line.last(), pts.first(), pts.last()) {
        if d2(last, end) < d2(last, first) {
            pts.reverse();
        }
        if d2(last, pts[0]) < 1e-8 {
            pts.remove(0);
        }
    }
    line.extend(pts);
}

/// Centre lines (feet) of a rebar set of `n` bars at spacing `sp` filling the box `lo`..`hi`
/// (feet) of bars of radius `r`: one straight bar for a line-like box, parallel straight bars
/// for a flat box, closed rectangular ties along the distribution axis for a solid box.
fn box_bars(lo: V3, hi: V3, n: i64, sp: f64, r: f64) -> Vec<Vec<V3>> {
    let e = sub(hi, lo);
    if e.iter().any(|x| !x.is_finite() || *x < 0.0) || e.iter().all(|&x| x < 1e-6) {
        return vec![];
    }
    let thin_t = (4.0 * r).max(0.08);
    let thin: Vec<usize> = (0..3).filter(|&i| e[i] <= thin_t).collect();
    let c = mul(add(lo, hi), 0.5);
    let n = n.clamp(1, 400) as usize;
    let axis = |a: usize, t: f64| -> V3 {
        let mut p = c;
        p[a] = t;
        p
    };
    // the axis whose extent best matches the set's span of (n - 1) spacings
    let pick = |cands: &[usize]| -> usize {
        let want = if sp > 0.0 && n > 1 {
            (n - 1) as f64 * sp + 2.0 * r
        } else {
            0.0
        };
        if want > 0.0 {
            *cands
                .iter()
                .min_by(|&&a, &&b| {
                    (e[a] - want)
                        .abs()
                        .partial_cmp(&(e[b] - want).abs())
                        .unwrap()
                })
                .unwrap()
        } else {
            *cands
                .iter()
                .min_by(|&&a, &&b| e[a].partial_cmp(&e[b]).unwrap())
                .unwrap()
        }
    };
    let at = |d: usize, k: usize, n: usize| -> f64 {
        let (a, b) = (lo[d] + r.min(e[d] / 2.0), hi[d] - r.min(e[d] / 2.0));
        if n <= 1 {
            (a + b) / 2.0
        } else {
            a + (b - a) * k as f64 / (n - 1) as f64
        }
    };
    match thin.len() {
        3 => vec![],
        2 => {
            let a = (0..3).find(|i| !thin.contains(i)).unwrap();
            vec![vec![axis(a, lo[a]), axis(a, hi[a])]]
        }
        1 => {
            let plane: Vec<usize> = (0..3).filter(|i| !thin.contains(i)).collect();
            let (d, a) = if n <= 1 {
                let a = if e[plane[0]] >= e[plane[1]] {
                    plane[0]
                } else {
                    plane[1]
                };
                (if a == plane[0] { plane[1] } else { plane[0] }, a)
            } else {
                let d = pick(&plane);
                (d, if d == plane[0] { plane[1] } else { plane[0] })
            };
            (0..n)
                .map(|k| {
                    let mut p = axis(a, lo[a]);
                    let mut q = axis(a, hi[a]);
                    p[d] = at(d, k, n);
                    q[d] = p[d];
                    vec![p, q]
                })
                .collect()
        }
        _ => {
            let d = pick(&[0, 1, 2]);
            let o: Vec<usize> = (0..3).filter(|&i| i != d).collect();
            let (u, v) = (o[0], o[1]);
            let (u0, u1, v0, v1) = (lo[u] + r, hi[u] - r, lo[v] + r, hi[v] - r);
            (0..n)
                .map(|k| {
                    let t = at(d, k, n);
                    [(u0, v0), (u1, v0), (u1, v1), (u0, v1), (u0, v0)]
                        .iter()
                        .map(|&(a, b)| {
                            let mut p = c;
                            p[d] = t;
                            p[u] = a;
                            p[v] = b;
                            p
                        })
                        .collect()
                })
                .collect()
        }
    }
}

/// Appends a hexagonal tube of radius `r` along the polyline `line` (feet) to `m` (metres).
fn tube(m: &mut Mesh, line: &[V3], r: f64) {
    let line: Vec<V3> = line
        .iter()
        .copied()
        .filter(|p| p.iter().all(|x| x.is_finite()))
        .collect();
    if line.len() < 2 {
        return;
    }
    const K: usize = 6;
    let mut prev_u: Option<V3> = None;
    let base = m.pos.len() as u32;
    let mut rings = 0u32;
    for i in 0..line.len() {
        let a = line[i.saturating_sub(1)];
        let b = line[(i + 1).min(line.len() - 1)];
        let Some(t) = norm(sub(b, a)) else { continue };
        // a frame carried along the line (no twist)
        let u = prev_u
            .and_then(|u| norm(sub(u, mul(t, dot(u, t)))))
            .or_else(|| norm(cross(t, [0.0, 0.0, 1.0])))
            .or_else(|| norm(cross(t, [1.0, 0.0, 0.0])));
        let Some(u) = u else { continue };
        let v = cross(t, u);
        prev_u = Some(u);
        for k in 0..K {
            let ang = std::f64::consts::TAU * k as f64 / K as f64;
            let q = add(line[i], add(mul(u, r * ang.cos()), mul(v, r * ang.sin())));
            m.pos.push([
                (q[0] * FEET) as f32,
                (q[1] * FEET) as f32,
                (q[2] * FEET) as f32,
            ]);
        }
        rings += 1;
    }
    for i in 0..rings.saturating_sub(1) {
        for k in 0..K as u32 {
            let (a, b) = (
                base + i * K as u32 + k,
                base + i * K as u32 + (k + 1) % K as u32,
            );
            let (c, d) = (a + K as u32, b + K as u32);
            m.tri.push([a, b, d]);
            m.tri.push([a, d, c]);
        }
    }
}

/// Terrain: the Delaunay triangulation (in plan) of surveyed points (feet), kept within
/// the boundary polygon (indices into `pts`) when there is one. Mesh in metres.
fn terrain(pts: &[V3], bnd: &[usize]) -> Option<Mesh> {
    if pts.len() < 3 || pts.len() > 20000 {
        return None;
    }
    let p2: Vec<[f64; 2]> = pts.iter().map(|p| [p[0], p[1]]).collect();
    let tri = delaunay(&p2);
    let poly: Vec<[f64; 2]> = bnd.iter().filter_map(|&k| p2.get(k).copied()).collect();
    let mut m = Mesh::default();
    m.pos = pts
        .iter()
        .map(|p| {
            [
                (p[0] * FEET) as f32,
                (p[1] * FEET) as f32,
                (p[2] * FEET) as f32,
            ]
        })
        .collect();
    for t in tri {
        let c = [
            (p2[t[0]][0] + p2[t[1]][0] + p2[t[2]][0]) / 3.0,
            (p2[t[0]][1] + p2[t[1]][1] + p2[t[2]][1]) / 3.0,
        ];
        if poly.len() >= 3 && !inside(c, &poly) {
            continue;
        }
        m.tri.push([t[0] as u32, t[1] as u32, t[2] as u32]);
    }
    (!m.tri.is_empty()).then_some(m)
}

/// Delaunay triangulation (Bowyer-Watson) of points in the plane: triangles of indices,
/// counter-clockwise. Duplicate points are skipped.
fn delaunay(p: &[[f64; 2]]) -> Vec<[usize; 3]> {
    let n = p.len();
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for q in p {
        for a in 0..2 {
            lo[a] = lo[a].min(q[a]);
            hi[a] = hi[a].max(q[a]);
        }
    }
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-9);
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
    let mut v: Vec<[f64; 2]> = p.to_vec();
    v.push([c[0] - 20.0 * span, c[1] - 10.0 * span]);
    v.push([c[0] + 20.0 * span, c[1] - 10.0 * span]);
    v.push([c[0], c[1] + 20.0 * span]);
    let ccw = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    };
    // triangles with their circumcircle (centre, squared radius)
    let circ = |t: [usize; 3], v: &[[f64; 2]]| {
        let (a, b, c) = (v[t[0]], v[t[1]], v[t[2]]);
        let d = 2.0 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
        let (a2, b2, c2) = (
            a[0] * a[0] + a[1] * a[1],
            b[0] * b[0] + b[1] * b[1],
            c[0] * c[0] + c[1] * c[1],
        );
        let ux = (a2 * (b[1] - c[1]) + b2 * (c[1] - a[1]) + c2 * (a[1] - b[1])) / d;
        let uy = (a2 * (c[0] - b[0]) + b2 * (a[0] - c[0]) + c2 * (b[0] - a[0])) / d;
        ([ux, uy], (a[0] - ux).powi(2) + (a[1] - uy).powi(2))
    };
    let mut tris: Vec<([usize; 3], [f64; 2], f64)> = Vec::new();
    let st = [n, n + 1, n + 2];
    let (cc, r2) = circ(st, &v);
    tris.push((st, cc, r2));
    let eps = 1e-12 * span * span;
    let mut seen: crate::det::HashSet<(i64, i64)> = crate::det::HashSet::default();
    for i in 0..n {
        let q = v[i];
        let key = (
            (q[0] / (span * 1e-9)).round() as i64,
            (q[1] / (span * 1e-9)).round() as i64,
        );
        if !seen.insert(key) {
            continue;
        }
        let mut edges: Vec<[usize; 2]> = Vec::new();
        tris.retain(|(t, c, r2)| {
            if (q[0] - c[0]).powi(2) + (q[1] - c[1]).powi(2) < r2 - eps {
                edges.extend([[t[0], t[1]], [t[1], t[2]], [t[2], t[0]]]);
                false
            } else {
                true
            }
        });
        // the boundary of the cavity: the edges of a single removed triangle
        let mut count: HashMap<(usize, usize), i32> = HashMap::default();
        for e in &edges {
            *count.entry((e[0].min(e[1]), e[0].max(e[1]))).or_default() += 1;
        }
        for e in edges {
            if count[&(e[0].min(e[1]), e[0].max(e[1]))] == 1 {
                let mut t = [e[0], e[1], i];
                if ccw(v[t[0]], v[t[1]], v[t[2]]) < 0.0 {
                    t.swap(0, 1);
                }
                let (cc, r2) = circ(t, &v);
                if r2.is_finite() {
                    tris.push((t, cc, r2));
                }
            }
        }
    }
    tris.into_iter()
        .map(|(t, ..)| t)
        .filter(|t| t.iter().all(|&k| k < n))
        .collect()
}

/// Roof slabs from their layer faces: the faces sharing a plane direction and an extent
/// (the envelope, in plane coordinates) are the layers of one slope; the slope is the
/// envelope's rectangle between the outermost layers. Mesh in metres.
fn layer_slabs(planes: &[(V3, V3, V3, [f64; 4])]) -> Option<Mesh> {
    let mut used = vec![false; planes.len()];
    let mut m = Mesh::default();
    for i in 0..planes.len() {
        if used[i] {
            continue;
        }
        let (o, x, y, env) = planes[i];
        if !env.iter().all(|v| v.is_finite())
            || (env[2] - env[0]).abs() < 1e-3
            || (env[3] - env[1]).abs() < 1e-3
            || (env[2] - env[0]).abs() > 3e3
            || (env[3] - env[1]).abs() > 3e3
        {
            continue;
        }
        let Some(n) = norm(cross(x, y)) else { continue };
        let (mut d0, mut d1) = (0.0f64, 0.0f64);
        for j in i..planes.len() {
            let (oj, xj, yj, ej) = planes[j];
            if used[j]
                || dot(xj, x) < 0.999
                || dot(yj, y) < 0.999
                || (0..4).any(|k| (ej[k] - env[k]).abs() > 1e-3)
            {
                continue;
            }
            let dd = sub(oj, o);
            if dot(dd, x).abs() > 1e-3 || dot(dd, y).abs() > 1e-3 {
                continue;
            }
            used[j] = true;
            let d = dot(dd, n);
            d0 = d0.min(d);
            d1 = d1.max(d);
        }
        if d1 - d0 < 1e-3 {
            continue; // a single face: no thickness known
        }
        let c = |u: f64, v: f64, d: f64| {
            let p = add(add(o, add(mul(x, u), mul(y, v))), mul(n, d));
            [
                (p[0] * FEET) as f32,
                (p[1] * FEET) as f32,
                (p[2] * FEET) as f32,
            ]
        };
        let base = m.pos.len() as u32;
        for k in 0..8 {
            m.pos.push(c(
                if k & 1 == 0 { env[0] } else { env[2] },
                if k & 2 == 0 { env[1] } else { env[3] },
                if k & 4 == 0 { d0 } else { d1 },
            ));
        }
        for q in [
            [0, 1, 3, 2],
            [4, 6, 7, 5],
            [0, 4, 5, 1],
            [2, 3, 7, 6],
            [0, 2, 6, 4],
            [1, 5, 7, 3],
        ] {
            m.tri.push([base + q[0], base + q[1], base + q[2]]);
            m.tri.push([base + q[0], base + q[2], base + q[3]]);
        }
    }
    (!m.tri.is_empty()).then_some(m)
}

/// A wall along the polyline `path` (feet, its z ignored): the band `±half` about it in
/// plan, from `z0` to `z1` (feet). Mesh in metres.
fn swept_wall(path: &[V3], half: f64, z0: f64, z1: f64) -> Option<Mesh> {
    let flat: Vec<V3> = path.iter().map(|p| [p[0], p[1], 0.0]).collect();
    swept_band(&flat, half, z0, z1)
}

/// The band `±half` about the polyline `path` in plan, from `z + dz0` to `z + dz1` at each
/// point (feet). Mesh in metres.
fn swept_band(path: &[V3], half: f64, dz0: f64, dz1: f64) -> Option<Mesh> {
    let n = path.len();
    if n < 2 || !(dz1 > dz0) {
        return None;
    }
    // offset directions at the vertices (mitred)
    let mut sides: Vec<([f64; 2], [f64; 2])> = Vec::with_capacity(n);
    for i in 0..n {
        let a = path[i.saturating_sub(1)];
        let b = path[(i + 1).min(n - 1)];
        let Some(t) = norm([b[0] - a[0], b[1] - a[1], 0.0]) else {
            return None;
        };
        let nn = [-t[1], t[0]];
        let p = [path[i][0], path[i][1]];
        sides.push((
            [p[0] - nn[0] * half, p[1] - nn[1] * half],
            [p[0] + nn[0] * half, p[1] + nn[1] * half],
        ));
    }
    let mut m = Mesh::default();
    let mut push = |q: [f64; 2], z: f64| -> u32 {
        m.pos.push([
            (q[0] * FEET) as f32,
            (q[1] * FEET) as f32,
            (z * FEET) as f32,
        ]);
        (m.pos.len() - 1) as u32
    };
    // per vertex: inner bottom, inner top, outer bottom, outer top
    let ids: Vec<[u32; 4]> = sides
        .iter()
        .zip(path)
        .map(|(&(a, b), p)| {
            [
                push(a, p[2] + dz0),
                push(a, p[2] + dz1),
                push(b, p[2] + dz0),
                push(b, p[2] + dz1),
            ]
        })
        .collect();
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
fn sized(
    parts: &[Mesh],
    pb: &[Option<([f32; 3], [f32; 3])>],
    w: f64,
    h: f64,
    t: &Trf,
    b: ([f64; 3], [f64; 3]),
    tol: f64,
) -> Option<Mesh> {
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
    set.list("m_paramSet")
        .iter()
        .find(|p| p.int("m_paramId") == Some(id))
        .and_then(|p| match p.get("m_value") {
            Some(G::F(v)) if v.is_finite() => Some(*v),
            _ => None,
        })
}

impl Shapes {
    /// Element classes read.
    const CLASSES: [&'static str; 40] = [
        "ParamElemFamily",
        "MaterialElem",
        "ContFooting",
        "Rebar",
        "RebarInSystem",
        "RebarSystem",
        "RebarBarType",
        "SiteSurface",
        "Roof",
        "TopRail",
        "HandRail",
        "BaseRailing",
        "StairsRun",
        "StairsLanding",
        "PartMaker",
        "ProfileRoof",
        "SWall",
        "Floor",
        "Ceiling",
        "BuildingPad",
        "RoofSoffit",
        "Toposolid",
        "ExtrusionElem",
        "RevolutionElem",
        "SweepElem",
        "BlendElem",
        "SweptBlendElem",
        "AngularDim",
        "Family",
        "FamilyInstance",
        "FamilySymbol",
        "VarSketch",
        "Sketch",
        "SketchPlane",
        "Path3d",
        "SysMullionFamSym",
        "StairsSupport",
        "FilledRegion",
        "FormElem",
        "RefPlane",
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
        // a member of a group type not placed: not drawn
        if tr.root.int("m_unplacedOwnerId").unwrap_or(-1) != -1 {
            return;
        }
        if s.isa(t, "RbsSingleCurve") {
            self.mep_curve(s, s.name(t), tr);
        } else {
            self.element(s, s.name(t), tr)
        }
    }

    pub fn partition(&mut self, s: &Schema, d: &[u8], w: usize) {
        let want = Self::wants(s);
        elements(s, d, w, &|t| want[t as usize], |_, t, _, tr| {
            self.take(s, t, &tr)
        });
    }

    /// `/Global/ContentDocuments`: one segment per family document stored in the project
    /// (`ContentMarker`, `ContentKey` GUID, size), whose root element id (the `m_famId` of the
    /// document's elements) follows the `NOBLE_SecondaryDataStorage` tag:
    /// `[tag] [root id] [-1] 01 00 00 00`.
    pub fn content_docs(&mut self, s: &Schema, d: &[u8]) {
        let (Some(cm), Some(ck), Some(ns)) = (
            s.tag_of("ContentMarker"),
            s.tag_of("ContentKey"),
            s.tag_of("NOBLE_SecondaryDataStorage"),
        ) else {
            return;
        };
        let mut head = cm.to_le_bytes().to_vec();
        head.extend([0xff; 4]);
        head.extend(ck.to_le_bytes());
        head.extend([0xff; 4]);
        let starts: Vec<usize> = d
            .windows(head.len())
            .enumerate()
            .filter(|(_, w)| *w == &head[..])
            .map(|(i, _)| i)
            .collect();
        let tag = ns.to_le_bytes();
        for (n, &p) in starts.iter().enumerate() {
            let end = starts.get(n + 1).copied().unwrap_or(d.len());
            if p + 28 > end {
                continue;
            }
            let guid: [u8; 16] = d[p + 12..p + 28].try_into().unwrap();
            let seg = &d[p + 28..end];
            let root = seg
                .windows(2)
                .enumerate()
                .filter(|(_, w)| *w == tag)
                .find_map(|(k, _)| {
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

    /// How the geometry of family document `root` fits type `sym` of instance `inst` (see
    /// `Stretch`). `solid`: the box of the family's solids (feet, family frame): the extents
    /// of the type (its left … top references held by no plane) count where the solids reach
    /// them. `doc_ext`: by family document, the extents of a type of the document's own sizes,
    /// once looked for among `types` (the family's types: id, faces).
    fn stretch_for(
        &self,
        inst: i64,
        root: i64,
        sym: i64,
        solid: Option<(V3, V3)>,
        doc_ext: &mut HashMap<i64, Option<Vec<Option<f64>>>>,
        types: &dyn Fn(i64) -> Vec<i64>,
    ) -> Option<Stretch> {
        let faces = self.sym_faces.get(&sym)?;
        let planes = self.ref_planes.get(&root)?;
        let axis = |(o, n): (V3, V3)| -> Option<(usize, f64)> {
            (0..3).find(|&a| n[a].abs() > 0.999).map(|a| (a, o[a]))
        };
        let named = |k: i64| (0..=8).contains(&k);
        if faces.len() != 9 + planes.iter().filter(|p| !named(p.1)).count() {
            return None;
        }
        let mut by_kind: [Option<(V3, V3)>; 9] = [None; 9];
        for p in planes.iter().filter(|p| named(p.1)) {
            by_kind[p.1 as usize] = Some((p.2, p.3));
        }
        // each plane of the document and its face in a type
        let pairs = |faces: &[Option<(V3, V3)>]| -> Vec<((V3, V3), (V3, V3))> {
            let named_pairs = by_kind.iter().zip(faces).filter_map(|(d, t)| Some(((*d)?, (*t)?)));
            let rest = planes.iter().filter(|p| !named(p.1)).zip(&faces[9..]).filter_map(|(p, t)| Some(((p.2, p.3), (*t)?)));
            named_pairs.chain(rest).collect()
        };
        let mine = pairs(faces);
        // the extents of a type of the document's sizes (all its planes where the document's are)
        let ext = doc_ext.entry(root).or_insert_with(|| {
            types(root).into_iter().filter_map(|t| self.sym_faces.get(&t)).filter(|f| f.len() == faces.len()).find(|f| {
                pairs(f).iter().all(|(d, t)| match (axis(*d), axis(*t)) {
                    (Some((a, x)), Some((b, y))) => a == b && (x - y).abs() < 1e-5,
                    _ => true,
                })
            }).map(|f| f[..9].iter().map(|t| t.and_then(axis).map(|(_, x)| x)).collect())
        });
        let build = |sg: [f64; 3]| -> Option<Stretch> {
            let mut axes: [Vec<(f64, f64)>; 3] = Default::default();
            for (d, t) in &mine {
                if let (Some((a, x)), Some((b, y))) = (axis(*d), axis(*t)) {
                    if a == b {
                        axes[a].push((x, y * sg[a]));
                    }
                }
            }
            // the sides of the solids no plane holds: as the type's extents
            if let (Some(ext), Some((lo, hi))) = (ext.as_ref(), solid) {
                for k in [0, 2, 3, 5, 6, 8] {
                    let a = k / 3;
                    let side = if k % 3 == 0 { lo[a] } else { hi[a] };
                    let (Some(Some(d)), Some(Some(t))) = (ext.get(k), faces.get(k)) else { continue };
                    if by_kind[k].is_some() || (d - side).abs() > 1e-3 || axes[a].iter().any(|(x, _)| (x - d).abs() < 1e-6) {
                        continue;
                    }
                    if let Some((b, y)) = axis(*t) {
                        if b == a {
                            axes[a].push((*d, y * sg[a]));
                        }
                    }
                }
            }
            Stretch::new(axes)
        };
        // a flipped instance's own type is flipped with it: its faces unflipped (taking the
        // reading whose planes move least)
        let mut best = build([1.0; 3]);
        if let Some(sg) = self.inst_signs.get(&inst) {
            if let Some(b) = build(*sg) {
                if best.as_ref().is_none_or(|a| b.amount() < a.amount()) {
                    best = Some(b);
                }
            }
        }
        best.filter(|s| s.amount() > 1e-6)
    }

    /// Whether family form `id` is read: shown, or hidden by default but shown by a family
    /// parameter for some types (kept with that parameter, see `form_shown`).
    fn keep_form(&mut self, id: i64, tr: &Tree, s: &Schema) -> bool {
        let Some(shown) = family_shown(tr, s) else { return false };
        if shown.1 != -1 {
            self.form_shown.insert(id, shown);
        }
        shown.0 || shown.1 != -1
    }

    /// Whether family form `fid` shows in a family instance of type `sym`: the value `sym`
    /// gives the family parameter driving its visibility, else its own.
    fn form_shows(&self, fid: i64, sym: i64) -> bool {
        let Some(&(def, p)) = self.form_shown.get(&fid) else { return true };
        let Some(name) = self.param_names.get(&p) else { return def };
        self.sym_ints
            .get(&sym)
            .and_then(|v| v.iter().find(|(q, _)| self.param_names.get(q) == Some(name)))
            .map_or(def, |(_, v)| *v != 0)
    }

    /// Sweep `f` rebuilt for type `sym`'s sizes: its path moved by `st` (its arcs kept round,
    /// their radius from their moved centre and ends), its profile family's sizes as the
    /// parameters of `sym` drive them (a profile sketch moved by `st`). None when its path is
    /// not made of lines and arcs.
    fn resweep(&self, f: &Form2, sym: i64, st: &Stretch) -> Option<Mesh> {
        if let Kind::SweptBlend { path, ends } = f.kind {
            let b = self.sketches.get(&path)?;
            if b.exact.is_empty() {
                return None;
            }
            let path2 = SketchData { user: b.user, plane: None, curves: b.exact.iter().map(|e| e.stretched(st, &|a| self.arc_angle(f.fam, sym, a))).collect(), exact: Vec::new() };
            let mut m = self.swept_blend(&path2, ends, Some(sym), Some(st))?;
            m.runs = vec![(0, f.mat)];
            m.vis = vec![(0, f.vis)];
            m.cats = vec![(0, if f.cat > 0 { crate::rvt_annot::scoped(f.fam, f.cat) } else { -1 })];
            return Some(m);
        }
        let Kind::Sweep { profile, path, sym: psym, off, flip } = f.kind else { return None };
        let b = self.sketches.get(&path)?;
        if b.exact.is_empty() {
            return None;
        }
        let path2 = SketchData { user: b.user, plane: None, curves: b.exact.iter().map(|e| e.stretched(st, &|a| self.arc_angle(f.fam, sym, a))).collect(), exact: Vec::new() };
        let a = match self.sketches.get(&profile) {
            Some(a) => SketchData {
                user: a.user,
                plane: a.plane.map(|(o, x, y)| (st.apply(o), x, y)),
                curves: a.curves.iter().map(|c| c.iter().map(|p| st.apply(*p)).collect()).collect(),
                exact: Vec::new(),
            },
            None => {
                let loops = self.sym_profile.get(&psym)?;
                let [sx, sy] = self.profile_scale(psym, sym, loops);
                let loops: Vec<Vec<V3>> = loops.iter().map(|l| l.iter().map(|q| [q[0] * sx, q[1] * sy, q[2]]).collect()).collect();
                profile_on_path(&loops, off, flip, &path2)?
            }
        };
        let mut m = sweep(&a, &path2)?;
        m.runs = vec![(0, f.mat)];
        m.vis = vec![(0, f.vis)];
        m.cats = vec![(0, if f.cat > 0 { crate::rvt_annot::scoped(f.fam, f.cat) } else { -1 })];
        Some(m)
    }

    /// The angle type `sym` gives an arc of family document `fam` of angle `span` (radians)
    /// when an angle dimension of that angle is labelled by a parameter: that parameter's
    /// value in `sym` (matched by name).
    fn arc_angle(&self, fam: i64, sym: i64, span: f64) -> Option<f64> {
        if std::env::var_os("RVT_ANGLE_DEBUG").is_some() {
            eprintln!("ANGLE fam {fam} sym {sym} span {span} dims {:?}", self.labelled_angles.get(&fam));
        }
        let (p, _) = self.labelled_angles.get(&fam)?.iter().find(|(_, v)| (v.abs() - span).abs() < 1e-6)?;
        let name = self.param_names.get(p)?;
        let v = self.sym_lengths.get(&sym)?.iter().find(|(q, _)| self.param_names.get(q) == Some(name)).map(|(_, v)| *v)?;
        ((v - span).abs() > 1e-9).then_some(v)
    }

    /// A swept blend: its two profiles across the ends of `path` (a straight one), joined.
    /// `sym`: the host type giving the profile families' sizes; `st`: the stretch moving its
    /// profile sketches.
    fn swept_blend(&self, path: &SketchData, ends: [(i64, i64, [f64; 3], bool); 2], sym: Option<i64>, st: Option<&Stretch>) -> Option<Mesh> {
        let (pc, po) = loops_from(&path.curves);
        let p = po.first().or(pc.first())?;
        let t = norm(sub(*p.last()?, p[0]))?;
        let place = |(sk, psym, off, flip): (i64, i64, [f64; 3], bool), o: V3| -> Option<SketchData> {
            if let Some(a) = self.sketches.get(&sk) {
                return Some(match st {
                    Some(st) => SketchData {
                        user: a.user,
                        plane: a.plane.map(|(o, x, y)| (st.apply(o), x, y)),
                        curves: a.curves.iter().map(|c| c.iter().map(|q| st.apply(*q)).collect()).collect(),
                        exact: Vec::new(),
                    },
                    None => SketchData { user: a.user, plane: a.plane, curves: a.curves.clone(), exact: Vec::new() },
                });
            }
            let loops = self.sym_profile.get(&psym)?;
            let [sx, sy] = sym.map_or([1.0; 2], |s| self.profile_scale(psym, s, loops));
            let loops: Vec<Vec<V3>> = loops.iter().map(|l| l.iter().map(|q| [q[0] * sx, q[1] * sy, q[2]]).collect()).collect();
            profile_at(&loops, off, flip, o, t)
        };
        let a = place(ends[0], p[0])?;
        let b = place(ends[1], *p.last()?)?;
        blend(&a, &b, 0.0, 0.0)
    }

    /// How a profile family's loops grow for type `sym` of its host: along its x and y, the
    /// ratio of the value `sym` gives each parameter of the host driving one of the profile's
    /// to the profile's own (the side it sizes: the one of its length, else by its name).
    fn profile_scale(&self, psym: i64, sym: i64, loops: &[Vec<V3>]) -> [f64; 2] {
        let mut sc = [1.0; 2];
        let (Some(driven), Some(own), Some(mine)) = (self.sym_driven.get(&psym), self.sym_lengths.get(&psym), self.sym_lengths.get(&sym)) else {
            return sc;
        };
        let ext = |a: usize| {
            let hi = loops.iter().flatten().map(|p| p[a]).fold(f64::NEG_INFINITY, f64::max);
            let lo = loops.iter().flatten().map(|p| p[a]).fold(f64::INFINITY, f64::min);
            hi - lo
        };
        let (ex, ey) = (ext(0), ext(1));
        for (i, (host, p)) in driven.iter().enumerate() {
            let (Some(&v0), Some(name)) = (own.get(p), self.param_names.get(host)) else { continue };
            let Some(v) = mine.iter().find(|(q, _)| self.param_names.get(q) == Some(name)).map(|(_, v)| *v) else { continue };
            if v0 <= 1e-9 || v <= 1e-9 {
                continue;
            }
            let near = |e: f64| (v0 - e).abs() < 1e-6;
            let a = match (near(ex), near(ey)) {
                (true, false) => 0,
                (false, true) => 1,
                _ => {
                    let n = self.param_names.get(p).map(|s| s.to_lowercase()).unwrap_or_default();
                    if ["haut", "height", "höhe", "alt", "h"].iter().any(|k| n.starts_with(k)) {
                        1
                    } else if ["larg", "width", "breit", "anch", "b", "w"].iter().any(|k| n.starts_with(k)) {
                        0
                    } else {
                        i.min(1)
                    }
                }
            };
            sc[a] = v / v0;
        }
        sc
    }

    /// `Family` element -> root of its family document (after `content_docs`).
    pub fn family_roots(&self) -> HashMap<i64, i64> {
        self.fam_guid.iter().filter_map(|(f, g)| Some((*f, *self.content.get(g)?))).collect()
    }

    fn mep_curve(&mut self, s: &Schema, cls: &str, tr: &Tree) {
        let r = &tr.root;
        let num = |k: &str| match r.get(k) {
            Some(G::F(v)) if v.is_finite() && *v > 1e-4 && *v < 100.0 => Some(*v),
            _ => None,
        };
        let Some(w) = num("m_dWidthOrDiameter") else {
            return;
        };
        let h = num("m_dHeight").unwrap_or(w);
        let Some(c) = r
            .ptr("m_pCurveDriver")
            .and_then(|i| tr.objs.get(i))
            .and_then(|d| d.ptr("m_pCrv"))
            .and_then(|i| tr.objs.get(i))
        else {
            return;
        };
        if s.name(c.tag) != "GLine" {
            return;
        }
        let (Some(o), Some(d)) = (v3(c.fs("m_origin")), v3(c.fs("m_dirVec"))) else {
            return;
        };
        let Some([t0, t1, ..]) = c.fs("m_endParams") else {
            return;
        };
        let Some(d) = norm(d) else { return };
        let up = v3(r.fs("m_vNormal"))
            .and_then(norm)
            .unwrap_or([0.0, 0.0, 1.0]);
        // round: pipes and conduits; ducts when their size reads as a diameter
        let size = tr
            .objs
            .iter()
            .find(|o| s.name(o.tag).ends_with("DesignPropertyManager"))
            .and_then(|o| match o.get("m_sOverallSize") {
                Some(G::S(t)) => Some(t.clone()),
                _ => None,
            });
        let round = cls.contains("Pipe")
            || (cls.contains("Conduit") && !cls.contains("CableTray"))
            || size.is_some_and(|t| t.contains(['ø', 'Ø', '⌀']));
        let at = |t: f64| [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t];
        self.mep.push(Mep {
            id: tr.id,
            a: at(*t0),
            b: at(*t1),
            w,
            h,
            up,
            round,
        });
    }

    fn element(&mut self, s: &Schema, cls: &str, tr: &Tree) {
        let id = tr.id;
        match cls {
            "ContFooting" => {
                // each segment: its profile loop swept along its straight driver line
                for seg in tr
                    .objs
                    .iter()
                    .filter(|o| s.name(o.tag) == "ContFootingSegment")
                {
                    let (Some(pl), Some(dc)) = (
                        seg.ptr("m_pProfile").and_then(|k| tr.objs.get(k)),
                        seg.ptr("m_pDriverCurve").and_then(|k| tr.objs.get(k)),
                    ) else {
                        continue;
                    };
                    let path = curve_points(dc, s);
                    let prof = loop_points(pl, tr, s);
                    if path.len() == 2 && prof.len() >= 3 {
                        self.footings.push((id, prof, path));
                    }
                }
            }
            "RebarBarType" => {
                if let Some(G::F(d)) = tr.root.get("m_barModelDiameter") {
                    if d.is_finite() && *d > 0.0 {
                        self.bar_diam.insert(id, *d);
                    }
                }
            }
            "Rebar" => {
                // the first bar's centre line, repeated at each included bar position
                let Some(ri) = tr.objs.iter().find(|o| s.name(o.tag) == "RebarInstance") else {
                    return;
                };
                let Some(cache) = ri.ptr("m_oCacheData").and_then(|k| tr.objs.get(k)) else {
                    // no cached centre lines (older files): the set rebuilt from its box later
                    let n = ri.int("m_numberOfBarPositions").unwrap_or(1).max(1);
                    let sp = ri.float("m_maxSpacing").unwrap_or(0.0);
                    self.rebar_sets
                        .push((id, ri.int("m_barTypeId").unwrap_or(-1), n, sp));
                    return;
                };
                let one = [
                    "m_noSelfIntersectingCenterlineCurves",
                    "m_selfIntersectingCenterlineCurves",
                ]
                .iter()
                .find_map(|f| cache.ptrs(f).first().and_then(|&k| tr.objs.get(k)));
                let Some(one) = one else { return };
                let mut line: Vec<V3> = Vec::new();
                for c in one
                    .ptrs("m_centerlineCurves")
                    .iter()
                    .filter_map(|&k| tr.objs.get(k))
                {
                    chain_onto(&mut line, curve_points(c, s));
                }
                if line.len() < 2 {
                    return;
                }
                let inc = match ri.get("m_includedBars") {
                    Some(G::Is(v)) => v.clone(),
                    _ => vec![],
                };
                let pos: Vec<Option<Trf>> = ri
                    .list("m_barPositionsRelativeToFirstBarArr")
                    .iter()
                    .map(Trf::from)
                    .collect();
                let ind: Vec<Option<Trf>> = ri
                    .list("m_individualBarsTrf")
                    .iter()
                    .map(Trf::from)
                    .collect();
                let mut bars = Vec::new();
                for k in 0..pos.len().max(1) {
                    if inc.get(k).is_some_and(|&v| v == 0) {
                        continue;
                    }
                    let b: Vec<V3> = line
                        .iter()
                        .map(|&p| {
                            let p = ind
                                .get(k)
                                .and_then(|t| t.as_ref())
                                .map_or(p, |t| t.apply(p));
                            pos.get(k)
                                .and_then(|t| t.as_ref())
                                .map_or(p, |t| t.apply(p))
                        })
                        .collect();
                    bars.push(b);
                }
                self.rebar
                    .push((id, ri.int("m_barTypeId").unwrap_or(-1), bars));
            }
            "RebarSystem" => {
                for (li, l) in tr.root.list("m_layers").iter().enumerate() {
                    let bt = l.int("m_barType").unwrap_or(-1);
                    let trf: Vec<Option<Trf>> = l
                        .list("m_individualLineTrf")
                        .iter()
                        .map(Trf::from)
                        .collect();
                    let curves = match l.get("m_uncutCurves") {
                        Some(G::Ps(v)) => v.clone(),
                        _ => vec![],
                    };
                    let inc = match l.get("m_includedUncutBars") {
                        Some(G::Is(v)) => v.clone(),
                        _ => vec![],
                    };
                    let lines: Vec<Option<Vec<V3>>> = curves
                        .iter()
                        .enumerate()
                        .map(|(k, c)| {
                            let c = c.and_then(|c| tr.objs.get(c))?;
                            if inc.get(k).is_some_and(|&v| v == 0) {
                                return None;
                            }
                            let mut pts = curve_points(c, s);
                            if let Some(Some(t)) = trf.get(k) {
                                pts = pts.into_iter().map(|p| t.apply(p)).collect();
                            }
                            (pts.len() >= 2).then_some(pts)
                        })
                        .collect();
                    // each uncut rebar instance (one RebarInSystem element) takes the
                    // next run of lines, as many as its bar positions
                    let insts = match l.get("m_uncutRebarInstances") {
                        Some(G::Ps(v)) => v.clone(),
                        _ => vec![],
                    };
                    if insts.is_empty() && lines.is_empty() {
                        let n = l.int("m_numberOfLines").unwrap_or(1).max(1);
                        self.system_sets.insert(
                            (id, li as i64, 0),
                            (bt, n, l.float("m_spacing").unwrap_or(0.0)),
                        );
                    }
                    let mut at = 0usize;
                    for (k, inst) in insts.iter().enumerate() {
                        let n = if insts.len() == 1 {
                            lines.len()
                        } else {
                            inst.and_then(|i| tr.objs.get(i))
                                .and_then(|o| o.int("m_numberOfBarPositions"))
                                .unwrap_or(0)
                                .max(0) as usize
                        };
                        if lines.is_empty() {
                            // no stored bar lines (older files): the set rebuilt from its box later
                            let o = inst.and_then(|i| tr.objs.get(i));
                            let n = o
                                .and_then(|o| o.int("m_numberOfBarPositions"))
                                .or_else(|| l.int("m_numberOfLines"))
                                .unwrap_or(1)
                                .max(1);
                            let sp = l
                                .float("m_spacing")
                                .or_else(|| o.and_then(|o| o.float("m_maxSpacing")))
                                .unwrap_or(0.0);
                            self.system_sets
                                .insert((id, li as i64, k as i64), (bt, n, sp));
                            continue;
                        }
                        let end = (at + n).min(lines.len());
                        let group: Vec<Vec<V3>> =
                            lines[at..end].iter().flatten().cloned().collect();
                        at = end;
                        if !group.is_empty() {
                            self.system_bars
                                .insert((id, li as i64, k as i64), (bt, group));
                        }
                    }
                }
            }
            "RebarInSystem" => {
                if let (Some(sys), Some(key)) = (tr.root.int("m_systemId"), tr.root.obj("m_key")) {
                    self.bars_in_system.push((
                        id,
                        sys,
                        key.int("m_layer").unwrap_or(0),
                        key.int("m_index").unwrap_or(0),
                    ));
                }
            }
            "SiteSurface" => {
                // the surveyed points of the latest triangulation of each driver
                let mut m = Mesh::default();
                for d in tr
                    .root
                    .ptrs("m_drivers")
                    .iter()
                    .filter_map(|&i| tr.objs.get(i))
                {
                    let Some(mm) = d.ptrs("m_pMementos").last().and_then(|&i| tr.objs.get(i))
                    else {
                        continue;
                    };
                    let pts: Vec<V3> = mm
                        .list("m_fixedVerts")
                        .iter()
                        .filter_map(|v| {
                            v3(v.fs("m_loc"))
                                .or_else(|| v.obj("m_locXYZ").and_then(|o| v3(o.fs("m_xyz"))))
                        })
                        .collect();
                    let bnd: Vec<usize> = mm
                        .list("m_boundary")
                        .first()
                        .map(|b| match b.get("m_boundary") {
                            Some(G::Is(v)) => {
                                v.iter().filter_map(|&k| usize::try_from(k).ok()).collect()
                            }
                            _ => vec![],
                        })
                        .unwrap_or_default();
                    if let Some(t) = terrain(&pts, &bnd) {
                        let base = m.pos.len() as u32;
                        m.pos.extend(t.pos);
                        m.tri.extend(
                            t.tri
                                .iter()
                                .map(|x| [x[0] + base, x[1] + base, x[2] + base]),
                        );
                    }
                }
                if m.tri.is_empty() {
                    // the triangulation not read (a faceted topology of another layout): the
                    // surface rebuilt from its cached contour lines (they run to its edge)
                    let mut pts: Vec<V3> = tr
                        .objs
                        .iter()
                        .filter(|o| s.name(o.tag) == "SurfaceContourData")
                        .flat_map(|o| o.list("m_arr"))
                        .flat_map(|c| c.list("m_arrXYZ"))
                        .filter_map(|q| v3(q.fs("m_xyz")))
                        .collect();
                    if pts.len() > 20000 {
                        let k = pts.len().div_ceil(20000);
                        pts = pts.into_iter().step_by(k).collect();
                    }
                    if let Some(t) = terrain(&pts, &[]) {
                        m = t;
                    }
                }
                if m.tri.is_empty() && std::env::var_os("RVT_FORM_DEBUG").is_some() {
                    eprintln!(
                        "SITE FAIL {id} failed {:?} objs {:?}",
                        tr.failed.map(|x| s.name(x)),
                        tr.objs.iter().map(|o| s.name(o.tag)).collect::<Vec<_>>()
                    );
                }
                if !m.tri.is_empty() {
                    self.breps.push((id, m));
                }
            }
            "Roof" => {
                if let Some(m) = layer_slabs(&planes_of(tr, "m_pRefLayerFaces")) {
                    self.breps.push((id, m));
                }
            }
            "TopRail" | "HandRail" => {
                if let Some(p) = tr
                    .objs
                    .iter()
                    .find(|o| s.name(o.tag) == "Path3dRailPathProvider")
                    .and_then(|o| o.int("m_path3dId"))
                {
                    self.rails.push((id, p));
                }
            }
            "BaseRailing" => {
                if let Some(sk) = tr.root.int("m_sketchId") {
                    let rails = tr
                        .root
                        .list("m_continuousRailIds")
                        .iter()
                        .filter_map(|r| r.int("second"))
                        .collect();
                    self.railings.push((id, sk, rails));
                }
            }
            "FormElem" => {
                // a conceptual form: its edge lines; two equal parallel loops offset along
                // their normal are an extrusion
                if !self.keep_form(id, tr, s) {
                    return;
                }
                let curves: Vec<Vec<V3>> = tr
                    .objs
                    .iter()
                    .filter(|o| {
                        s.name(o.tag) == "GLine"
                            && o.obj("m_GInfo")
                                .and_then(|g| g.int("m_categoryId"))
                                .is_some_and(|c| c != -1)
                    })
                    .map(|o| curve_points(o, s))
                    .filter(|p| p.len() == 2)
                    .collect();
                let (closed, _) = loops_from(&curves);
                if let [a, b] = closed.as_slice() {
                    let strip = |l: &Vec<V3>| -> Vec<V3> {
                        let mut l = l.clone();
                        if l.len() > 3
                            && dot(sub(l[0], *l.last().unwrap()), sub(l[0], *l.last().unwrap()))
                                < 1e-10
                        {
                            l.pop();
                        }
                        l
                    };
                    let (a, b) = (strip(a), strip(b));
                    let cen = |l: &[V3]| {
                        mul(
                            l.iter().fold([0.0; 3], |c, &p| add(c, p)),
                            1.0 / l.len() as f64,
                        )
                    };
                    let d = sub(cen(&b), cen(&a));
                    let mut nn = [0.0; 3];
                    for i in 0..a.len() {
                        nn = add(nn, cross(a[i], a[(i + 1) % a.len()]));
                    }
                    let same = a.len() == b.len()
                        && a.iter().all(|&p| {
                            b.iter()
                                .any(|&q| dot(sub(add(p, d), q), sub(add(p, d), q)) < 1e-6)
                        });
                    if let (true, Some(nz)) = (same && a.len() >= 3, norm(nn)) {
                        let h = dot(d, nz);
                        if h.abs() > 1e-6 && dot(d, d).sqrt() - h.abs() < 1e-6 {
                            self.forms.push(Form {
                                id,
                                fam: tr.root.int("m_famId").unwrap_or(-1),
                                cat: tr.root.int("m_categoryId").unwrap_or(-1),
mat: mat_of(tr, s),
void: tr.root.int("m_cutting").unwrap_or(0) != 0,
                                vis: vis_flags(tr),
                                sketch: -1,
                                loops: vec![a],
                                start: 0.0,
                                end: h,
                            });
                        }
                    }
                }
            }
            "FilledRegion" => {
                // a family's plan symbol: kept as a stand-in for families with no solids
                if let (Some(fam), Some(sk)) = (
                    tr.root.int("m_famId").filter(|&f| f > 0),
                    tr.root.int("m_sketchId"),
                ) {
                    self.regions.push((id, fam, sk));
                }
            }
            "StairsSupport" => {
                if let Some(m) = stored_brep(s, tr) {
                    self.breps.push((id, m));
                } else if let Some(m) = stringer(s, tr) {
                    self.stringers.push((id, m));
                }
            }
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
                if let Some(g) = tr
                    .objs
                    .iter()
                    .find(|o| s.name(o.tag) == "StairsElementMembership")
                    .and_then(|o| o.int("m_groupId"))
                    .filter(|&g| g > 0)
                {
                    self.stair_parts.push((id, g));
                }
                if let Some(m) = stored_brep(s, tr) {
                    self.breps.push((id, m));
                }
            }
            "SWall" => {
                let Some(dr) = tr.objs.iter().find(|o| s.name(o.tag) == "VWallDriver") else {
                    return;
                };
                let Some(c) = dr.ptr("m_pCrv").and_then(|i| tr.objs.get(i)) else {
                    return;
                };
                if s.name(c.tag) != "GLine" {
                    // curved wall: its location curve swept by the wall's thickness (twice the
                    // key reference offset, about the curve) between its bottom and top faces
                    let path = curve_points(c, s);
                    let half = match tr.root.get("m_keyRefOffset") {
                        Some(G::F(k)) if k.is_finite() && *k > 1e-3 && *k < 10.0 => *k,
                        _ => 0.25,
                    };
                    if path.len() >= 2 {
                        self.curved_walls
                            .push((id, path, half, planes_of(&tr, "m_pRefFaces")));
                    }
                    return;
                }
                let (Some(o), Some(dv)) = (v3(c.fs("m_origin")), v3(c.fs("m_dirVec"))) else {
                    return;
                };
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
                let curtain = tr.objs.iter().any(|o| {
                    s.name(o.tag).contains("Curtain") || s.name(o.tag).contains("Mullion")
                });
                self.walls.push(Wall {
                    id,
                    o,
                    d: dv,
                    ends,
                    loops,
                    planes: planes_of(&tr, "m_pRefFaces"),
                    cutters,
                    curtain,
                });
            }
            "Floor" | "Ceiling" | "BuildingPad" | "RoofSoffit" | "Toposolid" => {
                let loops = helper_loops(&tr, s, "ExtrusionHelper");
                let range = match (tr.root.get("m_bottom"), tr.root.get("m_top")) {
                    (Some(G::F(b)), Some(G::F(t)))
                        if b.is_finite() && t.is_finite() && t - b > 1e-6 && (t - b) < 1e3 =>
                    {
                        Some((*b, *t))
                    }
                    _ => None,
                };
                if !loops.is_empty() {
                    self.slabs.push(Slab {
                        id,
                        loops,
                        range,
                        planes: planes_of(&tr, "m_pRefFaces"),
                    });
                }
            }
            "ExtrusionElem" => {
                if !self.keep_form(id, tr, s) {
                    return;
                }
                let Some(h) = tr
                    .objs
                    .iter()
                    .find(|o| s.name(o.tag) == "ExtrusionElemExtrusionHelper")
                else {
                    return;
                };
                let loops = helper_loops(&tr, s, "ExtrusionElemExtrusionHelper");
                if loops.is_empty() {
                    return;
                }
                let start = param(&tr, -1001800).unwrap_or(0.0);
                let end = param(&tr, -1001801).unwrap_or(1.0);
                self.forms.push(Form {
                    id,
                    fam: tr.root.int("m_famId").unwrap_or(-1),
                    cat: tr.root.int("m_categoryId").unwrap_or(-1),
mat: mat_of(tr, s),
void: tr.root.int("m_cutting").unwrap_or(0) != 0,
                    vis: vis_flags(tr),
                    sketch: h.int("m_sketchId").unwrap_or(-1),
                    loops,
                    start,
                    end,
                });
            }
            "VarSketch" | "Sketch" => {
                let mut plane = None;
                if let Some(p) = tr.root.ptr("m_pPlane").and_then(|i| tr.objs.get(i)) {
                    if let (Some(o), Some(x), Some(y)) =
                        (v3(p.fs("m_origin")), v3(p.fs("m_xVec")), v3(p.fs("m_yVec")))
                    {
                        self.sketch_planes.insert(id, (o, x, y));
                        plane = Some((o, x, y));
                    }
                }
                let user = tr.root.int("m_userId").unwrap_or(-1);
                if user > 0 {
                    let objs: Vec<&Obj> = tr
                        .root
                        .ptrs("m_absorbedCurves")
                        .iter()
                        .filter_map(|&i| tr.objs.get(i))
                        .collect();
                    let curves = objs
                        .iter()
                        .map(|c| curve_points(c, s))
                        .filter(|p| p.len() >= 2)
                        .collect();
                    let exact: Vec<Exact> = objs.iter().filter_map(|c| Exact::of(c, s)).collect();
                    let exact = if exact.len() == objs.len() { exact } else { Vec::new() };
                    self.sketches.insert(
                        id,
                        SketchData {
                            user,
                            plane,
                            curves,
                            exact,
                        },
                    );
                }
            }
            "Path3d" => {
                // 3D sweep path: open or closed curve loops, kept like a path sketch
                let user = tr.root.int("m_userId").unwrap_or(-1);
                let objs: Vec<&Obj> = tr
                    .root
                    .ptrs("m_curveLoops")
                    .iter()
                    .filter_map(|&i| tr.objs.get(i))
                    .flat_map(|l| l.ptrs("m_curves"))
                    .filter_map(|i| tr.objs.get(i))
                    .collect();
                let curves: Vec<Vec<V3>> = objs
                    .iter()
                    .map(|c| curve_points(c, s))
                    .filter(|p| p.len() >= 2)
                    .collect();
                let exact: Vec<Exact> = objs.iter().filter_map(|c| Exact::of(c, s)).collect();
                let exact = if exact.len() == objs.len() { exact } else { Vec::new() };
                if user > 0 && !curves.is_empty() {
                    self.sketches.insert(
                        id,
                        SketchData {
                            user,
                            plane: None,
                            curves,
                            exact,
                        },
                    );
                }
            }
            "RevolutionElem" | "SweepElem" | "BlendElem" | "SweptBlendElem" => {
                if !self.keep_form(id, tr, s) {
                    return;
                }
                let fam = tr.root.int("m_famId").unwrap_or(-1);
                let kind =
                    match cls {
                        "RevolutionElem" => Kind::Revolution {
                            sketch: tr.root.int("m_sketchId").unwrap_or(-1),
                            a0: param(tr, -1001802).unwrap_or(0.0),
                            a1: param(tr, -1001803).unwrap_or(std::f64::consts::TAU),
                        },
                        "SweepElem" => {
                            let Some(pd) = tr.root.obj("m_profileData").or_else(|| {
                                tr.root.ptr("m_profileData").and_then(|i| tr.objs.get(i))
                            }) else {
                                return;
                            };
                            Kind::Sweep {
                                profile: pd.int("m_profileSketchId").unwrap_or(-1),
                                path: tr
                                    .root
                                    .int("m_pathSketchId")
                                    .filter(|&v| v > 0)
                                    .or_else(|| tr.root.int("m_path3dId"))
                                    .unwrap_or(-1),
                                sym: pd.int("m_profileFamSymId").unwrap_or(-1),
                                off: [
                                    pd.float("m_xOffset").unwrap_or(0.0),
                                    pd.float("m_yOffset").unwrap_or(0.0),
                                    pd.float("m_angle").unwrap_or(0.0),
                                ],
                                flip: pd.int("m_flippedX").unwrap_or(0) != 0,
                            }
                        }
                        "SweptBlendElem" => {
                            let pd = tr.root.list("m_profileData");
                            let end = |o: &Obj| {
                                (
                                    o.int("m_profileSketchId").unwrap_or(-1),
                                    o.int("m_profileFamSymId").unwrap_or(-1),
                                    [o.float("m_xOffset").unwrap_or(0.0), o.float("m_yOffset").unwrap_or(0.0), o.float("m_angle").unwrap_or(0.0)],
                                    o.int("m_flippedX").unwrap_or(0) != 0,
                                )
                            };
                            let (Some(a), Some(b)) = (pd.first(), pd.get(1)) else { return };
                            Kind::SweptBlend {
                                path: tr.root.int("m_pathSketchId").filter(|&v| v > 0).or_else(|| tr.root.int("m_path3dId")).unwrap_or(-1),
                                ends: [end(a), end(b)],
                            }
                        }
                        _ => Kind::Blend {
                            first: param(tr, -1001804).unwrap_or(0.0),
                            second: param(tr, -1001805).unwrap_or(1.0),
                        },
                    };
                self.forms2.push(Form2 {
                    id,
                    fam,
                    cat: tr.root.int("m_categoryId").unwrap_or(-1),
                    mat: mat_of(tr, s),
                    void: tr.root.int("m_cutting").unwrap_or(0) != 0,
                    vis: vis_flags(tr),
                    kind,
                });
            }
            "Family" => {
                let guid = tr
                    .root
                    .ptr("m_oFamDoc")
                    .and_then(|i| tr.objs.get(i))
                    .and_then(|o| {
                        let g = o.get("m_contentDocGUID")?;
                        let g = match g {
                            G::O(x) => x.get("m_guid")?,
                            _ => g,
                        };
                        match g {
                            G::Raw(b) if b.len() == 16 => {
                                Some(<[u8; 16]>::try_from(&b[..]).unwrap())
                            }
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
                            G::O(x) => x.fields.iter().find_map(|(_, g)| {
                                if let G::I(v) = g {
                                    Some(*v)
                                } else {
                                    None
                                }
                            }),
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
                let Some(ii) = tr.root.ptr("m_pInstanceInfo").and_then(|i| tr.objs.get(i)) else {
                    return;
                };
                let Some(t) = ii.obj("m_Trf").and_then(Trf::from) else {
                    return;
                };
                // flipped (hand: `m_flippedX`, facing: `m_flippedY`): its family mirrored on
                // its own axes before being placed (flipped both ways it is only turned, which
                // its placement already holds)
                let (sx, sy) = flip_signs(tr.root.int("m_flippedX").unwrap_or(0) != 0, tr.root.int("m_flippedY").unwrap_or(0) != 0);
                let t = t.after(&Trf::new([sx, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 1.0], [0.0; 3]));
                let sym = ii.int("m_symbolId").unwrap_or(-1);
                let master = tr.root.int("m_masterSymbolId").unwrap_or(-1);
                if master != -1 && master != sym && (sx < 0.0 || sy < 0.0) {
                    self.inst_signs.insert(id, [sx, sy, 1.0]);
                }
                let own = family_elem_values(tr, s);
                if !own.is_empty() {
                    self.elem_values.insert(id, own);
                }
                self.instances
                    .push((id, sym, tr.root.int("m_famId").unwrap_or(-1), t));
                // geometry stored in the instance itself (masses, copied geometry)
                if let Some(g) = tr
                    .objs
                    .iter()
                    .position(|o| s.name(o.tag) == "Geometry" && !o.ptrs("m_pFaces").is_empty())
                {
                    if let Some(m) = brep_of(s, tr, g) {
                        self.own_geom.push((id, m));
                    }
                }
                if tr
                    .objs
                    .iter()
                    .any(|o| matches!(s.name(o.tag), "MullionFamRef" | "CurtainCellFamRef"))
                {
                    self.curtain.push((id, t.clone()));
                }
                // beams and braces: the location line, for a box along it when the family solids are not found
                let crv = tr
                    .root
                    .ptr("m_pCurveDriver")
                    .and_then(|i| tr.objs.get(i))
                    .and_then(|d| {
                        let sb = d.fs("m_setbacks").map(|s| s.to_vec()).unwrap_or_default();
                        d.ptr("m_pCrv")
                            .and_then(|i| tr.objs.get(i))
                            .map(|c| (c, sb))
                    });
                if let Some((c, sb)) = crv {
                    if let (Some(o), Some(dv), Some([t0, t1, ..])) = (
                        v3(c.fs("m_origin")),
                        v3(c.fs("m_dirVec")),
                        c.fs("m_endParams"),
                    ) {
                        let (s0, s1) = (
                            sb.first().copied().unwrap_or(0.0).max(0.0),
                            sb.get(1).copied().unwrap_or(0.0).max(0.0),
                        );
                        let (a, b) = (t0 + s0, t1 - s1);
                        if b - a > 1e-3 {
                            self.framing
                                .push((id, add(o, mul(dv, a)), add(o, mul(dv, b))));
                        }
                    }
                }
            }
            "AngularDim" => {
                let fam = tr.root.int("m_famId").unwrap_or(-1);
                if fam > 0 {
                    // its segments (inline objects): the parameter labelling each, its value
                    fn walk(o: &Obj, out: &mut Vec<(i64, f64)>) {
                        if let (Some(p), Some(G::F(v))) = (o.int("m_paramId").filter(|p| *p > 0), o.get("m_lockedValue")) {
                            out.push((p, *v));
                        }
                        for (_, g) in &o.fields {
                            match g {
                                G::O(x) => walk(x, out),
                                G::L(v) => v.iter().for_each(|x| walk(x, out)),
                                _ => {}
                            }
                        }
                    }
                    let mut found = Vec::new();
                    walk(&tr.root, &mut found);
                    tr.objs.iter().for_each(|o| walk(o, &mut found));
                    self.labelled_angles.entry(fam).or_default().extend(found);
                }
            }
            "RefPlane" => {
                let fam = tr.root.int("m_famId").unwrap_or(-1);
                if fam > 0 {
                    if let Some((o, n)) = tr.root.ptr("m_pSurface").and_then(|i| tr.objs.get(i)).and_then(plane_of) {
                        self.ref_planes.entry(fam).or_default().push((id, tr.root.int("m_refName").unwrap_or(-1), o, n));
                    }
                }
            }
            "SysMullionFamSym" => {
                // centre line and section of a system mullion (rectangular or circular)
                let Some(r) = tr.objs.iter().find(|o| s.name(o.tag) == "MullionSymFamRef") else {
                    return;
                };
                let Some(c) = r.ptr("m_pCurve").and_then(|k| tr.objs.get(k)) else {
                    return;
                };
                let line = curve_points(c, s);
                let Some(dd) = v3(r.fs("m_pathNorm")).and_then(norm) else {
                    return;
                };
                let pv = tr
                    .root
                    .ptr("m_pParamValueSetDouble")
                    .and_then(|k| tr.objs.get(k));
                let get = |pid: i64| {
                    pv.and_then(|o| {
                        o.list("m_paramSet")
                            .iter()
                            .find(|p| p.int("m_paramId") == Some(pid))
                            .and_then(|p| match p.get("m_value") {
                                Some(G::F(v)) if v.is_finite() => Some(*v),
                                _ => None,
                            })
                    })
                };
                let prof = tr
                    .root
                    .ptr("m_sysProfile")
                    .and_then(|k| tr.objs.get(k))
                    .map(|o| s.name(o.tag).to_string())
                    .unwrap_or_default();
                let round = prof.contains("Circ");
                // MULLION_WIDTH1, MULLION_WIDTH2, MULLION_THICK, MULLION_OFFSET
                // (circular: MULLION_RADIUS for both)
                let (w1, w2, depth) = if round {
                    let r = get(-1007302).or(get(-1007300)).unwrap_or(0.1);
                    (r, r, 2.0 * r)
                } else {
                    (
                        get(-1007300).unwrap_or(0.08),
                        get(-1007301).unwrap_or(0.08),
                        get(-1007304).unwrap_or(0.3),
                    )
                };
                if line.len() >= 2 && w1 + w2 > 1e-4 && depth > 1e-4 {
                    self.mullion_syms.insert(
                        id,
                        MullionSym {
                            line,
                            depth_dir: dd,
                            w1,
                            w2,
                            depth,
                            offset: get(-1007351).unwrap_or(0.0),
                            round,
                        },
                    );
                }
            }
            "ParamElemFamily" => {
                if let Some(c) = tr.objs.iter().find_map(|o| match o.get("m_caption") {
                    Some(G::S(c)) => Some(c.clone()),
                    _ => None,
                }) {
                    self.param_names.insert(id, c);
                }
            }
            "MaterialElem" => {
                // a cut pattern (past the built-in solid fill): its sections are hatched
                if tr.root.ptr("m_pMaterial").and_then(|i| tr.objs.get(i)).and_then(|m| m.int("m_cutPatternId")).is_some_and(|p| p > 3) {
                    self.mat_cut.insert(id);
                }
                if tr.root.ptr("m_pMaterial").and_then(|i| tr.objs.get(i)).and_then(|m| m.int("m_surfacePatternId")).is_some_and(|p| p > 0) {
                    self.mat_patterned.insert(id);
                }
                if let Some(c) = tr.root.ptr("m_pMaterial").and_then(|i| tr.objs.get(i)).and_then(|m| m.int("m_color")) {
                    self.mat_colors.insert(id, [(c & 255) as u8, ((c >> 8) & 255) as u8, ((c >> 16) & 255) as u8]);
                }
            }
            "FamilySymbol" => {
                self.elem_values.insert(id, family_elem_values(tr, s));
                let faces: Vec<Option<(V3, V3)>> = tr
                    .root
                    .ptrs("m_refFaces")
                    .into_iter()
                    .map(|k| {
                        tr.objs
                            .get(k)
                            .and_then(|f| f.ptr("m_pSurf"))
                            .and_then(|i| tr.objs.get(i))
                            .and_then(plane_of)
                    })
                    .collect();
                if faces.len() > 9 {
                    self.sym_faces.insert(id, faces);
                }
                if let Some(ps) = tr.root.ptr("m_pParams").and_then(|k| tr.objs.get(k)) {
                    let v: HashMap<i64, f64> = ps
                        .list("m_params")
                        .iter()
                        .filter_map(|p| match p.get("m_value") {
                            Some(G::F(v)) if v.is_finite() => Some((p.int("m_paramId")?, *v)),
                            _ => None,
                        })
                        .collect();
                    if !v.is_empty() {
                        self.sym_lengths.insert(id, v);
                    }
                    let ints: HashMap<i64, i64> = ps.list("m_params").iter().filter_map(|p| Some((p.int("m_paramId")?, p.int("m_int")?))).collect();
                    if !ints.is_empty() {
                        self.sym_ints.insert(id, ints);
                    }
                }
                let driven: Vec<(i64, i64)> = tr
                    .objs
                    .iter()
                    .filter(|o| s.name(o.tag) == "FamilyParametrizedElemParamsCell")
                    .flat_map(|o| o.list("m_paramDrivenData").iter())
                    .filter_map(|d| Some((d.int("m_famParamId")?, d.int("m_elemPropId")?)))
                    .collect();
                if !driven.is_empty() {
                    self.sym_driven.insert(id, driven);
                }
                self.symbols.insert(
                    id,
                    (
                        tr.root.int("m_famId").unwrap_or(-1),
                        tr.root.int("m_familyId").unwrap_or(-1),
                    ),
                );
                // a profile family's symbol keeps its profile as curve loops
                let loops: Vec<Vec<V3>> = tr
                    .objs
                    .iter()
                    .filter(|o| s.name(o.tag) == "CurveLoop")
                    .map(|l| loop_points(l, tr, s))
                    .filter(|l| l.len() >= 3 && l.iter().all(|p| p[2].abs() < 1e-6))
                    .collect();
                if !loops.is_empty() {
                    self.sym_profile.insert(id, loops);
                }
                if let Some(ps) = tr.root.ptr("m_pParams").and_then(|k| tr.objs.get(k)) {
                    let get = |pid: i64| {
                        ps.list("m_params")
                            .iter()
                            .find(|p| p.int("m_paramId") == Some(pid))
                            .and_then(|p| match p.get("m_value") {
                                Some(G::F(v)) if v.is_finite() && *v > 0.05 => Some(*v),
                                _ => None,
                            })
                    };
                    // FAMILY_WIDTH_PARAM, FAMILY_HEIGHT_PARAM
                    if let (Some(w), Some(h)) = (get(-1001301), get(-1001300)) {
                        self.sym_size.insert(id, (w, h));
                    }
                    // a rectangular footing (STRUCTURAL_FOUNDATION_WIDTH and _LENGTH): its
                    // outline is its solid when the outline has those sizes
                    if let (Some(w), Some(l), Some([x0, y0, z0, x1, y1, z1])) = (
                        get(-1001568),
                        get(-1001569),
                        tr.root.obj("m_outline").and_then(|o| o.fs("m_minmax")),
                    ) {
                        let ok = |a: f64, b: f64| (a - b).abs() < 0.01;
                        if (ok(x1 - x0, w) && ok(y1 - y0, l) || ok(x1 - x0, l) && ok(y1 - y0, w))
                            && z1 - z0 > 0.01
                        {
                            self.footing_outline
                                .insert(id, ([*x0, *y0, *z0], [*x1, *y1, *z1]));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Builds the meshes, by element id. `boxes`: the element boxes (metres) by id, for the
    /// openings cut in the walls and to check the placed family solids.
    pub fn finish(
        self,
        boxes: &HashMap<i64, ([f64; 3], [f64; 3])>,
        family_doc: bool,
    ) -> HashMap<i64, Mesh> {
        self.finish_with(boxes, family_doc, &|_| {})
    }

    /// Same, telling `progress` how far it is (the family instances take most of the time).
    pub fn finish_with(
        self,
        boxes: &HashMap<i64, ([f64; 3], [f64; 3])>,
        family_doc: bool,
        progress: &dyn Fn(f32),
    ) -> HashMap<i64, Mesh> {
        self.finish_full(boxes, family_doc, progress).0
    }

    /// Same, with how each family instance's family was fitted to its sizes (by instance).
    pub fn finish_full(
        mut self,
        boxes: &HashMap<i64, ([f64; 3], [f64; 3])>,
        family_doc: bool,
        progress: &dyn Fn(f32),
    ) -> (HashMap<i64, Mesh>, HashMap<i64, Stretch>) {
        let mut out: HashMap<i64, Mesh> = HashMap::default();
        let mut stretches: HashMap<i64, Stretch> = HashMap::default();
        // the reference planes of each family document by id (the order of its types' faces)
        for v in self.ref_planes.values_mut() {
            v.sort_by_key(|p| p.0);
        }
        let t0 = std::time::Instant::now();
        if std::env::var_os("RVT_SHAPE_DEBUG").is_some() {
            eprintln!(
                "walls {} slabs {} forms {} sketches {} instances {} symbols {}",
                self.walls.len(),
                self.slabs.len(),
                self.forms.len(),
                self.sketch_planes.len(),
                self.instances.len(),
                self.symbols.len()
            );
        }
        for (id, m) in &self.breps {
            let mut m = m.clone();
            // a flat sheet (a roof's top face) in a thicker box: thickened down to the box
            if let (Some((lo, hi)), Some((blo, bhi))) = (m.bbox(), boxes.get(id)) {
                if hi[2] - lo[2] < 0.01
                    && bhi[2] - blo[2] > 0.02
                    && (hi[2] as f64 - bhi[2]).abs() < 0.05
                {
                    thicken_down(&mut m, blo[2] as f32);
                }
            }
            out.insert(*id, m);
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
            let zs: Vec<f64> = planes
                .iter()
                .filter(|(_, x, y, _)| norm(cross(*x, *y)).is_some_and(|n| n[2].abs() > 0.99))
                .map(|(o, ..)| o[2])
                .collect();
            let (mut z0, mut z1) = (
                zs.iter().copied().fold(f64::INFINITY, f64::min),
                zs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            );
            if !(z1 - z0 > 1e-3) {
                let Some((lo, hi)) = boxes.get(id) else {
                    continue;
                };
                (z0, z1) = (lo[2] / FEET, hi[2] / FEET);
            }
            if let Some(m) = swept_wall(path, *half, z0, z1) {
                out.insert(*id, m);
            }
        }
        // a stairs: its runs and landings
        let mut stairs: HashMap<i64, Mesh> = HashMap::default();
        for &(p, g) in &self.stair_parts {
            if let Some(m) = out.get(&p) {
                let e = stairs.entry(g).or_default();
                let base = e.pos.len() as u32;
                e.pos.extend(&m.pos);
                e.tri.extend(
                    m.tri
                        .iter()
                        .map(|t| [t[0] + base, t[1] + base, t[2] + base]),
                );
            }
        }
        for (g, m) in stairs {
            out.entry(g).or_insert(m);
        }
        self.rails_and_railings(boxes, &mut out);
        // rebar: bars as hexagonal tubes of the bar type's diameter
        let diam = |bt: i64| self.bar_diam.get(&bt).copied().unwrap_or(0.04);
        for (id, bt, bars) in &self.rebar {
            let mut m = Mesh::default();
            for b in bars {
                tube(&mut m, b, diam(*bt) / 2.0);
            }
            if !m.tri.is_empty() {
                out.insert(*id, m);
            }
        }
        for &(id, sys, layer, k) in &self.bars_in_system {
            if let Some((bt, lines)) = self.system_bars.get(&(sys, layer, k)) {
                let mut m = Mesh::default();
                for line in lines {
                    tube(&mut m, line, diam(*bt) / 2.0);
                }
                if !m.tri.is_empty() {
                    out.insert(id, m);
                }
            }
        }
        // rebar sets without stored centre lines: parallel bars (or closed ties) laid out in
        // the element's box at the set's count and spacing
        let sets = self
            .rebar_sets
            .iter()
            .map(|&(id, bt, n, sp)| (id, bt, n, sp))
            .chain(
                self.bars_in_system
                    .iter()
                    .filter_map(|&(id, sys, layer, k)| {
                        if self.system_bars.contains_key(&(sys, layer, k)) {
                            return None;
                        }
                        // a set of its own, else one bar (bars cut by an area's boundary are one per element)
                        self.system_sets
                            .get(&(sys, layer, k))
                            .map(|&(bt, n, sp)| (id, bt, n, sp))
                            .or_else(|| {
                                let bt = self
                                    .system_sets
                                    .iter()
                                    .find(|(key, _)| key.0 == sys && key.1 == layer)
                                    .or_else(|| {
                                        self.system_sets.iter().find(|(key, _)| key.0 == sys)
                                    })
                                    .map_or(-1, |(_, v)| v.0);
                                Some((id, bt, 1, 0.0))
                            })
                    }),
            );
        let sets: Vec<_> = sets.collect();
        for (id, bt, n, sp) in sets {
            if out.contains_key(&id) {
                continue;
            }
            let Some(&(lo, hi)) = boxes.get(&id) else {
                continue;
            };
            let r = diam(bt) / 2.0;
            let mut m = Mesh::default();
            for b in box_bars(mul(lo, 1.0 / FEET), mul(hi, 1.0 / FEET), n, sp, r) {
                tube(&mut m, &b, r);
            }
            if !m.tri.is_empty() {
                out.insert(id, m);
            }
        }
        for (id, m) in &self.stringers {
            let mut m = m.clone();
            // trimmed at the floor and landing it bears on: the box's bottom
            if let Some((lo, _)) = boxes.get(id) {
                for p in &mut m.pos {
                    p[2] = p[2].max(lo[2] as f32);
                }
            }
            out.entry(*id).or_insert(m);
        }
        for sl in &self.slabs {
            if let Some(m) = slab(sl) {
                out.insert(sl.id, m);
            }
        }
        if std::env::var_os("RVT_SHAPE_DEBUG").is_some() {
            let mut h: HashMap<i64, usize> = HashMap::default();
            for v in self
                .forms
                .iter()
                .map(|f| f.vis)
                .chain(self.forms2.iter().map(|f| f.vis))
            {
                *h.entry(v).or_default() += 1;
            }
            let mut h: Vec<_> = h.into_iter().collect();
            h.sort_by_key(|x| std::cmp::Reverse(x.1));
            eprintln!(
                "form visibility flags {:?}",
                &h[..h.len().min(16)]
                    .iter()
                    .map(|(v, n)| format!("{v:#x}:{n}"))
                    .collect::<Vec<_>>()
            );
        }
        // family solids, in their family's coordinates
        progress(0.1);
        let mut by_fam: HashMap<i64, Vec<Mesh>> = HashMap::default();
        let mut voids: HashMap<i64, Vec<Mesh>> = HashMap::default();
        // the form each of a family's solids comes from (same order)
        let mut src_of: HashMap<i64, Vec<i64>> = HashMap::default();
        // the solids hidden by default that a family parameter shows for some types
        let mut extra_fam: HashMap<i64, Vec<(i64, Mesh)>> = HashMap::default();
        for f in self.forms.iter() {
            if let Some(mut m) = self.form(f) {
                if f.void {
                    if !family_doc {
                        voids.entry(f.fam).or_default().push(m);
                    }
                    continue;
                }
                m.runs = vec![(0, f.mat)];
                m.vis = vec![(0, f.vis)];
                m.cats = vec![(0, if f.cat > 0 { crate::rvt_annot::scoped(f.fam, f.cat) } else { -1 })];
                if self.form_shown.get(&f.id).is_some_and(|s| !s.0) {
                    if !family_doc {
                        extra_fam.entry(f.fam).or_default().push((f.id, m));
                    }
                } else if family_doc {
                    // the family file itself: its solids as they are
                    out.insert(f.id, m);
                } else {
                    by_fam.entry(f.fam).or_default().push(m);
                    src_of.entry(f.fam).or_default().push(f.id);
                }
            }
        }
        for f in self.forms2.iter() {
            let m = match f.kind {
                Kind::Revolution { sketch, a0, a1 } => self
                    .sketches
                    .get(&sketch)
                    .and_then(|sk| revolution(sk, a0, a1)),
                Kind::Sweep {
                    profile,
                    path,
                    sym,
                    off,
                    flip,
                } => match (self.sketches.get(&profile), self.sketches.get(&path)) {
                    (Some(a), Some(b)) => sweep(a, b),
                    (None, Some(b)) => self
                        .sym_profile
                        .get(&sym)
                        .and_then(|l| profile_on_path(l, off, flip, b))
                        .and_then(|a| sweep(&a, b)),
                    _ => None,
                },
                Kind::SweptBlend { path, ends } => self.sketches.get(&path).and_then(|p| self.swept_blend(p, ends, None, None)),
                Kind::Blend { first, second } => {
                    let mut sk: Vec<(&i64, &SketchData)> = self
                        .sketches
                        .iter()
                        .filter(|(_, d)| d.user == f.id)
                        .collect();
                    sk.sort_by_key(|(id, _)| **id);
                    match sk.as_slice() {
                        [(_, a), (_, b)] => blend(a, b, first, second),
                        _ => None,
                    }
                }
            };
            if m.is_none() && std::env::var_os("RVT_FORM_DEBUG").is_some() {
                let kind = match f.kind {
                    Kind::Revolution { sketch, .. } => format!(
                        "revolution sketch {sketch} known {}",
                        self.sketches.contains_key(&sketch)
                    ),
                    Kind::Sweep { profile, path, .. } => format!(
                        "sweep profile {profile} {} path {path} {}",
                        self.sketches.contains_key(&profile),
                        self.sketches.contains_key(&path)
                    ),
                    Kind::Blend { .. } => "blend".into(),
                    Kind::SweptBlend { path, .. } => format!("swept blend path {path} {}", self.sketches.contains_key(&path)),
                };
                eprintln!("FORM2 FAIL {} fam {} {kind}", f.id, f.fam);
            }
            if let Some(mut m) = m {
                if f.void {
                    if !family_doc {
                        voids.entry(f.fam).or_default().push(m);
                    }
                    continue;
                }
                m.runs = vec![(0, f.mat)];
                m.vis = vec![(0, f.vis)];
                m.cats = vec![(0, if f.cat > 0 { crate::rvt_annot::scoped(f.fam, f.cat) } else { -1 })];
                if self.form_shown.get(&f.id).is_some_and(|s| !s.0) {
                    if !family_doc {
                        extra_fam.entry(f.fam).or_default().push((f.id, m));
                    }
                } else if family_doc {
                    out.insert(f.id, m);
                } else {
                    by_fam.entry(f.fam).or_default().push(m);
                    src_of.entry(f.fam).or_default().push(f.id);
                }
            }
        }
        if family_doc {
            return (out, stretches);
        }
        // the voids of each family cut the solids they reach
        progress(0.25);
        for (fam, vs) in &voids {
            let Some(solids) = by_fam.get_mut(fam) else { continue };
            for m in solids.iter_mut() {
                for v in vs {
                    let (Some(a), Some(b)) = (m.bbox(), v.bbox()) else { continue };
                    if (0..3).any(|k| a.1[k] < b.0[k] || b.1[k] < a.0[k]) {
                        continue;
                    }
                    if let Some((pos, tri)) = crate::rvt_csg::subtract((&m.pos, &m.tri), (&v.pos, &v.tri)) {
                        let runs = m.runs.first().map(|r| vec![(0, r.1)]).unwrap_or_default();
                        let vis = m.vis.first().map(|r| vec![(0, r.1)]).unwrap_or_default();
                        let cats = m.cats.first().map(|r| vec![(0, r.1)]).unwrap_or_default();
                        *m = Mesh { pos, tri, runs, patterned: m.patterned, cut_hatched: m.cut_hatched, vis, cats };
                    }
                }
            }
        }
        // families drawn only as plan symbols (no solids): their filled regions as 1 cm plates
        let solid: crate::det::HashSet<i64> = by_fam.keys().copied().collect();
        for &(id, fam, sk) in &self.regions {
            if solid.contains(&fam) {
                continue;
            }
            let Some(sd) = self.sketches.get(&sk) else {
                continue;
            };
            let (closed, _) = loops_from(&sd.curves);
            if closed.is_empty() {
                continue;
            }
            let f = Form {
                id,
                fam,
                cat: -1,
                mat: Mat::None,
                void: false,
                vis: -1,
                sketch: sk,
                loops: closed,
                start: 0.0,
                end: 0.01 / FEET,
            };
            if let Some(m) = self.form(&f) {
                by_fam.entry(fam).or_default().push(m);
            }
        }
        let plan_only: crate::det::HashSet<i64> = self
            .regions
            .iter()
            .map(|r| r.1)
            .filter(|f| !solid.contains(f) && by_fam.contains_key(f))
            .collect();
        let mut plan_insts: Vec<i64> = Vec::new();
        if std::env::var_os("RVT_SHAPE_DEBUG").is_some() {
            eprintln!("forms built in {:?}", t0.elapsed());
        }
        // a `Family` element → the root of its family document (ContentDocuments GUID)
        // (the family document's elements have the root as their `m_famId`)
        let mut owner: HashMap<i64, i64> = HashMap::default();
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
                .and_then(|v| {
                    v.iter()
                        .find_map(|s| owner.get(s).copied().filter(|&r| r > 0))
                })
                .or_else(|| {
                    self.fam_guid
                        .get(&fam)
                        .and_then(|g| self.content.get(g))
                        .copied()
                })
        };
        if std::env::var_os("RVT_SHAPE_DEBUG").is_some() {
            let roots = self
                .fam_guid
                .keys()
                .filter(|f| root_of(**f).is_some())
                .count();
            eprintln!(
                "families {} content docs {} resolved {} solids by doc {} forms2 {}",
                self.fam_guid.len(),
                self.content.len(),
                roots,
                by_fam.len(),
                self.forms2.len()
            );
            let hex = |g: &[u8; 16]| g.iter().map(|b| format!("{b:02x}")).collect::<String>();
            for (f, g) in self.fam_guid.iter().take(6) {
                eprintln!(
                    "  family {f} guid {} root {:?} solids {}",
                    hex(g),
                    root_of(*f),
                    root_of(*f).is_some_and(|r| by_fam.contains_key(&r))
                );
            }
            for (g, r) in self.content.iter().take(4) {
                eprintln!("  content {} → {r}", hex(g));
            }
            eprintln!(
                "  solids keys {:?}",
                by_fam.keys().take(12).collect::<Vec<_>>()
            );
        }
        // instances: family doc of the symbol, nested instances inside family docs
        let hosts: crate::det::HashSet<i64> = self
            .instances
            .iter()
            .map(|i| i.2)
            .filter(|&o| o > 0)
            .collect();
        let fam_of = |sym: i64| -> Option<i64> {
            let &(_, b) = self.symbols.get(&sym)?;
            [root_of(b), Some(b)]
                .into_iter()
                .flatten()
                .find(|f| *f > 0 && (by_fam.contains_key(f) || hosts.contains(f)))
        };
        let mut nested: HashMap<i64, Vec<(i64, Trf)>> = HashMap::default();
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
        fn flat(
            fam: i64,
            by_fam: &HashMap<i64, Vec<Mesh>>,
            nested: &HashMap<i64, Vec<(i64, Trf)>>,
            symbols_only: &crate::det::HashSet<i64>,
            depth: u32,
            memo: &mut HashMap<i64, Parts>,
        ) -> Parts {
            if let Some(m) = memo.get(&fam) {
                return m.clone();
            }
            let mut out: Vec<Mesh> = by_fam.get(&fam).cloned().unwrap_or_default();
            let mut size: usize = out.iter().map(|m| m.pos.len()).sum();
            if depth < 4 {
                for (f, ti) in nested.get(&fam).map(|v| v.as_slice()).unwrap_or(&[]) {
                    // a 2D symbol nested in a family (flow arrows…) has no volume: Revit
                    // shows it only in the views and states its parameters select
                    if *f == fam || size > CAP || symbols_only.contains(f) {
                        continue;
                    }
                    for m in flat(*f, by_fam, nested, symbols_only, depth + 1, memo).iter() {
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
                let c = [
                    if k & 1 == 0 { b.0[0] } else { b.1[0] },
                    if k & 2 == 0 { b.0[1] } else { b.1[1] },
                    if k & 4 == 0 { b.0[2] } else { b.1[2] },
                ];
                let q = t.apply([c[0] as f64 / FEET, c[1] as f64 / FEET, c[2] as f64 / FEET]);
                for a in 0..3 {
                    lo[a] = lo[a].min(q[a] * FEET);
                    hi[a] = hi[a].max(q[a] * FEET);
                }
            }
            (lo, hi)
        }
        let cover = 0.6;
        let mut memo: HashMap<i64, Parts> = HashMap::default();
        // the types of each family document, and the extents of one of its own sizes
        let mut types_of: HashMap<i64, Vec<i64>> = HashMap::default();
        for &t in self.sym_faces.keys() {
            if let Some(f) = fam_of(t) {
                types_of.entry(f).or_default().push(t);
            }
        }
        let mut doc_ext: HashMap<i64, Option<Vec<Option<f64>>>> = HashMap::default();
        let forms2_by_id: HashMap<i64, usize> = self.forms2.iter().enumerate().map(|(k, f)| (f.id, k)).collect();
        let mut part_boxes: HashMap<i64, std::rc::Rc<Vec<Option<([f32; 3], [f32; 3])>>>> =
            HashMap::default();
        let dbg = std::env::var_os("RVT_SHAPE_DEBUG").is_some();
        let mut st = [0usize; 6];
        progress(0.4);
        let step = (self.instances.len() / 100).max(1);
        for (k, &(id, sym, owner, t)) in self.instances.iter().enumerate() {
            if k % step == 0 {
                progress(0.4 + 0.55 * k as f32 / self.instances.len() as f32);
            }
            if owner > 0 {
                continue;
            }
            st[0] += 1;
            let Some(f) = fam_of(sym) else {
                if dbg {
                    let sy = self.symbols.get(&sym);
                    let r = sy.and_then(|&(_, b)| root_of(b));
                    eprintln!(
                        "NOFAM {id} sym {sym} {:?} root {:?} guid {} content-root-known {}",
                        sy,
                        r,
                        sy.is_some_and(|&(_, b)| self.fam_guid.contains_key(&b)),
                        r.is_some_and(|r| by_fam.contains_key(&r) || hosts.contains(&r))
                    );
                }
                continue;
            };
            st[1] += 1;
            if plan_only.contains(&f) {
                plan_insts.push(id);
            }
            let parts = flat(f, &by_fam, &nested, &plan_only, 0, &mut memo);
            if parts.iter().all(|m| m.tri.is_empty()) {
                continue;
            }
            st[2] += 1;
            // its type's sizes (or its own): the family's solids stretched to them
            let solid = parts.iter().filter_map(|m| m.bbox()).fold(None, |acc: Option<(V3, V3)>, (lo, hi)| {
                let (lo, hi) = (lo.map(|x| x as f64 / FEET), hi.map(|x| x as f64 / FEET));
                Some(match acc {
                    Some((a, b)) => ([a[0].min(lo[0]), a[1].min(lo[1]), a[2].min(lo[2])], [b[0].max(hi[0]), b[1].max(hi[1]), b[2].max(hi[2])]),
                    None => (lo, hi),
                })
            });
            let stretch = self.stretch_for(id, f, sym, solid, &mut doc_ext, &|r| types_of.get(&r).cloned().unwrap_or_default());
            let parts: Parts = match &stretch {
                Some(st) => {
                    // its sweeps rebuilt for the sizes (arcs kept round, profiles resized),
                    // unless a void of the family cuts them; its other solids stretched
                    let src = src_of.get(&f).filter(|_| !voids.contains_key(&f));
                    std::rc::Rc::new(
                        parts
                            .iter()
                            .enumerate()
                            .map(|(k, m)| {
                                let re = src.and_then(|v| v.get(k))
                                    .and_then(|fid| forms2_by_id.get(fid))
                                    .and_then(|&i| self.resweep(&self.forms2[i], sym, st));
                                if std::env::var("RVT_STRETCH_DEBUG").is_ok_and(|v| v == id.to_string()) {
                                    eprintln!("  part {k} form {:?} resweep {} pts {}", src.and_then(|v| v.get(k)), re.is_some(), m.pos.len());
                                }
                                re.unwrap_or_else(|| st.mesh(m))
                            })
                            .collect(),
                    )
                }
                None => parts,
            };
            // the solids a family parameter hides or shows for its type
            let hide: Vec<usize> = src_of.get(&f).map_or(Vec::new(), |v| {
                v.iter().enumerate().filter(|(k, fid)| *k < parts.len() && !self.form_shows(**fid, sym)).map(|(k, _)| k).collect()
            });
            let shown: Vec<&(i64, Mesh)> = extra_fam.get(&f).map_or(Vec::new(), |v| v.iter().filter(|(fid, _)| self.form_shows(*fid, sym)).collect());
            let changed = !hide.is_empty() || !shown.is_empty();
            let parts: Parts = if changed {
                let mut v: Vec<Mesh> = parts.as_ref().clone();
                for k in hide {
                    v[k] = Mesh::default();
                }
                for (fid, m) in shown {
                    let m = match &stretch {
                        Some(st) => forms2_by_id.get(fid).and_then(|&i| self.resweep(&self.forms2[i], sym, st)).unwrap_or_else(|| st.mesh(m)),
                        None => m.clone(),
                    };
                    v.push(m);
                }
                std::rc::Rc::new(v)
            } else {
                parts
            };
            let pb = match (&stretch, changed) {
                (None, false) => part_boxes
                    .entry(f)
                    .or_insert_with(|| std::rc::Rc::new(parts.iter().map(|m| m.bbox()).collect()))
                    .clone(),
                _ => std::rc::Rc::new(parts.iter().map(|m| m.bbox()).collect()),
            };
            if let Some(s) = stretch {
                if std::env::var("RVT_STRETCH_DEBUG").is_ok_and(|v| v == id.to_string()) {
                    eprintln!("STRETCH {id} sym {sym} fam {f}: {s:?}");
                }
                stretches.insert(id, s);
            }
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
            let inside = |(lo, hi): ([f64; 3], [f64; 3])| {
                (0..3).all(|a| lo[a] >= blo[a] - tol && hi[a] <= bhi[a] + tol)
            };
            let placed: Vec<Option<([f64; 3], [f64; 3])>> =
                pb.iter().map(|b| b.map(|b| placed_box(b, &t))).collect();
            let keep: Vec<usize> = (0..parts.len())
                .filter(|&k| placed[k].is_some_and(inside))
                .collect();
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
            // (or every solid lands inside it: its box also holds the reference planes, lines
            // and texts of its family, as an equipment's maintenance zone; doors and windows
            // excepted, their default type being stretched below)
            let covers = !keep.is_empty()
                && (covered == 3
                    || (covered == 2 && keep.len() == parts.len())
                    || (keep.len() == parts.len() && !self.sym_size.contains_key(&sym)));
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
                if std::env::var_os("RVT_SHAPE_DEBUG2").is_some() {
                    for b in placed.iter().flatten() {
                        eprintln!("   part {:?} {:?}", b.0, b.1);
                    }
                }
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
            if t.axis_aligned() {
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
                        continue;
                    }
                }
            }
            // else, when every solid lies in the box (a box that also holds a light's
            // throw or a fixture's clearance), the solids as placed
            let with_box = placed.iter().filter(|p| p.is_some()).count();
            let not_driven =
                !self.framing.iter().any(|f| f.0 == id) && !self.curtain.iter().any(|c| c.0 == id);
            if !keep.is_empty() && keep.len() == with_box && not_driven {
                let mut m = Mesh::default();
                for p in parts.iter() {
                    append(&mut m, p, &t);
                }
                st[5] += 1;
                out.insert(id, m);
                continue;
            }
            // else nearly every solid in the box (a fixture of another type whose few
            // extra solids stick out): the solids in the box
            if keep.len() * 10 >= with_box * 9 && keep.len() >= 3 && not_driven {
                let mut m = Mesh::default();
                for &k in &keep {
                    append(&mut m, &parts[k], &t);
                }
                st[5] += 1;
                out.insert(id, m);
                continue;
            }
            // else the solids, turned as placed, fit in the box but lie beside it (an
            // insertion point stored for another host face): moved to the box's centre
            if not_driven {
                let mut m = Mesh::default();
                for p in parts.iter() {
                    append(&mut m, p, &t);
                }
                if let Some((lo, hi)) = m.bbox() {
                    let fits = (0..3).all(|a| {
                        let (e, be) = ((hi[a] - lo[a]) as f64, bhi[a] - blo[a]);
                        e <= be + 0.02 && (be < 0.1 || e >= 0.4 * be)
                    });
                    if fits {
                        let d: [f64; 3] = std::array::from_fn(|a| {
                            (blo[a] + bhi[a]) / 2.0 - (lo[a] + hi[a]) as f64 / 2.0
                        });
                        for v in m.pos.iter_mut() {
                            for a in 0..3 {
                                v[a] += d[a] as f32;
                            }
                        }
                        st[5] += 1;
                        out.insert(id, m);
                    }
                }
            }
        }
        if dbg {
            eprintln!("instances placed in {:?}", t0.elapsed());
            eprintln!(
                "instances top {} family found {} solids {} trimmed {} off box {} fitted {}",
                st[0], st[1], st[2], st[3], st[4], st[5]
            );
        }
        progress(0.96);
        // wall footings: the profile swept along the driver line; the driver stops at the
        // wall's ends, the footing at its joins, so an axis-aligned sweep is extended or
        // trimmed to the footing's box along its axis
        for (id, prof, path) in &self.footings {
            let (mut prof, mut path) = (prof.clone(), path.clone());
            if let (Some(d), Some(&(blo, bhi))) = (norm(sub(path[1], path[0])), boxes.get(id)) {
                if let Some(k) = (0..3).find(|&k| d[k].abs() > 0.9999) {
                    let (lo, hi) = (blo[k] / FEET, bhi[k] / FEET);
                    let (s0, s1) = if d[k] > 0.0 { (lo, hi) } else { (hi, lo) };
                    let shift = s0 - path[0][k];
                    for p in &mut prof {
                        p[k] += shift;
                    }
                    path[0][k] = s0;
                    path[1][k] = s1;
                }
            }
            if let Some(m) = prism(&prof, &path) {
                let e = out.entry(*id).or_default();
                let b = e.pos.len() as u32;
                e.pos.extend(m.pos);
                e.tri
                    .extend(m.tri.iter().map(|t| [t[0] + b, t[1] + b, t[2] + b]));
            }
        }
        // plan-symbol plates: the family document holds one type's sizes, the instance's
        // box its own: the plate stretched over the box's footprint
        for id in plan_insts {
            let (Some(m), Some(&(blo, bhi))) = (out.get_mut(&id), boxes.get(&id)) else {
                continue;
            };
            let Some((lo, hi)) = m.bbox() else { continue };
            for p in &mut m.pos {
                for a in 0..2 {
                    let e = (hi[a] - lo[a]) as f64;
                    if e > 1e-4 {
                        p[a] = (blo[a] + (p[a] - lo[a]) as f64 / e * (bhi[a] - blo[a])) as f32;
                    }
                }
            }
        }
        // rectangular isolated footings with no family solids: their type's outline
        for &(id, sym, owner, t) in &self.instances {
            if owner > 0 || out.contains_key(&id) {
                continue;
            }
            if let Some(&(lo, hi)) = self.footing_outline.get(&sym) {
                let mut m = Mesh::default();
                append(&mut m, &cuboid(lo, hi), &t);
                if near_box(&m, boxes.get(&id), 0.05) {
                    out.insert(id, m);
                }
            }
        }
        // instances holding their own geometry (masses)
        for (id, m) in &self.own_geom {
            if !out.contains_key(id) && near_box(m, boxes.get(id), 0.3) {
                out.insert(*id, m.clone());
            }
        }
        let mut nf = 0;
        for (id, a, b) in &self.framing {
            if out.contains_key(id) {
                continue;
            }
            let Some(&(lo, hi)) = boxes.get(id) else {
                continue;
            };
            if let Some(m) = framing_box(*a, *b, lo, hi) {
                nf += 1;
                out.insert(*id, m);
            }
        }
        // system mullions: their type's centre line and section, placed
        let mut nm = 0;
        for &(id, sym, owner, t) in &self.instances {
            if owner > 0 || out.contains_key(&id) {
                continue;
            }
            let Some(ms) = self.mullion_syms.get(&sym) else {
                continue;
            };
            let Some(local) = mullion(ms) else { continue };
            // the placement of a mullion holds the transposed rotation
            let m3 = t.m;
            let tt = Trf::new(
                [
                    m3[0], m3[3], m3[6], m3[1], m3[4], m3[7], m3[2], m3[5], m3[8],
                ],
                t.or,
            );
            let mut m = Mesh::default();
            append(&mut m, &local, &tt);
            if dbg {
                eprintln!(
                    "MULLION {id} sym {sym} mesh {:?} box {:?} trf {:?}",
                    m.bbox(),
                    boxes.get(&id),
                    t.m
                );
            }
            // the section's position across the line (border mullions sit inside the edge)
            // and the end trims come from the box: the mullion is centred on it when its
            // sizes are the box's within the trims
            let ok = match (m.bbox(), boxes.get(&id)) {
                (Some((lo, hi)), Some((blo, bhi))) => {
                    let fits =
                        (0..3).all(|k| ((hi[k] - lo[k]) as f64 - (bhi[k] - blo[k])).abs() <= 0.06);
                    if fits {
                        let d: Vec<f32> = (0..3)
                            .map(|k| {
                                ((blo[k] + bhi[k]) * 0.5 - (lo[k] + hi[k]) as f64 * 0.5) as f32
                            })
                            .collect();
                        for p in &mut m.pos {
                            for k in 0..3 {
                                p[k] += d[k];
                            }
                        }
                    }
                    fits
                }
                (_, None) => true,
                _ => false,
            };
            if ok {
                nm += 1;
                out.insert(id, m);
            }
        }
        let mut nc = 0;
        for (id, t) in &self.curtain {
            if out.contains_key(id) {
                continue;
            }
            let Some(&(lo, hi)) = boxes.get(id) else {
                continue;
            };
            if let Some(m) = oriented_box(t, lo, hi) {
                nc += 1;
                out.insert(*id, m);
            }
        }
        if dbg {
            eprintln!(
                "framing boxes {nf} of {}, curtain boxes {nc} of {}, system mullions {nm}",
                self.framing.len(),
                self.curtain.len()
            );
        }
        // materials of the solids: the instance's own parameter values, else its type's
        let syms: HashMap<i64, i64> = self.instances.iter().map(|i| (i.0, i.1)).collect();
        for (id, m) in out.iter_mut() {
            if m.runs.is_empty() {
                continue;
            }
            // the family document's parameter, matched by name to the project's
            let named = |v: &HashMap<i64, i64>, p: i64| v.get(&p).copied().or_else(|| {
                let n = self.param_names.get(&p)?;
                v.iter().find(|(k, _)| self.param_names.get(k) == Some(n)).map(|(_, e)| *e)
            });
            let value = |p: i64| self.elem_values.get(id).and_then(|v| named(v, p)).or_else(|| syms.get(id).and_then(|s| self.elem_values.get(s)).and_then(|v| named(v, p)));
            for r in m.runs.iter_mut() {
                let e = match r.1 {
                    Mat::Param(p) => value(p),
                    Mat::Elem(e) => Some(e),
                    _ => None,
                };
                if e.is_some_and(|e| self.mat_cut.contains(&e)) {
                    m.cut_hatched = true;
                }
                if e.is_some_and(|e| self.mat_patterned.contains(&e)) {
                    m.patterned = true;
                }
                r.1 = e.and_then(|e| self.mat_colors.get(&e)).map_or(Mat::None, |c| Mat::Color(*c));
            }
        }
        (out, stretches)
    }

    /// Rails: a 5 cm square bar hanging under their top line; railings: balusters along
    /// their top rail (else their path sketch) down to the bottom of their box.
    fn rails_and_railings(
        &self,
        boxes: &HashMap<i64, ([f64; 3], [f64; 3])>,
        out: &mut HashMap<i64, Mesh>,
    ) {
        let path_of = |id: i64| -> Vec<V3> {
            let Some(sk) = self.sketches.get(&id) else {
                return vec![];
            };
            let mut pts: Vec<V3> = Vec::new();
            for c in &sk.curves {
                for &p in c {
                    if pts
                        .last()
                        .is_none_or(|q| dot(sub(*q, p), sub(*q, p)).sqrt() > 1e-6)
                    {
                        pts.push(p);
                    }
                }
            }
            pts
        };
        let bar = 0.05 / FEET;
        let mut rail_path: HashMap<i64, Vec<V3>> = HashMap::default();
        for &(id, p) in &self.rails {
            let pts = path_of(p);
            if let Some(m) = swept_band(&pts, bar / 2.0, -bar, 0.0) {
                out.insert(id, m);
                rail_path.insert(id, pts);
            }
        }
        for (id, sk, rails) in &self.railings {
            let Some(&(blo, bhi)) = boxes.get(id) else {
                continue;
            };
            let (zlo, zhi) = (blo[2] / FEET, bhi[2] / FEET);
            // posts from the top rail's line (less the bar), else from the sketch up to the box top
            let (top, drop) = match rails.iter().find_map(|r| rail_path.get(r)) {
                Some(p) => {
                    let zmin = p.iter().map(|q| q[2]).fold(f64::INFINITY, f64::min);
                    (p.clone(), (zmin - zlo).clamp(0.3 / FEET, 2.0 / FEET))
                }
                None => {
                    let p = path_of(*sk);
                    if p.is_empty() {
                        continue;
                    }
                    let base = p
                        .iter()
                        .map(|q| q[2])
                        .fold(f64::INFINITY, f64::min)
                        .max(zlo);
                    let h = zhi - base;
                    if !(h > 0.1) {
                        continue;
                    }
                    (
                        p.iter().map(|q| [q[0], q[1], q[2] - base + zhi]).collect(),
                        h,
                    )
                }
            };
            let post = 0.02 / FEET;
            let step = 0.12 / FEET;
            let mut m = Mesh::default();
            for i in 0..top.len().saturating_sub(1) {
                let (a, b) = (top[i], top[i + 1]);
                let len = dot(sub(b, a), sub(b, a)).sqrt();
                let n = (len / step).ceil().max(1.0) as usize;
                for k in 0..=n {
                    if k == n && i + 2 < top.len() {
                        continue; // shared with the next segment
                    }
                    let t = k as f64 / n as f64;
                    let c = add(a, mul(sub(b, a), t));
                    let pst = [[c[0] - post, c[1], c[2]], [c[0] + post, c[1], c[2]]];
                    if let Some(q) = swept_band(&pst, post, -drop, -bar.min(drop * 0.5)) {
                        let base = m.pos.len() as u32;
                        m.pos.extend(q.pos);
                        m.tri.extend(
                            q.tri
                                .iter()
                                .map(|t| [t[0] + base, t[1] + base, t[2] + base]),
                        );
                    }
                }
            }
            if !m.tri.is_empty() {
                out.insert(*id, m);
            }
        }
    }

    fn wall(&self, w: &Wall, boxes: &HashMap<i64, ([f64; 3], [f64; 3])>) -> Option<Mesh> {
        let z = [0.0, 0.0, 1.0];
        let d = norm([w.d[0], w.d[1], 0.0])?;
        let n = norm(cross(d, z))?;
        let f = Frame {
            o: w.o,
            u: d,
            v: z,
            n,
        };
        // thickness: side faces (normal ∥ n) and the faces across the wall (ends, top, bottom)
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for &(o, x, y, env) in &w.planes {
            let Some(pn) = norm(cross(x, y)) else {
                continue;
            };
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
            let Some(pn) = norm(cross(x, y)) else {
                continue;
            };
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
        let mut loops: Vec<Vec<[f64; 2]>> = w
            .loops
            .iter()
            .map(|l| l.iter().map(|&p| f.uv(p)).collect())
            .collect();
        if loops.is_empty() {
            if !(z1 - z0 > 1e-3 && z1 - z0 < 3e3) {
                return None;
            }
            loops.push(vec![[t0, z0], [t1, z0], [t1, z1], [t0, z1]]);
        }
        // move the profile's end points to the end faces when those lie a little beyond (joins)
        let umin = loops
            .iter()
            .flatten()
            .map(|p| p[0])
            .fold(f64::INFINITY, f64::min);
        let umax = loops
            .iter()
            .flatten()
            .map(|p| p[0])
            .fold(f64::NEG_INFINITY, f64::max);
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
        let lo_end = if e0 < umin && umin - e0 < reach {
            e0
        } else {
            umin
        };
        let hi_end = if e1 > umax && e1 - umax < reach {
            e1
        } else {
            umax
        };
        for p in loops.iter_mut().flatten() {
            if (p[0] - umin).abs() < 1e-6 {
                p[0] = lo_end;
            } else if (p[0] - umax).abs() < 1e-6 {
                p[0] = hi_end;
            }
        }
        // the loops must lie in the wall's plane
        if w.loops
            .iter()
            .flatten()
            .any(|&p| dot(sub(p, w.o), n).abs() > (hi - lo).abs() + 1.0)
        {
            return None;
        }
        let mut cuts = Vec::new();
        for c in &w.cutters {
            let Some((blo, bhi)) = boxes.get(c) else {
                continue;
            };
            let mut r = [
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ];
            let mut wr = (f64::INFINITY, f64::NEG_INFINITY);
            for k in 0..8 {
                let p = [
                    if k & 1 == 0 { blo[0] } else { bhi[0] },
                    if k & 2 == 0 { blo[1] } else { bhi[1] },
                    if k & 4 == 0 { blo[2] } else { bhi[2] },
                ];
                let p = mul(p, 1.0 / FEET);
                let q = f.uv(p);
                r = [
                    r[0].min(q[0]),
                    r[1].min(q[1]),
                    r[2].max(q[0]),
                    r[3].max(q[1]),
                ];
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
        if fm
            .loops
            .iter()
            .flatten()
            .any(|&p| dot(sub(p, o), nz).abs() > 1e-3 * (1.0 + dot(sub(p, o), sub(p, o)).sqrt()))
        {
            return None;
        }
        let loops: Vec<Vec<[f64; 2]>> = fm
            .loops
            .iter()
            .map(|l| l.iter().map(|&p| f.uv(p)).collect())
            .collect();
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
fn uniform_fit(
    parts: &[Mesh],
    placed: &[Option<([f64; 3], [f64; 3])>],
    t: &Trf,
    to: ([f64; 3], [f64; 3]),
) -> Option<Mesh> {
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for (l, h) in placed.iter().flatten() {
        for a in 0..3 {
            lo[a] = lo[a].min(l[a]);
            hi[a] = hi[a].max(h[a]);
        }
    }
    let o = [t.or[0] * FEET, t.or[1] * FEET, t.or[2] * FEET];
    // factor per axis from the box size, on the axes with some extent
    let mut ks: Vec<f64> = (0..3)
        .filter(|&a| hi[a] - lo[a] > 0.01 && to.1[a] - to.0[a] > 0.01)
        .map(|a| (to.1[a] - to.0[a]) / (hi[a] - lo[a]))
        .collect();
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
            let w = t.apply([
                q[0] as f64 * k / FEET,
                q[1] as f64 * k / FEET,
                q[2] as f64 * k / FEET,
            ]);
            m.pos.push([
                (w[0] * FEET) as f32,
                (w[1] * FEET) as f32,
                (w[2] * FEET) as f32,
            ]);
        }
        m.tri.extend(p.tri.iter().map(|tr| {
            if flip {
                [tr[0] + base, tr[2] + base, tr[1] + base]
            } else {
                [tr[0] + base, tr[1] + base, tr[2] + base]
            }
        }));
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
            p[a] = if s < 1e-4 {
                ((to.0[a] + to.1[a]) * 0.5) as f32
            } else {
                (to.0[a] + (p[a] - from.0[a]) as f64 * k[a]) as f32
            };
        }
    }
    true
}

fn slab(sl: &Slab) -> Option<Mesh> {
    let (b, t) = match sl.range {
        Some(r) => r,
        None => {
            // horizontal faces
            let zs: Vec<f64> = sl
                .planes
                .iter()
                .filter(|(_, x, y, _)| norm(cross(*x, *y)).is_some_and(|n| n[2].abs() > 0.99))
                .map(|(o, _, _, _)| o[2])
                .collect();
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
    let f = Frame {
        o: [0.0; 3],
        u: [1.0, 0.0, 0.0],
        v: [0.0, 1.0, 0.0],
        n: [0.0, 0.0, 1.0],
    };
    let loops: Vec<Vec<[f64; 2]>> = sl
        .loops
        .iter()
        .map(|l| l.iter().map(|p| [p[0], p[1]]).collect())
        .collect();
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
    fn stretch_between_planes() {
        // planes at 0 and 1 moved to 0 and 2; beyond the last, points move with it
        let st = super::Stretch::new([vec![(0.0, 0.0), (1.0, 2.0)], vec![], vec![(0.0, 0.0)]]).unwrap();
        assert!((st.map(0, 0.5) - 1.0).abs() < 1e-12);
        assert!((st.map(0, 3.0) - 4.0).abs() < 1e-12);
        assert!((st.map(0, -1.0) + 1.0).abs() < 1e-12);
        assert_eq!(st.map(1, 7.0), 7.0);
        // planes that would cross: no stretch
        assert!(super::Stretch::new([vec![(0.0, 1.0), (1.0, 0.0)], vec![], vec![]]).is_none());
    }

    #[test]
    fn arc_start_tangent() {
        // a quarter circle in 36 chords: its direction at the start is the tangent, not the
        // first chord's
        let p: Vec<[f64; 3]> = (0..=36).map(|i| {
            let t = std::f64::consts::FRAC_PI_2 * i as f64 / 36.0;
            [t.cos(), t.sin(), 0.0]
        }).collect();
        let t = super::start_tangent(&p).unwrap();
        assert!(t[0].abs() < 1e-9 && (t[1] - 1.0).abs() < 1e-9, "{t:?}");
        // a straight path: its direction
        let t = super::start_tangent(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]).unwrap();
        assert!((t[0] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn double_flip_is_a_turn() {
        assert_eq!(super::flip_signs(true, true), (1.0, 1.0));
        assert_eq!(super::flip_signs(true, false), (-1.0, 1.0));
        assert_eq!(super::flip_signs(false, true), (1.0, -1.0));
    }

    #[test]
    fn prism_box() {
        let prof = vec![
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ];
        let m = prism(&prof, &[[0.0, 0.0, 0.0], [0.0, 5.0, 0.0]]).unwrap();
        let vol: f64 = m
            .tri
            .iter()
            .map(|t| {
                let p = |k: u32| m.pos[k as usize].map(|x| x as f64 / FEET);
                dot(p(t[0]), cross(p(t[1]), p(t[2]))) / 6.0
            })
            .sum();
        assert!(
            (vol - 10.0).abs() < 1e-3,
            "{vol} {} {:?}",
            m.tri.len(),
            m.pos
        );
        let c = cuboid([0.0; 3], [1.0, 2.0, 3.0]);
        assert_eq!(c.tri.len(), 12);
    }

    #[test]
    fn delaunay_grid() {
        let p: Vec<[f64; 2]> = (0..25)
            .map(|k| [(k % 5) as f64, (k / 5) as f64 + 0.01 * (k % 3) as f64])
            .collect();
        let t = super::delaunay(&p);
        // the triangles tile the convex hull (about 4 x 4), counter-clockwise
        let area: f64 = t
            .iter()
            .map(|t| {
                let (a, b, c) = (p[t[0]], p[t[1]], p[t[2]]);
                0.5 * ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]))
            })
            .inspect(|&a| assert!(a > 0.0))
            .sum();
        assert!((area - 16.0).abs() < 0.1, "{area}");
    }

    #[test]
    fn curved_wall_band() {
        // quarter circle of radius 10 ft, 1 ft thick, 10 ft high
        let path: Vec<[f64; 3]> = (0..=16)
            .map(|k| {
                let a = std::f64::consts::FRAC_PI_2 * k as f64 / 16.0;
                [10.0 * a.cos(), 10.0 * a.sin(), 0.0]
            })
            .collect();
        let m = super::swept_wall(&path, 0.5, 0.0, 10.0).unwrap();
        let (lo, hi) = m.bbox().unwrap();
        assert!((hi[2] - 3.048).abs() < 1e-4 && lo[2].abs() < 1e-6);
        assert!((hi[0] - 10.5 * 0.3048).abs() < 1e-3, "{hi:?}");
        assert_eq!(m.tri.len(), 16 * 8 + 4);
    }

    use super::*;

    fn volume(pts: &[V3], tri: &[[u32; 3]]) -> f64 {
        tri.iter()
            .map(|t| {
                dot(
                    pts[t[0] as usize],
                    cross(pts[t[1] as usize], pts[t[2] as usize]),
                ) / 6.0
            })
            .sum()
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
        let c = vec![
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            vec![[1.0, 1.0, 0.0], [1.0, 0.0, 0.0]],
            vec![[1.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
        ];
        let l = chain(c);
        assert_eq!(l.len(), 3);
        assert_eq!(l[2], [1.0, 1.0, 0.0]);
    }
}

/// Appends an ellipsoid centred on `c` with radii `r` (metres) to `m`.
fn ellipsoid(m: &mut Mesh, c: [f64; 3], r: [f64; 3]) {
    const LAT: u32 = 6;
    const LON: u32 = 10;
    let base = m.pos.len() as u32;
    m.pos.push([c[0] as f32, c[1] as f32, (c[2] - r[2]) as f32]);
    for i in 1..LAT {
        let th = std::f64::consts::PI * i as f64 / LAT as f64;
        let (z, s) = (-th.cos(), th.sin());
        for k in 0..LON {
            let ph = std::f64::consts::TAU * k as f64 / LON as f64;
            m.pos.push([
                (c[0] + r[0] * s * ph.cos()) as f32,
                (c[1] + r[1] * s * ph.sin()) as f32,
                (c[2] + r[2] * z) as f32,
            ]);
        }
    }
    let top = m.pos.len() as u32;
    m.pos.push([c[0] as f32, c[1] as f32, (c[2] + r[2]) as f32]);
    let ring = |i: u32, k: u32| base + 1 + i * LON + k % LON;
    for k in 0..LON {
        m.tri.push([base, ring(0, k + 1), ring(0, k)]);
        m.tri.push([top, ring(LAT - 2, k), ring(LAT - 2, k + 1)]);
    }
    for i in 0..LAT - 2 {
        for k in 0..LON {
            let (a, b, c2, d) = (
                ring(i, k),
                ring(i, k + 1),
                ring(i + 1, k),
                ring(i + 1, k + 1),
            );
            m.tri.push([a, b, d]);
            m.tri.push([a, d, c2]);
        }
    }
}

/// Appends an axis-aligned cuboid between `lo` and `hi` (metres) to `m`.
fn block(m: &mut Mesh, lo: [f64; 3], hi: [f64; 3]) {
    let c = cuboid(mul(lo, 1.0 / FEET), mul(hi, 1.0 / FEET));
    let base = m.pos.len() as u32;
    m.pos.extend(c.pos);
    m.tri.extend(
        c.tri
            .iter()
            .map(|t| [t[0] + base, t[1] + base, t[2] + base]),
    );
}

/// A simple stand-in for planting and entourage (RPC content: Revit stores a picture and a
/// box, not a mesh). Trees get a trunk and a crown, people a body and a head, cars a body and
/// a cabin, all fitted to the element box (metres).
pub fn proxy(category: i64, lo: [f64; 3], hi: [f64; 3]) -> Option<Mesh> {
    let d: V3 = sub(hi, lo);
    if d.iter().any(|x| !x.is_finite() || *x <= 0.01) || d.iter().any(|x| *x > 200.0) {
        return None;
    }
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
    let mut m = Mesh::default();
    match category {
        // planting: trunk on the lower third, crown above
        -2001360 => {
            let trunk_top = lo[2] + d[2] * 0.4;
            let r = (d[0].min(d[1]) * 0.06).clamp(0.03, 0.4);
            tube(
                &mut m,
                &[
                    mul([c[0], c[1], lo[2]], 1.0 / FEET),
                    mul([c[0], c[1], trunk_top + d[2] * 0.1], 1.0 / FEET),
                ],
                r / FEET,
            );
            let rz = (hi[2] - trunk_top) * 0.5;
            ellipsoid(
                &mut m,
                [c[0], c[1], trunk_top + rz],
                [d[0] * 0.5, d[1] * 0.5, rz],
            );
        }
        -2001370 if d[2] > d[0].max(d[1]) => {
            // a standing person: body then head
            let head = (d[2] * 0.12).min(d[0].min(d[1]) * 0.5);
            let body_top = hi[2] - head * 2.0;
            ellipsoid(
                &mut m,
                [c[0], c[1], (lo[2] + body_top) * 0.5],
                [d[0] * 0.4, d[1] * 0.4, (body_top - lo[2]) * 0.5],
            );
            ellipsoid(&mut m, [c[0], c[1], hi[2] - head], [head, head, head]);
        }
        -2001370 => {
            // a vehicle: body on the lower half, a narrower cabin above
            let mid = lo[2] + d[2] * 0.5;
            block(
                &mut m,
                [lo[0], lo[1], lo[2] + d[2] * 0.12],
                [hi[0], hi[1], mid],
            );
            let (ax, sx, sy) = if d[0] >= d[1] {
                (0, d[0] * 0.2, d[1] * 0.06)
            } else {
                (1, d[0] * 0.06, d[1] * 0.2)
            };
            let _ = ax;
            block(
                &mut m,
                [lo[0] + sx, lo[1] + sy, mid],
                [hi[0] - sx, hi[1] - sy, hi[2]],
            );
        }
        _ => return None,
    }
    (!m.tri.is_empty()).then_some(m)
}
