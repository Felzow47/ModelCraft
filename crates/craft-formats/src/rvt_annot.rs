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
//! - radial dimensions (`RadialDim`: arc centre `m_center`, point on the arc `m_refPnts`,
//!   radius `m_ArrSegInfo[0].m_values[0].m_value`, text at `m_ArrSegInfo[0].m_origin`): line
//!   from the centre, arrow on the arc, "R" and the radius;
//! - angular dimensions (`AngularDim`: the two points on the dimension arc `m_refPnts`, arc
//!   centre in its `GArc`, plane normal `m_planeNormal`, angle in radians in
//!   `m_values[0].m_value`): the arc between the two points, ticks, the angle in degrees;
//! - spot elevations and coordinates (`SpotElevation`: point `m_refPnts`, leader bend
//!   `m_bendOrPosPtOffset` from it, shoulder `m_horizLeaderLength`; values in
//!   `m_ArrSegInfo[0].m_values`: elevation first, east and north next; a
//!   `SpotCoordinateState` object marks a spot coordinate);
//! - dimension types (`DimensionStyle`: tick mark `m_arrowHeadStyleId`, text size parameter
//!   -1006433) and their tick marks (`LeaderStyle`: diagonal or arrow, filled, closed, width
//!   angle -1006426, size -1006414);
//! - tags (`IndependentTag`, `RoomTag`): placement and tagged element, drawn by the sheets
//!   with the tag symbol and the tagged element's text parameters.
//!
//! Clean-room, from byte observations only (see docs/rvt-rea-notes.md).

use crate::rvt_browser::Schema;
use crate::rvt_geom::{Obj, Tree, G};
use crate::rvt_shape::curve_points_within;
use crate::det::HashMap;

type V3 = [f64; 3];
type P2 = [f64; 2];

pub(crate) const CLASSES: [&str; 25] = [
    "ParamElemFamily",
    "ParamElemExternal",
    "ParamElemProject",
    "CategoryElem",
    "DimensionStyle",
    "FontElem",
    "LeaderStyle",
    "SpotElevation",
    "RadialDim",
    "AngularDim",
    "CurveElem",
    "GStyleElem",
    "PenWidthTableElem",
    "LinePatternElem",
    "FillPatternElem",
    "FilledRegionAttributes",
    "FilledRegion",
    "LinearDimString",
    "IndependentTag",
    "KeynoteTag",
    "RoomTag",
    "RoomElem",
    "LevelRoomPlan",
    "UnitsElem",
    "SpotElevationStyle",
];

/// An element's own graphics in a view (its lines' colour, pen, pattern; its fill's colour),
/// each unset when not overridden.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LineOver {
    pub color: Option<[u8; 3]>,
    pub pen: Option<i64>,
    pub pattern: Option<i64>,
    pub fill: Option<[u8; 3]>,
}

/// A dimension's flags (`m_flags`) marking a constraint (a lock kept from a dimension, shown
/// only when constraints are revealed) or a dimension Revit does not draw (0x40: none of the
/// project's dimensions so marked shows in Revit's prints).
const CONSTRAINT: i64 = 0x10 | 0x40;

/// Paper sizes (feet): dimension text, tick, extension line gap and overshoot.
const DIM_TEXT: f64 = 2.5 / 304.8;
const TICK: f64 = 1.2 / 304.8;
const EXT_GAP: f64 = 1.5 / 304.8;
const EXT_OVER: f64 = 2.0 / 304.8;
/// Most hatch lines drawn for one filled region.
const MAX_HATCH: usize = 4000;

/// A tick mark or arrowhead (`LeaderStyle`): `m_tickType` 8 = arrow (0 = diagonal),
/// `m_arrowFilled`, `m_arrowClosed`, width angle (parameter -1006426, radians) and size on
/// paper (parameter -1006414, feet).
#[derive(Debug, Clone, Copy)]
struct Mark {
    arrow: bool,
    filled: bool,
    closed: bool,
    angle: f64,
    size: f64,
}

/// How a room or space is bounded (see `Annots::enclosure`): its area (square feet), not
/// enclosed, redundant (another room already holds its circuit).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Enclosure {
    Area(f64),
    Open,
    Redundant,
}

/// A number format (`FormatOptions`): its unit (Revit's display unit: 0 metres, 1
/// centimetres, 2 millimetres, 3 feet, 4 feet and fractional inches, 5 fractional inches, 6
/// inches), accuracy, digit grouping (`m_bUseGrouping`); or the project's format for its kind
/// (`m_bUseDefault`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Fmt {
    pub unit: i64,
    pub acc: f64,
    pub group: bool,
    pub default: bool,
}

impl Fmt {
    /// The format stored in `o`: its display unit, else its unit's type id (recent files).
    pub(crate) fn of(o: &Obj) -> Option<Fmt> {
        let acc = num(o, "m_accuracy")?;
        let unit = o.int("m_displayUnits").or_else(|| match o.obj("m_unitTypeId").and_then(|u| u.get("m_typeId")) {
            Some(G::S(u)) => Some(unit_code(u)),
            _ => None,
        })?;
        Some(Fmt { unit, acc: if acc > 0.0 { acc } else { 1.0 }, group: o.int("m_bUseGrouping").unwrap_or(0) != 0, default: o.int("m_bUseDefault").unwrap_or(0) != 0 })
    }
}

/// A length unit's type id (`autodesk.unit.unit:millimeters-1.0.0`…) as a display unit code.
fn unit_code(u: &str) -> i64 {
    if u.contains("unit:millimeters") {
        2
    } else if u.contains("unit:centimeters") {
        1
    } else if u.contains("unit:meters") {
        0
    } else if u.contains("unit:feetFractionalInches") {
        4
    } else if u.contains("unit:fractionalInches") {
        5
    } else if u.contains("unit:inches") {
        6
    } else {
        3
    }
}

/// The digits of `s` (a formatted number) grouped by three with `sep`, its decimal point
/// written `point`.
fn grouped(s: &str, sep: Option<char>, point: char) -> String {
    let (sign, body) = s.strip_prefix('-').map_or(("", s), |b| ("-", b));
    let (int, frac) = body.split_once('.').map_or((body, None), |(i, f)| (i, Some(f)));
    let mut out = String::from(sign);
    let n = int.chars().count();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (n - i) % 3 == 0 && int.chars().all(|c| c.is_ascii_digit()) {
            if let Some(g) = sep {
                out.push(g);
            }
        }
        out.push(c);
    }
    if let Some(f) = frac {
        out.push(point);
        out.push_str(f);
    }
    out
}

/// A spot elevation or coordinate type (`SpotElevationStyle`): its number format, indicators
/// (north / south, east / west, elevation), the values on its top and bottom lines
/// (`m_seTopValue` / `m_seBotValue`: 1 north / south, 2 east / west), the text's offset from
/// the leader end along it (`m_seHorizOffset`) and above the leader (parameter -1006401),
/// on paper (feet), its arrowhead, font and symbol.
#[derive(Debug, Clone)]
struct SpotStyle {
    fmt: Option<Fmt>,
    ns: String,
    ew: String,
    elev: String,
    suffix: bool,
    top: i64,
    bottom: i64,
    along: f64,
    above: f64,
    arrow: i64,
    font: i64,
    symbol: bool,
}

/// A double parameter of an element (`m_pParamValueSetDouble`).
pub(crate) fn param_f(tr: &Tree, id: i64) -> Option<f64> {
    let set = tr.root.ptr("m_pParamValueSetDouble").and_then(|i| tr.objs.get(i))?;
    set.list("m_paramSet").iter().find(|o| o.int("m_paramId") == Some(id)).and_then(|o| num(o, "m_value"))
}

/// An element id parameter of an element (`m_pParamValueSetElementId`).
fn param_id(tr: &Tree, id: i64) -> Option<i64> {
    let set = tr.root.ptr("m_pParamValueSetElementId").and_then(|i| tr.objs.get(i))?;
    set.list("m_paramSet").iter().find(|o| o.int("m_paramId") == Some(id)).and_then(|o| o.int("m_value"))
}

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
    id: i64,
    owner: i64,
    style: i64,
    pts: Vec<V3>,
    sketch: i64,
    vis: Visible,
    /// In a family: the views and detail levels showing it (`m_famElemVisibility`, -1: all).
    fev: i64,
    /// Revision cloud edge (`Cloud`): radius and sweep of its two arcs, mirrored.
    cloud: Option<[f64; 5]>,
}

/// Points of a revision cloud edge `a`..`b`: each scallop is two arcs, the repeat `unit` long.
fn cloud_points(a: P2, b: P2, c: [f64; 5], unit: f64) -> Vec<P2> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = (dx * dx + dy * dy).sqrt();
    let (r1, s1, r2, s2) = (c[0], c[1], c[2], c[3]);
    let (c1, c2) = (2.0 * r1 * (s1 * 0.5).sin(), 2.0 * r2 * (s2 * 0.5).sin());
    if len < 1e-9 || c1 + c2 < 1e-9 {
        return vec![a, b];
    }
    let n = ((len / unit).round() as usize).clamp(1, 200);
    let k = len / n as f64 / (c1 + c2);
    let (ux, uy) = (dx / len, dy / len);
    let side = if c[4] != 0.0 { -1.0 } else { 1.0 };
    let mut out = Vec::new();
    for i in 0..n {
        let start = i as f64 * len / n as f64;
        for (r, sw, ch, x0) in [(r1, s1, c1, 0.0), (r2, s2, c2, c1)] {
            for t in 0..=8 {
                let th = sw * (t as f64 / 8.0 - 0.5);
                let (x, y) = ((r * th.sin() + ch * 0.5 + x0) * k, (r * th.cos() - r * (sw * 0.5).cos()) * k * side);
                out.push([a[0] + ux * (start + x) - uy * y, a[1] + uy * (start + x) + ux * y]);
            }
        }
    }
    out
}

/// Whether a family document's element shows: its "Visible" value, or the family parameter
/// it follows; the views and detail levels showing it (`m_famElemVisibility`, -1: all).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Visible {
    default: bool,
    param: Option<i64>,
    fev: i64,
}

impl Visible {
    pub(crate) fn of(tr: &Tree, s: &Schema) -> Visible {
        let default = tr.root.ptr("m_pParamValueSetInt").and_then(|i| tr.objs.get(i)).and_then(|o| o.list("m_paramSet").iter().find(|p| p.int("m_paramId") == Some(VISIBLE_PARAM)).and_then(|p| p.int("m_value"))).map_or(true, |v| v != 0);
        let param = tr.objs.iter().filter(|o| s.name(o.tag) == "FamilyParametrizedElemParamsCell").flat_map(|o| o.list("m_paramDrivenData").iter()).find(|d| d.int("m_elemPropId") == Some(VISIBLE_PARAM)).and_then(|d| d.int("m_famParamId"));
        let fev = tr.root.obj("m_famElemVisibility").and_then(|o| o.int("m_flags")).unwrap_or(-1);
        Visible { default, param, fev }
    }
    /// Whether it shows at detail level `detail` (1 coarse, 2 medium, 3 fine).
    pub(crate) fn at_detail(&self, detail: i64) -> bool {
        self.fev == -1 || self.fev & (1 << (12 + detail)) != 0
    }
    pub(crate) fn shows(&self, vals: &HashMap<i64, bool>) -> bool {
        self.param.and_then(|p| vals.get(&p).copied()).unwrap_or(self.default)
    }
}

/// Key of an element id of family document `fam`: the ids of each document are its own (the
/// low ones repeat from one to the next). Ids of the project, or built in (negative), stay.
pub(crate) fn scoped(fam: i64, id: i64) -> i64 {
    if fam > 0 && id > 0 {
        (fam << 28) | id
    } else {
        id
    }
}

/// Invisible lines (`OST_InvisibleLines`) and reference lines (`OST_ReferenceLines`).
const INVISIBLE_LINES: i64 = -2000064;
const REFERENCE_LINES: i64 = -2000083;

/// The id an element of a family document refers to, back from `scoped`: a family loaded in
/// a project may refer to the project's own elements (its subcategories) by their ids.
pub(crate) fn unscoped(id: i64) -> i64 {
    if id >= 1 << 28 {
        id & ((1 << 28) - 1)
    } else {
        id
    }
}

/// Text values of a family instance's or type's own family parameters (`FamilyParams`).
fn family_param_strs(tr: &Tree, s: &Schema) -> HashMap<i64, String> {
    tr.objs
        .iter()
        .filter(|o| s.name(o.tag) == "FamilyParams")
        .flat_map(|o| o.list("m_params").iter())
        .filter_map(|p| match p.get("m_str") {
            Some(G::S(v)) if !v.is_empty() => Some((p.int("m_paramId")?, v.clone())),
            _ => None,
        })
        .collect()
}

/// Length of a revision cloud's scallop on paper (feet).
const CLOUD_ARC: f64 = 0.2;

/// The "Visible" parameter of family elements.
pub(crate) const VISIBLE_PARAM: i64 = -1006205;

#[derive(Debug)]
struct Dim {
    id: i64,
    owner: i64,
    /// dimension type (`m_styleSymbolId` → `DimensionStyle`)
    style: i64,
    refs: Vec<V3>,
    line: (V3, V3),
    texts: Vec<V3>,
    /// Each segment's text moved from its place (`m_offset`: model feet along the line's
    /// direction and across it), drawn with a leader.
    moved: Vec<[f64; 2]>,
    /// Each segment's text as typed over its value (`m_values[0].m_oTextFields`): prefix,
    /// replacing text (empty: the value), suffix ("250mm WEIR" for a weir's height, P-603).
    typed: Vec<Option<[String; 3]>>,
}

/// A radial or angular dimension (model feet): centre, points on the arc, text position,
/// value (radius in feet or angle in radians), plane normal.
#[derive(Debug)]
struct ArcDim {
    id: i64,
    owner: i64,
    style: i64,
    angular: bool,
    center: V3,
    refs: Vec<V3>,
    text: Option<V3>,
    value: f64,
    normal: V3,
}

/// A spot elevation or coordinate (model feet): point, leader bend, shoulder length, lines of
/// text (values in feet, coordinate or not).
#[derive(Debug)]
struct Spot {
    id: i64,
    owner: i64,
    at: V3,
    bend: Option<V3>,
    /// shoulder from the bend, signed along the view's horizontal (`m_horizLeaderLength`)
    shoulder: f64,
    values: Vec<f64>,
    coord: bool,
    style: i64,
    /// where the text was moved from its own place (`m_ArrSegInfo[0].m_offset`, view plane)
    moved: [f64; 2],
}

/// A tag: owner view, tag symbol, head point (model feet), rotation, tagged element.
#[derive(Debug)]
pub(crate) struct Tag {
    pub owner: i64,
    pub symbol: i64,
    pub head: V3,
    pub angle: f64,
    pub target: i64,
    pub id: i64,
    /// Leader: elbow and end offsets from the head (view plane, feet), when shown.
    pub leader: Option<([f64; 2], [f64; 2])>,
}

/// Annotation geometry of one view, in its plane coordinates (model feet); widths and text
/// heights on paper (feet).
#[derive(Default)]
pub(crate) struct Drawn {
    pub strokes: Vec<(Vec<P2>, f64, [u8; 3])>,
    pub fills: Vec<(Vec<[P2; 3]>, [u8; 3])>,
    /// (position, direction, height, text) centred above the position.
    pub texts: Vec<(P2, P2, f64, String)>,
    /// Where each whole annotation (dimension, spot) starts: strokes, fills, texts. Shown
    /// whole or not at all by an annotation crop.
    pub wholes: Vec<(usize, usize, usize)>,
    /// A dimension value's segment: its text's index, the segment's ends on the dimension line
    /// (a chain crossing an annotation crop shows the values whose segment is inside).
    pub spans: Vec<(usize, [P2; 2])>,
}

impl Drawn {
    fn begin_whole(&mut self) {
        self.wholes.push((self.strokes.len(), self.fills.len(), self.texts.len()));
    }
}

#[derive(Default)]
pub struct Annots {
    styles: HashMap<i64, Style>,
    /// keynote tag -> its key value
    pub(crate) keys: HashMap<i64, String>,
    /// keynote tags: key value, keynote text
    pub(crate) keynotes: Vec<(String, String)>,
    /// category -> colour of its projection style, and that style
    cat_colors: HashMap<i64, [u8; 3]>,
    cat_styles: HashMap<i64, i64>,
    /// projection line style (scoped) -> its category (scoped)
    gstyle_cats: HashMap<i64, i64>,
    /// family document -> its reference lines (id, points)
    ref_lines: HashMap<i64, Vec<(i64, Vec<V3>)>>,
    /// family document -> its free lines (index in `curves`), once asked
    fam_lines: std::sync::OnceLock<HashMap<i64, Vec<usize>>>,
    /// a family document's subcategory (scoped) -> its parent and name, and the project's
    /// subcategory of that parent and name (whose style the project sets)
    cat_names: HashMap<i64, (i64, String)>,
    subcats: HashMap<(i64, String), i64>,
    pens: Vec<f64>,
    /// model pens by view scale (1:n): (n, widths on paper, feet)
    model_pens: Vec<(i64, Vec<f64>)>,
    /// line pattern id → (length, drawn) on paper (feet)
    line_patterns: HashMap<i64, Vec<(f64, bool)>>,
    fill_patterns: HashMap<i64, Pattern>,
    /// filled region type → (fill pattern, category giving its colour)
    region_types: HashMap<i64, (i64, i64)>,
    curves: Vec<Curve>,
    /// Area boundary lines (`AreaMeasureCurveData`): their level, their points (model feet).
    pub(crate) area_lines: Vec<(i64, Vec<V3>)>,
    /// (owner, sketch, type, visibility, id)
    regions: Vec<(i64, i64, i64, Visible, i64)>,
    /// (family or family document, parameter name) -> parameter, and parameter -> name
    fam_params: HashMap<(i64, String), i64>,
    param_names: HashMap<i64, String>,
    dims: Vec<Dim>,
    /// dimension type → (tick mark `LeaderStyle`, text size on paper)
    /// dimension type -> its tick mark, text size (a parameter), font, whether it is one of
    /// Revit's own types for constraints, its length format
    dim_styles: HashMap<i64, (i64, Option<f64>, i64, bool, Option<Fmt>)>,
    /// dimension type -> the gap between its line and its text (paper feet)
    dim_text_gaps: HashMap<i64, f64>,
    /// spot elevation and coordinate types
    spot_styles: HashMap<i64, SpotStyle>,
    /// the project's formats: lengths, duct sizes, areas (and whether areas show their unit)
    len_fmt: Option<Fmt>,
    duct_fmt: Option<Fmt>,
    area_fmt: Option<(Fmt, bool)>,
    /// the project's format of each kind of value (Revit's unit types, in their order), whether
    /// it shows its unit; each parameter's kind (`m_unitType` of its definition)
    unit_fmts: Vec<Option<(Fmt, bool)>>,
    param_units: HashMap<i64, i64>,
    /// the parameters holding a text (`ParamDefString`)
    pub(crate) text_params: crate::det::HashSet<i64>,
    /// room or space -> the closed circuit of bounding elements it stands in
    /// (`m_cachedCircuitId`, -1: none)
    room_circuits: HashMap<i64, i64>,
    /// room or space -> its level's room plan (`LevelRoomPlan` listing it), and each room
    /// plan's rooms (in its order) and circuits' areas (square feet)
    room_plans: HashMap<i64, i64>,
    plan_rooms: HashMap<i64, Vec<i64>>,
    circuit_areas: HashMap<(i64, i64), f64>,
    /// the project's digit grouping symbol (`m_digitGroupingSymbol`) and decimal point
    /// (`m_decimalSymbol`)
    grouper: Option<char>,
    comma: bool,
    /// font element -> its size (feet): the text size of the types using it
    font_sizes: HashMap<i64, f64>,
    /// tick marks / arrowheads (`LeaderStyle`)
    marks: HashMap<i64, Mark>,
    arc_dims: Vec<ArcDim>,
    spots: Vec<Spot>,
    /// Angle display accuracy (degrees).
    angle_acc: Option<f64>,
    pub(crate) tags: Vec<Tag>,
    /// text parameters of rooms, family instances and types (tag labels)
    pub(crate) strs: HashMap<i64, HashMap<i64, String>>,
    /// family instance → its type
    pub(crate) inst_type: HashMap<i64, i64>,
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

/// An integer field `k` of `o` or of an object inside it.
fn find_int(o: &Obj, k: &str) -> Option<i64> {
    o.int(k).or_else(|| {
        o.fields.iter().find_map(|(_, g)| match g {
            G::O(x) => find_int(x, k),
            G::L(l) => l.iter().find_map(|x| find_int(x, k)),
            _ => None,
        })
    })
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
        // a member of a group type not placed: not drawn
        if r.int("m_unplacedOwnerId").unwrap_or(-1) != -1 {
            return;
        }
        // family documents: their lines and regions, filed under the negated document root
        let fam = r.int("m_famId").unwrap_or(-1);
        if fam != -1 && !(fam > 0 && matches!(s.name(t), "ParamElemFamily" | "CurveElem" | "FilledRegion" | "GStyleElem" | "FilledRegionAttributes" | "FillPatternElem" | "LinePatternElem" | "CategoryElem")) {
            return;
        }
        let owner = if fam > 0 { -fam } else { r.int("m_ownerDBViewId").unwrap_or(-1) };
        let obj = |k: &str| r.ptr(k).and_then(|i| tr.objs.get(i));
        match s.name(t) {
            "ParamElemFamily" | "ParamElemExternal" | "ParamElemProject" => {
                if let Some(u) = tr.objs.iter().find_map(|o| o.int("m_unitType")) {
                    self.param_units.insert(tr.id, u);
                }
                if tr.objs.iter().any(|o| s.name(o.tag) == "ParamDefString") {
                    self.text_params.insert(tr.id);
                }
                if s.name(t) != "ParamElemFamily" {
                    return;
                }
                if let Some(c) = tr.objs.iter().find_map(|o| match o.get("m_caption") {
                    Some(G::S(c)) => Some(c.clone()),
                    _ => None,
                }) {
                    self.fam_params.insert((fam, c), tr.id);
                    self.param_names.insert(tr.id, tr.objs.iter().find_map(|o| match o.get("m_caption") {
                        Some(G::S(c)) => Some(c.clone()),
                        _ => None,
                    }).unwrap_or_default());
                }
            }
            // area boundary lines: on their level, for the area plans to find their areas in
            "CurveElem" if tr.objs.iter().any(|o| s.name(o.tag) == "AreaMeasureCurveData") => {
                if let Some(c) = obj("m_pCurveDriver").and_then(|d| d.ptr("m_pCrv")).and_then(|i| tr.objs.get(i)) {
                    let pts = curve_points_within(c, s, 0.002);
                    if pts.len() >= 2 {
                        self.area_lines.push((r.int("m_assocLevelId").unwrap_or(-1), pts));
                    }
                }
            }
            "CurveElem" if owner != -1 => {
                // construction curves (reference lines): nothing drawn; a 2D family's are kept,
                // as its types move its lines with them
                if fam > 0 && tr.objs.iter().any(|o| s.name(o.tag) == "SkeletonCurveData") {
                    if let Some(c) = obj("m_pCurveDriver").and_then(|d| d.ptr("m_pCrv")).and_then(|i| tr.objs.get(i)) {
                        let pts = curve_points_within(c, s, 0.002);
                        if pts.len() >= 2 {
                            self.ref_lines.entry(fam).or_default().push((tr.id, pts));
                        }
                    }
                    return;
                }
                let Some(c) = obj("m_pCurveDriver").and_then(|d| d.ptr("m_pCrv")).and_then(|i| tr.objs.get(i)) else { return };
                // a multiline (brace…): its real curve, the driver being only the chord
                let c = tr.objs.iter().find(|o| s.name(o.tag) == "GNurbSpline").filter(|_| tr.objs.iter().any(|o| s.name(o.tag) == "MultilineData")).unwrap_or(c);
                let pts = curve_points_within(c, s, 0.002);
                if pts.len() < 2 {
                    return;
                }
                let sketch = tr.objs.iter().find(|o| s.name(o.tag) == "SketchMembership").and_then(|o| o.int("m_groupId")).unwrap_or(-1);
                let cloud = tr.objs.iter().find(|o| s.name(o.tag) == "Cloud").and_then(|o| {
                    let (p1, p2) = (o.fs("m_params1")?, o.fs("m_params2")?);
                    (p1.len() == 2 && p2.len() == 2).then(|| [num(o, "m_rad1").unwrap_or(0.2), p1[1] - p1[0], num(o, "m_rad2").unwrap_or(0.35), p2[1] - p2[0], o.int("m_mirrored").unwrap_or(0) as f64])
                });
                let fev = r.obj("m_famElemVisibility").and_then(|o| o.int("m_flags")).unwrap_or(-1);
                self.curves.push(Curve { id: tr.id, owner, style: r.int("m_categoryId").unwrap_or(-1), pts, sketch, vis: Visible::of(tr, s), fev, cloud });
            }
            "CategoryElem" => {
                if let (Some(G::S(name)), Some(parent)) = (obj("m_pCategory").and_then(|c| c.get("m_name")), obj("m_pCategory").and_then(|c| c.int("m_parentCategoryId"))) {
                    if !name.is_empty() && parent != -1 {
                        if fam > 0 {
                            self.cat_names.insert(scoped(fam, tr.id), (parent, name.clone()));
                        } else {
                            self.subcats.insert((parent, name.clone()), tr.id);
                        }
                    }
                }
            }
            "GStyleElem" => {
                if let Some(g) = obj("m_pGStyle") {
                    let st = Style { pen: g.int("m_penNumber").unwrap_or(1), color: color(g.int("m_color").unwrap_or(0)), pattern: g.int("m_linePatternId").unwrap_or(-3000010) };
                    if std::env::var("GSTYLE_DEBUG").is_ok_and(|v| r.int("m_categoryId").is_some_and(|c| v.split(',').any(|x| x == c.to_string()))) {
                        eprintln!("GSTYLE {} fam {fam} cat {:?} type {:?} {:?}", tr.id, r.int("m_categoryId"), r.int("m_gstyleType"), g.fields);
                    }
                    if r.int("m_gstyleType") == Some(1) {
                        if let Some(c) = r.int("m_categoryId") {
                            self.cat_colors.insert(scoped(fam, c), st.color);
                            self.cat_styles.insert(scoped(fam, c), scoped(fam, tr.id));
                            self.gstyle_cats.insert(scoped(fam, tr.id), scoped(fam, c));
                        }
                    }
                    self.styles.insert(scoped(fam, tr.id), st);
                }
            }
            "PenWidthTableElem" => {
                if let Some(p) = obj("m_pPenWidthTable").and_then(|p| p.obj("m_draftPenInfo")).and_then(|d| d.fs("m_pens")) {
                    self.pens = p.to_vec();
                }
                if let Some(t) = obj("m_pPenWidthTable") {
                    self.model_pens = t.list("m_modelPenInfo").iter().filter_map(|e| Some((e.int("m_invertedScale")?, e.fs("m_pens")?.to_vec()))).collect();
                    self.model_pens.sort_by_key(|e| e.0);
                }
            }
            "LinePatternElem" => {
                if let Some(p) = tr.objs.iter().find(|o| s.name(o.tag) == "LinePattern") {
                    let segs: Vec<(f64, bool)> = p.list("m_segs").iter().filter_map(|g| Some((num(g, "m_data")?, g.int("m_type")? != 1))).map(|(l, d)| (if d && l < 1e-6 { 0.6 / 304.8 } else { l }, d)).collect();
                    self.line_patterns.insert(scoped(fam, tr.id), segs);
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
                    let pat = Pattern { grids, scale: num(p, "m_scale").filter(|s| *s > 1e-9).unwrap_or(1.0), model: p.int("m_fpTarget").unwrap_or(0) == 1 };
                    self.fill_patterns.insert(scoped(fam, tr.id), pat);
                }
            }
            "FilledRegionAttributes" => {
                if let Some(f) = r.int("m_fillPatternElemId") {
                    let v = (f, r.obj("m_lineAndTextAttr").and_then(|a| a.int("m_categoryId")).unwrap_or(-1));
                    self.region_types.insert(scoped(fam, tr.id), v);
                }
            }
            "FilledRegion" if owner != -1 => {
                if let (Some(sk), Some(at)) = (r.int("m_sketchId"), r.int("m_attrId")) {
                    self.regions.push((owner, sk, at, Visible::of(tr, s), tr.id));
                }
            }
            "DimensionStyle" => {
                let size = param_f(tr, -1006433).filter(|v| *v > 1e-6 && *v < 0.1);
                // its text's font (which holds the text size)
                let font = tr.objs.iter().find_map(|o| o.int("m_fontId")).unwrap_or(-1);
                // Revit's own types for constraints (never drawn)
                let internal = tr.objs.iter().any(|o| matches!(o.get("m_name"), Some(G::S(n)) if n.starts_with("SecretInternal")));
                let fmt = obj("m_oLinFormatOptions").and_then(Fmt::of);
                self.dim_styles.insert(tr.id, (r.int("m_arrowHeadStyleId").unwrap_or(-1), size, font, internal, fmt));
                // the gap between its line and its text (parameter -1006401, paper feet)
                if let Some(g) = param_f(tr, -1006401).filter(|v| *v >= 0.0 && *v < 0.1) {
                    self.dim_text_gaps.insert(tr.id, g);
                }
            }
            "SpotElevationStyle" => {
                let st = |k: &str| match r.get(k) {
                    Some(G::S(v)) => v.clone(),
                    _ => String::new(),
                };
                // its symbol: an annotation family (none: the leader's arrowhead only)
                let symbol = [-1006436, -1006430].iter().any(|p| param_id(tr, *p).is_some_and(|v| v > 0));
                self.spot_styles.insert(
                    tr.id,
                    SpotStyle {
                        fmt: obj("m_oLinFormatOptions").and_then(Fmt::of),
                        ns: st("m_seIndNS"),
                        ew: st("m_seIndEW"),
                        elev: st("m_seIndElevation"),
                        suffix: r.int("m_seIndType").unwrap_or(0) == 1,
                        top: r.int("m_seTopValue").unwrap_or(1),
                        bottom: r.int("m_seBotValue").unwrap_or(2),
                        along: num(r, "m_seHorizOffset").unwrap_or(0.0),
                        above: param_f(tr, -1006401).unwrap_or(0.5 / 304.8),
                        arrow: r.int("m_arrowHeadStyleId").unwrap_or(-1),
                        font: tr.objs.iter().find_map(|o| o.int("m_fontId")).unwrap_or(-1),
                        symbol,
                    },
                );
            }
            "FontElem" => {
                if let Some(h) = obj("m_pFont").and_then(|f| num(f, "m_size")).filter(|h| *h > 1e-5 && *h < 1.0) {
                    self.font_sizes.insert(tr.id, h);
                }
            }
            "LeaderStyle" => {
                let (Some(angle), Some(size)) = (param_f(tr, -1006426), param_f(tr, -1006414)) else { return };
                if size > 1e-6 && size < 0.1 {
                    let arrow = r.int("m_tickType").unwrap_or(0) == 8;
                    self.marks.insert(tr.id, Mark { arrow, filled: r.int("m_arrowFilled").unwrap_or(0) != 0, closed: r.int("m_arrowClosed").unwrap_or(0) != 0, angle, size });
                }
            }
            "LinearDimString" if owner > 0 => {
                let refs: Vec<V3> = r.fs("m_refPnts").unwrap_or(&[]).chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
                let Some(l) = tr.objs.iter().find(|o| s.name(o.tag) == "GLine") else { return };
                let (Some(o), Some(d)) = (v3(l.fs("m_origin")), v3(l.fs("m_dirVec"))) else { return };
                let texts = r.list("m_ArrSegInfo").iter().filter_map(|g| v3(g.fs("m_origin"))).collect();
                let moved = r.list("m_ArrSegInfo").iter().map(|g| g.fs("m_offset").filter(|o| o.len() == 2).map_or([0.0; 2], |o| [o[0], o[1]])).collect();
                let field = |f: &Obj, k: &str| match f.get(k) {
                    Some(G::S(v)) => v.clone(),
                    _ => String::new(),
                };
                let typed = r
                    .list("m_ArrSegInfo")
                    .iter()
                    .map(|g| {
                        let f = g.list("m_values").first()?.ptr("m_oTextFields").and_then(|i| tr.objs.get(i))?;
                        Some([field(f, "m_prefix"), field(f, "m_valueOverride"), field(f, "m_suffix")])
                    })
                    .collect();
                // a constraint (a dimension kept as a lock): not drawn
                if r.int("m_flags").unwrap_or(0) & CONSTRAINT != 0 {
                    return;
                }
                if refs.len() >= 2 {
                    self.dims.push(Dim { id: tr.id, owner, style: r.int("m_styleSymbolId").unwrap_or(-1), refs, line: (o, d), texts, moved, typed });
                }
            }
            "SpotElevation" if owner > 0 => {
                let Some(at) = v3(r.fs("m_refPnts")) else { return };
                let values: Vec<f64> = r.list("m_ArrSegInfo").first().map(|g| g.list("m_values").iter().filter_map(|v| num(v, "m_value")).collect()).unwrap_or_default();
                let coord = tr.objs.iter().any(|o| s.name(o.tag) == "SpotCoordinateState");
                let leader = r.int("m_leader").unwrap_or(1) != 0;
                let bend = v3(r.fs("m_bendOrPosPtOffset")).filter(|_| leader).map(|o| [at[0] + o[0], at[1] + o[1], at[2] + o[2]]);
                let moved = r.list("m_ArrSegInfo").first().and_then(|g| g.fs("m_offset")).filter(|o| o.len() >= 2 && o[0].is_finite() && o[1].is_finite()).map_or([0.0; 2], |o| [o[0], o[1]]);
                let style = r.int("m_styleSymbolId").unwrap_or(-1);
                self.spots.push(Spot { id: tr.id, owner, at, bend, shoulder: num(r, "m_horizLeaderLength").filter(|_| leader).unwrap_or(0.0), values, coord, style, moved });
            }
            "RadialDim" | "AngularDim" if owner > 0 => {
                if r.int("m_flags").unwrap_or(0) & CONSTRAINT != 0 {
                    return;
                }
                let angular = s.name(t) == "AngularDim";
                let refs: Vec<V3> = r.fs("m_refPnts").unwrap_or(&[]).chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
                let seg = r.list("m_ArrSegInfo").first();
                let value = seg.and_then(|g| g.list("m_values").first().and_then(|v| num(v, "m_value"))).or_else(|| seg.and_then(|g| num(g, "m_lockedValue")));
                let center = if angular { tr.objs.iter().find(|o| s.name(o.tag) == "GArc").and_then(|a| v3(a.fs("m_center"))) } else { v3(r.fs("m_center")) };
                let (Some(center), Some(value)) = (center, value.filter(|v| v.is_finite() && *v > 0.0)) else { return };
                if refs.len() >= if angular { 2 } else { 1 } {
                    let normal = v3(r.fs("m_planeNormal")).unwrap_or([0.0, 0.0, 1.0]);
                    self.arc_dims.push(ArcDim { id: tr.id, owner, style: r.int("m_styleSymbolId").unwrap_or(-1), angular, center, refs, text: seg.and_then(|g| v3(g.fs("m_origin"))), value, normal });
                }
            }
            "IndependentTag" | "KeynoteTag" if owner > 0 => {
                let target = r
                    .obj("m_taggedEntitiesCell")
                    .map(|c| c.ptrs("m_taggedEntities"))
                    .and_then(|p| p.first().and_then(|&i| tr.objs.get(i)))
                    .and_then(|e| e.obj("m_taggedId"))
                    .or_else(|| r.obj("m_taggedElementId"))
                    .and_then(|x| x.int("m_linkInstOrHostId"))
                    .unwrap_or(-1);
                // the head: its orientation cell (recent files), else its absolute position
                let (head, angle) = match obj("m_oTagOrientationCell") {
                    Some(oc) => (v3(oc.fs("m_headLocation")), num(oc, "m_rotationAngle").unwrap_or(0.0)),
                    None => (v3(r.fs("m_absHeadPos")), if r.int("m_orientation") == Some(1) { std::f64::consts::FRAC_PI_2 } else { 0.0 }),
                };
                let Some(head) = head else { return };
                if let Some(G::S(k)) = r.get("m_keyValue") {
                    self.keys.insert(tr.id, k.clone());
                    // its text (a keynote legend lists it)
                    let text = match r.get("m_keynoteText") {
                        Some(G::S(t)) => t.clone(),
                        _ => String::new(),
                    };
                    self.keynotes.push((k.clone(), text));
                }
                let off = |k: &str| r.fs(k).filter(|v| v.len() >= 2).map(|v| [v[0], v[1]]);
                let leader = (r.int("m_leader").unwrap_or(0) != 0).then(|| (off("m_elbowOffset").unwrap_or([0.0; 2]), off("m_endOffset").unwrap_or([0.0; 2])));
                self.tags.push(Tag { owner, symbol: r.int("m_symbolId").unwrap_or(-1), head, angle, target, id: tr.id, leader });
            }
            "RoomTag" if owner > 0 => {
                let Some(head) = v3(r.fs("m_point")) else { return };
                let target = r.obj("m_taggedRoomElemId").and_then(|x| x.int("m_linkInstOrHostId")).unwrap_or(-1);
                self.tags.push(Tag { owner, symbol: r.int("m_symbolId").unwrap_or(-1), head, angle: num(r, "m_rotationAngle").unwrap_or(0.0), target, id: tr.id, leader: None });
            }
            "RoomElem" | "FamilySymbol" => {
                if let Some(c) = r.obj("m_cachedCircuitId").and_then(|c| c.int("m_id")) {
                    self.room_circuits.insert(tr.id, c);
                }
                let mut m = strs();
                m.extend(family_param_strs(tr, s));
                if !m.is_empty() {
                    self.strs.insert(tr.id, m);
                }
            }
            "FamilyInstance" => {
                if let Some(sym) = obj("m_pInstanceInfo").and_then(|ii| ii.int("m_symbolId")) {
                    self.inst_type.insert(tr.id, sym);
                }
                let mut m = strs();
                m.extend(family_param_strs(tr, s));
                if !m.is_empty() {
                    self.strs.insert(tr.id, m);
                }
            }
            "LevelRoomPlan" => {
                let Some(t) = obj("m_planTopology") else { return };
                for c in t.list("m_circuits") {
                    if let (Some(id), Some(a)) = (c.obj("m_id").and_then(|x| x.int("m_id")), num(c, "m_area")) {
                        self.circuit_areas.insert((tr.id, id), a);
                    }
                }
                if let Some(G::Ids(rooms)) = t.get("m_rooms") {
                    for &rm in rooms {
                        self.room_plans.insert(rm, tr.id);
                    }
                    self.plan_rooms.insert(tr.id, rooms.clone());
                }
            }
            "UnitsElem" => {
                let spec = |id: &str| find_format(r, id).or_else(|| tr.objs.iter().find_map(|o| find_format(o, id)));
                // older files: the formats by unit kind, in Revit's order (length first, angle
                // 4th, duct size 17th)
                let kinds: Vec<&Obj> = r.obj("m_units").map(|u| u.ptrs("m_formatOptionsArr").iter().filter_map(|&i| tr.objs.get(i)).collect()).unwrap_or_default();
                if let Some(f) = spec("autodesk.spec.aec:length-1.0.0").or_else(|| kinds.first().copied()).and_then(Fmt::of) {
                    self.len_fmt = Some(f);
                }
                if let Some(f) = spec("autodesk.spec.aec:angle-1.0.0").or_else(|| kinds.get(3).copied()) {
                    self.angle_acc = num(f, "m_accuracy").filter(|a| *a > 0.0);
                }
                if let Some(f) = spec("autodesk.spec.aec.hvac:ductSize-1.0.0").or_else(|| kinds.get(16).copied()).and_then(Fmt::of) {
                    self.duct_fmt = Some(f);
                }
                self.unit_fmts = kinds.iter().map(|o| Fmt::of(o).map(|f| (f, o.int("m_unitSymbol").is_some_and(|s| s != 0)))).collect();
                // areas (2nd kind): square metres (12) or feet (11), their unit symbol shown or not
                if let Some(o) = spec("autodesk.spec.aec:area-1.0.0").or_else(|| kinds.get(1).copied()) {
                    let type_id = |k: &str| match o.obj(k).and_then(|u| u.get("m_typeId")) {
                        Some(G::S(u)) => Some(u.clone()),
                        _ => None,
                    };
                    if let Some(mut f) = Fmt::of(o) {
                        if let Some(u) = type_id("m_unitTypeId") {
                            f.unit = if u.contains("squareFeet") { 11 } else { 12 };
                        }
                        let symbol = o.int("m_unitSymbol").map(|s| s != 0).unwrap_or_else(|| type_id("m_symbolTypeId").is_some_and(|s| !s.is_empty()));
                        self.area_fmt = Some((f, symbol));
                    }
                }
                // digit grouping: dot, comma, space, apostrophe (Revit's order); decimal point:
                // dot or comma
                let field = |k: &str| find_int(r, k).or_else(|| tr.objs.iter().find_map(|o| find_int(o, k)));
                self.grouper = match field("m_digitGroupingSymbol") {
                    Some(0) => Some('.'),
                    Some(1) => Some(','),
                    Some(2) => Some(' '),
                    Some(3) => Some('\''),
                    _ => None,
                };
                self.comma = field("m_decimalSymbol") == Some(1);
            }
            _ => {}
        }
    }

    pub fn owns(&self, view: i64) -> bool {
        self.curves.iter().any(|c| c.owner == view) || self.regions.iter().any(|r| r.0 == view) || self.dims.iter().any(|d| d.owner == view) || self.arc_dims.iter().any(|d| d.owner == view) || self.spots.iter().any(|d| d.owner == view) || self.tags.iter().any(|t| t.owner == view)
    }

    /// The tick mark of dimension type `style`, if known.
    fn mark_of(&self, style: i64) -> Option<Mark> {
        self.dim_styles.get(&style).and_then(|d| self.marks.get(&d.0)).copied()
    }

    /// Whether dimension type `style` is one Revit keeps for constraints (its dimensions are
    /// locks, not drawn).
    fn constraint_style(&self, style: i64) -> bool {
        self.dim_styles.get(&style).is_some_and(|d| d.3)
    }

    /// Dimension text size (paper feet) of dimension type `style`: its font's size, else its
    /// text size parameter.
    fn dim_text(&self, style: i64) -> f64 {
        self.dim_styles.get(&style).and_then(|d| self.font_sizes.get(&d.2).copied().or(d.1)).unwrap_or(DIM_TEXT)
    }

    /// An end mark at `p`: a diagonal tick across the line `d` (unit), or an arrowhead with its
    /// tip at `p` and its body towards `back` (unit).
    fn end_mark(&self, mark: Option<Mark>, p: P2, d: P2, back: P2, k: f64, out: &mut Drawn) {
        match mark {
            Some(m) if m.arrow => {
                let (len, h) = (m.size / k, (m.angle * 0.5).clamp(0.02, 1.5));
                let side: Vec<P2> = [-1.0, 1.0]
                    .iter()
                    .map(|sg| {
                        let (cs, sn) = (h.cos(), h.sin() * sg);
                        let l = len / cs.max(0.2);
                        [p[0] + (back[0] * cs - back[1] * sn) * l, p[1] + (back[0] * sn + back[1] * cs) * l]
                    })
                    .collect();
                if m.filled {
                    out.fills.push((vec![[p, side[0], side[1]]], [0; 3]));
                } else if m.closed {
                    out.strokes.push((vec![p, side[0], side[1], p], self.pen(1), [0; 3]));
                } else {
                    out.strokes.push((vec![side[0], p, side[1]], self.pen(1), [0; 3]));
                }
            }
            _ => {
                let half = mark.map_or(TICK, |m| m.size * 0.5) / k;
                let nv = [-d[1], d[0]];
                let a = [(d[0] + nv[0]) * half, (d[1] + nv[1]) * half];
                out.strokes.push((vec![[p[0] - a[0], p[1] - a[1]], [p[0] + a[0], p[1] + a[1]]], self.pen(3), [0; 3]));
            }
        }
    }

    /// The pieces of the paper polyline `pts` drawn in the projection line style of `category`
    /// (its pen, colour and line pattern), as (points, weight, colour); None when the category
    /// has no style, empty when its lines are invisible.
    pub(crate) fn category_line(&self, category: i64, pts: &[P2]) -> Option<Vec<(Vec<P2>, f64, [u8; 3])>> {
        let st = self.styles.get(self.cat_styles.get(&category)?)?;
        if st.pattern == -1 {
            return Some(Vec::new());
        }
        let w = self.pen(st.pen);
        Some(match self.line_patterns.get(&st.pattern).filter(|p| p.iter().any(|s| !s.1) && p.iter().all(|s| s.0 > 0.0)) {
            Some(pat) => dashes(pts, pat, 0.0).into_iter().map(|piece| (piece, w, st.color)).collect(),
            None => vec![(pts.to_vec(), w, st.color)],
        })
    }

    /// `category_line` with the category's model pen at view scale `scale` (paper / model), as
    /// Revit draws a model category's lines.
    pub(crate) fn category_line_at(&self, category: i64, pts: &[P2], scale: f64) -> Option<Vec<(Vec<P2>, f64, [u8; 3])>> {
        let st = self.styles.get(self.cat_styles.get(&category)?)?;
        let w = self.model_pen(st.pen, scale);
        Some(self.category_line(category, pts)?.into_iter().map(|(p, _, c)| (p, w, c)).collect())
    }

    /// The projection style of `category`: (pen, colour, line pattern).
    pub(crate) fn category_style(&self, category: i64) -> Option<(i64, [u8; 3], i64)> {
        let st = self.styles.get(self.cat_styles.get(&category)?)?;
        Some((st.pen, st.color, st.pattern))
    }

    /// The free lines of family document `root` (its model and symbolic lines, not the
    /// sketches of its solids and regions): points in the family's frame, line style (see
    /// `family_line_style`), the views and detail levels showing it (`m_famElemVisibility`)
    /// and its visibility parameter.
    /// (each line drawn belongs to a sketch too; those of a form's or region's sketch have no
    /// visibility of their own)
    pub(crate) fn family_lines(&self, root: i64) -> Vec<(&[V3], i64, i64, &Visible)> {
        let by = self.fam_lines.get_or_init(|| {
            let mut by: HashMap<i64, Vec<usize>> = HashMap::default();
            for (k, c) in self.curves.iter().enumerate().filter(|(_, c)| c.owner < -1 && (c.fev != -1 || c.sketch == -1)) {
                by.entry(-c.owner).or_default().push(k);
            }
            by
        });
        by.get(&root).into_iter().flatten().map(|&k| &self.curves[k]).filter(|c| !self.invisible_style(root, c.style)).map(|c| (c.pts.as_slice(), c.style, c.fev, &c.vis)).collect()
    }

    /// The project's subcategory of a line of family document `root` in line style `style`
    /// (its subcategory's, or the project's of the same name), when known.
    pub(crate) fn family_line_subcategory(&self, root: i64, style: i64) -> Option<i64> {
        let g = [scoped(root, style), style].into_iter().find(|g| self.styles.contains_key(g))?;
        let c = *self.gstyle_cats.get(&g)?;
        Some(self.cat_names.get(&c).and_then(|k| self.subcats.get(k)).copied().unwrap_or(c))
    }

    /// Whether line style `style` (of family document `doc`, or the project's) is the one of
    /// invisible lines (`<Invisible lines>`).
    pub(crate) fn invisible_style(&self, doc: i64, style: i64) -> bool {
        [scoped(doc, style), style].iter().filter_map(|g| self.gstyle_cats.get(g)).any(|c| *c == INVISIBLE_LINES)
    }

    /// Whether line style `style` is the one of reference lines.
    pub(crate) fn reference_line_style(&self, style: i64) -> bool {
        self.gstyle_cats.get(&style) == Some(&REFERENCE_LINES)
    }

    /// The reference lines of family document `root`, by id.
    pub(crate) fn reference_lines(&self, root: i64) -> Vec<&Vec<V3>> {
        let mut v: Vec<&(i64, Vec<V3>)> = self.ref_lines.get(&root).map(|v| v.iter().collect()).unwrap_or_default();
        v.sort_by_key(|r| r.0);
        v.into_iter().map(|r| &r.1).collect()
    }

    /// The look of a line of family document `root` in line style `style` (its own, or the
    /// project's once the family is loaded): its subcategory's projection style, the project's
    /// for that subcategory's name (its object styles win), else the line style itself.
    /// (pen, colour, line pattern; pattern -1: invisible lines)
    pub(crate) fn family_line_style(&self, root: i64, style: i64) -> Option<(i64, [u8; 3], i64)> {
        let g = [scoped(root, style), style].into_iter().find(|g| self.styles.contains_key(g))?;
        self.gstyle_cats.get(&g).and_then(|c| self.subcategory_style(*c)).or_else(|| self.styles.get(&g).map(|s| (s.pen, s.color, s.pattern)))
    }

    /// The projection style of a family document's subcategory (`scoped`): the project's
    /// subcategory of the same name when the project has it (its object styles win), else the
    /// family's own.
    pub(crate) fn subcategory_style(&self, sub: i64) -> Option<(i64, [u8; 3], i64)> {
        self.cat_names.get(&sub).and_then(|k| self.subcats.get(k)).and_then(|c| self.category_style(*c)).or_else(|| self.category_style(sub)).or_else(|| self.category_style(unscoped(sub)))
    }

    /// Colour of a category (black when unknown).
    pub(crate) fn style_color(&self, category: i64) -> [u8; 3] {
        self.cat_colors.get(&category).copied().unwrap_or([0; 3])
    }

    /// Line families of fill pattern `id` as drawn on paper at view scale `k`: (angle,
    /// spacing in feet); empty for a solid fill, None when the pattern is unknown.
    pub(crate) fn pattern_lines(&self, id: i64, k: f64) -> Option<Vec<(f64, f64)>> {
        let p = self.fill_patterns.get(&id)?;
        let unit = if p.model { p.scale * k } else { p.scale };
        Some(p.grids.iter().map(|g| (g.angle, g.offset.abs() * unit)).filter(|g| g.1 > 1e-6).collect())
    }

    /// Width on paper (feet) of model pen `n` in a view at scale `scale` (paper / model): the
    /// table's column for that scale, else the next coarser one.
    pub(crate) fn model_pen(&self, n: i64, scale: f64) -> f64 {
        let inv = if scale > 1e-9 { (1.0 / scale).round() as i64 } else { 100 };
        let n = n.clamp(1, 16) as usize;
        self.model_pens
            .iter()
            .find(|e| e.0 >= inv)
            .or(self.model_pens.last())
            .and_then(|e| e.1.get(n - 1).copied())
            .filter(|w| *w > 0.0)
            .unwrap_or_else(|| self.pen(n as i64))
    }

    /// Segments of line pattern `id` (lengths on paper, drawn or not), when it is a dashed one.
    pub(crate) fn line_pattern(&self, id: i64) -> Option<&[(f64, bool)]> {
        self.line_patterns.get(&id).filter(|p| p.iter().any(|s| !s.1) && p.iter().all(|s| s.0 > 0.0)).map(|p| p.as_slice())
    }

    pub(crate) fn pen(&self, n: i64) -> f64 {
        let n = n.clamp(1, 16) as usize;
        self.pens.get(n - 1).copied().filter(|w| *w > 0.0).unwrap_or(0.00085 * n as f64)
    }

    /// A length in the project's unit and accuracy (feet in), its digits not grouped (as level
    /// heads write their elevation).
    pub fn length(&self, ft: f64) -> String {
        self.length_as(ft, self.len_fmt.map(|f| Fmt { group: false, ..f }))
    }

    /// A length (feet in) written in format `fmt`: the project's when there is none or when it
    /// says so; its digits grouped as the project groups them when the format does.
    pub(crate) fn length_as(&self, ft: f64, fmt: Option<Fmt>) -> String {
        let f = match fmt {
            Some(f) if !f.default => f,
            _ => self.len_fmt.unwrap_or(Fmt { unit: 2, acc: 1.0, group: false, default: false }),
        };
        let acc = f.acc;
        let dec = (-acc.log10()).ceil().max(0.0) as usize;
        let round = |v: f64, acc: f64| (v / acc).round() * acc;
        let number = |v: f64, unit: &str| grouped(&format!("{:.*}", dec, round(v, acc)), f.group.then_some(self.grouper).flatten(), if self.comma { ',' } else { '.' }) + unit;
        match f.unit {
            2 => number(ft * 304.8, ""),
            1 => number(ft * 30.48, ""),
            0 => number(ft * 0.3048, ""),
            4 => {
                let step = if acc > 0.0 { acc * 12.0 } else { 1.0 / 8.0 };
                let inches = round(ft * 12.0, step.max(1.0 / 256.0));
                let (f, i) = ((inches / 12.0).floor(), inches - (inches / 12.0).floor() * 12.0);
                format!("{f}' - {}\"", (i * 1000.0).round() / 1000.0)
            }
            5 | 6 => number(ft * 12.0, "\""),
            _ => number(ft, "'"),
        }
    }

    /// Value `v` (Revit's internal unit) of parameter `param` written in format `own` (a
    /// label's), else the project's for its kind, with its unit when the format shows it.
    /// shortcut: air flow only (cubic metres an hour), add kinds as sheets show them.
    pub(crate) fn number(&self, param: i64, v: f64, own: Option<(Fmt, bool)>) -> Option<String> {
        const AIR_FLOW: i64 = 15;
        let kind = *self.param_units.get(&param)?;
        if kind != AIR_FLOW {
            return None;
        }
        let (f, symbol) = match own {
            Some((f, s)) if !f.default => (f, s),
            _ => (*self.unit_fmts.get(kind as usize)?)?,
        };
        let (x, unit) = match f.unit {
            66 => (v * 0.028316846592 * 3600.0, " m³/h"),
            _ => return None,
        };
        let dec = (-f.acc.log10()).ceil().max(0.0) as usize;
        let s = grouped(&format!("{:.*}", dec, (x / f.acc).round() * f.acc), f.group.then_some(self.grouper).flatten(), if self.comma { ',' } else { '.' });
        Some(if symbol { s + unit } else { s })
    }

    /// An area (square feet in) in the project's area unit and accuracy.
    pub(crate) fn area(&self, ft2: f64) -> String {
        let (f, symbol) = self.area_fmt.unwrap_or((Fmt { unit: 12, acc: 1.0, group: false, default: false }, true));
        let (v, unit) = if f.unit == 11 { (ft2, " ft²") } else { (ft2 * 0.3048 * 0.3048, " m²") };
        let dec = (-f.acc.log10()).ceil().max(0.0) as usize;
        let s = grouped(&format!("{:.*}", dec, (v / f.acc).round() * f.acc), f.group.then_some(self.grouper).flatten(), if self.comma { ',' } else { '.' });
        if symbol { s + unit } else { s }
    }

    /// How room or space `id` is bounded, as its area shows it: the area of the closed circuit
    /// it stands in (the first of its rooms there holds it, the others are redundant), else
    /// not enclosed; `None` for other elements.
    pub(crate) fn enclosure(&self, id: i64) -> Option<Enclosure> {
        let c = *self.room_circuits.get(&id)?;
        let plan = self.room_plans.get(&id);
        let Some(&a) = plan.and_then(|p| self.circuit_areas.get(&(*p, c))) else { return Some(Enclosure::Open) };
        let first = plan.and_then(|p| self.plan_rooms.get(p)).and_then(|rs| rs.iter().find(|r| self.room_circuits.get(r) == Some(&c)));
        Some(if first.is_some_and(|f| *f != id) { Enclosure::Redundant } else { Enclosure::Area(a) })
    }

    /// A duct size in millimetres written as the project writes duct sizes (its digits grouped
    /// when it groups them).
    pub(crate) fn size_mm(&self, mm: i64) -> String {
        grouped(&mm.to_string(), self.duct_fmt.filter(|f| f.group).and(self.grouper), if self.comma { ',' } else { '.' })
    }

    /// The length format of dimension type `style`.
    fn dim_fmt(&self, style: i64) -> Option<Fmt> {
        self.dim_styles.get(&style).and_then(|d| d.4)
    }

    /// The annotations owned by `owner`, in its plane: `proj` maps model feet there, `k` is the
    /// view scale (paper / model).
    pub(crate) fn draw(&self, owner: i64, proj: &dyn Fn(V3) -> P2, k: f64, out: &mut Drawn) {
        self.draw_with(owner, proj, k, &HashMap::default(), &|_| false, out);
    }

    /// `draw`, leaving out the annotations `hide` says the view hides, each drawn as the view
    /// overrides it (`over`: an element's own graphics in the view).
    pub(crate) fn draw_hiding(&self, owner: i64, proj: &dyn Fn(V3) -> P2, k: f64, hide: &dyn Fn(i64) -> bool, over: &dyn Fn(i64) -> Option<LineOver>, out: &mut Drawn) {
        self.draw_over(owner, proj, k, &HashMap::default(), hide, over, out);
    }

    /// Family document `root`'s parameter of the same name as family parameter `param`.
    pub(crate) fn doc_param(&self, root: i64, param: i64) -> Option<i64> {
        self.fam_params.get(&(root, self.param_names.get(&param)?.clone())).copied()
    }

    /// `draw`, the family elements following the parameters `vals` (document ids), leaving
    /// out the elements `hide` gives.
    pub(crate) fn draw_with(&self, owner: i64, proj: &dyn Fn(V3) -> P2, k: f64, vals: &HashMap<i64, bool>, hide: &dyn Fn(i64) -> bool, out: &mut Drawn) {
        self.draw_over(owner, proj, k, vals, hide, &|_| None, out);
    }

    /// `draw_with`, each element drawn as `over` overrides it.
    #[allow(clippy::too_many_arguments)]
    fn draw_over(&self, owner: i64, proj: &dyn Fn(V3) -> P2, k: f64, vals: &HashMap<i64, bool>, hide: &dyn Fn(i64) -> bool, over: &dyn Fn(i64) -> Option<LineOver>, out: &mut Drawn) {
        let k = k.max(1e-6);
        let doc = if owner < -1 { -owner } else { 0 };
        // (a family's lines and regions show as its parameters say; in a project view their
        // visibility, left from a family they were copied from, means nothing)
        let shows = |v: &Visible| owner > 0 || v.shows(vals);
        let hidden: crate::det::HashSet<i64> = self.regions.iter().filter(|r| r.0 == owner && (!shows(&r.3) || hide(r.4))).map(|r| r.1).collect();
        // detail lines
        // (in a view, a line without a style is a sketch line of another element, such as a
        // plan region's boundary: not drawn)
        if std::env::var("ANNOT_DEBUG").is_ok_and(|v| v == (-owner).to_string()) {
            for c in self.curves.iter().filter(|c| c.owner == owner) {
                eprintln!("  curve {} style {} sketch {} shows {} sketch hidden {} vis {:?} fev {:#x} pts {:?}", c.id, c.style, c.sketch, c.vis.shows(vals), hidden.contains(&c.sketch), c.vis, c.fev, c.pts.iter().map(|p| [(p[0] * 1000.0).round() / 1000.0, (p[1] * 1000.0).round() / 1000.0]).collect::<Vec<_>>().first());
            }
            eprintln!("  vals {vals:?}");
        }
        for c in self.curves.iter().filter(|c| c.owner == owner && shows(&c.vis) && !hidden.contains(&c.sketch) && !hide(c.id) && !(owner > 0 && c.style == -1)) {
            // (a family's line: its subcategory's style, the project's when the project has a
            // subcategory of that name, as its object styles win)
            let st = (doc > 0)
                .then(|| self.family_line_style(doc, c.style))
                .flatten()
                .map(|(pen, color, pattern)| Style { pen, color, pattern })
                .or_else(|| self.styles.get(&scoped(doc, c.style)).or_else(|| self.styles.get(&c.style)).cloned())
                .unwrap_or(Style { pen: 1, color: [0; 3], pattern: -3000010 });
            // (its own graphics in the view)
            let st = match over(c.id) {
                Some(o) => Style { pen: o.pen.unwrap_or(st.pen), color: o.color.unwrap_or(st.color), pattern: o.pattern.unwrap_or(st.pattern) },
                None => st,
            };
            if st.pattern == -1 || self.invisible_style(doc, c.style) {
                continue; // invisible lines
            }
            let mut pts: Vec<P2> = c.pts.iter().map(|p| proj(*p)).collect();
            if let (Some(cl), 2) = (c.cloud, pts.len()) {
                pts = cloud_points(pts[0], pts[1], cl, CLOUD_ARC / k);
            }
            let w = self.pen(st.pen);
            match self.line_patterns.get(&scoped(doc, st.pattern)).or_else(|| self.line_patterns.get(&st.pattern)).filter(|p| p.iter().any(|s| !s.1) && p.iter().all(|s| s.0 > 0.0)) {
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
        for (_, sk, at, _, rid) in self.regions.iter().filter(|r| r.0 == owner && shows(&r.3) && !hide(r.4)) {
            let edges: Vec<[P2; 2]> = self.curves.iter().filter(|c| c.sketch == *sk).flat_map(|c| c.pts.windows(2).map(|w| [proj(w[0]), proj(w[1])]).collect::<Vec<_>>()).collect();
            if edges.len() < 3 {
                continue;
            }
            if std::env::var("REGION_DEBUG").is_ok_and(|v| v == owner.to_string()) {
                eprintln!("REGION {rid} type {at} -> {:?} pattern {:?}", self.region_types.get(&scoped(doc, *at)).or_else(|| self.region_types.get(at)), self.region_types.get(at).and_then(|t| self.fill_patterns.get(&t.0)).map(|p| p.grids.len()));
            }
            let Some(&(f, style)) = self.region_types.get(&scoped(doc, *at)).or_else(|| self.region_types.get(at)) else { continue };
            let Some(pat) = self.fill_patterns.get(&scoped(doc, f)).or_else(|| self.fill_patterns.get(&f)) else { continue };
            let tint = self.cat_colors.get(&scoped(doc, style)).map_or_else(|| self.style_color(style), |c| *c);
            let tint = over(*rid).and_then(|o| o.fill).unwrap_or(tint);
            if pat.grids.is_empty() {
                out.fills.push((fill(&edges), tint));
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
                                out.strokes.push((piece, self.pen(1), tint));
                            }
                        } else {
                            out.strokes.push((line, self.pen(1), tint));
                        }
                    }
                }
            }
        }
        // linear dimensions
        for dm in self.dims.iter().filter(|d| d.owner == owner && !hide(d.id) && !self.constraint_style(d.style)) {
            out.begin_whole();
            let o = proj(dm.line.0);
            let q = proj([dm.line.0[0] + dm.line.1[0], dm.line.0[1] + dm.line.1[1], dm.line.0[2] + dm.line.1[2]]);
            let mut d = [q[0] - o[0], q[1] - o[1]];
            let l = sdot(d, d).sqrt();
            if l < 1e-9 {
                continue; // seen along its line
            }
            d = [d[0] / l, d[1] / l];
            // (its direction as stored, before turned to read: what a moved text's offset is
            // measured along, per foot of the model)
            let (stored, per_foot) = (d, l / sdot([dm.line.1[0], dm.line.1[1]], [dm.line.1[0], dm.line.1[1]]).max(1e-18).sqrt().max(dm.line.1[2].abs()).max(1e-9));
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
                ts.push(t);
            }
            let (t0, t1) = ts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, &t| (a.0.min(t), a.1.max(t)));
            // end marks: a tick on each witness line, or arrows pointing out of each segment
            let mark = self.mark_of(dm.style);
            for &t in &ts {
                let f = [o[0] + d[0] * t, o[1] + d[1] * t];
                if mark.is_some_and(|m| m.arrow) {
                    if t > t0 + 1e-9 {
                        self.end_mark(mark, f, d, [-d[0], -d[1]], k, out);
                    }
                    if t < t1 - 1e-9 {
                        self.end_mark(mark, f, d, d, k, out);
                    }
                } else {
                    self.end_mark(mark, f, d, d, k, out);
                }
            }
            let ov = EXT_OVER / k;
            out.strokes.push((vec![[o[0] + d[0] * (t0 - ov), o[1] + d[1] * (t0 - ov)], [o[0] + d[0] * (t1 + ov), o[1] + d[1] * (t1 + ov)]], w, [0; 3]));
            for (i, p) in ts.windows(2).enumerate() {
                let v = (p[1] - p[0]).abs();
                if v < 1e-6 {
                    continue;
                }
                let mid = (p[0] + p[1]) * 0.5;
                let value = self.length_as(v, self.dim_fmt(dm.style));
                let text = match dm.typed.get(i).and_then(|t| t.as_ref()) {
                    Some([pre, over, suf]) => format!("{pre}{}{suf}", if over.is_empty() { &value } else { over }),
                    None => value,
                };
                // its text: the bottom of its letters' descent (its box's bottom) the type's gap
                // off the line, as Revit sets it, or where it was moved to, centred along the
                // line
                let gap = self.dim_text_gaps.get(&dm.style).copied().unwrap_or(1.0 / 304.8) / k;
                let pos = dm.texts.get(i).map(|x| proj(*x)).map(|x| {
                    let c = sdot([x[0] - o[0], x[1] - o[1]], nv);
                    [o[0] + d[0] * mid + nv[0] * c.max(gap), o[1] + d[1] * mid + nv[1] * c.max(gap)]
                });
                let mut pos = pos.unwrap_or([o[0] + d[0] * mid + nv[0] * gap, o[1] + d[1] * mid + nv[1] * gap]);
                // moved off its place (too long for its segment): there, a leader from the
                // segment's middle on the line to a shoulder as long as the text is high, along
                // the line, ending at its end nearest the middle, halfway up its letters
                // ("910" on AC-293)
                if let Some(mv) = dm.moved.get(i).filter(|m| m[0].abs() + m[1].abs() > 1e-9) {
                    let sn = [-stored[1], stored[0]];
                    let off = [(mv[0] * stored[0] + mv[1] * sn[0]) * per_foot, (mv[0] * stored[1] + mv[1] * sn[1]) * per_foot];
                    pos = [pos[0] + off[0], pos[1] + off[1]];
                    let h = self.dim_text(dm.style) / k;
                    let half = sw_format::arial_width(&text, sw_format::font_size((h * k * 0.3048) as f32), false) as f64 / 0.3048 / k * 0.5;
                    let line_mid = [o[0] + d[0] * mid, o[1] + d[1] * mid];
                    let along = sdot([pos[0] - line_mid[0], pos[1] - line_mid[1]], d);
                    let toward = if along > 0.0 { -1.0 } else { 1.0 };
                    let up = h * 0.8;
                    let end = [pos[0] + d[0] * half * toward + nv[0] * up, pos[1] + d[1] * half * toward + nv[1] * up];
                    let elbow = [end[0] + d[0] * h * toward, end[1] + d[1] * h * toward];
                    out.strokes.push((vec![line_mid, elbow, end], w, [0; 3]));
                }
                out.spans.push((out.texts.len(), [[o[0] + d[0] * p[0], o[1] + d[1] * p[0]], [o[0] + d[0] * p[1], o[1] + d[1] * p[1]]]));
                out.texts.push((pos, d, self.dim_text(dm.style), text));
            }
        }
        // radial and angular dimensions
        for dm in self.arc_dims.iter().filter(|d| d.owner == owner && !hide(d.id) && !self.constraint_style(d.style)) {
            out.begin_whole();
            let w = self.pen(1);
            let c = proj(dm.center);
            let read = |d: P2| if d[0] < -1e-6 || (d[0].abs() <= 1e-6 && d[1] < 0.0) { [-d[0], -d[1]] } else { d };
            if !dm.angular {
                let p = proj(dm.refs[0]);
                let mut d = [p[0] - c[0], p[1] - c[1]];
                let l = sdot(d, d).sqrt();
                if l < 1e-9 {
                    continue; // seen edge on
                }
                d = [d[0] / l, d[1] / l];
                // the line runs from the centre to the arc, or on to the text past it
                let tt = dm.text.map(|t| sdot([proj(t)[0] - c[0], proj(t)[1] - c[1]], d)).unwrap_or(l);
                let end = tt.max(l);
                out.strokes.push((vec![c, [c[0] + d[0] * end, c[1] + d[1] * end]], w, [0; 3]));
                // arrow on the arc, pointing outwards (the type's mark, an open arrow by default)
                let mark = self.mark_of(dm.style).or(Some(Mark { arrow: true, filled: false, closed: false, angle: 0.7, size: TICK * 2.0 }));
                self.end_mark(mark, p, d, [-d[0], -d[1]], k, out);
                let dr = read(d);
                let nv = [-dr[1], dr[0]];
                let mid = l * 0.5;
                let pos = dm.text.map(proj).unwrap_or([c[0] + d[0] * mid + nv[0] * 0.6 / 304.8 / k, c[1] + d[1] * mid + nv[1] * 0.6 / 304.8 / k]);
                out.texts.push((pos, dr, self.dim_text(dm.style), format!("R {}", self.length_as(dm.value, self.dim_fmt(dm.style)))));
                continue;
            }
            // angular: the arc through the two points, around the centre, in the dimension's plane
            let n = dm.normal;
            let u0 = [dm.refs[0][0] - dm.center[0], dm.refs[0][1] - dm.center[1], dm.refs[0][2] - dm.center[2]];
            let r0 = (u0[0] * u0[0] + u0[1] * u0[1] + u0[2] * u0[2]).sqrt();
            if r0 < 1e-9 {
                continue;
            }
            let u = [u0[0] / r0, u0[1] / r0, u0[2] / r0];
            let v = [n[1] * u[2] - n[2] * u[1], n[2] * u[0] - n[0] * u[2], n[0] * u[1] - n[1] * u[0]];
            let w2 = [dm.refs[1][0] - dm.center[0], dm.refs[1][1] - dm.center[1], dm.refs[1][2] - dm.center[2]];
            let th = (w2[0] * v[0] + w2[1] * v[1] + w2[2] * v[2]).atan2(w2[0] * u[0] + w2[1] * u[1] + w2[2] * u[2]);
            let at = |t: f64| proj([dm.center[0] + r0 * (t.cos() * u[0] + t.sin() * v[0]), dm.center[1] + r0 * (t.cos() * u[1] + t.sin() * v[1]), dm.center[2] + r0 * (t.cos() * u[2] + t.sin() * v[2])]);
            let steps = ((th.abs() * 24.0).ceil() as usize).clamp(4, 96);
            let arc: Vec<P2> = (0..=steps).map(|i| at(th * i as f64 / steps as f64)).collect();
            let span = arc.windows(2).map(|q| ((q[1][0] - q[0][0]).powi(2) + (q[1][1] - q[0][1]).powi(2)).sqrt()).sum::<f64>();
            if span < 1e-9 {
                continue; // seen edge on
            }
            out.strokes.push((arc, w, [0; 3]));
            let mark = self.mark_of(dm.style);
            for t in [0.0, th] {
                let p = at(t);
                // along the arc, towards its other end
                let q = at(if t == 0.0 { th.signum() * 0.01 } else { th - th.signum() * 0.01 });
                let mut d = [q[0] - p[0], q[1] - p[1]];
                let l = sdot(d, d).sqrt().max(1e-12);
                d = [d[0] / l, d[1] / l];
                self.end_mark(mark, p, d, d, k, out);
            }
            let (pm, qm) = (at(th * 0.5), at(th * 0.5 + 0.01));
            let mut d = [qm[0] - pm[0], qm[1] - pm[1]];
            let l = sdot(d, d).sqrt().max(1e-12);
            d = read([d[0] / l, d[1] / l]);
            let pos = dm.text.map(proj).unwrap_or(pm);
            out.texts.push((pos, d, self.dim_text(dm.style), self.angle(dm.value)));
        }
        // spot elevations and coordinates, as Revit lays them out: the leader from the point to
        // its bend and along its shoulder, the type's arrowhead on the point; the values in
        // the type's format, after their indicators
        for sp in self.spots.iter().filter(|d| d.owner == owner && !hide(d.id)) {
            out.begin_whole();
            let w = self.pen(1);
            let st = self.spot_styles.get(&sp.style);
            let size = st.and_then(|s| self.font_sizes.get(&s.font).copied()).unwrap_or(DIM_TEXT);
            let p = proj(sp.at);
            let b = sp.bend.map(proj);
            // the leader's end: the shoulder's, along the view's horizontal (no leader: the point)
            let e = b.map_or(p, |b| [b[0] + sp.shoulder, b[1]]);
            if let Some(b) = b {
                out.strokes.push((vec![p, b, e], w, [0; 3]));
                let back = [b[0] - p[0], b[1] - p[1]];
                let l = sdot(back, back).sqrt();
                if let Some(m) = st.and_then(|s| self.marks.get(&s.arrow)).copied().filter(|_| l > 1e-9) {
                    let u = [back[0] / l, back[1] / l];
                    self.end_mark(Some(m), p, u, u, k, out);
                }
            }
            // its symbol (an annotation family, not drawn yet): a small triangle on the point
            if st.map_or(true, |s| s.symbol) {
                let a = TICK * 1.5 / k;
                out.strokes.push((vec![p, [p[0] - a, p[1] + a * 1.6], [p[0] + a, p[1] + a * 1.6], p], w, [0; 3]));
            }
            let value = |v: f64| self.length_as(v, st.and_then(|s| s.fmt));
            let with = |ind: &str, v: String| match (ind.is_empty(), st.is_some_and(|s| s.suffix)) {
                (true, _) => v,
                (false, false) => format!("{ind} {v}"),
                (false, true) => format!("{v} {ind}"),
            };
            let lines: Vec<String> = if sp.coord && sp.values.len() >= 3 {
                // values: elevation, east, north
                let line = |kind: i64| match kind {
                    1 => Some(with(st.map_or("N", |s| s.ns.as_str()), value(sp.values[2]))),
                    2 => Some(with(st.map_or("E", |s| s.ew.as_str()), value(sp.values[1]))),
                    _ => None,
                };
                [st.map_or(1, |s| s.top), st.map_or(2, |s| s.bottom)].into_iter().filter_map(line).collect()
            } else if let Some(v) = sp.values.first().filter(|v| v.abs() < 1e29) {
                vec![with(st.map_or("", |s| s.elev.as_str()), value(*v))]
            } else {
                continue;
            };
            // the text: its last line's bottom the type's offset above the leader's end, its
            // lines centred the type's offset from that end back along the shoulder, then
            // where it was moved; its lines Revit's distance apart
            let dir = if sp.shoulder != 0.0 { sp.shoulder.signum() } else if b.is_some_and(|b| b[0] < p[0]) { -1.0 } else { 1.0 };
            let along = st.map_or(0.0, |s| s.along) / k;
            let above = st.map_or(0.5 / 304.8, |s| s.above) / k;
            let at = [e[0] - dir * along + sp.moved[0], e[1] + above + sp.moved[1]];
            let pitch = sw_format::line_pitch(sw_format::font_size((size * 0.3048) as f32), (false, false), (false, false)) as f64 / 0.3048 / k;
            for (i, t) in lines.iter().rev().enumerate() {
                out.texts.push(([at[0], at[1] + i as f64 * pitch], [1.0, 0.0], size, t.clone()));
            }
        }
    }

    /// An angle (radians) in degrees at the project's angle accuracy.
    pub fn angle(&self, rad: f64) -> String {
        let acc = self.angle_acc.unwrap_or(0.01);
        let dec = (-acc.log10()).ceil().max(0.0) as usize;
        format!("{:.*}°", dec, (rad.to_degrees() / acc).round() * acc)
    }
}

/// Pieces of a polyline drawn with a dash pattern (lengths along the line, drawn or not),
/// starting `phase` into the pattern.
pub(crate) fn dashes(pts: &[P2], pat: &[(f64, bool)], phase: f64) -> Vec<Vec<P2>> {
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
pub(crate) fn fill(edges: &[[P2; 2]]) -> Vec<[P2; 3]> {
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
    fn end_marks() {
        let a = Annots::default();
        let mut out = Drawn::default();
        // filled arrow, tip at the origin, body towards +x, 30° wide, 3 mm long at 1:1
        let m = Mark { arrow: true, filled: true, closed: false, angle: 30f64.to_radians(), size: 0.01 };
        a.end_mark(Some(m), [0.0, 0.0], [1.0, 0.0], [1.0, 0.0], 1.0, &mut out);
        let t = out.fills[0].0[0];
        assert_eq!(t[0], [0.0, 0.0]);
        assert!(t[1][0] > 0.0099 && t[2][0] > 0.0099);
        assert!((t[1][1] + t[2][1]).abs() < 1e-12 && (t[1][1].abs() / t[1][0] - 15f64.to_radians().tan()).abs() < 1e-9);
        // diagonal: one stroke across the point
        a.end_mark(None, [0.0, 0.0], [1.0, 0.0], [1.0, 0.0], 1.0, &mut out);
        let s = &out.strokes[0].0;
        assert!((s[0][0] + s[1][0]).abs() < 1e-12 && s[1][0] > 0.0 && s[1][1] > 0.0);
    }

    #[test]
    fn lengths() {
        let a = Annots::default();
        assert_eq!(a.length(1.0), "305");
    }

    #[test]
    fn number_formats() {
        // digits grouped by three, the decimal point as the project writes it
        assert_eq!(grouped("1300", Some(' '), '.'), "1 300");
        assert_eq!(grouped("-2510", Some(' '), '.'), "-2 510");
        assert_eq!(grouped("25000.0", Some(' '), ','), "25 000,0");
        assert_eq!(grouped("700", Some(' '), '.'), "700");
        assert_eq!(grouped("1300", None, '.'), "1300");
        // a project grouping its lengths: a type's own format without grouping (a spot in
        // metres to the millimetre), a dimension type using the project's (grouped)
        let mut a = Annots { grouper: Some(' '), len_fmt: Some(Fmt { unit: 2, acc: 1.0, group: true, default: false }), ..Default::default() };
        assert_eq!(a.length_as(129316.977 / 0.3048, Some(Fmt { unit: 0, acc: 0.001, group: false, default: false })), "129316.977");
        assert_eq!(a.length_as(3249.0 / 304.8, Some(Fmt { unit: 2, acc: 1.0, group: false, default: true })), "3 249");
        assert_eq!(a.length(3249.0 / 304.8), "3249");
        // duct sizes as the project groups them
        assert_eq!(a.size_mm(1300), "1300");
        a.duct_fmt = Some(Fmt { unit: 2, acc: 1.0, group: true, default: false });
        assert_eq!(a.size_mm(1300), "1 300");
    }
}
