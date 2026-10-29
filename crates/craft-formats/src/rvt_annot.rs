//! Revit view annotations drawn on the sheets, in the placed views (or on the sheet itself):
//!
//! - detail lines (`CurveElem` owned by a view, curve in `CurveElemDriver.m_pCrv`) drawn with
//!   their line style (`m_categoryId` → `GStyleElem.m_pGStyle`: pen number, colour, line
//!   pattern); the pen gives the printed weight (`PenWidthTableElem`, annotation pens
//!   `m_draftPenInfo`), the pattern its dashes (`LinePatternElem`, dash/space lengths on
//!   paper); pattern -1 is "invisible lines" (not drawn);
//! - filled regions (`FilledRegion`: boundary = the curves of its sketch, `SketchMembership`;
//!   type `FilledRegionAttributes.m_fillPatternElemId` → `FillPattern`): solid fill when the
//!   pattern has no grid, else its hatch lines (`FillGrid`: angle, origin, shift and spacing,
//!   dashes) clipped to the boundary (even-odd);
//! - linear dimensions (`LinearDimString`: reference points `m_refPnts`, dimension line
//!   `GLine`, text positions `m_ArrSegInfo[].m_origin`): dimension and extension lines, tick
//!   marks, the measured lengths in the project length unit (`UnitsElem`, spec
//!   `autodesk.spec.aec:length-1.0.0`);
//! - tags (`IndependentTag`, `RoomTag`): placement and tagged element, drawn by the sheets
//!   with the tag symbol and the tagged element's text parameters.
//!
//! Clean-room, from byte observations only (see docs/rvt-rea-notes.md).

use crate::rvt_browser::Schema;
use crate::rvt_geom::{Obj, Tree, G};
use crate::rvt_shape::curve_points;
use std::collections::HashMap;

type V3 = [f64; 3];
type P2 = [f64; 2];

pub(crate) const CLASSES: [&str; 12] = [
    "CurveElem",
    "GStyleElem",
    "PenWidthTableElem",
    "LinePatternElem",
    "FillPatternElem",
    "FilledRegionAttributes",
    "FilledRegion",
    "LinearDimString",
    "IndependentTag",
    "RoomTag",
    "RoomElem",
    "UnitsElem",
];

/// Paper sizes (feet): dimension text, tick, extension line gap and overshoot.
const DIM_TEXT: f64 = 2.5 / 304.8;
const TICK: f64 = 1.2 / 304.8;
const EXT_GAP: f64 = 1.5 / 304.8;
const EXT_OVER: f64 = 2.0 / 304.8;
/// Most hatch lines drawn for one filled region.
const MAX_HATCH: usize = 4000;

#[derive(Debug, Clone)]
struct Style {
    pen: i64,
    color: [u8; 3],
    pattern: i64,
}

#[derive(Debug, Clone)]
struct Grid {
    angle: f64,
    origin: P2,
    shift: f64,
    offset: f64,
    segs: Vec<f64>,
}

#[derive(Debug, Clone)]
struct Pattern {
    grids: Vec<Grid>,
    scale: f64,
    /// Model pattern (sizes in model units) rather than drafting (sizes on paper).
    model: bool,
}

#[derive(Debug)]
struct Curve {
    owner: i64,
    style: i64,
    pts: Vec<V3>,
    sketch: i64,
}

#[derive(Debug)]
struct Dim {
    owner: i64,
    refs: Vec<V3>,
    line: (V3, V3),
    texts: Vec<V3>,
}

/// A tag: owner view, tag symbol, head point (model feet), rotation, tagged element.
#[derive(Debug)]
pub(crate) struct Tag {
    pub owner: i64,
    pub symbol: i64,
    pub head: V3,
    pub angle: f64,
    pub target: i64,
}

/// Annotation geometry of one view, in its plane coordinates (model feet); widths and text
/// heights on paper (feet).
#[derive(Default)]
pub(crate) struct Drawn {
    pub strokes: Vec<(Vec<P2>, f64, [u8; 3])>,
    pub fills: Vec<(Vec<[P2; 3]>, [u8; 3])>,
    /// (position, direction, height, text) centred above the position.
    pub texts: Vec<(P2, P2, f64, String)>,
}

#[derive(Default)]
pub struct Annots {
    styles: HashMap<i64, Style>,
    pens: Vec<f64>,
    /// line pattern id → (length, drawn) on paper (feet)
    line_patterns: HashMap<i64, Vec<(f64, bool)>>,
    fill_patterns: HashMap<i64, Pattern>,
    /// filled region type → fill pattern
    region_types: HashMap<i64, i64>,
    curves: Vec<Curve>,
    /// (owner, sketch, type)
    regions: Vec<(i64, i64, i64)>,
    dims: Vec<Dim>,
    pub(crate) tags: Vec<Tag>,
    /// text parameters of rooms, family instances and types (tag labels)
    pub(crate) strs: HashMap<i64, HashMap<i64, String>>,
    /// family instance → its type
    pub(crate) inst_type: HashMap<i64, i64>,
    /// length unit type and accuracy
    unit: Option<(String, f64)>,
}

fn num(o: &Obj, k: &str) -> Option<f64> {
    match o.get(k)? {
        G::F(v) if v.is_finite() => Some(*v),
        G::I(v) => Some(*v as f64),
        _ => None,
    }
}
fn v3(s: Option<&[f64]>) -> Option<V3> {
    let s = s?;
    (s.len() >= 3 && s[..3].iter().all(|x| x.is_finite())).then(|| [s[0], s[1], s[2]])
}
fn sdot(a: P2, b: P2) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn color(c: i64) -> [u8; 3] {
    // Windows order: red in the low byte
    [(c & 255) as u8, ((c >> 8) & 255) as u8, ((c >> 16) & 255) as u8]
}

/// The format options stored for a spec in a `UnitsElem` (searched through the tree).
fn find_format<'a>(o: &'a Obj, spec: &str) -> Option<&'a Obj> {
    if let (Some(G::O(f)), Some(G::O(s))) = (o.get("first"), o.get("second")) {
        if matches!(f.get("m_typeId"), Some(G::S(x)) if x == spec) {
            return Some(s);
        }
    }
    for (_, g) in &o.fields {
        let r = match g {
            G::O(x) => find_format(x, spec),
            G::L(l) => l.iter().find_map(|x| find_format(x, spec)),
            _ => None,
        };
        if r.is_some() {
            return r;
        }
    }
    None
}

impl Annots {
    pub fn take(&mut self, s: &Schema, t: u16, tr: &Tree, strs: impl FnOnce() -> HashMap<i64, String>) {
        let r = &tr.root;
        if r.int("m_famId").unwrap_or(-1) != -1 {
            return; // family documents
        }
        let owner = r.int("m_ownerDBViewId").unwrap_or(-1);
        let obj = |k: &str| r.ptr(k).and_then(|i| tr.objs.get(i));
        match s.name(t) {
            "CurveElem" if owner > 0 => {
                let Some(c) = obj("m_pCurveDriver").and_then(|d| d.ptr("m_pCrv")).and_then(|i| tr.objs.get(i)) else { return };
                let pts = curve_points(c, s);
                if pts.len() < 2 {
                    return;
                }
                let sketch = tr.objs.iter().find(|o| s.name(o.tag) == "SketchMembership").and_then(|o| o.int("m_groupId")).unwrap_or(-1);
                self.curves.push(Curve { owner, style: r.int("m_categoryId").unwrap_or(-1), pts, sketch });
            }
            "GStyleElem" => {
                if let Some(g) = obj("m_pGStyle") {
                    self.styles.insert(tr.id, Style { pen: g.int("m_penNumber").unwrap_or(1), color: color(g.int("m_color").unwrap_or(0)), pattern: g.int("m_linePatternId").unwrap_or(-3000010) });
                }
            }
            "PenWidthTableElem" => {
                if let Some(p) = obj("m_pPenWidthTable").and_then(|p| p.obj("m_draftPenInfo")).and_then(|d| d.fs("m_pens")) {
                    self.pens = p.to_vec();
                }
            }
            "LinePatternElem" => {
                if let Some(p) = tr.objs.iter().find(|o| s.name(o.tag) == "LinePattern") {
                    let segs: Vec<(f64, bool)> = p.list("m_segs").iter().filter_map(|g| Some((num(g, "m_data")?, g.int("m_type")? != 1))).map(|(l, d)| (if d && l < 1e-6 { 0.6 / 304.8 } else { l }, d)).collect();
                    self.line_patterns.insert(tr.id, segs);
                }
            }
            "FillPatternElem" => {
                if let Some(p) = obj("m_pFillPattern") {
                    let grids = p
                        .ptrs("m_gridsArr")
                        .iter()
                        .filter_map(|&i| tr.objs.get(i))
                        .filter_map(|g| {
                            let o = g.fs("m_origin").filter(|o| o.len() >= 2)?;
                            let d = g.fs("m_deltas").filter(|d| d.len() >= 2)?;
                            Some(Grid { angle: num(g, "m_angle")?, origin: [o[0], o[1]], shift: d[0], offset: d[1], segs: g.fs("m_segs").unwrap_or(&[]).to_vec() })
                        })
                        .collect();
                    self.fill_patterns.insert(tr.id, Pattern { grids, scale: num(p, "m_scale").filter(|s| *s > 1e-9).unwrap_or(1.0), model: p.int("m_fpTarget").unwrap_or(0) == 1 });
                }
            }
            "FilledRegionAttributes" => {
                if let Some(f) = r.int("m_fillPatternElemId") {
                    self.region_types.insert(tr.id, f);
                }
            }
            "FilledRegion" if owner > 0 => {
                if let (Some(sk), Some(at)) = (r.int("m_sketchId"), r.int("m_attrId")) {
                    self.regions.push((owner, sk, at));
                }
            }
            "LinearDimString" if owner > 0 => {
                let refs: Vec<V3> = r.fs("m_refPnts").unwrap_or(&[]).chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
                let Some(l) = tr.objs.iter().find(|o| s.name(o.tag) == "GLine") else { return };
                let (Some(o), Some(d)) = (v3(l.fs("m_origin")), v3(l.fs("m_dirVec"))) else { return };
                let texts = r.list("m_ArrSegInfo").iter().filter_map(|g| v3(g.fs("m_origin"))).collect();
                if refs.len() >= 2 {
                    self.dims.push(Dim { owner, refs, line: (o, d), texts });
                }
            }
            "IndependentTag" if owner > 0 => {
                let target = r
                    .obj("m_taggedEntitiesCell")
                    .map(|c| c.ptrs("m_taggedEntities"))
                    .and_then(|p| p.first().and_then(|&i| tr.objs.get(i)))
                    .and_then(|e| e.obj("m_taggedId"))
                    .and_then(|x| x.int("m_linkInstOrHostId"))
                    .unwrap_or(-1);
                let Some(oc) = obj("m_oTagOrientationCell") else { return };
                let Some(head) = v3(oc.fs("m_headLocation")) else { return };
                self.tags.push(Tag { owner, symbol: r.int("m_symbolId").unwrap_or(-1), head, angle: num(oc, "m_rotationAngle").unwrap_or(0.0), target });
            }
            "RoomTag" if owner > 0 => {
                let Some(head) = v3(r.fs("m_point")) else { return };
                let target = r.obj("m_taggedRoomElemId").and_then(|x| x.int("m_linkInstOrHostId")).unwrap_or(-1);
                self.tags.push(Tag { owner, symbol: r.int("m_symbolId").unwrap_or(-1), head, angle: num(r, "m_rotationAngle").unwrap_or(0.0), target });
            }
            "RoomElem" | "FamilySymbol" => {
                let m = strs();
                if !m.is_empty() {
                    self.strs.insert(tr.id, m);
                }
            }
            "FamilyInstance" => {
                if let Some(sym) = obj("m_pInstanceInfo").and_then(|ii| ii.int("m_symbolId")) {
                    self.inst_type.insert(tr.id, sym);
                }
                let m = strs();
                if !m.is_empty() {
                    self.strs.insert(tr.id, m);
                }
            }
            "UnitsElem" => {
                if let Some(f) = find_format(r, "autodesk.spec.aec:length-1.0.0").or_else(|| tr.objs.iter().find_map(|o| find_format(o, "autodesk.spec.aec:length-1.0.0"))) {
                    let u = match f.obj("m_unitTypeId").and_then(|u| u.get("m_typeId")) {
                        Some(G::S(x)) => x.clone(),
                        _ => String::new(),
                    };
                    self.unit = Some((u, num(f, "m_accuracy").filter(|a| *a > 0.0).unwrap_or(1.0)));
                }
            }
            _ => {}
        }
    }

    pub fn owns(&self, view: i64) -> bool {
        self.curves.iter().any(|c| c.owner == view) || self.regions.iter().any(|r| r.0 == view) || self.dims.iter().any(|d| d.owner == view) || self.tags.iter().any(|t| t.owner == view)
    }

    pub(crate) fn pen(&self, n: i64) -> f64 {
        let n = n.clamp(1, 16) as usize;
        self.pens.get(n - 1).copied().filter(|w| *w > 0.0).unwrap_or(0.00085 * n as f64)
    }

    /// A length in the project unit (feet in).
    pub fn length(&self, ft: f64) -> String {
        let (u, acc) = self.unit.clone().unwrap_or(("autodesk.unit.unit:millimeters".into(), 1.0));
        let dec = |acc: f64| (-acc.log10()).ceil().max(0.0) as usize;
        let round = |v: f64, acc: f64| (v / acc).round() * acc;
        if u.contains("unit:millimeters") {
            format!("{:.*}", dec(acc), round(ft * 304.8, acc))
        } else if u.contains("unit:centimeters") {
            format!("{:.*}", dec(acc), round(ft * 30.48, acc))
        } else if u.contains("unit:meters") {
            format!("{:.*}", dec(acc), round(ft * 0.3048, acc))
        } else if u.contains("unit:feetFractionalInches") {
            let step = if acc > 0.0 { acc * 12.0 } else { 1.0 / 8.0 };
            let inches = round(ft * 12.0, step.max(1.0 / 256.0));
            let (f, i) = ((inches / 12.0).floor(), inches - (inches / 12.0).floor() * 12.0);
            format!("{f}' - {}\"", (i * 1000.0).round() / 1000.0)
        } else if u.contains("unit:inches") {
            format!("{:.*}\"", dec(acc), round(ft * 12.0, acc))
        } else {
            format!("{:.*}'", dec(acc), round(ft, acc))
        }
    }

    /// The annotations owned by `owner`, in its plane: `proj` maps model feet there, `k` is the
    /// view scale (paper / model).
    pub(crate) fn draw(&self, owner: i64, proj: &dyn Fn(V3) -> P2, k: f64, out: &mut Drawn) {
        let k = k.max(1e-6);
        // detail lines
        for c in self.curves.iter().filter(|c| c.owner == owner) {
            let st = self.styles.get(&c.style).cloned().unwrap_or(Style { pen: 1, color: [0; 3], pattern: -3000010 });
            if st.pattern == -1 {
                continue; // invisible lines
            }
            let pts: Vec<P2> = c.pts.iter().map(|p| proj(*p)).collect();
            let w = self.pen(st.pen);
            match self.line_patterns.get(&st.pattern).filter(|p| p.iter().any(|s| !s.1) && p.iter().all(|s| s.0 > 0.0)) {
                Some(pat) => {
                    let pat: Vec<(f64, bool)> = pat.iter().map(|(l, d)| (l / k, *d)).collect();
                    for piece in dashes(&pts, &pat, 0.0) {
                        out.strokes.push((piece, w, st.color));
                    }
                }
                None => out.strokes.push((pts, w, st.color)),
            }
        }
        // filled regions
        for (_, sk, at) in self.regions.iter().filter(|r| r.0 == owner) {
            let edges: Vec<[P2; 2]> = self.curves.iter().filter(|c| c.sketch == *sk).flat_map(|c| c.pts.windows(2).map(|w| [proj(w[0]), proj(w[1])]).collect::<Vec<_>>()).collect();
            if edges.len() < 3 {
                continue;
            }
            let Some(pat) = self.region_types.get(at).and_then(|f| self.fill_patterns.get(f)) else { continue };
            if pat.grids.is_empty() {
                out.fills.push((fill(&edges), [0; 3]));
                continue;
            }
            let unit = if pat.model { pat.scale } else { pat.scale / k };
            let o0 = proj([0.0; 3]);
            let mut n = 0;
            for g in &pat.grids {
                let off = g.offset.abs() * unit;
                if off * k < 0.3 / 304.8 {
                    continue; // denser than 0.3 mm on paper
                }
                let (d, nv) = ([g.angle.cos(), g.angle.sin()], [-g.angle.sin(), g.angle.cos()]);
                let org = [o0[0] + g.origin[0] * unit, o0[1] + g.origin[1] * unit];
                let cs: Vec<f64> = edges.iter().flat_map(|e| e.iter().map(|p| sdot([p[0] - org[0], p[1] - org[1]], nv))).collect();
                let (cmin, cmax) = cs.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, &c| (a.0.min(c), a.1.max(c)));
                let segs: Vec<(f64, bool)> = g.segs.iter().enumerate().map(|(i, l)| (l.abs() * unit, i % 2 == 0)).map(|(l, dr)| (if dr && l < 1e-9 { 0.5 / 304.8 / k } else { l }, dr)).collect();
                for i in (cmin / off).floor() as i64..=(cmax / off).ceil() as i64 {
                    n += 1;
                    if n > MAX_HATCH {
                        break;
                    }
                    let b = [org[0] + nv[0] * off * i as f64 + d[0] * g.shift * unit * i as f64, org[1] + nv[1] * off * i as f64 + d[1] * g.shift * unit * i as f64];
                    // crossings of the line b + s·d with the boundary
                    let mut ss: Vec<f64> = Vec::new();
                    for e in &edges {
                        let (pa, pb) = (sdot([e[0][0] - b[0], e[0][1] - b[1]], nv), sdot([e[1][0] - b[0], e[1][1] - b[1]], nv));
                        if (pa > 0.0) != (pb > 0.0) {
                            let t = pa / (pa - pb);
                            let x = [e[0][0] + (e[1][0] - e[0][0]) * t, e[0][1] + (e[1][1] - e[0][1]) * t];
                            ss.push(sdot([x[0] - b[0], x[1] - b[1]], d));
                        }
                    }
                    ss.sort_by(f64::total_cmp);
                    for p in ss.chunks_exact(2) {
                        let line = vec![[b[0] + d[0] * p[0], b[1] + d[1] * p[0]], [b[0] + d[0] * p[1], b[1] + d[1] * p[1]]];
                        if segs.len() >= 2 && segs.iter().all(|s| s.0 > 0.0) {
                            for piece in dashes(&line, &segs, p[0]) {
                                out.strokes.push((piece, self.pen(1), [0; 3]));
                            }
                        } else {
                            out.strokes.push((line, self.pen(1), [0; 3]));
                        }
                    }
                }
            }
        }
        // linear dimensions
        for dm in self.dims.iter().filter(|d| d.owner == owner) {
            let o = proj(dm.line.0);
            let q = proj([dm.line.0[0] + dm.line.1[0], dm.line.0[1] + dm.line.1[1], dm.line.0[2] + dm.line.1[2]]);
            let mut d = [q[0] - o[0], q[1] - o[1]];
            let l = sdot(d, d).sqrt();
            if l < 1e-9 {
                continue; // seen along its line
            }
            d = [d[0] / l, d[1] / l];
            // text reads left to right, bottom to top
            if d[0] < -1e-6 || (d[0].abs() <= 1e-6 && d[1] < 0.0) {
                d = [-d[0], -d[1]];
            }
            let nv = [-d[1], d[0]];
            let w = self.pen(1);
            let mut ts: Vec<f64> = Vec::new();
            for r in &dm.refs {
                let p = proj(*r);
                let t = sdot([p[0] - o[0], p[1] - o[1]], d);
                let f = [o[0] + d[0] * t, o[1] + d[1] * t];
                let e = [f[0] - p[0], f[1] - p[1]];
                let el = sdot(e, e).sqrt();
                if el > EXT_GAP / k {
                    let u = [e[0] / el, e[1] / el];
                    out.strokes.push((vec![[p[0] + u[0] * EXT_GAP / k, p[1] + u[1] * EXT_GAP / k], [f[0] + u[0] * EXT_OVER / k, f[1] + u[1] * EXT_OVER / k]], w, [0; 3]));
                }
                let a = [(d[0] + nv[0]) * TICK / k, (d[1] + nv[1]) * TICK / k];
                out.strokes.push((vec![[f[0] - a[0], f[1] - a[1]], [f[0] + a[0], f[1] + a[1]]], self.pen(3), [0; 3]));
                ts.push(t);
            }
            let (t0, t1) = ts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, &t| (a.0.min(t), a.1.max(t)));
            let ov = EXT_OVER / k;
            out.strokes.push((vec![[o[0] + d[0] * (t0 - ov), o[1] + d[1] * (t0 - ov)], [o[0] + d[0] * (t1 + ov), o[1] + d[1] * (t1 + ov)]], w, [0; 3]));
            for (i, p) in ts.windows(2).enumerate() {
                let v = (p[1] - p[0]).abs();
                if v < 1e-6 {
                    continue;
                }
                let mid = (p[0] + p[1]) * 0.5;
                let pos = dm.texts.get(i).map(|x| proj(*x)).map(|x| {
                    // keep the stored offset across the line, centre along it
                    let c = sdot([x[0] - o[0], x[1] - o[1]], nv);
                    [o[0] + d[0] * mid + nv[0] * c.max(0.4 / 304.8 / k), o[1] + d[1] * mid + nv[1] * c.max(0.4 / 304.8 / k)]
                });
                let pos = pos.unwrap_or([o[0] + d[0] * mid + nv[0] * 0.6 / 304.8 / k, o[1] + d[1] * mid + nv[1] * 0.6 / 304.8 / k]);
                out.texts.push((pos, d, DIM_TEXT, self.length(v)));
            }
        }
    }
}

/// Pieces of a polyline drawn with a dash pattern (lengths along the line, drawn or not),
/// starting `phase` into the pattern.
fn dashes(pts: &[P2], pat: &[(f64, bool)], phase: f64) -> Vec<Vec<P2>> {
    let period: f64 = pat.iter().map(|p| p.0).sum();
    let total: f64 = pts.windows(2).map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt()).sum();
    if period <= 1e-12 || total / period > 5000.0 {
        return vec![pts.to_vec()];
    }
    let at = |s: f64| -> P2 {
        let mut acc = 0.0;
        for w in pts.windows(2) {
            let l = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
            if acc + l >= s && l > 0.0 {
                let t = (s - acc) / l;
                return [w[0][0] + (w[1][0] - w[0][0]) * t, w[0][1] + (w[1][1] - w[0][1]) * t];
            }
            acc += l;
        }
        *pts.last().unwrap()
    };
    let mut out = Vec::new();
    let mut s = -(phase.rem_euclid(period));
    let mut i = 0;
    while s < total {
        let (l, dr) = pat[i % pat.len()];
        let (a, b) = (s.max(0.0), (s + l).min(total));
        if dr && b > a {
            out.push(vec![at(a), at(b)]);
        }
        s += l;
        i += 1;
    }
    out
}

/// Even-odd fill of the area enclosed by `edges`, as triangles (slabs between the vertices'
/// heights, trapezoids between pairs of crossing edges).
fn fill(edges: &[[P2; 2]]) -> Vec<[P2; 3]> {
    let mut ys: Vec<f64> = edges.iter().flat_map(|e| [e[0][1], e[1][1]]).collect();
    ys.sort_by(f64::total_cmp);
    ys.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let mut tris = Vec::new();
    let x_at = |e: &[P2; 2], y: f64| e[0][0] + (e[1][0] - e[0][0]) * (y - e[0][1]) / (e[1][1] - e[0][1]);
    for w in ys.windows(2) {
        let (y0, y1) = (w[0], w[1]);
        let ym = (y0 + y1) * 0.5;
        let mut xs: Vec<(f64, f64, f64)> = edges.iter().filter(|e| (e[0][1] - ym) * (e[1][1] - ym) < 0.0).map(|e| (x_at(e, ym), x_at(e, y0), x_at(e, y1))).collect();
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        for p in xs.chunks_exact(2) {
            let (a, b) = (p[0], p[1]);
            tris.push([[a.1, y0], [b.1, y0], [b.2, y1]]);
            tris.push([[a.1, y0], [b.2, y1], [a.2, y1]]);
        }
        if tris.len() > 200_000 {
            break;
        }
    }
    tris
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fill_square_with_hole() {
        let sq = |a: f64, b: f64| vec![[[a, a], [b, a]], [[b, a], [b, b]], [[b, b], [a, b]], [[a, b], [a, a]]];
        let mut e = sq(0.0, 4.0);
        e.extend(sq(1.0, 3.0));
        let area: f64 = fill(&e).iter().map(|t| ((t[1][0] - t[0][0]) * (t[2][1] - t[0][1]) - (t[2][0] - t[0][0]) * (t[1][1] - t[0][1])).abs() * 0.5).sum();
        assert!((area - 12.0).abs() < 1e-9, "{area}");
    }
    #[test]
    fn dash_pattern() {
        let p = dashes(&[[0.0, 0.0], [10.0, 0.0]], &[(2.0, true), (1.0, false)], 0.0);
        assert_eq!(p.len(), 4);
        assert!((p[3][1][0] - 10.0).abs() < 1e-9);
    }
    #[test]
    fn lengths() {
        let a = Annots::default();
        assert_eq!(a.length(1.0), "305");
    }
}
