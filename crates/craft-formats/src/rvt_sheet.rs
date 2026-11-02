//! Revit sheets (`DBViewSheet`) drawn flat: what Revit shows when a sheet is opened, rebuilt
//! from the elements the sheet owns (`m_ownerDBViewId`):
//!
//! - the title block and other annotation families (`FamilyInstance` placed by its transform):
//!   their symbol keeps its 2D drawing (`FamilySymbol.m_nonBRepGeomPtrs` curves, texts in
//!   `m_textNoteData`, labels in `m_tagNoteData` filled with the sheet and project parameter
//!   values, schedules such as the revision schedule);
//! - the placed views (`Viewport`: centre `m_offset`, `m_scale`): the view's crop region
//!   (`Viewer.m_boundedSpace`), the edges of the model elements it sees projected along the
//!   view axes, or the saved image of a rendering; the view title below;
//! - texts (`TextNote`, rich text `RtfData` reduced to plain text) and schedules
//!   (`ScheduleInstance`: title and header rows stored in the schedule).
//!
//! Lengths are feet in the file, metres in the output (paper). Clean-room, from byte
//! observations only (see docs/rvt-rea-notes.md).

use crate::rvt_browser::Schema;
use crate::rvt_geom::{Obj, Tree, G};
use crate::rvt_shape::{curve_points, Mesh, Trf};
use std::collections::HashMap;
use sw_format::{Fill2d, Image2d, Rgba, Sheet2d, Stroke2d, Text2d, ViewFrame, Viewport2d};

const FEET: f64 = 0.3048;
type V3 = [f64; 3];
type P2 = [f64; 2];

fn v3(s: Option<&[f64]>) -> Option<V3> {
    let s = s?;
    (s.len() >= 3 && s[..3].iter().all(|x| x.is_finite())).then(|| [s[0], s[1], s[2]])
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn num(o: &Obj, k: &str) -> Option<f64> {
    match o.get(k)? {
        G::F(v) if v.is_finite() => Some(*v),
        G::I(v) => Some(*v as f64),
        _ => None,
    }
}
fn text_of(o: &Obj, k: &str) -> Option<String> {
    match o.get(k)? {
        G::S(s) => Some(s.clone()),
        _ => None,
    }
}

/// Plain text of a rich text (RTF) stream: groups of tables and destinations skipped,
/// `\par` / `\line` as line breaks, `\'hh` (Windows-1252) and `\uN` characters decoded.
pub fn rtf_text(b: &[u8]) -> String {
    rtf_parse(b).0
}

/// [`rtf_text`] and whether most of the text is bold (`\b`).
fn rtf_parse(b: &[u8]) -> (String, bool) {
    fn cp1252(c: u8) -> char {
        const HI: [u16; 32] = [
            0x20ac, 0x81, 0x201a, 0x192, 0x201e, 0x2026, 0x2020, 0x2021, 0x2c6, 0x2030, 0x160, 0x2039, 0x152, 0x8d, 0x17d, 0x8f, 0x90, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014, 0x2dc, 0x2122, 0x161, 0x203a, 0x153, 0x9d, 0x17e, 0x178,
        ];
        if (0x80..0xa0).contains(&c) {
            char::from_u32(HI[(c - 0x80) as usize] as u32).unwrap_or('?')
        } else {
            c as char
        }
    }
    if !b.starts_with(b"{\\rtf") {
        return (String::from_utf8_lossy(b).trim_end_matches('\0').to_string(), false);
    }
    let mut out = String::new();
    let mut stack: Vec<(bool, i64, bool)> = Vec::new();
    let (mut skip, mut uc, mut bold) = (false, 1i64, false);
    let mut pending_skip = 0i64;
    // visible characters written, and how many of them bold
    let (mut seen, mut shown, mut bolds, mut was_bold) = (0usize, 0usize, 0usize, false);
    let mut count = |out: &str, seen: usize, was_bold: bool| {
        let n = out[seen..].chars().filter(|c| !c.is_whitespace()).count();
        shown += n;
        if was_bold {
            bolds += n;
        }
    };
    let mut i = 0;
    while i < b.len() {
        count(&out, seen, was_bold);
        (seen, was_bold) = (out.len(), bold);
        let c = b[i];
        match c {
            b'{' => {
                stack.push((skip, uc, bold));
                i += 1;
            }
            b'}' => {
                if let Some((s, u, w)) = stack.pop() {
                    skip = s;
                    uc = u;
                    bold = w;
                }
                i += 1;
            }
            b'\\' => {
                i += 1;
                let Some(&n) = b.get(i) else { break };
                if n.is_ascii_alphabetic() {
                    let s = i;
                    while i < b.len() && b[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word = &b[s..i];
                    let ps = i;
                    if i < b.len() && (b[i] == b'-' || b[i].is_ascii_digit()) {
                        i += 1;
                        while i < b.len() && b[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                    let param: Option<i64> = std::str::from_utf8(&b[ps..i]).ok().and_then(|t| t.parse().ok());
                    if i < b.len() && b[i] == b' ' {
                        i += 1;
                    }
                    match word {
                        b"par" | b"line" if !skip => out.push('\n'),
                        b"tab" if !skip => out.push('\t'),
                        b"endash" if !skip => out.push('\u{2013}'),
                        b"emdash" if !skip => out.push('\u{2014}'),
                        b"bullet" if !skip => out.push('\u{2022}'),
                        b"lquote" if !skip => out.push('\u{2018}'),
                        b"rquote" if !skip => out.push('\u{2019}'),
                        b"ldblquote" if !skip => out.push('\u{201c}'),
                        b"rdblquote" if !skip => out.push('\u{201d}'),
                        b"b" => bold = param != Some(0),
                        b"plain" => bold = false,
                        b"uc" => uc = param.unwrap_or(1),
                        b"u" => {
                            if !skip {
                                let v = param.unwrap_or(63);
                                let v = if v < 0 { v + 65536 } else { v };
                                out.push(char::from_u32(v as u32).unwrap_or('?'));
                            }
                            pending_skip = uc;
                            continue;
                        }
                        b"fonttbl" | b"colortbl" | b"stylesheet" | b"info" | b"pict" | b"object" | b"header" | b"footer" | b"listtable" | b"listoverridetable" | b"generator" => skip = true,
                        _ => {}
                    }
                } else {
                    i += 1;
                    match n {
                        b'\'' => {
                            let h = b.get(i..i + 2).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                            i += 2;
                            if pending_skip > 0 {
                                pending_skip -= 1;
                            } else if let (Some(h), false) = (h, skip) {
                                out.push(cp1252(h));
                            }
                        }
                        b'*' => skip = true,
                        b'~' if !skip => out.push('\u{a0}'),
                        b'\n' | b'\r' if !skip => out.push('\n'),
                        b'\\' | b'{' | b'}' if !skip => out.push(n as char),
                        _ => {}
                    }
                }
            }
            b'\r' | b'\n' | 0 => i += 1,
            _ => {
                i += 1;
                if pending_skip > 0 {
                    pending_skip -= 1;
                } else if !skip {
                    out.push(cp1252(c));
                }
            }
        }
    }
    count(&out, seen, was_bold);
    (out.trim_end_matches(['\n', ' ', '\0']).to_string(), shown > 0 && bolds * 2 > shown)
}

/// Length of a text leader's arrow, and margin of a text frame (feet on paper).
const ARROW: f64 = 0.0098;
/// Keynote number shown by keynote tags (`KEYNOTE_NUMBER`).
const KEYNOTE_NUMBER: i64 = -1140421;
/// Categories drawn in their MEP system's colour: ducts, pipes, flex ducts and pipes, their
/// fittings and accessories (not equipment or terminals).
pub const SYSTEM_COLORED: [i64; 8] = [-2008000, -2008010, -2008016, -2008020, -2008044, -2008049, -2008050, -2008055];
/// System abbreviation and size of MEP elements, computed by Revit for tags.
const SYSTEM_ABBREVIATION: i64 = -1150468;
const MEP_SIZE: i64 = -1114240;
const PIPE_DIAMETER: i64 = -1140225;
/// Level elevation parameter, shown by level heads.
const LEVEL_ELEV: i64 = -1007102;
const TEXT_MARGIN: f64 = 0.0066;

/// `m_height` of a text is its box: lines x size x this (measured on the 3, 5 and 8 mm types of
/// the public sample: 1.59, 1.57, 1.55).
const LINE_BOX: f64 = 1.57;

/// A text as stored (`TextNoteData`, `TagNoteData`, `TextNote`), in its own coordinates (feet).
#[derive(Clone, Debug)]
struct RawText {
    orig: V3,
    base: V3,
    height: f64,
    flags: i64,
    /// Text type (`TextNoteAttributes`, `TagNoteAttributes`), and bold set in the text itself.
    attr: i64,
    bold: bool,
    /// Width its lines wrap at (`m_lineWidth`, paper feet; 0 when `m_bNoWrapping`).
    wrap: f64,
    /// Drawn width (`m_trueWidth`, paper feet).
    width: f64,
    /// Leaders: anchor at the text, elbow, end at the arrow.
    leaders: Vec<[V3; 3]>,
    /// Plain text, or for a label the parameters it shows: (param id, prefix, suffix, sample).
    text: String,
    params: Vec<(i64, String, String, String)>,
}

fn raw_text(o: &Obj, tr: &Tree) -> Option<RawText> {
    let orig = v3(o.fs("m_orig"))?;
    let base = v3(o.fs("m_baseVec")).unwrap_or([1.0, 0.0, 0.0]);
    let height = num(o, "m_height").filter(|h| *h > 0.0 && *h < 10.0)?;
    let flags = o.int("m_flags").unwrap_or(0);
    let (text, bold) = o
        .ptr("m_oRtfData")
        .or_else(|| o.ptr("m_oSavedRtfData"))
        .and_then(|i| tr.objs.get(i))
        .and_then(|r| match r.get("m_arrRtfStream") {
            Some(G::Is(v)) => Some(rtf_parse(&v.iter().map(|&x| x as u8).collect::<Vec<_>>())),
            _ => None,
        })
        .unwrap_or_default();
    let params = o
        .list("m_paramDataFormattingArr")
        .iter()
        .map(|p| (p.int("m_paramId").unwrap_or(-1), text_of(p, "m_prefix").unwrap_or_default(), text_of(p, "m_suffix").unwrap_or_default(), text_of(p, "m_sampleText").unwrap_or_default()))
        .collect();
    let wrap = if o.int("m_bNoWrapping").unwrap_or(0) != 0 { 0.0 } else { num(o, "m_lineWidth").filter(|w| w.is_finite() && *w > 0.0).unwrap_or(0.0) };
    let xyz = |l: &Obj, k: &str| v3(l.fs(k)).or_else(|| l.obj(k).and_then(|x| v3(x.fs("m_xyz"))));
    let leaders = o.ptrs("m_leaders").iter().filter_map(|&i| tr.objs.get(i)).filter_map(|l| Some([xyz(l, "m_anchor")?, xyz(l, "m_elbow")?, xyz(l, "m_end")?])).collect();
    let width = num(o, "m_trueWidth").filter(|w| w.is_finite() && *w > 0.0).unwrap_or(0.0);
    Some(RawText { orig, base, height, flags, attr: o.int("m_attrId").unwrap_or(-1), bold, wrap, width, leaders, text, params })
}

/// A schedule's stored table: title / header sections (rows heights, column widths, cells).
#[derive(Clone, Debug, Default)]
struct Table {
    width: f64,
    /// Sections shown, top to bottom: (title section, column widths, row heights, cells
    /// (row, col, text)).
    sections: Vec<(bool, Vec<f64>, Vec<f64>, Vec<(usize, usize, String)>)>,
    /// Title shown when its cell is empty (the schedule's name).
    name: String,
    /// Rows stacked downwards from the position (`m_bBuildScheduleTopDown`), else upwards.
    top_down: bool,
    /// Fixed height below the title (`m_isFixedHeight`, `m_segZeroHeight`): Revit fills it
    /// with empty body rows.
    fixed: Option<f64>,
    /// Least row height drawn below the title (feet), from the body text size.
    pitch: f64,
}

/// Row height Revit draws for a schedule text of `pt` points: 1.92 x the size (measured on a
/// 9 pt title block revision schedule: 0.240 in rows, stored rows of 0.167 in drawn as 0.240).
fn row_pitch(pt: f64) -> f64 {
    pt / 72.0 / 12.0 * 1.92
}

/// A schedule drawn by a family symbol (title block revision schedule), as stored in its
/// `DBViewScheduleData`.
fn symbol_table(d: &Obj, tr: &Tree) -> Option<Table> {
    let data = d.ptr("m_oScheduleData").and_then(|i| tr.objs.get(i))?;
    let mut t = table(data, tr);
    let schema = d.ptr("m_oScheduleSchema").and_then(|i| tr.objs.get(i));
    t.name = text_of(d, "m_name").unwrap_or_default();
    t.top_down = schema.and_then(|s| s.int("m_bBuildScheduleTopDown")).unwrap_or(0) != 0;
    if d.int("m_isFixedHeight").unwrap_or(0) != 0 {
        t.fixed = num(d, "m_segZeroHeight").filter(|h| *h > 0.0 && *h < 10.0);
    }
    t.pitch = body_pitch(schema, tr);
    Some(t)
}

/// Row height for the body text of a schedule (its first field's style, points).
fn body_pitch(schema: Option<&Obj>, tr: &Tree) -> f64 {
    let pt = schema
        .and_then(|s| s.ptrs("m_pFields").first().copied())
        .and_then(|i| tr.objs.get(i))
        .and_then(|f| f.ptr("m_oStyle"))
        .and_then(|i| tr.objs.get(i))
        .and_then(|s| num(s, "m_textSize"))
        .filter(|p| *p > 1.0 && *p < 72.0)
        .unwrap_or(9.0);
    row_pitch(pt)
}

fn table(data: &Obj, tr: &Tree) -> Table {
    let mut t = Table { width: num(data, "m_WidthInFeet").unwrap_or(0.0), ..Default::default() };
    for sec in data.ptrs("m_SectionData").iter().filter_map(|&i| tr.objs.get(i)) {
        if sec.int("m_hideSection").unwrap_or(0) != 0 {
            continue;
        }
        let cols: Vec<f64> = sec.ptrs("m_cols").iter().filter_map(|&i| tr.objs.get(i)).map(|c| num(c, "m_dWidth").unwrap_or(0.0)).collect();
        let rows: Vec<f64> = sec.ptrs("m_rows").iter().filter_map(|&i| tr.objs.get(i)).map(|r| num(r, "m_dHeight").unwrap_or(0.0)).collect();
        let mut cells = Vec::new();
        for rc in sec.ptrs("m_tableCells").iter().filter_map(|&i| tr.objs.get(i)) {
            for c in rc.ptrs("m_arrCells").iter().filter_map(|&i| tr.objs.get(i)) {
                let txt = text_of(c, "m_text").unwrap_or_default();
                if !txt.is_empty() {
                    cells.push((c.int("m_row").unwrap_or(0).max(0) as usize, c.int("m_column").unwrap_or(0).max(0) as usize, txt));
                }
            }
        }
        if !rows.is_empty() {
            t.sections.push((sec.int("m_index").unwrap_or(0) == 0, cols, rows, cells));
        }
    }
    t
}

#[derive(Default, Debug)]
struct Sym {
    /// Its family (`m_familyId`): the images of the family document belong to it.
    family: i64,
    curves: Vec<Vec<V3>>,
    texts: Vec<RawText>,
    tags: Vec<RawText>,
    /// Schedules drawn by the symbol (revision schedule of a title block): (position, table).
    tables: Vec<(V3, Table)>,
    outline: Option<[f64; 4]>,
    /// Its text parameters.
    strs: HashMap<i64, String>,
    /// Its yes/no and integer family parameters.
    ints: HashMap<i64, i64>,
}

#[derive(Debug, Clone)]
struct Crop {
    orig: V3,
    bx: V3,
    by: V3,
    bz: V3,
    off: [f64; 6],
    active: [bool; 6],
    /// Annotation crop, when switched on (`Viewer.m_bAnnotationCropActive`): offsets past the
    /// crop on paper (feet), left, right, top, bottom (`m_leftRightTopBottomOffsets`).
    annot: Option<[f64; 4]>,
}

#[derive(Default, Debug, Clone)]
struct ViewInfo {
    name: String,
    class: String,
    horz: Option<V3>,
    vert: Option<V3>,
    dir: Option<V3>,
    image: i64,
    /// Plan cut plane height above the level (`PlanViewRange.m_offsets[0]`).
    cut: Option<f64>,
    /// Plan view range as stored, absolute elevations (feet; ±infinity = unlimited, None =
    /// not stored): cut plane, top, bottom, view depth (`m_cutPlaneElev`, `m_topClipElev`,
    /// `m_bottomClipElev`, `m_levelBelowElev`).
    range_abs: [Option<f64>; 4],
    /// Plan view range relative to the view's level (`PlanViewRange2` offsets: cut, top,
    /// bottom, depth) and where each is measured from (`PlanViewRange.m_levelPos`: 0 the
    /// view's level, 1 the level above, -1 unlimited).
    range_rel: Option<([f64; 4], [i64; 4])>,
    /// Elements hidden in this view (`HiddenElementsViewSettings.m_hiddenElements`) and the
    /// categories switched off in its visibility settings
    /// (`IckyExcludedCategoriesSetPtrWrapper.m_categoryIds`).
    hidden: std::collections::HashSet<i64>,
    hidden_cats: std::collections::HashSet<i64>,
    /// Workset visibility set in this view (`WorksetVisibilityViewSettings`: workset GUID →
    /// hidden), and the worksets it hides once resolved against the global setting.
    ws_set: HashMap<[u8; 16], bool>,
    hidden_ws: std::collections::HashSet<u32>,
    /// Paper size of a model foot (1 / `m_scale`).
    scale: f64,
    /// The view it depends on (`m_primaryViewId`), whose annotations it shows.
    primary: i64,
    /// Plan regions it shows (`DBViewPlanViewDepthCutter.m_planRegionFaceTagMap`).
    regions: Vec<i64>,
    /// Its view template (`m_viewTemplateId`), which then sets its view range.
    template: i64,
    /// The view's level (`m_genElemId`) and its viewer element (`m_viewerId`).
    level: i64,
    viewer: i64,
    strs: HashMap<i64, String>,
}

/// A grid line (`Grid`): its ends in plan (feet), the heights its plane spans, its type.
#[derive(Debug)]
struct Grid {
    name: String,
    a: V3,
    b: V3,
    z: (f64, f64),
    attr: i64,
}

#[derive(Debug)]
struct Vp {
    owner: i64,
    offset: V3,
    label: V3,
    label_len: f64,
    scale: f64,
    view: i64,
    number: String,
}

/// A `BoundedSpace` (crop region, section box): origin, basis rows, the six offsets and
/// which of them are active.
fn bounded(b: &Obj) -> Option<Crop> {
    let (orig, m, off) = (v3(b.fs("m_orig"))?, b.fs("m_basis")?, b.fs("m_boundOffset")?);
    if m.len() != 9 || off.len() != 6 || !m.iter().chain(off.iter()).all(|x| x.is_finite()) {
        return None;
    }
    let act: Vec<i64> = match b.get("m_boundActive") {
        Some(G::Is(v)) => v.clone(),
        _ => vec![0; 6],
    };
    Some(Crop {
        orig,
        bx: [m[0], m[1], m[2]],
        by: [m[3], m[4], m[5]],
        bz: [m[6], m[7], m[8]],
        off: off.try_into().ok()?,
        active: std::array::from_fn(|k| act.get(k).copied().unwrap_or(0) != 0),
        annot: None,
    })
}

/// What a view keeps of the model, in feet: an oriented box (origin, axes u, v, depth;
/// bounds along each axis, ±infinity when unlimited).
#[derive(Debug, Clone, Copy)]
struct Clip {
    orig: V3,
    ax: [V3; 3],
    lo: V3,
    hi: V3,
}

impl Clip {
    fn coords(&self, p: V3) -> V3 {
        let d = sub(p, self.orig);
        [dot(d, self.ax[0]), dot(d, self.ax[1]), dot(d, self.ax[2])]
    }
    /// The part of segment a-b (clip coordinates) inside the box.
    fn segment(&self, a: V3, b: V3) -> Option<(V3, V3)> {
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        for k in 0..3 {
            let d = b[k] - a[k];
            for (p, q) in [(-d, a[k] - self.lo[k]), (d, self.hi[k] - a[k])] {
                if !q.is_finite() {
                    continue; // unlimited side
                }
                if p.abs() < 1e-15 {
                    if q < 0.0 {
                        return None;
                    }
                    continue;
                }
                let t = q / p;
                if p < 0.0 {
                    t0 = t0.max(t);
                } else {
                    t1 = t1.min(t);
                }
                if t0 > t1 {
                    return None;
                }
            }
        }
        let at = |t: f64| std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t);
        Some((at(t0), at(t1)))
    }
    /// Range of the box lo..hi (feet, model axes) along each clip axis.
    fn box_range(&self, lo: V3, hi: V3) -> (V3, V3) {
        let (mut a, mut b) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for k in 0..8 {
            let c = self.coords([if k & 1 == 0 { lo[0] } else { hi[0] }, if k & 2 == 0 { lo[1] } else { hi[1] }, if k & 4 == 0 { lo[2] } else { hi[2] }]);
            for i in 0..3 {
                a[i] = a[i].min(c[i]);
                b[i] = b[i].max(c[i]);
            }
        }
        (a, b)
    }
    fn misses(&self, lo: V3, hi: V3) -> bool {
        let (a, b) = self.box_range(lo, hi);
        (0..3).any(|k| a[k] > self.hi[k] || b[k] < self.lo[k])
    }
}

/// Collects the sheet content, partition by partition (see [`Sheets::take`]).
#[derive(Default)]
pub struct Sheets {
    symbols: HashMap<i64, Sym>,
    /// (owner view, symbol, transform)
    instances: Vec<(i64, i64, Trf)>,
    /// Text parameters of the placed 2D families, by index in `instances`.
    inst_strs: Vec<HashMap<i64, String>>,
    /// Yes/no and integer family parameters of the placed 2D families, same index.
    inst_ints: Vec<HashMap<i64, i64>>,
    /// Element ids of the placed 2D families, same index.
    inst_ids: Vec<i64>,
    /// Flips of the placed 2D families (`m_flippedX`, `m_flippedY`), same index.
    inst_flips: Vec<(bool, bool)>,
    /// Levels as each view shows them: (view, level, free end, bubble end along the view's
    /// horizontal; `ExtentElem`, `DatumPlaneRec`).
    datums: Vec<(i64, i64, f64, f64)>,
    level_names: HashMap<i64, String>,
    grids: Vec<Grid>,
    /// element -> its MEP system, system -> its type, type -> its line colour
    elem_systems: HashMap<i64, i64>,
    system_types: HashMap<i64, i64>,
    system_colors: HashMap<i64, [u8; 3]>,
    /// system type -> its abbreviation; MEP curve -> its size as written in tags
    system_abbr: HashMap<i64, String>,
    mep_sizes: HashMap<i64, String>,
    /// MEP curve -> its direction (model); tag families turned with what they tag
    mep_dirs: HashMap<i64, V3>,
    /// MEP curve -> its ends (model feet), its diameter (mm, as tags write it)
    mep_lines: HashMap<i64, (V3, V3)>,
    mep_diams: HashMap<i64, String>,
    turning_tags: std::collections::HashSet<i64>,
    /// Plan regions: (owner view, id, (outline, bottom, top)), absolute feet.
    plan_regions: Vec<(i64, i64, (Vec<P2>, f64, f64))>,
    /// level -> corners of its plane (`m_Envelope`: its extent, feet) and its type
    level_env: HashMap<i64, [f64; 4]>,
    level_types: HashMap<i64, i64>,
    /// level type -> symbol of its head (`m_familyTagId`)
    level_heads: HashMap<i64, i64>,
    /// The model's box (feet).
    model_box: (V3, V3),
    /// level -> its default ends (`m_freeEnd`, `m_bubbleEnd`, model feet)
    level_ends: HashMap<i64, (V3, V3)>,
    texts: Vec<(i64, RawText)>,
    viewports: Vec<Vp>,
    /// (owner view, origin, schedule view id)
    sched_inst: Vec<(i64, V3, i64)>,
    tables: HashMap<i64, Table>,
    /// view id → crop
    crops: HashMap<i64, Crop>,
    views: HashMap<i64, ViewInfo>,
    /// image symbol id → (holder id, width, height feet)
    image_syms: HashMap<i64, (i64, f64, f64)>,
    /// image instance id → symbol id
    image_inst: HashMap<i64, i64>,
    /// image symbol id → its family (images loaded in a family)
    image_fam: HashMap<i64, i64>,
    /// images placed in family documents: (image symbol, centre, base direction, width,
    /// height), family coordinates (feet)
    fam_images: Vec<(i64, V3, V3, f64, f64)>,
    /// text type → its category, giving its colour (`m_lineAndTextAttr.m_categoryId`)
    text_styles: HashMap<i64, i64>,
    /// text type -> its font, font -> its size (feet on paper; `FontElem`)
    text_fonts: HashMap<i64, i64>,
    font_sizes: HashMap<i64, f64>,
    /// text types with a frame (`m_textBoxVisibility`)
    boxed_types: std::collections::HashSet<i64>,
    /// text types drawn bold (`m_lineAndTextAttr.m_bBold`)
    bold_types: std::collections::HashSet<i64>,
    holders: HashMap<i64, Vec<u8>>,
    holder_bytes: usize,
    project: HashMap<i64, String>,
    levels: Vec<f64>,
    /// level id → elevation (feet)
    level_z: HashMap<i64, f64>,
    /// 3D section boxes (`ModelClipBox` switched on), by viewer element id
    section_boxes: HashMap<i64, Crop>,
    /// view annotations (detail lines, filled regions, dimensions, tags)
    ann: crate::rvt_annot::Annots,
    /// Worksets hidden in the views that do not set them (`WorksetVisibilitySettingsElem`).
    global_hidden_ws: std::collections::HashSet<[u8; 16]>,
    /// element id → its workset (partition id)
    elem_ws: HashMap<i64, u32>,
    /// `Family` element -> root of its family document, and what those documents draw.
    fam_roots: HashMap<i64, i64>,
    fam_drawn: std::sync::Mutex<HashMap<(i64, Vec<(i64, bool)>), std::sync::Arc<crate::rvt_annot::Drawn>>>,
    /// Images decoded once (a title block logo repeats on every sheet).
    decoded: std::sync::Mutex<HashMap<i64, Option<std::sync::Arc<Rgba>>>>,
}

const CLASSES: [&str; 22] = [
    "Family",
    "FontElem",
    "PlanRegion",
    "Grid",
    "GridAttributes",
    "LevelAttributes",
    "ExtentElem",
    "FamilySymbol",
    "FamilyInstance",
    "Viewport",
    "TextNote",
    "ScheduleInstance",
    "Viewer",
    "ImageSymbol",
    "ImageInstance",
    "ImageHolder",
    "ProjectInfo",
    "Level",
    "ModelClipBox",
    "TextNoteAttributes",
    "TagNoteAttributes",
    "WorksetVisibilitySettingsElem",
];
/// Most bytes of saved images kept.
const MAX_IMAGES: usize = 96 << 20;

fn strings(tr: &Tree) -> HashMap<i64, String> {
    let Some(set) = tr.root.ptr("m_pParamValueSetAString").and_then(|i| tr.objs.get(i)) else { return HashMap::new() };
    set.list("m_paramSet")
        .iter()
        .filter_map(|p| {
            let id = p.int("m_paramId")?;
            let v = match p.get("m_value")? {
                G::S(s) => s.clone(),
                G::O(o) => o.fields.iter().find_map(|(_, g)| if let G::S(s) = g { Some(s.clone()) } else { None })?,
                _ => return None,
            };
            Some((id, v))
        })
        .collect()
}

impl Sheets {
    pub fn wants(s: &Schema) -> Vec<bool> {
        let mut want = vec![false; 65536];
        for c in CLASSES.iter().chain(crate::rvt_annot::CLASSES.iter()) {
            if let Some(t) = s.tag_of(c) {
                want[t as usize] = true;
            }
        }
        for &t in s.classes.keys() {
            if s.isa(t, "DBView") || s.isa(t, "Viewer") || s.isa(t, "RbsCurve") || s.isa(t, "RbsSystem") || s.isa(t, "RbsSystemType") {
                want[t as usize] = true;
            }
        }
        want
    }

    pub fn take(&mut self, s: &Schema, t: u16, tr: &Tree) {
        let r = &tr.root;
        // MEP systems: the element's system, the system's type, the type's colour
        if s.isa(t, "RbsSystemType") {
            if let Some(G::S(a)) = r.get("m_abbreviation") {
                self.system_abbr.insert(tr.id, a.clone());
            }
            if let Some(o) = r.ptr("m_oSystemOverrides").and_then(|i| tr.objs.get(i)) {
                let gs = o.ptr("m_oOverrideGraphicSettings").and_then(|i| tr.objs.get(i));
                let c = gs.and_then(|g| g.ptr("m_oProjPenColor")).and_then(|i| tr.objs.get(i)).and_then(|c| c.int("m_color")).filter(|c| (0..1 << 24).contains(c));
                if let Some(c) = c {
                    self.system_colors.insert(tr.id, [(c & 255) as u8, ((c >> 8) & 255) as u8, ((c >> 16) & 255) as u8]);
                }
            }
            return;
        }
        if s.isa(t, "RbsSystem") {
            if let Some(ty) = r.int("m_typeId") {
                self.system_types.insert(tr.id, ty);
            }
            return;
        }
        if r.int("m_famId").unwrap_or(-1) == -1 {
            let sys = tr.objs.iter().find_map(|o| o.int("m_systemId").or_else(|| o.int("m_idSystem")).filter(|v| *v > 0));
            if let Some(sys) = sys {
                self.elem_systems.insert(tr.id, sys);
            }
        }
        if s.isa(t, "RbsCurve") {
            if let Some(c) = r.ptr("m_pCurveDriver").and_then(|i| tr.objs.get(i)).and_then(|d| d.ptr("m_pCrv")).and_then(|i| tr.objs.get(i)) {
                if let (Some(o), Some(d)) = (v3(c.fs("m_origin")), v3(c.fs("m_dirVec"))) {
                    self.mep_dirs.insert(tr.id, d);
                    if let Some([t0, t1, ..]) = c.fs("m_endParams") {
                        let at = |t: f64| [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t];
                        self.mep_lines.insert(tr.id, (at(*t0), at(*t1)));
                    }
                }
            }
            if let Some(d) = num(r, "m_dWidthOrDiameter").filter(|v| *v > 0.0) {
                self.mep_diams.insert(tr.id, format!("{}", (d * FEET * 1000.0).round() as i64));
            }
            // size as Revit writes it (mm): "715x610", "Ø355"
            let mm = |k: &str| num(r, k).filter(|v| *v > 0.0).map(|v| (v * FEET * 1000.0).round() as i64);
            let duct = s.name(t).contains("Duct");
            if let Some(w) = mm("m_dWidthOrDiameter") {
                let size = match mm("m_dHeight") {
                    Some(h) if duct && h != w => format!("{w}x{h}"),
                    _ => format!("Ø{w}"),
                };
                self.mep_sizes.insert(tr.id, size);
            }
            return;
        }
        let owner = r.int("m_ownerDBViewId").unwrap_or(-1);
        let cls = s.name(t);
        self.ann.take(s, t, tr, || strings(tr));
        match cls {
            "FamilySymbol" => {
                let curves: Vec<Vec<V3>> = r.ptrs("m_nonBRepGeomPtrs").iter().filter_map(|&i| tr.objs.get(i)).map(|c| curve_points(c, s)).filter(|p| p.len() >= 2).collect();
                let flat = |v: &[V3]| v.iter().all(|p| p[2].abs() < 1e-6);
                let outline = r.obj("m_outline").and_then(|o| o.fs("m_minmax")).filter(|m| m.len() == 6 && m.iter().all(|x| x.is_finite()) && (m[2] - m[5]).abs() < 1e-6).map(|m| [m[0], m[1], m[3], m[4]]);
                // 2D families only (title blocks, annotation symbols)
                if outline.is_none() || !curves.iter().all(|c| flat(c)) {
                    return;
                }
                let texts: Vec<RawText> = r.ptrs("m_textNoteData").iter().filter_map(|&i| tr.objs.get(i)).filter_map(|o| raw_text(o, tr)).collect();
                let tags: Vec<RawText> = r.ptrs("m_tagNoteData").iter().filter_map(|&i| tr.objs.get(i)).filter_map(|o| raw_text(o, tr)).collect();
                let mut tables = Vec::new();
                if let Some(all) = tr.objs.iter().find(|o| s.name(o.tag) == "AllScheduleInstanceRegenData") {
                    let datas: Vec<&Obj> = all.ptrs("m_dBViewScheduleDataArr").iter().filter_map(|&i| tr.objs.get(i)).collect();
                    for si in all.ptrs("m_scheduleInstanceDataArr").iter().filter_map(|&i| tr.objs.get(i)) {
                        let Some(pos) = si.obj("m_position").and_then(|p| v3(p.fs("m_xyz"))) else { continue };
                        let Some(d) = si.int("m_dBViewScheduleDataIndex").and_then(|k| datas.get(k as usize)) else { continue };
                        if let Some(t) = symbol_table(d, tr) {
                            tables.push((pos, t));
                        }
                    }
                }
                self.symbols.insert(tr.id, Sym { family: r.int("m_familyId").unwrap_or(-1), curves, texts, tags, tables, outline, strs: family_strs(tr, s), ints: family_ints(tr, s) });
            }
            "FamilyInstance" if owner > 0 => {
                let Some(ii) = r.ptr("m_pInstanceInfo").and_then(|i| tr.objs.get(i)) else { return };
                let Some(t) = ii.obj("m_Trf").and_then(Trf::from) else { return };
                self.instances.push((owner, ii.int("m_symbolId").unwrap_or(-1), t));
                self.inst_strs.push(family_strs(tr, s));
                self.inst_ints.push(family_ints(tr, s));
                self.inst_ids.push(tr.id);
                self.inst_flips.push((r.int("m_flippedX").unwrap_or(0) != 0, r.int("m_flippedY").unwrap_or(0) != 0));
            }
            "TextNote" if owner > 0 => {
                if let Some(t) = raw_text(r, tr) {
                    self.texts.push((owner, t));
                }
            }
            "Viewport" if owner > 0 => {
                let (Some(offset), Some(view)) = (v3(r.fs("m_offset")), r.int("m_dbViewId")) else { return };
                self.viewports.push(Vp {
                    owner,
                    offset,
                    label: v3(r.fs("m_labelOrigin")).unwrap_or(offset),
                    label_len: num(r, "m_labelLength").unwrap_or(0.0),
                    scale: num(r, "m_scale").unwrap_or(1.0),
                    view,
                    number: text_of(r, "m_detailNumber").unwrap_or_default(),
                });
            }
            "ScheduleInstance" if owner > 0 => {
                let Some(o) = r.obj("origin_XYZ").and_then(|o| v3(o.fs("m_xyz"))).or_else(|| v3(r.fs("m_origin"))) else { return };
                self.sched_inst.push((owner, o, r.int("m_scheduleId").unwrap_or(-1)));
            }
            _ if cls == "Viewer" || s.isa(t, "Viewer") => {
                // Viewer and its kinds (Section, InteriorElevArrow…)
                let (Some(c), Some(view)) = (r.obj("m_boundedSpace").and_then(bounded), r.int("m_dbViewId")) else {
                    return;
                };
                let mut c = c;
                // annotation crop (paper feet: left, right, top, bottom past the crop)
                if r.int("m_bAnnotationCropActive").unwrap_or(0) != 0 {
                    c.annot = r.ptr("m_pAnnotationCropOffsets").and_then(|i| tr.objs.get(i)).and_then(|o| o.fs("m_leftRightTopBottomOffsets")).and_then(|f| f.try_into().ok());
                }
                self.crops.insert(view, c);
            }
            "ModelClipBox" => {
                // 3D view section box: only when switched on
                let Some(b) = r.obj("m_boundedSpace").filter(|b| b.int("m_isOn").unwrap_or(0) != 0) else {
                    return;
                };
                if let (Some(c), Some(viewer)) = (bounded(b), r.int("m_viewer3dId")) {
                    self.section_boxes.insert(viewer, c);
                }
            }
            "ImageSymbol" => {
                if let Some(h) = r.int("m_idImageHolder") {
                    let (w, hh) = (num(r, "m_widthInFeet").unwrap_or(0.0), num(r, "m_heightInFeet").unwrap_or(0.0));
                    self.image_syms.insert(tr.id, (h, w, hh));
                }
                if let Some(f) = r.int("m_famId").filter(|f| *f > 0) {
                    self.image_fam.insert(tr.id, f);
                }
            }
            "ImageInstance" => {
                let Some(info) = r.ptr("m_pImageInfo").and_then(|i| tr.objs.get(i)) else { return };
                let Some(sym) = info.int("m_symbolId") else { return };
                self.image_inst.insert(tr.id, sym);
                // placed in a family document (logo of a title block): centre and size
                if r.int("m_famId").unwrap_or(-1) > 0 {
                    let (Some(w), Some(h), Some(t)) = (num(info, "m_dWidth"), num(info, "m_dHeight"), info.obj("m_Trf").and_then(Trf::from)) else { return };
                    let base = t.lin(v3(info.fs("m_baseVec")).unwrap_or([1.0, 0.0, 0.0]));
                    self.fam_images.push((sym, t.apply([0.0, 0.0, 0.0]), base, w, h));
                }
            }
            "TextNoteAttributes" | "TagNoteAttributes" => {
                if r.obj("m_lineAndTextAttr").and_then(|a| a.int("m_bBold")).unwrap_or(0) != 0 {
                    self.bold_types.insert(tr.id);
                }
                if let Some(c) = r.obj("m_lineAndTextAttr").and_then(|a| a.int("m_categoryId")) {
                    self.text_styles.insert(tr.id, c);
                }
                // its font, which holds the text size
                if let Some(f) = r.obj("m_lineAndTextAttr").and_then(|a| a.int("m_fontId")) {
                    self.text_fonts.insert(tr.id, f);
                }
                if r.int("m_textBoxVisibility").unwrap_or(0) != 0 {
                    self.boxed_types.insert(tr.id);
                }
            }
            "ImageHolder" => {
                if let Some(G::Is(v)) = r.ptr("m_pRasterImage").and_then(|i| tr.objs.get(i)).and_then(|o| o.get("m_compressedImage")) {
                    if self.holder_bytes + v.len() <= MAX_IMAGES {
                        self.holder_bytes += v.len();
                        self.holders.insert(tr.id, v.iter().map(|&x| x as u8).collect());
                    }
                }
            }
            "FontElem" => {
                if let Some(h) = r.ptr("m_pFont").and_then(|i| tr.objs.get(i)).and_then(|f| num(f, "m_size")).filter(|h| *h > 1e-5 && *h < 1.0) {
                    self.font_sizes.insert(tr.id, h);
                }
            }
            "ProjectInfo" => self.project.extend(strings(tr)),
            "Grid" if r.int("m_famId").unwrap_or(-1) == -1 => {
                let Some(p) = r.ptr("m_pSurface").and_then(|i| tr.objs.get(i)) else { return };
                let (Some(o), Some(x), Some(c)) = (v3(p.fs("m_origin")), v3(p.fs("m_xVec")), p.obj("m_Envelope").and_then(|e| e.fs("m_corners")).filter(|c| c.len() == 4)) else { return };
                // vertical planes only: their line in plan
                if x[2].abs() > 1e-6 {
                    return;
                }
                let at = |u: f64| [o[0] + x[0] * u, o[1] + x[1] * u, o[2]];
                self.grids.push(Grid { name: text_of(r, "m_text").unwrap_or_default(), a: at(c[0]), b: at(c[2]), z: (o[2] + c[1], o[2] + c[3]), attr: r.int("m_attrId").unwrap_or(-1) });
            }
            "PlanRegion" if r.int("m_famId").unwrap_or(-1) == -1 => {
                let elev = |k: &str| num(r, k).filter(|v| v.abs() < 1e8);
                let (Some(cut), Some(lo)) = (elev("m_cutPlaneElev"), elev("m_viewDepthPlaneElev").or(elev("m_bottomClipPlaneElev"))) else { return };
                let lo = elev("m_bottomClipPlaneElev").map_or(lo, |b| b.min(lo));
                let Some(lp) = r.ptrs("m_bndyCurvesLoops").first().and_then(|&i| tr.objs.get(i)) else { return };
                let pts: Vec<P2> = lp.ptrs("m_curves").iter().filter_map(|&i| tr.objs.get(i)).flat_map(|c| curve_points(c, s)).map(|p| [p[0], p[1]]).collect();
                if pts.len() >= 3 {
                    self.plan_regions.push((r.int("m_ownerDBViewId").unwrap_or(-1), tr.id, (pts, lo.min(cut), lo.max(cut))));
                }
            }
            "GridAttributes" => {
                if let Some(h) = r.int("m_familyTagId") {
                    self.level_heads.insert(tr.id, h);
                }
            }
            "LevelAttributes" => {
                if let Some(h) = r.int("m_familyTagId") {
                    self.level_heads.insert(tr.id, h);
                }
            }
            "Family" => {
                if r.int("m_rotateTagWithComponent").unwrap_or(0) != 0 {
                    self.turning_tags.insert(tr.id);
                }
            }
            "ExtentElem" if r.int("m_famId").unwrap_or(-1) == -1 => {
                let view = r.int("m_dbViewId").unwrap_or(-1);
                for e in r.list("m_idExtents") {
                    let rec = e.obj("second").and_then(|o| o.ptr("m_oExtentData")).and_then(|i| tr.objs.get(i)).filter(|o| s.name(o.tag) == "DatumPlaneRec");
                    if let (Some(id), Some(d)) = (e.int("first"), rec) {
                        if let (Some(a), Some(b)) = (num(d, "m_freeEnd"), num(d, "m_bubbleEnd")) {
                            self.datums.push((view, id, a, b));
                        }
                    }
                }
            }
            "WorksetVisibilitySettingsElem" => {
                if let Some(G::L(l)) = r.get("m_invisibleWorksets") {
                    self.global_hidden_ws.extend(l.iter().filter_map(guid_of));
                }
            }
            "Level" => {
                if let Some(z) = r.ptr("m_pSurface").and_then(|i| tr.objs.get(i)).and_then(|p| v3(p.fs("m_origin"))) {
                    self.levels.push(z[2]);
                    self.level_z.insert(tr.id, z[2]);
                    self.level_names.insert(tr.id, text_of(r, "m_text").unwrap_or_default());
                    if let Some(a) = r.int("m_attrId") {
                        self.level_types.insert(tr.id, a);
                    }
                    if let Some(c) = r.ptr("m_pSurface").and_then(|i| tr.objs.get(i)).and_then(|p| p.obj("m_Envelope")).and_then(|e| e.fs("m_corners")).filter(|c| c.len() == 4) {
                        self.level_env.insert(tr.id, [c[0], c[1], c[2], c[3]]);
                    }
                    if let (Some(a), Some(b), -1) = (v3(r.fs("m_freeEnd")), v3(r.fs("m_bubbleEnd")), r.int("m_famId").unwrap_or(-1)) {
                        self.level_ends.insert(tr.id, (a, b));
                    }
                }
            }
            _ if s.isa(t, "DBView") => {
                let pvr = tr.objs.iter().find(|o| s.name(o.tag) == "PlanViewRange");
                let cut = pvr.and_then(|o| o.fs("m_offsets")).and_then(|v| v.first().copied()).filter(|v| v.is_finite() && *v > 0.3); // 0: not the cut plane (seen on upper levels)
                                                                                                                                      // absolute elevations kept with the view; ±1e9 = unlimited
                let abs = |k: &str| match r.get(k) {
                    Some(G::F(v)) if v.is_finite() => Some(if *v <= -1e8 {
                        f64::NEG_INFINITY
                    } else if *v >= 1e8 {
                        f64::INFINITY
                    } else {
                        *v
                    }),
                    _ => None,
                };
                let range_abs = [abs("m_cutPlaneElev"), abs("m_topClipElev"), abs("m_bottomClipElev"), abs("m_levelBelowElev")];
                let rel = tr
                    .objs
                    .iter()
                    .find(|o| s.name(o.tag) == "PlanViewRange2")
                    .and_then(|o| o.fs("m_offsets").or_else(|| o.fs("m_viewDepthPlaneOffsets")))
                    .or_else(|| pvr.and_then(|o| o.fs("m_offsets")));
                let pos: Vec<i64> = match pvr.and_then(|o| o.get("m_levelPos")) {
                    Some(G::Is(v)) => v.clone(),
                    _ => vec![],
                };
                let range_rel = rel
                    .filter(|v| v.len() >= 4 && v[..4].iter().all(|x| x.is_finite()))
                    .map(|v| ([v[0], v[1], v[2], v[3]], std::array::from_fn(|k| pos.get(k).copied().unwrap_or(0))));
                self.views.insert(
                    tr.id,
                    ViewInfo {
                        name: text_of(r, "m_viewName").unwrap_or_default(),
                        class: cls.to_string(),
                        scale: num(r, "m_scale").filter(|k| *k >= 1.0).map_or(1.0, |k| 1.0 / k),
                        horz: v3(r.fs("m_horzDir")),
                        vert: v3(r.fs("m_vertDir")),
                        dir: v3(r.fs("m_viewDir")),
                        image: r.int("m_imageInstanceId").unwrap_or(-1),
                        cut,
                        range_abs,
                        range_rel,
                        level: r.int("m_genElemId").unwrap_or(-1),
                        template: r.int("m_viewTemplateId").unwrap_or(-1),
                        primary: r.int("m_primaryViewId").unwrap_or(-1),
                        regions: tr.objs.iter().filter(|o| s.name(o.tag) == "DBViewPlanViewDepthCutter").flat_map(|o| o.list("m_planRegionFaceTagMap").iter()).filter_map(|e| e.int("first")).filter(|i| *i > 0).collect(),
                        viewer: r.int("m_viewerId").unwrap_or(-1),
                        strs: strings(tr),
                        hidden: ids_of(tr, s, "HiddenElementsViewSettings", "m_hiddenElements"),
                        hidden_cats: ids_of(tr, s, "IckyExcludedCategoriesSetPtrWrapper", "m_categoryIds"),
                        ws_set: workset_settings(tr, s),
                        hidden_ws: Default::default(),
                    },
                );
                if let Some(data) = r.ptr("m_pData").and_then(|i| tr.objs.get(i)).filter(|o| s.name(o.tag) == "ScheduleData") {
                    // placed on the sheet by its top left corner
                    // drawn at the stored row heights (Revit keeps them, even below the text size)
                    self.tables.insert(tr.id, Table { top_down: true, ..table(data, tr) });
                }
            }
            _ => {}
        }
    }

    /// How a text is drawn: bold (by its type or in its own rich text), its colour and its
    /// type's size.
    fn look(&self, t: &RawText) -> (bool, [u8; 3], Option<f64>) {
        (t.bold || self.bold_types.contains(&t.attr), self.text_styles.get(&t.attr).map_or([0; 3], |c| self.ann.style_color(*c)), self.text_fonts.get(&t.attr).and_then(|f| self.font_sizes.get(f)).copied())
    }

    fn image(&self, sym: i64) -> Option<(std::sync::Arc<Rgba>, f64, f64)> {
        let &(h, w, hh) = self.image_syms.get(&sym)?;
        let mut cache = self.decoded.lock().unwrap_or_else(|e| e.into_inner());
        let img = cache
            .entry(h)
            .or_insert_with(|| {
                let img = image::load_from_memory(self.holders.get(&h)?).ok()?.to_rgba8();
                Some(std::sync::Arc::new(Rgba { width: img.width(), height: img.height(), pixels: img.into_raw() }))
            })
            .clone()?;
        Some((img, w, hh))
    }

    /// Schedule views drawn as their stored table (title, headers, typed-in cells; the rows
    /// Revit computes from the elements are not stored), with a margin around it.
    pub fn schedules(&self, ids: &[i64]) -> Vec<Sheet2d> {
        let mut out = Vec::new();
        for &id in ids {
            let Some(tb) = self.tables.get(&id).filter(|t| !t.sections.is_empty()) else { continue };
            let name = self.views.get(&id).map(|v| v.name.clone()).unwrap_or_default();
            let mut sh = Sheet2d { id, ..Default::default() };
            self.draw_table(&mut sh, &Table { name, ..tb.clone() }, [0.0, 0.0, 0.0]);
            let mut e = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
            for q in sh.lines.iter().flatten() {
                e = [e[0].min(q[0]), e[1].min(q[1]), e[2].max(q[0]), e[3].max(q[1])];
            }
            if !e[0].is_finite() {
                continue;
            }
            let m = ((e[2] - e[0]).max(e[3] - e[1]) * 0.04).max(0.005);
            sh.extent = [e[0] - m, e[1] - m, e[2] + m, e[3] + m];
            out.push(sh);
        }
        out
    }

    /// The views that show levels of their own (sections, elevations).
    pub fn has_datums(&self, view: i64) -> bool {
        self.views.get(&view).is_some_and(|v| self.shows_levels(v)) && !self.level_ends.is_empty()
    }

    /// Extent of a level stored for a view.
    fn level_span(&self, id: i64, level: i64) -> Option<(f64, f64)> {
        let d = self.datums.iter().find(|d| d.1 == level && d.0 == id && (d.2 - d.3).abs() > 1e-6)?;
        Some((d.2.min(d.3), d.2.max(d.3)))
    }

    /// Extent of a level along a view's horizontal: its plane's corners, else the model's width.
    fn model_span(&self, proj: &dyn Fn(V3) -> P2) -> (f64, f64) {
        let (lo, hi) = self.model_box;
        let far = (f64::INFINITY, f64::NEG_INFINITY);
        // the levels' planes share their extent, some rare one apart
        let mut seen: HashMap<[i64; 4], (usize, [f64; 4])> = HashMap::new();
        for c in self.level_env.iter().filter(|(id, _)| self.level_ends.contains_key(id)).map(|(_, c)| c) {
            seen.entry(c.map(|x| x.round() as i64)).or_insert((0, *c)).0 += 1;
        }
        match seen.values().max_by_key(|v| v.0).map(|v| v.1).as_ref() {
            Some(c) => [[c[0], c[1]], [c[2], c[1]], [c[0], c[3]], [c[2], c[3]]].iter().map(|p| proj([p[0], p[1], 0.0])[0]).fold(far, |a, u| (a.0.min(u), a.1.max(u))),
            None => (0..8).map(|k| proj(corner(lo, hi, k))[0]).fold(far, |a, u| (a.0.min(u), a.1.max(u))),
        }
    }

    /// The model's box (metres), for `model_span`.
    pub fn set_model_box(&mut self, lo: [f64; 3], hi: [f64; 3]) {
        self.model_box = (lo.map(|x| x / FEET), hi.map(|x| x / FEET));
    }

    /// Whether a view shows levels: it looks horizontally (sections, elevations) and their
    /// category is not switched off.
    fn shows_levels(&self, view: &ViewInfo) -> bool {
        const LEVELS: i64 = -2000240;
        view.dir.is_some_and(|d| d[2].abs() < 1e-6) && !view.class.contains("Drafting") && !view.class.contains("Legend") && !view.class.contains("Schedule") && !view.hidden_cats.contains(&LEVELS)
    }

    /// Drafting views and legends drawn alone: their lines, symbols and texts, at the view's
    /// scale, with a margin around them.
    pub fn drawings(&self, ids: &[i64]) -> Vec<Sheet2d> {
        let edges = Edges { items: Vec::new() };
        let mut out = Vec::new();
        for &id in ids {
            let Some(view) = self.views.get(&id) else { continue };
            let vp = Vp { owner: -1, offset: [0.0; 3], label: [0.0; 3], label_len: 0.0, scale: view.scale, view: id, number: String::new() };
            let mut sh = Sheet2d { id, ..Default::default() };
            if self.draw_view(&mut sh, view, &vp, &edges).is_none() {
                continue;
            }
            let mut e = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
            let line = sh.lines.iter().flatten().chain(sh.strokes.iter().flat_map(|s| s.pts.iter()));
            for q in line.chain(sh.fills.iter().flat_map(|f| f.tris.iter().flatten())).chain(sh.texts.iter().map(|t| &t.pos)) {
                e = [e[0].min(q[0]), e[1].min(q[1]), e[2].max(q[0]), e[3].max(q[1])];
            }
            if !e[0].is_finite() {
                continue;
            }
            let m = ((e[2] - e[0]).max(e[3] - e[1]) * 0.04).max(0.005);
            sh.extent = [e[0] - m, e[1] - m, e[2] + m, e[3] + m];
            out.push(sh);
        }
        out
    }

    /// The sheets' drawings. `model`: what the placed views show, element id → (box min,
    /// max, metres; rebuilt mesh if any).
    pub fn finish(&self, sheet_ids: &[(i64, String, String)], model: &[(i64, i64, [f64; 3], [f64; 3], Option<&Mesh>)]) -> Vec<Sheet2d> {
        // model edges only when a sheet shows a model view
        let need = self.viewports.iter().any(|v| v.scale > 1e-6 && v.scale < 0.99 && self.views.get(&v.view).is_some_and(|w| w.class != "DBViewSchedule"));
        let edges = Edges::new(if need { model } else { &[] });
        sheet_ids.iter().map(|(id, number, name)| self.sheet(*id, number, name, &edges)).filter(|s| !s.lines.is_empty() || !s.texts.is_empty() || !s.images.is_empty() || !s.views.is_empty()).collect()
    }

    /// A plan's view range as absolute elevations (feet): what it keeps, from the bottom of
    /// the view depth up to the cut plane (looking down), or from the cut plane up to the
    /// view depth (reflected ceiling plans). Infinite ends are unlimited.
    fn plan_range(&self, v: &ViewInfo) -> Option<(f64, f64)> {
        // a view template sets the range, relative to the view's own level
        let t = self.views.get(&v.template).filter(|t| t.range_rel.is_some());
        if std::env::var_os("SHEET_DEBUG").is_some() {
            if let Some(t) = t {
                eprintln!("  template {} of {}: rel {:?} abs {:?}", t.name, v.name, t.range_rel, t.range_abs);
            }
        }
        let owned;
        let v = match t {
            Some(t) => {
                owned = ViewInfo { range_rel: t.range_rel, range_abs: [None; 4], ..v.clone() };
                &owned
            }
            None => v,
        };
        let lz = self.level_z.get(&v.level).copied();
        let above = lz.and_then(|z| self.levels.iter().copied().filter(|l| *l > z + 0.01).reduce(f64::min));
        // relative value k: None = unlimited
        let rel = |k: usize| -> Option<Option<f64>> {
            let (off, pos) = v.range_rel?;
            Some(match pos[k] {
                -1 => None,
                1 => above.map(|z| z + off[k]),
                _ => Some(lz? + off[k]),
            })
        };
        let cut = v.range_abs[0].filter(|x| x.is_finite()).or_else(|| rel(0).flatten())?;
        // "unlimited" in the relative range wins over a stored elevation (seen at 0)
        let unlimited = |k: usize| v.range_rel.is_some_and(|r| r.1[k] == -1);
        let depth: Option<f64> = if unlimited(3) { None } else { v.range_abs[3].or_else(|| rel(3).flatten()) };
        let bottom: Option<f64> = if unlimited(2) { None } else { v.range_abs[2].or_else(|| rel(2).flatten()) };
        let up = v.dir.is_some_and(|d| d[2] < -0.5) || depth.is_some_and(|d| d > cut + 1e-6);
        if up {
            Some((cut, depth.unwrap_or(f64::INFINITY)))
        } else {
            let lo = match (depth, bottom) {
                (Some(d), Some(b)) => d.min(b),
                (Some(d), None) => d,
                _ => f64::NEG_INFINITY,
            };
            Some((lo.min(cut), cut))
        }
    }

    /// What view `id` keeps of the model (feet): the 3D section box; for plans the crop
    /// region and the view range; for sections, elevations and details the crop region and
    /// the far / near clips. None: the whole model.
    fn clip_of(&self, id: i64, v: &ViewInfo) -> Option<Clip> {
        const INF: f64 = f64::INFINITY;
        let class = v.class.to_ascii_lowercase();
        if class.contains("3d") || class.contains("perspective") || class.contains("rendering") {
            let c = self.section_boxes.get(&v.viewer)?;
            let b = |k: usize| {
                if c.active[k] {
                    c.off[k]
                } else if k % 2 == 0 {
                    INF
                } else {
                    -INF
                }
            };
            return Some(Clip { orig: c.orig, ax: [c.bx, c.by, c.bz], lo: [b(1), b(3), b(5)], hi: [b(0), b(2), b(4)] });
        }
        let crop = self.crops.get(&id);
        let uv = crop.filter(|c| c.active[..4].iter().all(|a| *a));
        let (mut lo, mut hi) =
            ([uv.map_or(-INF, |c| c.off[1]), uv.map_or(-INF, |c| c.off[3]), -INF], [uv.map_or(INF, |c| c.off[0]), uv.map_or(INF, |c| c.off[2]), INF]);
        let clip = if class.contains("plan") {
            let (o, ax) = match crop {
                Some(c) if c.bz[2].abs() > 0.9 => (c.orig, [c.bx, c.by, c.bz]),
                _ => ([0.0; 3], [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]),
            };
            let range = self.plan_range(v).or_else(|| {
                // no stored range: the level below the crop origin up to the cut plane
                let c = crop?;
                let lv = self.levels.iter().copied().filter(|z| *z <= c.orig[2] + 0.01).fold(f64::NEG_INFINITY, f64::max);
                let lv = if lv.is_finite() && c.orig[2] - lv < 20.0 { lv } else { c.orig[2] };
                Some((lv - 0.05, lv + v.cut.unwrap_or(4.0)))
            });
            if let Some((zl, zh)) = range {
                let s = ax[2][2].signum();
                let (a, b) = ((zl - o[2]) * s, (zh - o[2]) * s);
                (lo[2], hi[2]) = (a.min(b), a.max(b));
            }
            Clip { orig: o, ax, lo, hi }
        } else {
            let c = crop?;
            // sections and elevations always cut at their line (near offset), far clip if set
            lo[2] = c.off[5];
            if c.active[4] {
                hi[2] = c.off[4];
            }
            Clip { orig: c.orig, ax: [c.bx, c.by, c.bz], lo, hi }
        };
        lo.iter().chain(hi.iter()).any(|x| x.is_finite()).then_some(clip)
    }

    /// Whether view `id` hides element `elem` of category `cat` (hidden in the view, or its
    /// category switched off in the view's visibility settings).
    pub fn view_hides(&self, id: i64, elem: i64, cat: i64) -> bool {
        self.views.get(&id).is_some_and(|v| self.hides(v, elem, cat))
    }

    /// Whether `view` does not show element `elem` (0: a merged zone) of category `cat`: its
    /// category switched off, the element hidden, or its workset hidden.
    fn hides(&self, view: &ViewInfo, elem: i64, cat: i64) -> bool {
        view.hidden_cats.contains(&cat) || (elem != 0 && (view.hidden.contains(&elem) || self.elem_ws.get(&elem).is_some_and(|w| view.hidden_ws.contains(w))))
    }

    /// Colour of an element's MEP system (its system type's projection line colour).
    pub fn system_color(&self, elem: i64) -> Option<[u8; 3]> {
        self.system_colors.get(self.system_types.get(self.elem_systems.get(&elem)?)?).copied()
    }

    /// Which family document holds each family's lines (see `family_drawing`).
    pub fn set_family_roots(&mut self, roots: HashMap<i64, i64>) {
        self.fam_roots = roots;
    }

    /// The lines and filled regions of a symbol's family document (family coordinates).
    /// `ints`: the instance's own values, over its type's.
    fn family_drawing(&self, sy: &Sym, ints: Option<&HashMap<i64, i64>>) -> Option<std::sync::Arc<crate::rvt_annot::Drawn>> {
        let root = *self.fam_roots.get(&sy.family)?;
        let vals: HashMap<i64, bool> = sy.ints.iter().chain(ints.into_iter().flatten()).filter_map(|(p, v)| Some((self.ann.doc_param(root, *p)?, *v != 0))).collect();
        let mut key: Vec<(i64, bool)> = vals.iter().map(|(a, b)| (*a, *b)).collect();
        key.sort();
        let mut cache = self.fam_drawn.lock().unwrap_or_else(|e| e.into_inner());
        let d = cache.entry((root, key)).or_insert_with(|| {
            let mut d = crate::rvt_annot::Drawn::default();
            self.ann.draw_with(-root, &|p| [p[0], p[1]], 1.0, &vals, &mut d);
            std::sync::Arc::new(d)
        });
        Some(d.clone())
    }

    /// Lines of a symbol placed by `tf`: those of its family document, else its own.
    fn put_symbol(&self, out: &mut Sheet2d, sy: &Sym, tf: &Trf, ints: Option<&HashMap<i64, i64>>) {
        let at = |p: P2| {
            let q = tf.apply([p[0], p[1], 0.0]);
            [q[0], q[1]]
        };
        if let Some(d) = self.family_drawing(sy, ints) {
            if !d.strokes.is_empty() || !d.fills.is_empty() || sy.curves.is_empty() {
                put_drawn(out, &d, &at);
                return;
            }
        }
        let w = (self.ann.pen(1) * FEET) as f32;
        for c in &sy.curves {
            out.strokes.push(Stroke2d { pts: c.iter().map(|p| tf.apply(*p)).map(|q| [(q[0] * FEET) as f32, (q[1] * FEET) as f32]).collect(), width: w, color: [0; 3] });
        }
    }

    /// A text note's leaders (line and arrow) and its frame when its type shows one.
    fn put_text_marks(&self, out: &mut Sheet2d, t: &RawText, tf: &Trf) {
        let m = |x: f64| (x * FEET) as f32;
        let w = m(self.ann.pen(1));
        let p2 = |p: V3| [p[0], p[1]];
        for l in &t.leaders {
            let (a, e, z) = (p2(tf.apply(l[0])), p2(tf.apply(l[1])), p2(tf.apply(l[2])));
            out.strokes.push(Stroke2d { pts: [a, e, z].iter().map(|q| [m(q[0]), m(q[1])]).collect(), width: w, color: [0; 3] });
            // arrow at the end
            let (dx, dy) = (e[0] - z[0], e[1] - z[1]);
            let len = (dx * dx + dy * dy).sqrt();
            if len > 1e-9 {
                let (ux, uy) = (dx / len, dy / len);
                let (s, h) = (ARROW, ARROW * 0.2);
                let (bx, by) = (z[0] + ux * s, z[1] + uy * s);
                out.fills.push(Fill2d { tris: vec![[[m(z[0]), m(z[1])], [m(bx - uy * h), m(by + ux * h)], [m(bx + uy * h), m(by - ux * h)]]], color: [0; 3] });
            }
        }
        let width = if t.width > 0.0 { t.width } else { t.wrap };
        if self.boxed_types.contains(&t.attr) && width > 0.0 {
            let o = p2(tf.apply(t.orig));
            let b = tf.lin(t.base);
            let l = (b[0] * b[0] + b[1] * b[1]).sqrt().max(1e-12);
            let (ux, uy) = (b[0] / l, b[1] / l);
            let along = [0.0, -0.5, -1.0][(t.flags & 3).min(2) as usize] * width;
            let down = if t.flags & 2048 != 0 { 1.0 } else if t.flags & 1024 != 0 { 0.5 } else { 0.0 } * t.height;
            let c = |a: f64, h: f64| [m(o[0] + ux * a - uy * h), m(o[1] + uy * a + ux * h)];
            let (a0, a1, h0, h1) = (along - TEXT_MARGIN, along + width + TEXT_MARGIN, down + TEXT_MARGIN, down - t.height - TEXT_MARGIN);
            out.strokes.push(Stroke2d { pts: vec![c(a0, h0), c(a1, h0), c(a1, h1), c(a0, h1), c(a0, h0)], width: w, color: [0; 3] });
        }
    }

    /// Worksets of a workshared project: `partitions` (`/Global/PartitionTable`) and
    /// `elems` (`/Global/ElemTable`). Each view hides the worksets it sets hidden, and those
    /// hidden globally that it does not set.
    pub fn set_worksets(&mut self, partitions: &[u8], elems: &[u8]) {
        let ws = crate::rvt_model::worksets(partitions);
        self.elem_ws = crate::rvt_model::elem_worksets(elems, &ws);
        if self.elem_ws.is_empty() {
            return;
        }
        for v in self.views.values_mut() {
            v.hidden_ws = ws.iter().filter(|(g, _)| v.ws_set.get(*g).copied().unwrap_or_else(|| self.global_hidden_ws.contains(*g))).map(|(_, &id)| id).collect();
        }
    }

    /// How the views look at the model: direction and up of each view, and what it keeps of
    /// the model (crop region, plan view range, depth clips, 3D section box), with the box
    /// that encloses it clamped to the model's box `lo`..`hi` (metres).
    pub fn frames(&self, lo: [f64; 3], hi: [f64; 3]) -> Vec<ViewFrame> {
        let unit = |v: V3| {
            let l = dot(v, v).sqrt();
            (l > 1e-9 && l.is_finite()).then(|| [v[0] / l, v[1] / l, v[2] / l])
        };
        let f32s = |v: V3| [v[0] as f32, v[1] as f32, v[2] as f32];
        let (flo, fhi) = (lo.map(|x| x / FEET), hi.map(|x| x / FEET));
        let mut out = Vec::new();
        for (&id, v) in &self.views {
            if v.class == "DBViewSchedule" || v.class.contains("Sheet") || v.class.contains("Drafting") {
                continue;
            }
            let (Some(dir), Some(up)) = (v.dir.and_then(unit), v.vert.and_then(unit)) else {
                continue;
            };
            let clip = self.clip_of(id, v);
            let mut region = None;
            let mut vclip = None;
            if let Some(c) = clip {
                // the box narrowed to the model, then its enclosing box
                let (mr0, mr1) = c.box_range(flo, fhi);
                let l: V3 = std::array::from_fn(|k| c.lo[k].max(mr0[k]));
                let h: V3 = std::array::from_fn(|k| c.hi[k].min(mr1[k]));
                if (0..3).all(|k| l[k] <= h[k]) {
                    let (mut a, mut b) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
                    for k in 0..8 {
                        let (u, w, d) = (if k & 1 == 0 { l[0] } else { h[0] }, if k & 2 == 0 { l[1] } else { h[1] }, if k & 4 == 0 { l[2] } else { h[2] });
                        for i in 0..3 {
                            let p = c.orig[i] + c.ax[0][i] * u + c.ax[1][i] * w + c.ax[2][i] * d;
                            a[i] = a[i].min(p);
                            b[i] = b[i].max(p);
                        }
                    }
                    let a: V3 = std::array::from_fn(|i| a[i].max(flo[i]) * FEET);
                    let b: V3 = std::array::from_fn(|i| b[i].min(fhi[i]) * FEET);
                    if (0..3).all(|i| b[i] > a[i] - 1e-6) && (0..3).any(|i| b[i] - a[i] > 0.01) {
                        region = Some((f32s(a), f32s(b)));
                    }
                }
                let far = |x: f64| {
                    if x.is_finite() {
                        (x * FEET) as f32
                    } else {
                        x.signum() as f32 * sw_format::CLIP_FAR
                    }
                };
                vclip = Some(sw_format::ViewClip { orig: f32s(c.orig.map(|x| x * FEET)), axes: c.ax.map(f32s), lo: c.lo.map(far), hi: c.hi.map(far) });
            }
            out.push(ViewFrame { id, dir: f32s(dir), up: f32s(up), region, clip: vclip, hide: Vec::new() });
        }
        out.sort_by_key(|f| f.id);
        out
    }

    fn value(&self, sheet: i64, number: &str, name: &str, scale: &str, p: i64) -> Option<String> {
        let v = self.views.get(&sheet);
        match p {
            -1007400 => return Some(name.to_string()),
            -1007401 => return Some(number.to_string()),
            -1007402 => return Some(scale.to_string()),
            _ => {}
        }
        v.and_then(|v| v.strs.get(&p)).or_else(|| self.project.get(&p)).cloned()
    }

    fn sheet(&self, id: i64, number: &str, name: &str, edges: &Edges<'_>) -> Sheet2d {
        let mut out = Sheet2d { id, ..Default::default() };
        let m = |x: f64| (x * FEET) as f32;
        let pt = |p: P2| [m(p[0]), m(p[1])];
        // scale shown by the title block: the views' common scale, or "as indicated"
        let mut scales: Vec<i64> = self.viewports.iter().filter(|v| v.owner == id && v.scale > 1e-6 && v.scale < 0.99).map(|v| (1.0 / v.scale).round() as i64).collect();
        scales.sort();
        scales.dedup();
        let scale = match scales.as_slice() {
            [] => String::new(),
            [k] => format!("1 : {k}"),
            _ => "Comme indiqué".to_string(),
        };
        let mut extent: Option<[f64; 4]> = None;
        let grow = |e: &mut Option<[f64; 4]>, p: P2| {
            let x = e.get_or_insert([p[0], p[1], p[0], p[1]]);
            x[0] = x[0].min(p[0]);
            x[1] = x[1].min(p[1]);
            x[2] = x[2].max(p[0]);
            x[3] = x[3].max(p[1]);
        };
        // families on the sheet (title block first: the largest)
        let mut block_tables: Vec<V3> = Vec::new();
        let mut insts: Vec<&(i64, i64, Trf)> = self.instances.iter().filter(|i| i.0 == id).collect();
        let area = |i: &&(i64, i64, Trf)| self.symbols.get(&i.1).and_then(|s| s.outline).map(|o| (o[2] - o[0]) * (o[3] - o[1])).unwrap_or(0.0);
        insts.sort_by(|a, b| area(b).total_cmp(&area(a)));
        for (k, (_, sym, tf)) in insts.into_iter().enumerate() {
            let Some(sy) = self.symbols.get(sym) else { continue };
            if std::env::var_os("SHEET_DEBUG").is_some() {
                eprintln!("  inst sym {sym} curves {} texts {:?} tags {:?} tables {}", sy.curves.len(), sy.texts.iter().map(|t| &t.text).collect::<Vec<_>>(), sy.tags.iter().map(|t| &t.params).collect::<Vec<_>>(), sy.tables.len());
            }
            if k == 0 {
                if let Some(o) = sy.outline {
                    for p in [[o[0], o[1], 0.0], [o[2], o[3], 0.0]] {
                        let q = tf.apply(p);
                        grow(&mut extent, [q[0], q[1]]);
                    }
                }
            }
            // images placed in the family (logos), under its lines
            for &(isym, c, base, w, h) in self.fam_images.iter().filter(|i| sy.family > 0 && self.image_fam.get(&i.0) == Some(&sy.family)) {
                let Some((img, _, _)) = self.image(isym) else { continue };
                let (c, b) = (tf.apply(c), tf.lin(base));
                let k = trf_scale(tf);
                // quarter turns of the image's base direction on the paper
                let turns = ((b[1].atan2(b[0]) / std::f64::consts::FRAC_PI_2).round().rem_euclid(4.0)) as u8;
                let (hw, hh) = if turns % 2 == 0 { (w * k * 0.5, h * k * 0.5) } else { (h * k * 0.5, w * k * 0.5) };
                out.images.push(Image2d { rect: [m(c[0] - hw), m(c[1] - hh), m(c[0] + hw), m(c[1] + hh)], image: img, turns });
            }
            for c in &sy.curves {
                out.lines.push(c.iter().map(|p| tf.apply(*p)).map(|q| pt([q[0], q[1]])).collect());
            }
            for t in &sy.texts {
                push_text(&mut out, t, Some(tf), trf_scale(tf), t.text.clone(), self.look(t));
            }
            for t in &sy.tags {
                let mut s = String::new();
                for (p, pre, suf, _) in &t.params {
                    let Some(v) = self.value(id, number, name, &scale, *p) else { continue };
                    if v.is_empty() {
                        continue;
                    }
                    if !s.is_empty() {
                        s.push(' ');
                    }
                    s += &format!("{pre}{v}{suf}");
                }
                push_text(&mut out, t, Some(tf), trf_scale(tf), s, self.look(t));
            }
            for (pos, tb) in &sy.tables {
                let at = tf.apply(*pos);
                self.draw_table(&mut out, tb, at);
                block_tables.push(at);
            }
        }
        for (_, t) in self.texts.iter().filter(|t| t.0 == id) {
            push_text(&mut out, t, None, 1.0, t.text.clone(), self.look(t));
        }
        // annotations drawn on the sheet itself (detail lines, filled regions…)
        let mut d = crate::rvt_annot::Drawn::default();
        self.ann.draw(id, &|p| [p[0], p[1]], 1.0, &mut d);
        put_drawn(&mut out, &d, &|p| p);
        for (_, o, sch) in self.sched_inst.iter().filter(|s| s.0 == id) {
            // the title block's own revision schedule, placed on the sheet too: drawn once
            if block_tables.iter().any(|p| (p[0] - o[0]).abs() < 0.02 && (p[1] - o[1]).abs() < 0.02) {
                continue;
            }
            if let Some(tb) = self.tables.get(sch) {
                self.draw_table(&mut out, tb, *o);
            }
        }
        // placed views
        for vp in self.viewports.iter().filter(|v| v.owner == id) {
            let Some(view) = self.views.get(&vp.view) else { continue };
            if view.class == "DBViewSchedule" {
                continue; // drawn by its ScheduleInstance
            }
            let c = [vp.offset[0], vp.offset[1]];
            let mut rect: Option<[f64; 4]> = None;
            if let Some((img, w, h)) = self.image_inst.get(&view.image).and_then(|s| self.image(*s)) {
                let r = [c[0] - w * 0.5, c[1] - h * 0.5, c[0] + w * 0.5, c[1] + h * 0.5];
                out.images.push(Image2d { rect: [m(r[0]), m(r[1]), m(r[2]), m(r[3])], image: img, turns: 0 });
                rect = Some(r);
            } else if vp.scale > 1e-6 {
                rect = self.draw_view(&mut out, view, vp, edges);
            }
            let Some(r) = rect else { continue };
            out.views.push(Viewport2d { rect: [m(r[0]), m(r[1]), m(r[2]), m(r[3])], view_id: vp.view, name: view.name.clone() });
            // view title: number, name and scale on a line below the view
            if vp.label_len > 1e-4 {
                let (l0, l1) = ([vp.label[0], vp.label[1]], [vp.label[0] + vp.label_len, vp.label[1]]);
                out.lines.push(vec![pt(l0), pt(l1)]);
                let h = 0.012; // about 3.5 mm, the usual size of view titles
                let head = if vp.number.is_empty() { view.name.clone() } else { format!("{}  {}", vp.number, view.name) };
                out.texts.push(Text2d { pos: pt([l0[0], l0[1] + h * 0.4]), dir: [1.0, 0.0], height: m(h), halign: 0, valign: 2, text: head, bold: false, wrap: 0.0, color: [0; 3] });
                if vp.scale > 1e-6 && vp.scale < 0.99 {
                    out.texts.push(Text2d { pos: pt([l0[0], l0[1] - h * 0.4]), dir: [1.0, 0.0], height: m(h * 0.7), halign: 0, valign: 0, text: format!("1 : {}", (1.0 / vp.scale).round()), bold: false, wrap: 0.0, color: [0; 3] });
                }
            }
        }
        // extents: the title block, else everything drawn
        let ext = extent.map(|e| [m(e[0]), m(e[1]), m(e[2]), m(e[3])]).or_else(|| {
            let mut e: Option<[f64; 4]> = None;
            for l in &out.lines {
                for p in l {
                    grow(&mut e, [p[0] as f64, p[1] as f64]);
                }
            }
            for r in out.views.iter().map(|v| v.rect).chain(out.images.iter().map(|i| i.rect)) {
                grow(&mut e, [r[0] as f64, r[1] as f64]);
                grow(&mut e, [r[2] as f64, r[3] as f64]);
            }
            for t in &out.texts {
                grow(&mut e, [t.pos[0] as f64, t.pos[1] as f64]);
            }
            e.map(|e| [e[0] as f32, e[1] as f32, e[2] as f32, e[3] as f32])
        });
        out.extent = ext.unwrap_or([0.0, 0.0, 0.841, 0.594]);
        out
    }

    /// A schedule's stored rows (title, headers): grid and cell texts, stacked downwards from
    /// `o` (top left corner) unless the table grows upwards; a fixed height table filled with
    /// empty body rows.
    fn draw_table(&self, out: &mut Sheet2d, tb: &Table, o: V3) {
        let m = |x: f64| (x * FEET) as f32;
        let up = !tb.top_down;
        let mut y = o[1];
        let width = if tb.width > 1e-6 { tb.width } else { tb.sections.iter().map(|s| s.1.iter().sum::<f64>()).fold(0.0, f64::max) };
        if width <= 1e-6 {
            return;
        }
        let rect = |out: &mut Sheet2d, y0: f64, y1: f64| out.lines.push(vec![[m(o[0]), m(y0)], [m(o[0] + width), m(y0)], [m(o[0] + width), m(y1)], [m(o[0]), m(y1)], [m(o[0]), m(y0)]]);
        // bottom of the title (top of the fixed height)
        let mut below_title = y;
        let sections: Vec<_> = if up { tb.sections.iter().rev().collect() } else { tb.sections.iter().collect() };
        for (title, cols, rows, cells) in sections {
            let rows_order: Vec<usize> = if up { (0..rows.len()).rev().collect() } else { (0..rows.len()).collect() };
            for r in rows_order {
                // at least a text line high: rows with text, and all rows below the title
                let texted = cells.iter().any(|c| c.0 == r);
                let h = if *title && !texted { rows[r] } else { rows[r].max(tb.pitch) }.max(0.005);
                let (y0, y1) = if up { (y, y + h) } else { (y - h, y) };
                rect(out, y0, y1);
                let mut row_cells: Vec<(usize, usize, String)> = cells.iter().filter(|c| c.0 == r).cloned().collect();
                if *title && row_cells.is_empty() && !tb.name.is_empty() {
                    row_cells.push((r, 0, tb.name.clone()));
                }
                let multi = cols.len() > 1 && row_cells.len() > 1;
                let mut x = o[0];
                for (k, w) in cols.iter().enumerate() {
                    if multi && k > 0 {
                        out.lines.push(vec![[m(x), m(y0)], [m(x), m(y1)]]);
                    }
                    if let Some(c) = row_cells.iter().find(|c| c.1 == k) {
                        let (cx, cw) = if multi { (x, *w) } else { (o[0], width) };
                        // letter height: most of the row, shrunk to fit the cell (no wrapping; a
                        // letter is about 0.75 of its height wide)
                        let lines = c.2.lines().count().max(1) as f64;
                        let chars = c.2.lines().map(|l| l.chars().count()).max().unwrap_or(0).max(1) as f64;
                        let th = (h * 0.6 / lines).min(0.0156).min(cw * 0.92 / (chars * 0.75));
                        out.texts.push(Text2d { pos: [m(cx + cw * 0.5), m((y0 + y1) * 0.5)], dir: [1.0, 0.0], height: m(th), halign: 1, valign: 1, text: c.2.clone(), ..Default::default() });
                    }
                    x += w;
                }
                y = if up { y1 } else { y0 };
            }
            if *title {
                below_title = y;
            }
        }
        // fixed height: empty body rows down to its bottom, with the columns of the headers
        let (Some(fixed), false) = (tb.fixed, up) else { return };
        let pitch = tb.pitch.max(0.005);
        let n = ((y - (below_title - fixed)) / pitch + 1e-6).floor().max(0.0) as usize;
        let cols = tb.sections.iter().rev().find(|s| !s.0).map(|s| s.1.clone()).unwrap_or_default();
        for _ in 0..n.min(500) {
            let (y0, y1) = (y - pitch, y);
            rect(out, y0, y1);
            let mut x = o[0];
            for w in cols.iter().take(cols.len().saturating_sub(1)) {
                x += w;
                out.lines.push(vec![[m(x), m(y0)], [m(x), m(y1)]]);
            }
            y = y0;
        }
    }

    /// A model view on the sheet: the edges of the elements it sees, clipped by its crop
    /// region, at the viewport's scale. Returns the frame (feet, paper).
    fn draw_view(&self, out: &mut Sheet2d, view: &ViewInfo, vp: &Vp, edges: &Edges<'_>) -> Option<[f64; 4]> {
        let crop = self.crops.get(&vp.view);
        // view axes: the crop basis, else the view directions
        let (o, bx, by, _bz) = match crop {
            Some(c) => (c.orig, c.bx, c.by, c.bz),
            None => (
                [0.0; 3],
                view.horz?,
                view.vert?,
                view.dir.map(|d| [-d[0], -d[1], -d[2]])?,
            ),
        };
        // what the view keeps (crop, view range, depth clips), segments clipped to it
        let keep = self.clip_of(vp.view, view);
        // a dependent view shows its primary view's annotations too
        let mine = |o: i64| o == vp.view || (view.primary > 0 && o == view.primary);
        // plan regions of the view: (outline in plan, bottom, top), absolute feet
        let regions: Vec<&(Vec<P2>, f64, f64)> = self.plan_regions.iter().filter(|r| mine(r.0) || view.regions.contains(&r.1)).map(|r| &r.2).collect();
        let mut uv_box = crop.filter(|c| c.active[0] && c.active[1] && c.active[2] && c.active[3]).map(|c| [c.off[1], c.off[3], c.off[0], c.off[2]]);
        let cropped = uv_box.is_some();
        let ft = |p: V3| [p[0] / FEET, p[1] / FEET, p[2] / FEET];
        let proj = |p: V3| -> P2 {
            let d = sub(p, o);
            [dot(d, bx), dot(d, by)]
        };
        // towards the viewer: the view direction (`m_viewDir`), else right × up
        let nz = {
            let c = view.dir.filter(|d| dot(*d, *d) > 1e-12).unwrap_or([bx[1] * by[2] - bx[2] * by[1], bx[2] * by[0] - bx[0] * by[2], bx[0] * by[1] - bx[1] * by[0]]);
            let l = dot(c, c).sqrt().max(1e-12);
            [c[0] / l, c[1] / l, c[2] / l]
        };
        let to_view = |p: V3| -> V3 {
            let d = sub(p, o);
            [dot(d, bx), dot(d, by), dot(d, nz)]
        };
        let back = |k: &Clip, c: V3| -> V3 { std::array::from_fn(|i| k.orig[i] + k.ax[0][i] * c[0] + k.ax[1][i] * c[1] + k.ax[2][i] * c[2]) };
        let hlr_on = std::env::var_os("RVT_NO_HLR").is_none();
        let mut segs: Vec<[V3; 2]> = Vec::new();
        // colour of each segment's element (MEP system), same index
        let mut seg_colors: Vec<Option<[u8; 3]>> = Vec::new();
        let mut tris: Vec<[V3; 3]> = Vec::new();
        // the cut plane (plans: the cut height; sections and elevations: their line), on the
        // side of the kept box towards the viewer, in clip coordinates
        let cut_z = keep.filter(|_| !view.class.to_ascii_lowercase().contains("3d")).and_then(|k| {
            let z = if dot(k.ax[2], nz) > 0.0 { k.hi[2] } else { k.lo[2] };
            z.is_finite().then_some(z)
        });
        let mut cuts: Vec<[V3; 2]> = Vec::new();
        // the cut pieces of each hatched element
        let mut hatch_runs: Vec<std::ops::Range<usize>> = Vec::new();
        // drafting views and legends draw no model, only their own lines, texts and symbols
        let drafting = view.class.contains("Drafting") || view.class.contains("Legend");
        for (id, cat, lo, hi, list, mesh) in edges.items.iter().filter(|_| !drafting) {
            // inside a plan region: its own view range
            let keep_e = match (&keep, regions.is_empty()) {
                (Some(k), false) => {
                    let c = ft([(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, 0.0]);
                    regions.iter().find(|r| inside_poly(&r.0, [c[0], c[1]])).map(|r| {
                        let s = k.ax[2][2].signum();
                        let (a, b) = ((r.1 - k.orig[2]) * s, (r.2 - k.orig[2]) * s);
                        Clip { orig: k.orig, ax: k.ax, lo: [k.lo[0], k.lo[1], a.min(b)], hi: [k.hi[0], k.hi[1], a.max(b)] }
                    }).or(Some(*k))
                }
                _ => keep,
            };
            if self.hides(view, *id, *cat) || keep_e.is_some_and(|k| k.misses(ft(*lo), ft(*hi))) {
                continue;
            }
            for e in list {
                let (p, q) = (ft(e[0]), ft(e[1]));
                let (p, q) = match &keep_e {
                    Some(k) => {
                        let Some((a, b)) = k.segment(k.coords(p), k.coords(q)) else {
                            continue;
                        };
                        (back(k, a), back(k, b))
                    }
                    None => (p, q),
                };
                let (a, b) = (to_view(p), to_view(q));
                if (a[0] - b[0]).abs() + (a[1] - b[1]).abs() > 1e-4 {
                    segs.push([a, b]);
                    seg_colors.push(self.system_color(*id).filter(|_| SYSTEM_COLORED.contains(cat)));
                }
            }
            // the faces that hide edges: the rebuilt solid, else the element's box
            if hlr_on && tris.len() < MAX_FACES {
                let mut add = |t: [V3; 3]| match &keep_e {
                    Some(k) => {
                        let mut poly: Vec<V3> = t.iter().map(|p| k.coords(*p)).collect();
                        for ax in 0..3 {
                            let (l, h) = (k.lo[ax], k.hi[ax]);
                            if l.is_finite() {
                                poly = crate::rvt_hlr::clip_poly(&poly, |c| c[ax] - l);
                            }
                            if h.is_finite() {
                                poly = crate::rvt_hlr::clip_poly(&poly, |c| h - c[ax]);
                            }
                            if poly.len() < 3 {
                                return;
                            }
                        }
                        let v: Vec<V3> = poly.iter().map(|c| to_view(back(k, *c))).collect();
                        for i in 1..v.len() - 1 {
                            tris.push([v[0], v[i], v[i + 1]]);
                        }
                    }
                    None => tris.push(t.map(to_view)),
                };
                match mesh {
                    Some(m) => {
                        for t in &m.tri {
                            add(t.map(|i| ft(m.pos.get(i as usize).copied().unwrap_or_default().map(|x| x as f64))));
                        }
                        // the element's section at the cut plane, drawn heavier
                        let cut_e = keep_e.as_ref().filter(|_| cut_z.is_some()).map(|k| if dot(k.ax[2], nz) > 0.0 { k.hi[2] } else { k.lo[2] }).filter(|z| z.is_finite());
                        if let (Some(k), Some(z)) = (keep_e.as_ref(), cut_e) {
                            let first = cuts.len();
                            if !NOT_CUT.contains(cat) && cuts.len() < MAX_SEGMENTS {
                                for t in &m.tri {
                                    let c = t.map(|i| k.coords(ft(m.pos.get(i as usize).copied().unwrap_or_default().map(|x| x as f64))));
                                    let mut pts: Vec<V3> = Vec::new();
                                    for (a, b) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
                                        let (da, db) = (a[2] - z, b[2] - z);
                                        if (da < 0.0) != (db < 0.0) && (da - db).abs() > 1e-12 {
                                            let f = da / (da - db);
                                            pts.push([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, z]);
                                        }
                                    }
                                    if pts.len() == 2 {
                                        if let Some((a, b)) = k.segment(pts[0], pts[1]) {
                                            let (a, b) = (to_view(back(k, a)), to_view(back(k, b)));
                                            if (a[0] - b[0]).abs() + (a[1] - b[1]).abs() > 1e-5 {
                                                cuts.push([a, b]);
                                            }
                                        }
                                    }
                                }
                            }
                            // cut walls, floors, roofs and columns: their section hatched
                            if HATCHED.contains(cat) && cuts.len() > first {
                                hatch_runs.push(first..cuts.len());
                            }
                        }
                    }
                    // a box is not the element's shape (curtain walls, stairs, rooms…):
                    // only small ones hide what is behind them
                    None if (0..3).all(|k| hi[k] - lo[k] < SOLID_BOX) => {
                        for bt in BOX_TRIS {
                            add(bt.map(|c| ft(corner(*lo, *hi, c))));
                        }
                    }
                    None => {}
                }
            }
            if segs.len() > MAX_SEGMENTS {
                break;
            }
        }
        // the view's annotations (not cut by the crop)
        let mut ann = crate::rvt_annot::Drawn::default();
        self.ann.draw(vp.view, &proj, vp.scale, &mut ann);
        if view.primary > 0 {
            self.ann.draw(view.primary, &proj, vp.scale, &mut ann);
        }
        if uv_box.is_none() {
            // no crop: the extents of what is seen
            let mut e = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
            let texts = self.texts.iter().filter(|t| mine(t.0)).map(|t| proj(t.1.orig));
            let placed = self.instances.iter().filter(|i| mine(i.0)).map(|i| proj(i.2.apply([0.0; 3])));
            let lv = self.level_ends.iter().filter(|_| vp.owner == -1 && self.shows_levels(view)).flat_map(|(id, _)| {
                let z = self.level_z.get(id).copied().unwrap_or(0.0);
                let v = proj([0.0, 0.0, z])[1];
                let (u0, u1) = self.level_span(vp.view, *id).unwrap_or_else(|| self.model_span(&proj));
                [[u0, v], [u1, v]]
            });
            for p in segs.iter().flatten().map(|p| [p[0], p[1]]).chain(ann.strokes.iter().flat_map(|s| s.0.iter().copied())).chain(texts).chain(placed).chain(lv) {
                e = [e[0].min(p[0]), e[1].min(p[1]), e[2].max(p[0]), e[3].max(p[1])];
            }
            if !e[0].is_finite() {
                return None;
            }
            uv_box = Some(e);
        }
        let ub = uv_box?;
        // a cropped view puts its origin (the crop's) at the viewport offset; without a crop
        // the centre of what is seen is used
        // (a crop region that is not active counts as none: 3D views and uncropped plans are
        // centred on what they show)
        // (legends and drafting views put their origin there too: their title then sits under
        // their lower left corner, as in Revit)
        let legend = view.class.contains("Legend") || view.class.contains("Drafting");
        let (cu, cv) = if cropped || legend { (0.0, 0.0) } else { ((ub[0] + ub[2]) * 0.5, (ub[1] + ub[3]) * 0.5) };
        let k = vp.scale;
        if std::env::var_os("SHEET_DEBUG").is_some() {
            eprintln!("  view {} {:?} {} crop {:?} ub {:?} offset {:?} label {:?} len {} k {}", vp.view, view.name, view.class, crop.map(|c| (c.orig, c.off, c.active)), ub, vp.offset, vp.label, vp.label_len, k);
        }
        let to_paper = |p: P2| [vp.offset[0] + (p[0] - cu) * k, vp.offset[1] + (p[1] - cv) * k];
        // annotation crop: the annotations wholly outside it are not drawn
        let acrop = crop.and_then(|c| c.annot).filter(|_| cropped && k > 0.0).map(|a| {
            let (p, q) = (to_paper([ub[0], ub[1]]), to_paper([ub[2], ub[3]]));
            [p[0] - a[0], p[1] - a[3], q[0] + a[1], q[1] + a[2]]
        });
        let inside = |p: P2| acrop.map_or(true, |r| p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3]);
        let mut ann = ann;
        if acrop.is_some() && std::env::var_os("SHEET_DEBUG").is_some() {
            eprintln!("    annotation crop {acrop:?}");
        }
        if acrop.is_some() {
            ann.strokes.retain(|s| s.0.iter().any(|p| inside(to_paper(*p))));
            ann.fills.retain(|f| f.0.iter().flatten().any(|p| inside(to_paper(*p))));
            ann.texts.retain(|t| inside(to_paper(t.0)));
        }
        let m = |x: f64| (x * FEET) as f32;
        // hidden lines removed: the faces in a depth buffer over the frame, then the edges
        // keep the parts in front of them
        let hlr = if hlr_on && !tris.is_empty() { crate::rvt_hlr::Hlr::new(ub, HLR_PIXELS) } else { None };
        let mut hlr = hlr;
        if let Some(h) = hlr.as_mut() {
            for t in &tris {
                h.tri(t[0], t[1], t[2]);
            }
        }
        let tol = hlr.as_ref().map_or(0.0, |h| 0.02 + h.pixel() * 0.25);
        let mut shown = 0usize;
        for (s, col) in segs.iter().zip(&seg_colors) {
            let parts = match &hlr {
                Some(h) => h.visible(s[0], s[1], tol),
                None => vec![(s[0], s[1])],
            };
            for (a, b) in parts {
                if let Some((a, b)) = clip([a[0], a[1]], [b[0], b[1]], ub) {
                    let (a, b) = (to_paper(a), to_paper(b));
                    match col {
                        Some(c) => out.strokes.push(Stroke2d { pts: vec![[m(a[0]), m(a[1])], [m(b[0]), m(b[1])]], width: m(self.ann.pen(1)), color: *c }),
                        None => out.fine.push([[m(a[0]), m(a[1])], [m(b[0]), m(b[1])]]),
                    }
                    shown += 1;
                }
            }
        }
        // hatching of the cut sections (diagonal lines, even-odd)
        let sections = hatch_runs.iter().map(|r| cuts[r.clone()].to_vec());
        for sec in sections {
            let edges: Vec<[P2; 2]> = sec.iter().map(|s| [to_paper([s[0][0], s[0][1]]), to_paper([s[1][0], s[1][1]])]).collect();
            for (a, b) in hatch(&edges, HATCH_GAP) {
                if let Some((a, b)) = clip(a, b, [to_paper([ub[0], ub[1]])[0], to_paper([ub[0], ub[1]])[1], to_paper([ub[2], ub[3]])[0], to_paper([ub[2], ub[3]])[1]]) {
                    out.strokes.push(Stroke2d { pts: vec![[m(a[0]), m(a[1])], [m(b[0]), m(b[1])]], width: m(self.ann.pen(1)), color: [96, 96, 96] });
                }
            }
        }
        // cut lines: on the front plane, never hidden
        let cut_w = m(CUT_PEN);
        for s in &cuts {
            if let Some((a, b)) = clip([s[0][0], s[0][1]], [s[1][0], s[1][1]], ub) {
                let (a, b) = (to_paper(a), to_paper(b));
                out.strokes.push(Stroke2d { pts: vec![[m(a[0]), m(a[1])], [m(b[0]), m(b[1])]], width: cut_w, color: [0; 3] });
            }
        }
        if std::env::var_os("SHEET_DEBUG").is_some() {
            eprintln!("    cut pieces {} at {cut_z:?}", cuts.len());
            eprintln!("    edges {} faces {} drawn {} shown pieces {shown} nz {nz:?} bz {:?} keep {:?}", segs.len(), tris.len(), hlr.as_ref().map_or(0, |h| h.faces), crop.map(|c| c.bz), keep.map(|k| (k.ax[2], k.lo[2], k.hi[2])));
        }
        put_drawn(out, &ann, &to_paper);
        // model → paper, for the view's texts and tags
        let mp = Trf::new([k * bx[0], k * by[0], 0.0, k * bx[1], k * by[1], 0.0, k * bx[2], k * by[2], 0.0], [vp.offset[0] - k * (dot(o, bx) + cu), vp.offset[1] - k * (dot(o, by) + cv), 0.0]);
        for (_, t) in self.texts.iter().filter(|t| mine(t.0)) {
            if !inside([mp.apply(t.orig)[0], mp.apply(t.orig)[1]]) {
                continue;
            }
            push_text(out, t, Some(&mp), 1.0, t.text.clone(), self.look(t));
            self.put_text_marks(out, t, &mp);
        }
        // 2D families placed in the view (detail components, symbols)
        for (k, (_, sym, tf)) in self.instances.iter().enumerate().filter(|(_, i)| mine(i.0)) {
            if self.inst_ids.get(k).is_some_and(|id| view.hidden.contains(id)) {
                continue;
            }
            let Some(sy) = self.symbols.get(sym) else { continue };
            let flip = self.inst_flips.get(k).copied().unwrap_or_default();
            // (annotation families: `tf` already holds the view's scale)
            let at = mp.after(tf);
            let pl = at.after(&Trf::new([if flip.0 { -1.0 } else { 1.0 }, 0.0, 0.0, 0.0, if flip.1 { -1.0 } else { 1.0 }, 0.0, 0.0, 0.0, 1.0], [0.0; 3]));
            // the drawing flips, its texts stay in place and readable
            self.put_symbol(out, sy, &pl, self.inst_ints.get(k));
            for t in &sy.texts {
                push_text(out, t, Some(&at), 1.0, t.text.clone(), self.look(t));
                self.put_text_marks(out, t, &at);
            }
            // labels: the instance's text parameters, else its type's, else the family's sample
            for t in &sy.tags {
                let vals: Vec<String> = t
                    .params
                    .iter()
                    .filter_map(|(p, pre, suf, sample)| {
                        let v = self.inst_strs.get(k).and_then(|x| x.get(p)).or_else(|| sy.strs.get(p)).unwrap_or(sample);
                        (!v.is_empty()).then(|| format!("{pre}{v}{suf}"))
                    })
                    .collect();
                push_text(out, t, Some(&at), 1.0, vals.join(" "), self.look(t));
            }
        }
        // grids, in plan views: their line cut to the view, their bubble at each end
        const GRIDS: i64 = -2000220;
        let plan = view.dir.is_some_and(|d| d[2].abs() > 0.99) && view.class.to_ascii_lowercase().contains("plan");
        for g in self.grids.iter().filter(|_| plan && !view.hidden_cats.contains(&GRIDS)) {
            if keep.is_some_and(|k| {
                let (zl, zh) = (k.orig[2] + k.lo[2] * k.ax[2][2], k.orig[2] + k.hi[2] * k.ax[2][2]);
                g.z.1 < zl.min(zh) || g.z.0 > zl.max(zh)
            }) {
                continue;
            }
            let Some((a, b)) = clip(proj(g.a), proj(g.b), ub) else { continue };
            let (p, q) = (to_paper(a), to_paper(b));
            let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-6 {
                continue;
            }
            let (ux, uy) = (dx / len, dy / len);
            let w = m(self.ann.pen(1));
            // dash-dot
            let mut t = 0.0;
            while t < len {
                for (l, draw) in [(0.04, true), (0.01, false), (0.004, true), (0.01, false)] {
                    let e = (t + l).min(len);
                    if draw && e > t {
                        out.strokes.push(Stroke2d { pts: vec![[m(p[0] + ux * t), m(p[1] + uy * t)], [m(p[0] + ux * e), m(p[1] + uy * e)]], width: w, color: [0; 3] });
                    }
                    t = e;
                }
            }
            if let Some(sy) = self.level_heads.get(&g.attr).and_then(|h| self.symbols.get(h)) {
                for (end, dir) in [(p, [-ux, -uy]), (q, [ux, uy])] {
                    // the bubble past the end of the line, upright
                    let r = sy.outline.map_or(0.0, |o| (o[3] - o[1]).abs() * 0.5);
                    let tf = Trf::new([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], [end[0] + dir[0] * r, end[1] + dir[1] * r, 0.0]);
                    self.put_symbol(out, sy, &tf, None);
                    for t in &sy.tags {
                        push_text(out, t, Some(&tf), 1.0, g.name.clone(), self.look(t));
                    }
                }
            }
        }
        // levels: their line and head; on a sheet, cut to what the view shows
        for (id, (p0, _)) in self.level_ends.iter().filter(|_| self.shows_levels(view)) {
            let (Some(z), Some(name)) = (self.level_z.get(id), self.level_names.get(id)) else { continue };
            let a = proj([p0[0], p0[1], *z]);
            let (mut u0, mut u1) = self.level_span(vp.view, *id).unwrap_or_else(|| self.model_span(&proj));
            if vp.owner != -1 {
                if a[1] < ub[1] || a[1] > ub[3] {
                    continue;
                }
                (u0, u1) = (u0.max(ub[0]), u1.min(ub[2]));
            }
            if u1 - u0 < 1e-6 {
                continue;
            }
            let (p, q) = (to_paper([u0, a[1]]), to_paper([u1, a[1]]));
            let w = m(self.ann.pen(1));
            let (dash, gap) = (0.04, 0.02);
            let mut x = p[0];
            while x < q[0] {
                let e = (x + dash).min(q[0]);
                out.strokes.push(Stroke2d { pts: vec![[m(x), m(p[1])], [m(e), m(p[1])]], width: w, color: [0; 3] });
                x = e + gap;
            }
            let head = self.level_types.get(id).and_then(|t| self.level_heads.get(t)).and_then(|h| self.symbols.get(h));
            match head {
                Some(sy) => {
                    let tf = Trf::new([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], [q[0], q[1], 0.0]);
                    self.put_symbol(out, sy, &tf, None);
                    for t in &sy.texts {
                        push_text(out, t, Some(&tf), 1.0, t.text.clone(), self.look(t));
                    }
                    for t in &sy.tags {
                        // labels of a level head: its name, its elevation
                        let text = t.params.iter().map(|(p, pre, suf, _)| format!("{pre}{}{suf}", if *p == LEVEL_ELEV { self.ann.length(*z) } else { name.clone() })).collect::<Vec<_>>().join(" ");
                        push_text(out, t, Some(&tf), 1.0, text, self.look(t));
                    }
                }
                None => {
                    let h = 0.0078;
                    out.texts.push(Text2d { pos: [m(q[0]), m(q[1] + h * 0.5)], dir: [1.0, 0.0], height: m(h), halign: 2, valign: 0, text: name.clone(), bold: false, wrap: 0.0, color: [0; 3] });
                }
            }
        }
        for tg in self.ann.tags.iter().filter(|t| mine(t.owner)) {
            let Some(sy) = self.symbols.get(&tg.symbol) else { continue };
            let h = mp.apply(tg.head);
            if !inside([h[0], h[1]]) {
                continue;
            }
            // a tag of a family turned with its component follows the tagged duct or pipe,
            // kept readable (a tag with a leader stays as placed)
            let angle = match self.mep_dirs.get(&tg.target).filter(|_| self.turning_tags.contains(&sy.family) && tg.leader.is_none()) {
                Some(d) => {
                    let p = mp.lin(*d);
                    let mut a = p[1].atan2(p[0]);
                    if a > std::f64::consts::FRAC_PI_2 + 1e-6 {
                        a -= std::f64::consts::PI;
                    } else if a <= -std::f64::consts::FRAC_PI_2 + 1e-6 {
                        a += std::f64::consts::PI;
                    }
                    if p[0].hypot(p[1]) > 1e-9 { a } else { tg.angle }
                }
                None => tg.angle,
            };
            let (c, s) = (angle.cos(), angle.sin());
            let tf = Trf::new([c, s, 0.0, -s, c, 0.0, 0.0, 0.0, 1.0], [h[0], h[1], 0.0]);
            if let Some((elbow, end)) = tg.leader {
                let k = vp.scale;
                let pt = |o: [f64; 2]| [m(h[0] + o[0] * k), m(h[1] + o[1] * k)];
                let mut pts = vec![pt([0.0; 2])];
                if elbow[0].hypot(elbow[1]) > 1e-9 {
                    pts.push(pt(elbow));
                }
                // an end left at the head: attached to the tagged curve, at its point nearest
                // the elbow
                let attached = (end[0].hypot(end[1]) < 1e-9).then(|| self.mep_lines.get(&tg.target)).flatten();
                match attached {
                    Some((a, b)) => {
                        let (pa, pb) = (mp.apply(*a), mp.apply(*b));
                        let e = [h[0] + elbow[0] * k, h[1] + elbow[1] * k];
                        let (dx, dy) = (pb[0] - pa[0], pb[1] - pa[1]);
                        let l2 = dx * dx + dy * dy;
                        let t = if l2 > 1e-18 { (((e[0] - pa[0]) * dx + (e[1] - pa[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
                        pts.push([m(pa[0] + dx * t), m(pa[1] + dy * t)]);
                    }
                    None => pts.push(pt(end)),
                }
                if pts.len() >= 2 {
                    out.strokes.push(Stroke2d { pts, width: m(self.ann.pen(1)), color: [0; 3] });
                }
            }
            self.put_symbol(out, sy, &tf, None);
            for t in &sy.texts {
                push_text(out, t, Some(&tf), 1.0, t.text.clone(), self.look(t));
            }
            // labels: the tagged element's text parameters, else its type's
            let ty = self.ann.inst_type.get(&tg.target);
            for t in &sy.tags {
                let vals: Vec<String> = t.params.iter().filter_map(|(p, pre, suf, _)| {
                    // a keynote shows its key; the others the tagged element's values
                    let computed = match *p {
                        // the key's last part, as numbered on the sheet ("AC-251.02" -> "2")
                        KEYNOTE_NUMBER => self.ann.keys.get(&tg.id).map(|k| {
                            let last = k.rsplit('.').next().unwrap_or(k).trim_start_matches('0');
                            if last.is_empty() { "0".into() } else { last.to_string() }
                        }),
                        SYSTEM_ABBREVIATION => self.elem_systems.get(&tg.target).and_then(|sy| self.system_types.get(sy)).and_then(|ty| self.system_abbr.get(ty)).cloned(),
                        MEP_SIZE => self.mep_sizes.get(&tg.target).cloned(),
                        PIPE_DIAMETER => self.mep_diams.get(&tg.target).cloned(),
                        _ => None,
                    };
                    let v = computed
                        .or_else(|| self.ann.strs.get(&tg.target).and_then(|x| x.get(p)).cloned())
                        .or_else(|| ty.and_then(|y| self.ann.strs.get(y)).and_then(|x| x.get(p)).cloned())?;
                    (!v.is_empty()).then(|| format!("{pre}{v}{suf}"))
                }).collect();
                push_text(out, t, Some(&tf), 1.0, vals.join(" "), self.look(t));
            }
        }
        let a = to_paper([ub[0], ub[1]]);
        let b = to_paper([ub[2], ub[3]]);
        Some([a[0], a[1], b[0], b[1]])
    }
}

/// Length scale of a transform in its plane.
fn trf_scale(tf: &Trf) -> f64 {
    let x = tf.lin([1.0, 0.0, 0.0]);
    (x[0] * x[0] + x[1] * x[1]).sqrt()
}

/// A text placed by `tf` (paper feet out), its height scaled by `hk`.
fn push_text(out: &mut Sheet2d, t: &RawText, tf: Option<&Trf>, hk: f64, text: String, (bold, color, typed): (bool, [u8; 3], Option<f64>)) {
    if text.trim().is_empty() {
        return;
    }
    let m = |x: f64| (x * FEET) as f32;
    let (o, b) = match tf {
        Some(tf) => (tf.apply(t.orig), tf.lin(t.base)),
        None => (t.orig, t.base),
    };
    let l = (b[0] * b[0] + b[1] * b[1]).sqrt().max(1e-9);
    // alignment flags: left / centre / right 64 / 128 / 256 (and 0 / 1 / 2 in the low bits),
    // top / middle / bottom 512 / 1024 / 2048
    let h = (t.flags & 3) as u8;
    let v = if t.flags & 2048 != 0 {
        2
    } else if t.flags & 1024 != 0 {
        1
    } else {
        0
    };
    // the type's size; else what the text box holds
    let size = typed.unwrap_or_else(|| text_size(t.height, t.wrap, &text)) * hk;
    // a text turned over (mirrored group) still reads from left to right
    let (b, h, v) = if b[0] / l < -0.99 { ([-b[0], -b[1], b[2]], 2 - h.min(2), 2 - v) } else { (b, h, v) };
    out.texts.push(Text2d { pos: [m(o[0]), m(o[1])], dir: [(b[0] / l) as f32, (b[1] / l) as f32], height: m(size), halign: h.min(2), valign: v, text, bold, wrap: m(t.wrap * hk), color });
}

/// Letter size of a text whose box is `box_h` high (feet): the box holds its lines as drawn,
/// so with a wrapping width `wrap` (0: none) the long paragraphs count for the lines they
/// wrap to (a letter about 0.8 of the size wide): the largest size whose lines fill the box.
fn text_size(box_h: f64, wrap: f64, text: &str) -> f64 {
    let paras: Vec<f64> = text.lines().map(|p| p.chars().count() as f64).collect();
    let top = box_h / LINE_BOX / paras.len().max(1) as f64;
    if wrap <= 0.0 {
        return top;
    }
    let lines = |size: f64| paras.iter().map(|c| (c * 0.8 * size / wrap - 1e-9).ceil().max(1.0)).sum::<f64>().max(1.0);
    let (mut lo, mut hi) = (0.0, top);
    for _ in 0..40 {
        let mid = (lo + hi) * 0.5;
        if lines(mid) * mid * LINE_BOX <= box_h * (1.0 + 1e-9) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    if lines(top) * top * LINE_BOX <= box_h * (1.0 + 1e-9) {
        top
    } else {
        lo
    }
}

/// Annotation geometry (`Drawn`, view plane) put on the paper by `to_paper`.
fn put_drawn(out: &mut Sheet2d, d: &crate::rvt_annot::Drawn, to_paper: &dyn Fn(P2) -> P2) {
    let m = |x: f64| (x * FEET) as f32;
    let pt = |p: P2| {
        let q = to_paper(p);
        [m(q[0]), m(q[1])]
    };
    for (pts, w, c) in &d.strokes {
        out.strokes.push(Stroke2d { pts: pts.iter().map(|p| pt(*p)).collect(), width: m(*w), color: *c });
    }
    for (tris, c) in &d.fills {
        out.fills.push(Fill2d { tris: tris.iter().map(|t| [pt(t[0]), pt(t[1]), pt(t[2])]).collect(), color: *c });
    }
    for (p, dir, h, text) in &d.texts {
        let (a, b) = (to_paper(*p), to_paper([p[0] + dir[0], p[1] + dir[1]]));
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = (dx * dx + dy * dy).sqrt().max(1e-12);
        out.texts.push(Text2d { pos: [m(a[0]), m(a[1])], dir: [(dx / l) as f32, (dy / l) as f32], height: m(*h), halign: 1, valign: 2, text: text.clone(), bold: false, wrap: 0.0, color: [0; 3] });
    }
}

/// Weight of the lines where the view cuts the elements (paper feet, 0.35 mm).
const CUT_PEN: f64 = 0.35 / 304.8;
/// Built-in categories Revit does not cut (furniture, fixtures, equipment, planting,
/// entourage, parking, site, topography): drawn in projection even across the cut plane.
const NOT_CUT: [i64; 13] = [-2000080, -2001100, -2001160, -2001120, -2001060, -2001040, -2001140, -2001350, -2001360, -2001370, -2001180, -2001260, -2001340];
/// Categories whose cut section is hatched: walls, floors, roofs, columns, structural columns.
const HATCHED: [i64; 5] = [-2000011, -2000032, -2000035, -2000100, -2001330];
/// Spacing of the hatch lines on paper (feet, about 1.5 mm).
const HATCH_GAP: f64 = 0.005;

/// Diagonal (45°) hatch lines `gap` apart inside the closed outline `edges` (even-odd).
fn hatch(edges: &[[P2; 2]], gap: f64) -> Vec<(P2, P2)> {
    let r = std::f64::consts::FRAC_1_SQRT_2;
    // rotated frame: u along the hatch, v across it
    let to = |p: P2| [(p[0] + p[1]) * r, (p[1] - p[0]) * r];
    let back = |u: f64, v: f64| [(u - v) * r, (u + v) * r];
    let e: Vec<[P2; 2]> = edges.iter().map(|s| [to(s[0]), to(s[1])]).collect();
    let (lo, hi) = e.iter().flatten().fold((f64::INFINITY, f64::NEG_INFINITY), |a, p| (a.0.min(p[1]), a.1.max(p[1])));
    let mut out = Vec::new();
    if !(hi - lo).is_finite() || (hi - lo) / gap > 4000.0 {
        return out;
    }
    let mut v = (lo / gap).ceil() * gap;
    while v <= hi {
        let mut us: Vec<f64> = e
            .iter()
            .filter_map(|s| {
                let (a, b) = (s[0], s[1]);
                ((a[1] > v) != (b[1] > v)).then(|| a[0] + (v - a[1]) / (b[1] - a[1]) * (b[0] - a[0]))
            })
            .collect();
        us.sort_by(|a, b| a.total_cmp(b));
        for w in us.chunks_exact(2) {
            if w[1] - w[0] > 1e-9 {
                out.push((back(w[0], v), back(w[1], v)));
            }
        }
        v += gap;
    }
    out
}

/// Most model edges drawn in one view.
const MAX_SEGMENTS: usize = 60_000;
/// Most faces drawn to hide edges in one view.
const MAX_FACES: usize = 1_500_000;
/// Depth buffer size (pixels on the longer side of a placed view).
const HLR_PIXELS: usize = 2400;
/// Element boxes (metres) small enough to stand for the element when hiding edges.
const SOLID_BOX: f64 = 0.0;

/// Liang-Barsky clip of a segment to a box [umin, vmin, umax, vmax].
fn clip(a: P2, b: P2, r: [f64; 4]) -> Option<(P2, P2)> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-d[0], a[0] - r[0]), (d[0], r[2] - a[0]), (-d[1], a[1] - r[1]), (d[1], r[3] - a[1])] {
        if p.abs() < 1e-15 {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let t = q / p;
        if p < 0.0 {
            t0 = t0.max(t);
        } else {
            t1 = t1.min(t);
        }
        if t0 > t1 {
            return None;
        }
    }
    Some(([a[0] + d[0] * t0, a[1] + d[1] * t0], [a[0] + d[0] * t1, a[1] + d[1] * t1]))
}

/// The ids listed in field `field` of the objects of class `class` in element `tr`.
/// GUID of a `PartitionGUID` (nested objects down to its 16 bytes).
fn guid_of(o: &Obj) -> Option<[u8; 16]> {
    match o.fields.first().map(|f| &f.1)? {
        G::O(x) => guid_of(x),
        G::Raw(r) => r.as_slice().try_into().ok(),
        _ => None,
    }
}

/// A view's workset visibility (`WorksetVisibilityViewSettings.m_worksetVisibilityPGUIDMap`:
/// workset GUID → shown): workset GUID → hidden. Worksets it does not list follow the global
/// setting.
fn workset_settings(tr: &Tree, s: &Schema) -> HashMap<[u8; 16], bool> {
    tr.objs
        .iter()
        .filter(|o| s.name(o.tag) == "WorksetVisibilityViewSettings")
        .filter_map(|o| match o.get("m_worksetVisibilityPGUIDMap") {
            Some(G::L(l)) => Some(l),
            _ => None,
        })
        .flatten()
        .filter_map(|e| match e.get("first")? {
            G::O(g) => Some((guid_of(g)?, e.int("second")? == 0)),
            _ => None,
        })
        .collect()
}

/// Text values of a family instance or type: its parameters, and those of its family
/// (`FamilyParams.m_params`: `m_paramId`, `m_str`).
fn family_strs(tr: &Tree, s: &Schema) -> HashMap<i64, String> {
    let mut out = strings(tr);
    for o in tr.objs.iter().filter(|o| s.name(o.tag) == "FamilyParams") {
        for p in o.list("m_params") {
            if let (Some(id), Some(v)) = (p.int("m_paramId"), text_of(p, "m_str").filter(|v| !v.is_empty())) {
                out.entry(id).or_insert(v);
            }
        }
    }
    out
}

/// Integer values of a family instance or type (`FamilyParams.m_params`: `m_int`).
fn family_ints(tr: &Tree, s: &Schema) -> HashMap<i64, i64> {
    tr.objs.iter().filter(|o| s.name(o.tag) == "FamilyParams").flat_map(|o| o.list("m_params").iter()).filter_map(|p| Some((p.int("m_paramId")?, p.int("m_int")?))).collect()
}

fn ids_of(tr: &Tree, s: &Schema, class: &str, field: &str) -> std::collections::HashSet<i64> {
    tr.objs
        .iter()
        .filter(|o| s.name(o.tag) == class)
        .filter_map(|o| match o.get(field) {
            Some(G::Ids(v)) | Some(G::Is(v)) => Some(v.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// The model's drawable edges (metres): feature edges of the rebuilt meshes, box edges of
/// the others, with each element's box and the faces that hide what is behind them.
struct Edges<'a> {
    items: Vec<(i64, i64, [f64; 3], [f64; 3], Vec<[V3; 2]>, Option<&'a Mesh>)>,
}

/// Whether point `p` is inside polygon `poly` (even-odd).
fn inside_poly(poly: &[P2], p: P2) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]) {
            inside = !inside;
        }
    }
    inside
}

/// Corners of box `k` (bit 0: x, 1: y, 2: z).
fn corner(lo: V3, hi: V3, k: usize) -> V3 {
    [if k & 1 == 0 { lo[0] } else { hi[0] }, if k & 2 == 0 { lo[1] } else { hi[1] }, if k & 4 == 0 { lo[2] } else { hi[2] }]
}

/// The 12 triangles of a box (corner indices).
const BOX_TRIS: [[usize; 3]; 12] = [[0, 1, 3], [0, 3, 2], [4, 6, 7], [4, 7, 5], [0, 4, 5], [0, 5, 1], [2, 3, 7], [2, 7, 6], [0, 2, 6], [0, 6, 4], [1, 5, 7], [1, 7, 3]];

impl<'a> Edges<'a> {
    fn new(model: &[(i64, i64, [f64; 3], [f64; 3], Option<&'a Mesh>)]) -> Edges<'a> {
        let items = model
            .iter()
            .map(|(id, cat, lo, hi, mesh)| {
                let mesh = mesh.filter(|m| !m.tri.is_empty());
                let list = match mesh {
                    Some(m) => feature_edges(m),
                    None => [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)].iter().map(|&(a, b)| [corner(*lo, *hi, a), corner(*lo, *hi, b)]).collect(),
                };
                (*id, *cat, *lo, *hi, list, mesh)
            })
            .collect();
        Edges { items }
    }
}

/// Edges of a mesh between faces that are not coplanar (or on its border).
fn feature_edges(m: &Mesh) -> Vec<[V3; 2]> {
    let q = |p: [f32; 3]| [(p[0] * 2000.0).round() as i64, (p[1] * 2000.0).round() as i64, (p[2] * 2000.0).round() as i64];
    let mut by: HashMap<([i64; 3], [i64; 3]), (Vec<[f64; 3]>, [V3; 2])> = HashMap::new();
    for t in &m.tri {
        let p: Vec<[f32; 3]> = t.iter().map(|&i| m.pos.get(i as usize).copied().unwrap_or_default()).collect();
        let d = |a: [f32; 3], b: [f32; 3]| [(b[0] - a[0]) as f64, (b[1] - a[1]) as f64, (b[2] - a[2]) as f64];
        let (u, v) = (d(p[0], p[1]), d(p[0], p[2]));
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let l = dot(n, n).sqrt();
        if l < 1e-12 {
            continue;
        }
        let n = [n[0] / l, n[1] / l, n[2] / l];
        for k in 0..3 {
            let (a, b) = (p[k], p[(k + 1) % 3]);
            let (ka, kb) = (q(a), q(b));
            let key = if ka <= kb { (ka, kb) } else { (kb, ka) };
            let e = by.entry(key).or_insert_with(|| (Vec::new(), [a.map(|x| x as f64), b.map(|x| x as f64)]));
            e.0.push(n);
        }
    }
    by.into_values()
        .filter(|(ns, _)| ns.len() != 2 || dot(ns[0], ns[1]) < 0.94)
        .map(|(_, e)| e)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_text_size() {
        // two lines, no wrapping: the box holds them
        assert!((text_size(2.0 * LINE_BOX * 0.01, 0.0, "ab\ncd") - 0.01).abs() < 1e-12);
        // one paragraph wrapped to 4 lines in its box
        let t = "x".repeat(100);
        let s = text_size(4.0 * LINE_BOX * 0.01, 0.25, &t);
        assert!((s - 0.01).abs() < 1e-8, "{s}");
    }

    #[test]
    fn rtf_plain() {
        let r = br"{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\fnil Arial;}}{\colortbl ;\red0\green0\blue0;}\viewkind4\uc1\pard\cf1\f0\fs20 Caf\'e9 \u8364?\par Ligne 2\par}";
        assert_eq!(rtf_text(r), "Café €\nLigne 2");
        let r = br"{\rtf1\ansi{\fonttbl{\f0\fnil Arial;}}\pard\qc\b\fs40 Base vie \endash  Piste\par 77\~777\par}";
        assert_eq!(rtf_parse(r), ("Base vie \u{2013} Piste\n77\u{a0}777".to_string(), true));
        assert!(!rtf_parse(br"{\rtf1 a \b b\b0  cde\par}").1);
    }

    #[test]
    fn clip_inside_and_across() {
        let r = [0.0, 0.0, 1.0, 1.0];
        assert!(clip([2.0, 2.0], [3.0, 3.0], r).is_none());
        let (a, b) = clip([-1.0, 0.5], [2.0, 0.5], r).unwrap();
        assert!((a[0] - 0.0).abs() < 1e-12 && (b[0] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn cube_feature_edges() {
        let mut pos = Vec::new();
        let mut tri = Vec::new();
        crate::rvt_model::push_box(&crate::rvt_model::ElemBox { id: 0, category: 0, min: [0.0; 3], max: [1.0; 3] }, &mut pos, &mut tri);
        let m = Mesh { pos, tri, ..Default::default() };
        assert_eq!(feature_edges(&m).len(), 12);
    }
}
