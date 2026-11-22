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
use crate::det::HashMap;
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

/// [`rtf_text`], whether most of the text is bold (`\b`), and the parts of it written their own
/// way (bold `\b`, italic `\i`, underlined `\ul`, superscript `\super`, subscript `\sub`),
/// covering it in order; its letters in capitals where `\caps` says so (ASCII ones only, as
/// Revit prints them: "RéGULATEUR").
fn rtf_parse(b: &[u8]) -> (String, bool, Vec<sw_format::TextSpan>) {
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
        return (String::from_utf8_lossy(b).trim_end_matches('\0').to_string(), false, Vec::new());
    }
    let mut out = String::new();
    let mut stack: Vec<(bool, i64, bool, bool, bool, i8, bool)> = Vec::new();
    let (mut skip, mut uc, mut bold, mut italic, mut ul, mut script, mut caps) = (false, 1i64, false, false, false, 0i8, false);
    let up = |c: char, caps: bool| if caps { c.to_ascii_uppercase() } else { c };
    let mut pending_skip = 0i64;
    // visible characters written, and how many of them bold; the parts written their own way
    let (mut seen, mut shown, mut bolds, mut was) = (0usize, 0usize, 0usize, (false, false, false, 0i8));
    let mut spans: Vec<sw_format::TextSpan> = Vec::new();
    let mut count = |out: &str, seen: usize, was: (bool, bool, bool, i8), spans: &mut Vec<sw_format::TextSpan>| {
        let n = out[seen..].chars().filter(|c| !c.is_whitespace()).count();
        shown += n;
        if was.0 {
            bolds += n;
        }
        let len = out.len() - seen;
        if len > 0 {
            match spans.last_mut() {
                Some(s) if (s.bold, s.italic, s.underline, s.script) == was => s.len += len,
                _ => spans.push(sw_format::TextSpan { len, bold: was.0, italic: was.1, underline: was.2, script: was.3 }),
            }
        }
    };
    let mut i = 0;
    while i < b.len() {
        count(&out, seen, was, &mut spans);
        (seen, was) = (out.len(), (bold, italic, ul, script));
        let c = b[i];
        match c {
            b'{' => {
                stack.push((skip, uc, bold, italic, ul, script, caps));
                i += 1;
            }
            b'}' => {
                if let Some((s, u, w, it, un, sc, cp)) = stack.pop() {
                    skip = s;
                    uc = u;
                    bold = w;
                    italic = it;
                    ul = un;
                    script = sc;
                    caps = cp;
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
                        b"i" => italic = param != Some(0),
                        b"ul" => ul = param != Some(0),
                        b"ulnone" => ul = false,
                        b"super" => script = 1,
                        b"sub" => script = -1,
                        b"nosupersub" => script = 0,
                        b"caps" => caps = param != Some(0),
                        b"plain" => (bold, italic, ul, script, caps) = (false, false, false, 0, false),
                        b"uc" => uc = param.unwrap_or(1),
                        b"u" => {
                            if !skip {
                                let v = param.unwrap_or(63);
                                let v = if v < 0 { v + 65536 } else { v };
                                out.push(up(char::from_u32(v as u32).unwrap_or('?'), caps));
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
                                out.push(up(cp1252(h), caps));
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
                    out.push(up(cp1252(c), caps));
                }
            }
        }
    }
    count(&out, seen, was, &mut spans);
    // (RichEdit's last \par ends the last paragraph; empty paragraphs before it are lines Revit
    // lays out: AC-100's notes end with one, set a line higher above their bottom anchor)
    let text = out.trim_end_matches(['\0', ' ']);
    let text = text.strip_suffix('\n').unwrap_or(text).trim_end_matches(' ').to_string();
    // (the parts cut to the text kept)
    let mut left = text.len();
    spans.retain_mut(|s| {
        s.len = s.len.min(left);
        left -= s.len;
        s.len > 0
    });
    // one look throughout: no parts (bold or not, as the text mostly is), unless it is italic,
    // underlined or a script
    if spans.len() == 1 && !(spans[0].italic || spans[0].underline || spans[0].script != 0) {
        spans.clear();
    }
    (text, shown > 0 && bolds * 2 > shown, spans)
}

/// Length of a text leader's arrow, and margin of a text frame (feet on paper).
const ARROW: f64 = 0.0098;
/// An integer parameter of an element (`ParamValueSetInt`), when set.
fn int_param(tr: &Tree, id: i64) -> Option<i64> {
    tr.objs.iter().flat_map(|o| o.list("m_paramSet").iter()).find(|p| p.int("m_paramId") == Some(id)).and_then(|p| p.int("m_value"))
}

/// Categories of duct and pipe insulation (`OST_DuctInsulations`, `OST_PipeInsulations`).
const DUCT_INSULATION: i64 = -2008123;
const PIPE_INSULATION: i64 = -2008122;
/// A view's detail level (`VIEW_DETAIL_LEVEL`: 1 coarse, 2 medium, 3 fine).
const VIEW_DETAIL_LEVEL: i64 = -1011002;

/// Built-in parameters shown by view references and view heads: the sheet number and detail
/// number of the view referred to (`VIEWER_SHEET_NUMBER`, `VIEWER_DETAIL_NUMBER`), its name.
const VIEWER_SHEET_NUMBER: i64 = -1006601;
const VIEWER_DETAIL_NUMBER: i64 = -1006602;
const VIEW_NAME: i64 = -1005112;
/// A view's "Family and Type", e.g. "Coupes (DE_AC_BS)".
const VIEW_TYPE: i64 = -1012106;
const LEVELS: i64 = -2000240;
/// A view mark's reference label ("Sim": a reference to an existing view).
const VIEWER_REFERENCE_LABEL: i64 = -1005120;
/// View title labels: the viewport's number, sheet and view name, the view's title on sheet
/// and scale.
const VIEWPORT_DETAIL_NUMBER: i64 = -1005201;
const VIEWPORT_SHEET_NUMBER: i64 = -1005206;
const VIEWPORT_VIEW_NAME: i64 = -1005203;
const VIEW_DESCRIPTION: i64 = -1005114;
const VIEW_SCALE: i64 = -1005150;
const VIEWPORT_SCALE: i64 = -1005204;
const VIEW_SCALE_PULLDOWN_METRIC: i64 = -1005151;
const VIEW_SCALE_PULLDOWN_IMPERIAL: i64 = -1005152;

/// Marks a keynote's key in a sheet text until the sheet numbers its keynotes.
const KEY_MARK: char = '\u{1}';

/// The parts of each segment (view coordinates, z towards the viewer) not lying on a nearer
/// segment of another element that `covers` (an element drawn as one line): as parameter
/// ranges along it, the whole `(0, 1)` when nothing lies over it. `tol`: how far apart two
/// lines may be and still be one (feet).
fn uncovered(segs: &[[V3; 2]], owner: &[u32], covers: &dyn Fn(u32) -> bool, tol: f64) -> Vec<Vec<(f64, f64)>> {
    // its line: its direction (one way) and its distance from the origin, both rounded
    let line = |s: &[V3; 2]| -> Option<([f64; 2], (i64, i64))> {
        let d = [s[1][0] - s[0][0], s[1][1] - s[0][1]];
        let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if l < 1e-9 {
            return None;
        }
        let mut u = [d[0] / l, d[1] / l];
        if u[0] < -1e-12 || (u[0].abs() <= 1e-12 && u[1] < 0.0) {
            u = [-u[0], -u[1]];
        }
        let off = -s[0][0] * u[1] + s[0][1] * u[0];
        Some((u, ((u[1].atan2(u[0]) * 1e4).round() as i64, (off / tol).round() as i64)))
    };
    let mut by: HashMap<(i64, i64), Vec<usize>> = HashMap::default();
    let mut dirs: Vec<Option<[f64; 2]>> = Vec::with_capacity(segs.len());
    for (k, s) in segs.iter().enumerate() {
        let l = line(s);
        dirs.push(l.map(|l| l.0));
        if let Some((_, key)) = l {
            by.entry(key).or_default().push(k);
        }
    }
    let mut out: Vec<Vec<(f64, f64)>> = vec![vec![(0.0, 1.0)]; segs.len()];
    for ks in by.values().filter(|ks| ks.len() > 1 && ks.iter().any(|&k| covers(owner[k]))) {
        for &i in ks {
            let Some(u) = dirs[i] else { continue };
            let s = &segs[i];
            // along the line: where its ends are, and its depth at a place
            let along = |p: &V3| p[0] * u[0] + p[1] * u[1];
            let (ia, ib) = (along(&s[0]), along(&s[1]));
            if (ib - ia).abs() < 1e-9 {
                continue;
            }
            let depth = |s: &[V3; 2], t: f64| {
                let (a, b) = (along(&s[0]), along(&s[1]));
                let f = if (b - a).abs() < 1e-12 { 0.0 } else { (t - a) / (b - a) };
                s[0][2] + (s[1][2] - s[0][2]) * f
            };
            let mut hid: Vec<(f64, f64)> = Vec::new();
            for &j in ks.iter().filter(|&&j| j != i && owner[j] != owner[i] && covers(owner[j])) {
                let o = &segs[j];
                let (ja, jb) = (along(&o[0]), along(&o[1]));
                let (lo, hi) = (ia.min(ib).max(ja.min(jb)), ia.max(ib).min(ja.max(jb)));
                if hi - lo < 1e-6 {
                    continue;
                }
                let mid = (lo + hi) * 0.5;
                if depth(o, mid) > depth(s, mid) + 1e-3 {
                    let (t0, t1) = ((lo - ia) / (ib - ia), (hi - ia) / (ib - ia));
                    hid.push((t0.min(t1).clamp(0.0, 1.0), t0.max(t1).clamp(0.0, 1.0)));
                }
            }
            if hid.is_empty() {
                continue;
            }
            hid.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut left = Vec::new();
            let mut t = 0.0;
            for (a, b) in hid {
                if a > t + 1e-9 {
                    left.push((t, a));
                }
                t = t.max(b);
            }
            if t < 1.0 - 1e-9 {
                left.push((t, 1.0));
            }
            out[i] = left;
        }
    }
    out
}

/// Keynotes numbered by sheet, as Revit does: the distinct keys shown, in the order their
/// first keynote was placed (element id), 1, 2…
fn number_keys(out: &mut Sheet2d) {
    let keys = key_order(out, false);
    // (the keynotes of tags not shown only took their numbers)
    out.texts.retain(|t| t.height > 0.0 || marked_key(&t.text).is_none());
    for t in out.texts.iter_mut() {
        if let Some((_, k)) = marked_key(&t.text) {
            t.text = keys.iter().position(|x| *x == k).map_or(String::new(), |i| (i + 1).to_string());
        }
    }
}

/// A keynote's marked text: mark, the keynote's id (20 digits), mark, its key.
fn marked_key(t: &str) -> Option<(String, String)> {
    t.strip_prefix(KEY_MARK).and_then(|r| r.split_once(KEY_MARK)).map(|(id, k)| (id.to_string(), k.to_string()))
}

/// The keys of the keynotes of a sheet so far, in the order they are numbered: of its tags
/// drawn, or `shown` false, also of those not shown (orphaned, numbered all the same).
fn key_order(out: &Sheet2d, shown: bool) -> Vec<String> {
    let mut placed: Vec<(String, String)> = out.texts.iter().filter(|t| !shown || t.height > 0.0).filter_map(|t| marked_key(&t.text)).collect();
    placed.sort();
    let mut keys: Vec<String> = Vec::new();
    for (_, k) in placed {
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    keys
}

/// Workset and family name of an element, as view filters test them (`ELEM_PARTITION_PARAM`,
/// `SYMBOL_FAMILY_NAME_PARAM`).
const WORKSET: i64 = -1002053;
const SYMBOL_FAMILY_NAME: i64 = -1002002;

/// Keynote number shown by keynote tags (`KEYNOTE_NUMBER`).
const KEYNOTE_NUMBER: i64 = -1140421;
/// Categories drawn in their MEP system's colour: ducts, pipes, flex ducts and pipes, their
/// fittings and accessories, air terminals (not equipment, often on several systems).
/// The categories whose elements hidden by others of them are drawn as hidden lines (ducts,
/// pipes, their fittings, accessories, flexible ones and insulation).
const MEP_HIDDEN_LINES: [i64; 10] = [-2008000, -2008010, -2008016, -2008020, -2008044, -2008049, -2008050, -2008055, -2008123, -2008122];
/// Hidden lines (`<Hidden>`, "Lignes cachées"), and a dashed pattern when it has none.
const HIDDEN_LINES: i64 = -2000066;
const HIDDEN_PATTERN: i64 = -3000011;
/// Ducts, flexible ducts, pipes, flexible pipes.
const DUCTS: i64 = -2008000;
const FLEX_DUCTS: i64 = -2008020;
const PIPES: i64 = -2008044;
const FLEX_PIPES: i64 = -2008050;
/// The categories a plan shows above its cut plane, up to its top clip plane (seen in Revit:
/// ducts, pipes, their fittings, accessories, insulation, flexible ones, air terminals, and
/// the like: equipment, cable trays, conduits, sprinklers; a family nested in one of them, as
/// its host shows: a sprinkler's ghost symbol, a generic model).
const ABOVE_CUT: [i64; 20] = [
    -2008000, -2008010, -2008016, -2008020, -2008013, -2008123, -2008124, -2008044, -2008049, -2008050, -2008055, -2008122, -2001140, -2008130, -2008126, -2008132, -2008128, -2008099, -2008001, -2008066,
];
/// Plumbing fixtures (`OST_PlumbingFixtures`).
const PLUMBING_FIXTURES: i64 = -2001160;
/// Sprinklers (`OST_Sprinklers`).
const SPRINKLERS: i64 = -2008099;
/// A sheet's current revision (`SHEET_CURRENT_REVISION`).
const SHEET_CURRENT_REVISION: i64 = -1007412;
/// Revisions (`OST_Revisions`), what a revision schedule lists.
const REVISIONS: i64 = -2006070;
/// A duct's centre line and the rise / drop symbols of ducts and pipes (subcategories).
const DUCT_CENTER_LINE: i64 = -2008001;
const DUCT_RISE_DROP: i64 = -2008062;
const PIPE_RISE_DROP: i64 = -2008069;
/// A text type's tab stops (`TEXT_TAB_SIZE`).
const TEXT_TAB_SIZE: i64 = -1006326;
/// Detail items (`OST_DetailComponents`).
const DETAIL_ITEMS: i64 = -2002000;
/// Generic annotations (`OST_GenericAnnotation`).
const GENERIC_ANNOTATIONS: i64 = -2000150;
/// Grids (`OST_Grids`).
const GRIDS: i64 = -2000220;
/// How far past a cropped view's annotation crop its datums' model ends are cut (paper feet:
/// 1/128"; 1 1/128" past the crop with Revit's default 1" annotation crop).
const DATUM_PAST_ANNOT: f64 = 1.0 / 128.0 / 12.0;
/// Keynote tags (`OST_KeynoteTags`): the category of keynote legends.
const KEYNOTE_TAGS: i64 = -2005029;
/// Mechanical equipment (`OST_MechanicalEquipment`).
const MECHANICAL_EQUIPMENT: i64 = -2001140;
pub const SYSTEM_COLORED: [i64; 9] = [-2008000, -2008010, -2008016, -2008020, -2008044, -2008049, -2008050, -2008055, -2008013];
/// System abbreviation and size of MEP elements, computed by Revit for tags.
const SYSTEM_ABBREVIATION: i64 = -1150468;
const MEP_SIZE: i64 = -1114240;
const PIPE_DIAMETER: i64 = -1140225;
/// A view range's top set to unlimited (feet above anything modelled).
const UNLIMITED: f64 = 1e6;
/// A cable tray's width (mm).
const CABLE_TRAY_WIDTH: i64 = -1140122;
/// The air flow of an air terminal (its connector's).
const TERMINAL_FLOW: i64 = -1013405;
/// A room's or space's area, computed by Revit for tags.
const ROOM_AREA: i64 = -1006902;
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
    /// What comes before each of a label's parameters: its leading spaces, or a line break.
    seps: Vec<(usize, bool)>,
    /// Each parameter's own format (unless it says the project's), whether it shows its unit.
    fmts: Vec<Option<(crate::rvt_annot::Fmt, bool)>>,
    /// The parts of its text written their own way (rich text), covering it.
    spans: Vec<sw_format::TextSpan>,
}

impl RawText {
    /// Its horizontal alignment, 0 / 1 / 2 left / centre / right: its flags 64 / 128 / 256 (the
    /// low bits 0 / 1 / 2 say how it reads in its own document: turned there, the other side,
    /// P-608's "Th" and "M" right of their anchor in a family, left in it).
    fn halign(&self) -> u8 {
        match self.flags & 0x1c0 {
            0x100 => 2,
            0x80 => 1,
            0x40 => 0,
            _ => (self.flags & 3).min(2) as u8,
        }
    }

    /// A label's values (the index of their parameter, their text) put together as the label
    /// sets them: each after its parameter's line break or leading spaces.
    fn join(&self, vals: &[(usize, String)]) -> String {
        let mut s = String::new();
        for (n, (i, v)) in vals.iter().enumerate() {
            if n > 0 {
                match self.seps.get(*i) {
                    Some(&(_, true)) => s.push('\n'),
                    Some(&(sp, false)) => s.push_str(&" ".repeat(sp)),
                    None => s.push(' '),
                }
            }
            s += v;
        }
        s
    }
}

fn raw_text(o: &Obj, tr: &Tree) -> Option<RawText> {
    let orig = v3(o.fs("m_orig"))?;
    let base = v3(o.fs("m_baseVec")).unwrap_or([1.0, 0.0, 0.0]);
    let height = num(o, "m_height").filter(|h| *h > 0.0 && *h < 10.0)?;
    let flags = o.int("m_flags").unwrap_or(0);
    let (text, bold, spans) = o
        .ptr("m_oRtfData")
        .or_else(|| o.ptr("m_oSavedRtfData"))
        .and_then(|i| tr.objs.get(i))
        .and_then(|r| match r.get("m_arrRtfStream") {
            Some(G::Is(v)) => {
                let bytes: Vec<u8> = v.iter().map(|&x| x as u8).collect();
                let parsed = rtf_parse(&bytes);
                // RTF_DEBUG=<part of a text>: the RTF of the texts holding it
                if std::env::var("RTF_DEBUG").is_ok_and(|q| parsed.0.contains(q.as_str())) {
                    eprintln!("RTF in {} {:?}: {}", tr.id, parsed.0, String::from_utf8_lossy(&bytes));
                }
                Some(parsed)
            }
            _ => None,
        })
        .unwrap_or_default();
    if let Some(q) = std::env::var_os("PARAM_DEBUG") {
        let q = q.to_string_lossy().to_string();
        if o.list("m_paramDataFormattingArr").iter().any(|p| text_of(p, "m_sampleText").is_some_and(|t| t.contains(q.as_str()))) {
            for p in o.list("m_paramDataFormattingArr") {
                eprintln!("PARAM {:?}", p.fields);
            }
        }
    }
    let params = o
        .list("m_paramDataFormattingArr")
        .iter()
        .map(|p| (p.int("m_paramId").unwrap_or(-1), text_of(p, "m_prefix").unwrap_or_default(), text_of(p, "m_suffix").unwrap_or_default(), text_of(p, "m_sampleText").unwrap_or_default()))
        .collect();
    let seps = o.list("m_paramDataFormattingArr").iter().map(|p| (p.int("m_leadingSpaces").unwrap_or(1).clamp(0, 20) as usize, p.int("m_bLeadingLineBreak").unwrap_or(0) != 0)).collect();
    let fmts = o.list("m_paramDataFormattingArr").iter().map(|p| p.obj("m_formatOptions").and_then(|f| crate::rvt_annot::Fmt::of(f).map(|x| (x, f.int("m_unitSymbol").is_some_and(|s| s != 0))))).collect();
    let wrap = if o.int("m_bNoWrapping").unwrap_or(0) != 0 { 0.0 } else { num(o, "m_lineWidth").filter(|w| w.is_finite() && *w > 0.0).unwrap_or(0.0) };
    let xyz = |l: &Obj, k: &str| v3(l.fs(k)).or_else(|| l.obj(k).and_then(|x| v3(x.fs("m_xyz"))));
    let leaders = o.ptrs("m_leaders").iter().filter_map(|&i| tr.objs.get(i)).filter_map(|l| Some([xyz(l, "m_anchor")?, xyz(l, "m_elbow")?, xyz(l, "m_end")?])).collect();
    let width = num(o, "m_trueWidth").filter(|w| w.is_finite() && *w > 0.0).unwrap_or(0.0);
    Some(RawText { orig, base, height, flags, attr: o.int("m_attrId").unwrap_or(-1), bold, wrap, width, leaders, text, params, seps, fmts, spans })
}

/// A schedule's stored table: title / header sections (rows heights, column widths, cells).
#[derive(Clone, Debug)]
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
    /// Its appearance (`ScheduleDefinition`): grid lines, outline, column headers, a blank
    /// row before the body (true for stored tables without these settings).
    grid: bool,
    outline: bool,
    headers: bool,
    spacer: bool,
    /// The look of its cells by (section, row, column), and of its body's columns.
    looks: HashMap<(usize, usize, usize), CellLook>,
    body_looks: Vec<CellLook>,
    /// A keynote legend: its rows are the keynotes (of the sheet it is on, when filtered).
    keynotes: Option<bool>,
    /// Its columns' fields (`ScheduleField.m_paramElemId`, hidden ones left out), and whether it
    /// lists revisions (`OST_Revisions`): its rows are then those of the sheet it is on.
    fields: Vec<i64>,
    revisions: bool,
    /// Merged cells (`m_cover*` of each cell): an area's first cell (section, row, column) ->
    /// how many more columns and rows it covers; the other cells of an area; whether the
    /// table says how its cells merge at all.
    spans: HashMap<(usize, usize, usize), (usize, usize)>,
    covered: crate::det::HashSet<(usize, usize, usize)>,
    merges: bool,
}

/// How a schedule cell is written (`TableCellStyle`): text size (points), bold, italic,
/// underlined (`m_flag` 4, 2, 8), horizontal alignment (0 left, 1 centre, 2 right),
/// background, text colour, text angle (degrees).
#[derive(Clone, Debug, Default)]
struct CellLook {
    size: f64,
    bold: bool,
    italic: bool,
    underline: bool,
    font: String,
    halign: i64,
    /// Where its text stands in its cell: 0 top, 4 middle, 8 bottom (Revit's flags).
    valign: i64,
    background: Option<[u8; 3]>,
    color: [u8; 3],
    angle: f64,
}

/// The look a `TableCellStyle` gives.
fn cell_look(o: &Obj) -> CellLook {
    let rgb = |c: i64| [(c & 255) as u8, ((c >> 8) & 255) as u8, ((c >> 16) & 255) as u8];
    let bg = o.int("m_backgroundColor").filter(|c| (0..0xffffff).contains(c)).map(rgb);
    let flag = o.int("m_flag").unwrap_or(0);
    CellLook {
        size: num(o, "m_textSize").unwrap_or(0.0),
        bold: flag & 4 != 0,
        italic: flag & 2 != 0,
        underline: flag & 8 != 0,
        font: text_of(o, "m_fontName").map(|f| f.trim().to_string()).unwrap_or_default(),
        halign: o.int("m_horizontalAlignment").unwrap_or(1),
        valign: o.int("m_verticalAlignment").unwrap_or(4),
        background: bg,
        color: o.int("m_textColor").filter(|c| (0..1 << 24).contains(c)).map_or([0; 3], rgb),
        // (tenths of a degree, as Windows fonts turn: AC-101's headings, 900, read upwards)
        angle: num(o, "m_textOrientation").unwrap_or(0.0) / 10.0,
    }
}

impl Default for Table {
    fn default() -> Table {
        Table {
            width: 0.0,
            sections: Vec::new(),
            name: String::new(),
            top_down: false,
            fixed: None,
            pitch: 0.0,
            grid: true,
            outline: true,
            headers: true,
            spacer: false,
            looks: HashMap::default(),
            body_looks: Vec::new(),
            keynotes: None,
            fields: Vec::new(),
            revisions: false,
            spans: HashMap::default(),
            covered: Default::default(),
            merges: false,
        }
    }
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
    if let Some(sc) = schema {
        t.fields = sc.ptrs("m_pFields").iter().filter_map(|&i| tr.objs.get(i)).filter(|f| f.int("m_hidden").unwrap_or(0) == 0).map(|f| f.int("m_paramElemId").unwrap_or(-1)).collect();
        t.revisions = sc.int("m_categoryId") == Some(REVISIONS);
    }
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
        let si = t.sections.len();
        for rc in sec.ptrs("m_tableCells").iter().filter_map(|&i| tr.objs.get(i)) {
            for c in rc.ptrs("m_arrCells").iter().filter_map(|&i| tr.objs.get(i)) {
                let txt = text_of(c, "m_text").unwrap_or_default();
                let (row, col) = (c.int("m_row").unwrap_or(0).max(0) as usize, c.int("m_column").unwrap_or(0).max(0) as usize);
                if let Some(st) = c.ptr("m_oTableCellStyle").and_then(|i| tr.objs.get(i)) {
                    t.looks.insert((si, row, col), cell_look(st));
                }
                // its merged area: how far it reaches left, right, up and down
                if let (Some(l), Some(r), Some(u), Some(d)) = (c.int("m_coverLeft"), c.int("m_coverRight"), c.int("m_coverTop"), c.int("m_coverBottom")) {
                    t.merges = true;
                    if l < 0 || u < 0 {
                        t.covered.insert((si, row, col));
                    } else if r > 0 || d > 0 {
                        t.spans.insert((si, row, col), (r.max(0) as usize, d.max(0) as usize));
                    }
                }
                if !txt.is_empty() {
                    cells.push((row, col, txt));
                }
            }
        }
        // its grand total row ("Total général: 64"), stored with the rows Revit last
        // computed: shown only with rows of its own (a list whose rows are not stored here,
        // a list of sheets, shows no stale total)
        let total = tr.objs.iter().find_map(|o| match o.get("m_grandTotalTitle") {
            Some(G::S(t)) if !t.trim().is_empty() => Some(t.trim().to_string()),
            _ => None,
        });
        if let (Some(total), false) = (total, sec.int("m_index").unwrap_or(0) == 0) {
            let is_total = |c: &(usize, usize, String)| c.2.trim_start().starts_with(total.as_str());
            let data = cells.iter().any(|c| c.0 > 0 && !is_total(c));
            if !data {
                cells.retain(|c| !is_total(c));
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
    id: i64,
    /// Its family (`m_familyId`): the images of the family document belong to it.
    family: i64,
    curves: Vec<Vec<V3>>,
    /// The line style of each of its curves (its reference lines: the reference lines' one,
    /// in the order of the family document's).
    curve_styles: Vec<i64>,
    texts: Vec<RawText>,
    tags: Vec<RawText>,
    /// Schedules drawn by the symbol (revision schedule of a title block): (position, table).
    /// Its schedules: where they stand, their table, their quarter turns counter-clockwise
    /// (`m_rotation`: 2 a quarter turn counter-clockwise, 1 clockwise).
    tables: Vec<(V3, Table, i64)>,
    outline: Option<[f64; 4]>,
    /// Its text parameters.
    strs: HashMap<i64, String>,
    /// Its yes/no and integer family parameters.
    ints: HashMap<i64, i64>,
    reals: HashMap<i64, f64>,
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
    /// The annotation crop's offsets, switched on or not (Revit's default: 1").
    annot_off: [f64; 4],
}

#[derive(Default, Debug, Clone)]
struct ViewInfo {
    name: String,
    /// Its title on sheets (`m_viewDescription`, "Titre sur la feuille"), when set.
    title: String,
    /// Its detail level (`VIEW_DETAIL_LEVEL`: 1 coarse, 2 medium, 3 fine; 0 unknown).
    detail: i64,
    /// The detail level it sets some categories to (`m_detailLevels`: category, level).
    cat_detail: Vec<(i64, i64)>,
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
    hidden: crate::det::HashSet<i64>,
    hidden_cats: crate::det::HashSet<i64>,
    /// Workset visibility set in this view (`WorksetVisibilityViewSettings`: workset GUID →
    /// hidden), and the worksets it hides once resolved against the global setting.
    ws_set: HashMap<[u8; 16], bool>,
    hidden_ws: crate::det::HashSet<u32>,
    /// Paper size of a model foot (1 / `m_scale`).
    scale: f64,
    /// The view it depends on (`m_primaryViewId`), whose annotations it shows.
    primary: i64,
    /// Plan regions it shows (`DBViewPlanViewDepthCutter.m_planRegionFaceTagMap`).
    regions: Vec<i64>,
    /// Its filters, in priority order (`FilterOverrides.m_oaSettings`).
    filters: Vec<FilterUse>,
    /// Elements drawn in the view with graphics of their own (Override Graphics in View, by
    /// element: `AdHocOverrides`).
    elem_looks: HashMap<i64, Look>,
    /// Its view template (`m_viewTemplateId`), which then sets its view range.
    template: i64,
    /// The view's level (`m_genElemId`) and its viewer element (`m_viewerId`).
    level: i64,
    viewer: i64,
    /// Its view type (`m_dbViewTypeId`).
    vtype: i64,
    strs: HashMap<i64, String>,
}

/// A view filter (`ParameterFilterElement`): the categories it applies to and its rules, all
/// of which an element must pass. A rule combination it cannot read makes it apply to nothing.
#[derive(Debug, Default)]
struct Filter {
    cats: crate::det::HashSet<i64>,
    rules: Vec<Rule>,
    unreadable: bool,
}

/// A filter rule: a parameter of the element (or of its type) compared with a value.
#[derive(Debug)]
struct Rule {
    param: i64,
    /// Evaluator class (`FilterStringEquals`, `FilterNumericGreater`…).
    op: String,
    value: RuleValue,
    inverse: bool,
}

#[derive(Debug)]
enum RuleValue {
    Int(i64),
    Num(f64),
    Str(String),
}

/// What a filter does in a view (`FilterGraphicSettings`): shown or not, and its projection
/// line colour, weight (pen) and pattern, projection fill (pattern, colour) and halftone.
#[derive(Debug, Clone)]
struct FilterUse {
    filter: i64,
    visible: bool,
    line: Option<[u8; 3]>,
    pen: Option<i64>,
    pattern: Option<i64>,
    fill: Option<(i64, [u8; 3])>,
    halftone: bool,
}

/// The graphics the filters of a view give an element.
#[derive(Debug, Clone, Default)]
struct Look {
    hidden: bool,
    line: Option<[u8; 3]>,
    pen: Option<i64>,
    pattern: Option<i64>,
    fill: Option<(i64, [u8; 3])>,
    halftone: bool,
}

/// A colour halftoned as Revit draws it on white paper (halfway to white).
fn halftone(c: [u8; 3]) -> [u8; 3] {
    c.map(|x| ((x as u16 + 255) / 2) as u8)
}

/// A grid line (`Grid`): its ends in plan (feet), the heights its plane spans, its type.
#[derive(Debug)]
struct Grid {
    id: i64,
    name: String,
    a: V3,
    b: V3,
    z: (f64, f64),
    attr: i64,
    /// its plane's origin and direction; `flip`: the datum's own parameter runs against it
    o: V3,
    x: V3,
    flip: bool,
    /// its plane's up (`m_yVec`, +1 or -1 along z): a section measures its ends that way
    up: f64,
}

/// A view reference: its owner view, point and direction (model feet), type and the view it
/// points to.
#[derive(Debug, Clone)]
struct RefViewer {
    id: i64,
    owner: i64,
    at: V3,
    dir: V3,
    attr: i64,
    target: i64,
}

/// A view's mark in other views (its viewer of kind `Section`): a callout's boundary or a
/// section's line. What it marks (`m_dbViewId`), the view owning it (a reference's own) and
/// the view it was drawn in (`m_parentDBViewId`), whether it is a reference to an existing
/// view, shown in that view only, hidden at scales coarser than `coarser`; its crop, a
/// sketched crop's outline (`m_nonRectangularCropAreaCurvesLoops`, model feet), its section
/// line (`m_frontCut`) and, by view, the point its leader reaches at its head.
#[derive(Debug, Clone)]
struct ViewMark {
    id: i64,
    target: i64,
    owner: i64,
    parent: i64,
    reference: bool,
    coarser: f64,
    crop: Crop,
    shape: Vec<Vec<[V3; 2]>>,
    front: Option<(V3, V3)>,
    /// A callout's head in each view showing it (`m_viewDataMapForCallout`): view, head point,
    /// its leader's end on the boundary and elbow unless the leader is straight.
    heads: Vec<(i64, V3, Option<(V3, V3)>)>,
    /// How it was made (`m_sectionCreationMethod`: 201 a section, 202 a callout).
    method: i64,
    /// Shown in the view it was drawn in only (`m_showInOneViewOnly`).
    one_view: bool,
}

/// A section's ends as one view shows it (`SectionGraphicsViewDataRec`): distances along its
/// front cut from its start (feet), the mark at each end (1 its head, 2 its tail, else none),
/// its line's middle left out, the lengths of line kept at each end then (else the default).
#[derive(Debug, Clone, Copy)]
struct SectionEnds {
    ends: [f64; 2],
    marks: [i64; 2],
    broken: bool,
    kept: [f64; 2],
}

/// A datum as one view shows it (`DatumPlaneRec`): its free end (end 1) and bubble end (end 2)
/// as parameters along the datum, in 2D (this view only) and 3D, and the view's overrides
/// (`m_endBubbleOverrides`: bits 0-1 free end bubble, 2-3 bubble end bubble, 1 shown, 2
/// hidden; bit 4 free end in 2D, bit 5 bubble end in 2D), and the elbows the view gives its
/// heads (`m_pFreeEndLeader`, `m_pLeader`: where the line leaves the datum, its elbow, where
/// the head stands; model feet).
#[derive(Debug, Clone, Copy)]
struct DatumRec {
    free: f64,
    bubble: f64,
    free3: f64,
    bubble3: f64,
    flags: i64,
    elbows: [Option<[V3; 3]>; 2],
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
    /// its type (`ViewportAttributes`)
    attr: i64,
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
        annot_off: [1.0 / 12.0; 4],
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

/// Clip coordinates `c` moved within the clip's depth (its finite limits).
fn depth_within(k: &Clip, mut c: V3) -> V3 {
    if k.lo[2].is_finite() {
        c[2] = c[2].max(k.lo[2] + 1e-4);
    }
    if k.hi[2].is_finite() {
        c[2] = c[2].min(k.hi[2] - 1e-4);
    }
    c
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
    /// Revit's own words (a room "Not Enclosed"…) in French: the file was saved in French.
    /// shortcut: Revit writes them in its UI language; the file's save locale stands in for it.
    pub french: bool,
    symbols: HashMap<i64, Sym>,
    /// (owner view, symbol, transform)
    instances: Vec<(i64, i64, Trf)>,
    /// Text parameters of the placed 2D families, by index in `instances`.
    inst_strs: Vec<HashMap<i64, String>>,
    /// Yes/no and integer family parameters of the placed 2D families, same index.
    inst_ints: Vec<HashMap<i64, i64>>,
    /// Number family parameters of the placed 2D families (their real value), same index.
    inst_reals: Vec<HashMap<i64, f64>>,
    /// Element ids of the placed 2D families, same index.
    inst_ids: Vec<i64>,
    /// Flips of the placed 2D families (`m_flippedX`, `m_flippedY`), same index.
    inst_flips: Vec<(bool, bool)>,
    /// The family document holding each placed family (`m_famId`; -1 in the project), same
    /// index: a family nested in another is drawn with it.
    inst_fams: Vec<i64>,
    /// Whether each placed family shows, following its family's parameters (nested ones).
    inst_vis: Vec<crate::rvt_annot::Visible>,
    /// The parameters of each placed family its host family sets (one nested in a family
    /// document): (its parameter, the host family document's; -1: its type), same index.
    inst_drives: Vec<Vec<(i64, i64)>>,
    /// Text and number parameters of model families placed in the project (their yes/no and
    /// integers in `model_insts`).
    model_vals: HashMap<i64, (HashMap<i64, String>, HashMap<i64, f64>)>,
    /// Element parameters of model families placed in the project and of types: parameter ->
    /// element (a family type parameter's value: a `NestedFamSymRefElem`).
    elem_params: HashMap<i64, HashMap<i64, i64>>,
    /// Types' names (`SymbolInfo.m_name`).
    sym_names: HashMap<i64, String>,
    /// `NestedFamSymRefElem`: the nested family type it names (family name, type name).
    nested_refs: HashMap<i64, (String, String)>,
    /// (family, type name) -> type, once drawing starts.
    syms_by_name: std::sync::OnceLock<HashMap<(i64, String), i64>>,
    /// Each sheet's revisions (`m_revNumMap`): (revision, its number on the sheet).
    sheet_revs: HashMap<i64, Vec<(i64, String)>>,
    /// The project's revisions (`ProjectRevision`): their sequence, and date, description,
    /// issued by, issued to.
    revisions: HashMap<i64, (i64, [String; 4])>,
    /// model families placed in the project: their type, placement (with their flips) and yes/no
    /// parameters; each type's yes/no parameters
    model_insts: HashMap<i64, (i64, Trf, HashMap<i64, i64>)>,
    /// a shared family nested in another, placed in the project -> its host's instance
    /// (`m_superInstanceId`)
    hosts: HashMap<i64, i64>,
    sym_ints: HashMap<i64, HashMap<i64, i64>>,
    /// insulation: (element, thickness in feet, its category), and once drawing starts, the
    /// insulation of each duct or pipe
    insulations: Vec<(i64, f64, i64)>,
    insulated: std::sync::OnceLock<HashMap<i64, (f64, i64)>>,
    /// family document -> the families placed in it (index), once drawing starts
    nested_by: std::sync::OnceLock<HashMap<i64, Vec<usize>>>,
    /// (view, datum) -> how the view shows it
    datum_recs: HashMap<(i64, i64), DatumRec>,
    level_names: HashMap<i64, String>,
    grids: Vec<Grid>,
    /// grid type -> bubble at its start, at its end
    grid_bubbles: HashMap<i64, (bool, bool)>,
    /// grid type -> where its bubbles show in sections and elevations (`m_bubbleLocInElev`: 1
    /// bottom, 2 top, 3 both)
    grid_elev_bubbles: HashMap<i64, i64>,
    /// grid type -> the category giving its lines' style
    grid_lines: HashMap<i64, i64>,
    /// element -> its MEP system, system -> its type, type -> its line colour
    elem_systems: HashMap<i64, i64>,
    /// elements joined to several MEP systems (equipment): no system colour of their own
    multi_system: crate::det::HashSet<i64>,
    system_types: HashMap<i64, i64>,
    system_colors: HashMap<i64, [u8; 3]>,
    /// system type -> the line weight (pen) and pattern its graphics set, when set
    system_lines: HashMap<i64, (Option<i64>, Option<i64>)>,
    /// element -> the elements its connectors are joined to
    elem_links: HashMap<i64, Vec<i64>>,
    /// element -> the flow its connector carries (cubic feet a second)
    elem_flows: HashMap<i64, f64>,
    /// view filters; family -> name; family type -> family; MEP curve -> its text values
    filters: HashMap<i64, Filter>,
    fam_names: HashMap<i64, String>,
    sym_family: HashMap<i64, i64>,
    curve_strs: HashMap<i64, HashMap<i64, String>>,
    /// system type -> its abbreviation; MEP curve -> its size as written in tags
    system_abbr: HashMap<i64, String>,
    mep_sizes: HashMap<i64, (i64, Option<i64>)>,
    /// MEP curve -> its size as Revit wrote it
    mep_size_texts: HashMap<i64, String>,
    /// MEP curve -> its direction (model); tag families turned with what they tag
    mep_dirs: HashMap<i64, V3>,
    /// MEP curve -> its ends (model feet), its diameter (mm, as tags write it)
    mep_lines: HashMap<i64, (V3, V3)>,
    mep_diams: HashMap<i64, String>,
    /// MEP curve -> its width seen from above (feet)
    mep_widths: HashMap<i64, f64>,
    /// duct -> its section's height (feet) and up (`m_vNormal`), and which way its air flows
    /// along its line (1 from its start, -1 from its end, 0 unknown: its connectors' `m_eDirection`
    /// at its ends, 1 in, 2 out)
    duct_sections: HashMap<i64, (f64, V3, i8)>,
    turning_tags: crate::det::HashSet<i64>,
    /// Plan regions: (owner view, id, (outline, bottom, top)), absolute feet.
    plan_regions: Vec<(i64, i64, (Vec<P2>, f64, f64, f64))>,
    /// level -> corners of its plane (`m_Envelope`: its extent, feet) and its type
    level_env: HashMap<i64, [f64; 4]>,
    /// level -> its plane's origin, from which a view measures its ends (`DatumPlaneRec`)
    level_origin: HashMap<i64, V3>,
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
    /// Pictures placed in views: (view, image type, centre, base direction, width, height),
    /// model feet.
    view_images: Vec<(i64, i64, V3, V3, f64, f64, bool)>,
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
    boxed_types: crate::det::HashSet<i64>,
    /// text types drawn bold (`m_lineAndTextAttr.m_bBold`)
    bold_types: crate::det::HashSet<i64>,
    /// text type -> italic, underlined, tab stops (paper feet; 0 unknown)
    type_faces: HashMap<i64, (bool, bool, f64)>,
    /// font element -> its typeface
    font_names: HashMap<i64, String>,
    holders: HashMap<i64, Vec<u8>>,
    holder_bytes: usize,
    project: HashMap<i64, String>,
    levels: Vec<f64>,
    /// level id → elevation (feet)
    level_z: HashMap<i64, f64>,
    /// 3D section boxes (`ModelClipBox` switched on), by viewer element id
    section_boxes: HashMap<i64, Crop>,
    /// scope boxes (`VolumeOfInterest`): id, box, the views it is explicitly shown in
    scope_boxes: Vec<(i64, Crop, Vec<i64>)>,
    /// view annotations (detail lines, filled regions, dimensions, tags)
    ann: crate::rvt_annot::Annots,
    /// Worksets hidden in the views that do not set them (`WorksetVisibilitySettingsElem`).
    global_hidden_ws: crate::det::HashSet<[u8; 16]>,
    /// element id → its workset (partition id)
    elem_ws: HashMap<i64, u32>,
    /// `Family` element -> root of its family document, and what those documents draw.
    fam_roots: HashMap<i64, i64>,
    /// family instance -> how its family was fitted to its sizes
    stretches: HashMap<i64, crate::rvt_shape::Stretch>,
    fam_drawn: std::sync::Mutex<HashMap<(i64, i64, Vec<(i64, bool)>), std::sync::Arc<crate::rvt_annot::Drawn>>>,
    /// viewport type -> (title shown, its extension line shown)
    vp_types: HashMap<i64, (bool, bool)>,
    /// view references (`ReferenceViewer`), and each sheet's number (once drawing starts)
    ref_viewers: Vec<RefViewer>,
    /// views' marks in other views (callouts, sections)
    marks: Vec<ViewMark>,
    /// family -> its category (`m_categoryId`)
    fam_cats: HashMap<i64, i64>,
    /// colour fill legends (`ColorFillLegend`): view, origin (model feet), type, category
    cf_legends: Vec<(i64, V3, i64, i64)>,
    /// colour fill legend types (`ColorFillSymbol`): rows' and title's (font, bold), swatch
    /// width and height (paper feet), title shown
    cf_types: HashMap<i64, ((i64, bool), (i64, bool), [f64; 2], bool)>,
    /// colour schemes (`ColorFillSchema`): title, parameter, area scheme, its values shown
    /// (colour, value) in order
    cf_schemes: HashMap<i64, (String, i64, i64, Vec<([u8; 3], String)>)>,
    /// view -> its colour schemes by category (`ColorFillSchemaSetting.m_categoryToSchema`)
    view_schemes: HashMap<i64, Vec<(i64, i64)>>,
    /// rooms, areas, spaces (`RoomElem`): their level, area scheme and point (model feet)
    spatials: Vec<(i64, i64, i64, [f64; 2])>,
    /// linked models' instances (`RvtLinkInstance`): their elements are not in this file
    link_insts: crate::det::HashSet<i64>,
    /// view type -> its section and callout tag types, its reference label, its view family
    /// (`m_systemFamilyIdx`) and its name (`DBViewType`)
    view_types: HashMap<i64, (i64, i64, String, i64, String)>,
    /// callout tag type -> its corners' radius (paper feet) and head family type
    callout_tags: HashMap<i64, (f64, i64)>,
    /// section type (`SectionAttributes`) -> its head's and its tail's family types
    section_types: HashMap<i64, (i64, i64)>,
    /// (view, section) -> the section's ends in that view (`SectionGraphicsViewDataRec`)
    section_ends: HashMap<(i64, i64), SectionEnds>,
    /// a view's mark (section, callout) -> the view it marks
    mark_views: HashMap<i64, i64>,
    sheet_numbers: std::sync::Mutex<HashMap<i64, String>>,
    /// Images decoded once (a title block logo repeats on every sheet).
    decoded: std::sync::Mutex<HashMap<i64, Option<std::sync::Arc<Rgba>>>>,
}

const CLASSES: [&str; 37] = [
    "SectionAttributes",
    "VolumeOfInterest",
    "ParameterFilterElement",
    "Family",
    "FontElem",
    "PlanRegion",
    "Grid",
    "GridAttributes",
    "LevelAttributes",
    "ReferenceViewer",
    "ViewportAttributes",
    "ReferenceViewerAttributes",
    "ExtentElem",
    "FamilySymbol",
    "FamilyInstance",
    "Viewport",
    "DBViewType",
    "RvtLinkInstance",
    "CalloutTag",
    "CalloutAttributes",
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
    "NestedFamSymRefElem",
    "ProjectRevision",
    "ColorFillLegend",
    "ColorFillSymbol",
    "ColorFillSchema",
];
/// Most bytes of saved images kept.
const MAX_IMAGES: usize = 96 << 20;

fn strings(tr: &Tree) -> HashMap<i64, String> {
    let Some(set) = tr.root.ptr("m_pParamValueSetAString").and_then(|i| tr.objs.get(i)) else { return HashMap::default() };
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
            if s.isa(t, "DBView") || s.isa(t, "Viewer") || s.isa(t, "RbsCurve") || s.isa(t, "RbsSystem") || s.isa(t, "RbsSystemType") || s.isa(t, "RbsInsulation") {
                want[t as usize] = true;
            }
        }
        want
    }

    pub fn take(&mut self, s: &Schema, t: u16, tr: &Tree) {
        let r = &tr.root;
        if std::env::var("ELEM_CLASS").is_ok_and(|v| v.split(',').any(|x| x == tr.id.to_string())) {
            eprintln!("ELEM {} class {} fields {:?}", tr.id, s.name(t), r.fields.iter().map(|(k, g)| format!("{k}={}", format!("{g:?}").chars().take(120).collect::<String>())).collect::<Vec<_>>());
        }
        // a member of a group type not placed: not drawn
        if r.int("m_unplacedOwnerId").unwrap_or(-1) != -1 {
            return;
        }
        // MEP systems: the element's system, the system's type, the type's colour
        if s.isa(t, "RbsSystemType") {
            if let Some(G::S(a)) = r.get("m_abbreviation") {
                self.system_abbr.insert(tr.id, a.clone());
            }
            if let Some(o) = r.ptr("m_oSystemOverrides").and_then(|i| tr.objs.get(i)) {
                let gs = o.ptr("m_oOverrideGraphicSettings").and_then(|i| tr.objs.get(i));
                let c = gs.and_then(|g| g.ptr("m_oProjPenColor")).and_then(|i| tr.objs.get(i)).and_then(|c| c.int("m_color")).filter(|c| (0..1 << 24).contains(c));
                let pen = gs.and_then(|g| g.ptr("m_oProjPenNumber")).and_then(|i| tr.objs.get(i)).and_then(|p| p.int("m_penNumber")).filter(|n| (1..=16).contains(n));
                let pattern = gs.and_then(|g| g.ptr("m_oProjLinePattern")).and_then(|i| tr.objs.get(i)).and_then(|p| p.int("m_linePatternId")).filter(|id| *id != 0 && *id != -1);
                if pen.is_some() || pattern.is_some() {
                    self.system_lines.insert(tr.id, (pen, pattern));
                }
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
            let systems: std::collections::BTreeSet<i64> = tr.objs.iter().filter_map(|o| o.int("m_systemId").or_else(|| o.int("m_idSystem")).filter(|v| *v > 0)).collect();
            // an equipment joined to several systems takes the colour of none (nor passes one on)
            if systems.len() > 1 {
                self.multi_system.insert(tr.id);
            } else if let Some(&sys) = systems.first() {
                self.elem_systems.insert(tr.id, sys);
            }
            if let Some(f) = tr.objs.iter().filter(|o| s.name(o.tag) == "SegmentConnectorCalculation" || o.get("m_dFlow").is_some()).find_map(|o| num(o, "m_dFlow")) {
                self.elem_flows.insert(tr.id, f);
            }
            // what its connectors are joined to (a fitting takes their system)
            let links: Vec<i64> = tr.objs.iter().filter(|o| s.name(o.tag) == "Connector").flat_map(|o| o.list("m_arrRefs").iter()).filter_map(|r| r.int("m_id")).filter(|v| *v > 0 && *v != tr.id).collect();
            if !links.is_empty() {
                self.elem_links.insert(tr.id, links);
            }
        }
        if s.isa(t, "RbsInsulation") {
            if let Some(th) = num(r, "m_thickness").filter(|v| *v > 0.0) {
                let cat = if s.isa(t, "RbsDuctInsulation") { DUCT_INSULATION } else { PIPE_INSULATION };
                self.insulations.push((tr.id, th, cat));
            }
        }
        if s.isa(t, "RbsCurve") {
            self.curve_strs.insert(tr.id, strings(tr));
            if let Some(c) = r.ptr("m_pCurveDriver").and_then(|i| tr.objs.get(i)).and_then(|d| d.ptr("m_pCrv")).and_then(|i| tr.objs.get(i)) {
                if let (Some(o), Some(d)) = (v3(c.fs("m_origin")), v3(c.fs("m_dirVec"))) {
                    self.mep_dirs.insert(tr.id, d);
                    if let Some([t0, t1, ..]) = c.fs("m_endParams") {
                        let at = |t: f64| [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t];
                        self.mep_lines.insert(tr.id, (at(*t0), at(*t1)));
                    }
                }
            }
            if let (true, Some(h), Some(up)) = (s.isa(t, "RbsDuctCurve"), num(r, "m_dHeight").filter(|v| *v > 0.0), v3(r.fs("m_vNormal"))) {
                let by = |name: &'static str| tr.objs.iter().filter(move |o| s.name(o.tag) == name).filter_map(|o| match o.get("m_pConnector") {
                    Some(G::R(k)) => Some((*k, o)),
                    _ => None,
                });
                let at: HashMap<u32, f64> = by("SegmentConnectorPosition").filter_map(|(k, o)| Some((k, num(o, "m_paramOnCurve")?))).collect();
                let mut flow = 0i8;
                for (k, o) in by("SegmentConnectorCalculation") {
                    let (Some(p), Some(dir)) = (at.get(&k).copied(), o.int("m_eDirection")) else { continue };
                    let (start, end) = (p < 1e-6, p > 1.0 - 1e-6);
                    match dir {
                        1 if start => flow = 1,
                        2 if end => flow = 1,
                        2 if start => flow = -1,
                        1 if end => flow = -1,
                        _ => {}
                    }
                }
                self.duct_sections.insert(tr.id, (h, up, flow));
            }
            if let Some(d) = num(r, "m_dWidthOrDiameter").filter(|v| *v > 0.0) {
                self.mep_diams.insert(tr.id, format!("{}", (d * FEET * 1000.0).round() as i64));
                // outside diameter of a pipe when known (its design properties)
                let outer = tr.objs.iter().find_map(|o| num(o, "m_dOuterDiameter")).filter(|v| *v > 0.0);
                self.mep_widths.insert(tr.id, outer.unwrap_or(d));
            }
            // its size as Revit wrote it ("1 300x700", "Ø355", "DN25": its design properties'
            // calculated size), else from its width and height (mm)
            let stored = r.ptr("m_pDesignPropManager").and_then(|i| tr.objs.get(i)).and_then(|m| match m.get("m_sCalculatedSize") {
                Some(G::S(v)) if !v.trim().is_empty() => Some(v.clone()),
                _ => None,
            });
            if let Some(v) = stored {
                self.mep_size_texts.insert(tr.id, v);
            }
            let mm = |k: &str| num(r, k).filter(|v| *v > 0.0).map(|v| (v * FEET * 1000.0).round() as i64);
            let duct = s.name(t).contains("Duct");
            if let Some(w) = mm("m_dWidthOrDiameter") {
                let h = mm("m_dHeight").filter(|h| duct && *h != w);
                self.mep_sizes.insert(tr.id, (w, h));
            }
            return;
        }
        let owner = r.int("m_ownerDBViewId").unwrap_or(-1);
        let cls = s.name(t);
        self.ann.take(s, t, tr, || strings(tr));
        match cls {
            "FamilySymbol" => {
                if let Some(f) = r.int("m_familyId") {
                    self.sym_family.insert(tr.id, f);
                }
                let ints = family_ints(tr, s);
                if !ints.is_empty() {
                    self.sym_ints.insert(tr.id, ints);
                }
                let elems = family_elems(tr, s);
                if !elems.is_empty() {
                    self.elem_params.insert(tr.id, elems);
                }
                if let Some(n) = r.ptr("m_symbolInfo").and_then(|i| tr.objs.get(i)).and_then(|o| text_of(o, "m_name")) {
                    self.sym_names.insert(tr.id, n);
                }
                // (its reference lines apart: line style of the reference lines' category)
                let mut geo: Vec<(i64, i64, Vec<V3>)> = r
                    .ptrs("m_nonBRepGeomPtrs")
                    .iter()
                    .filter_map(|&i| tr.objs.get(i))
                    .map(|c| {
                        let g = c.obj("m_GInfo");
                        (g.and_then(|g| g.int("m_tag")).unwrap_or(-1), g.and_then(|g| g.int("m_categoryId")).unwrap_or(-1), curve_points(c, s))
                    })
                    .filter(|c| c.2.len() >= 2)
                    .collect();
                geo.sort_by_key(|c| c.0);
                let curve_styles: Vec<i64> = geo.iter().map(|c| c.1).collect();
                let curves: Vec<Vec<V3>> = geo.into_iter().map(|c| c.2).collect();
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
                        if std::env::var("TABLE_FIELDS").is_ok_and(|v| v == tr.id.to_string()) {
                            eprintln!("TABLE in {} fields {:?}", tr.id, si.fields.iter().map(|(k, g)| format!("{k}={}", format!("{g:?}").chars().take(160).collect::<String>())).collect::<Vec<_>>());
                        }
                        let Some(pos) = si.obj("m_position").and_then(|p| v3(p.fs("m_xyz"))) else { continue };
                        let Some(d) = si.int("m_dBViewScheduleDataIndex").and_then(|k| datas.get(k as usize)) else { continue };
                        if let Some(t) = symbol_table(d, tr) {
                            let turns = match si.int("m_rotation").unwrap_or(0) {
                                2 => 1,
                                1 => -1,
                                _ => 0,
                            };
                            tables.push((pos, t, turns));
                        }
                    }
                }
                self.symbols.insert(tr.id, Sym { id: tr.id, family: r.int("m_familyId").unwrap_or(-1), curves, curve_styles, texts, tags, tables, outline, strs: family_strs(tr, s), ints: family_ints(tr, s), reals: family_reals(tr, s) });
            }
            "FamilyInstance" if owner > 0 || r.int("m_famId").unwrap_or(-1) > 0 => {
                if std::env::var("PHASE_DEBUG").is_ok_and(|v| v == owner.to_string()) {
                    eprintln!("INST {} owner {owner} created {:?} demolished {:?}", tr.id, r.int("m_createdPhaseId"), r.int("m_demolishedPhaseId"));
                }
                let Some(ii) = r.ptr("m_pInstanceInfo").and_then(|i| tr.objs.get(i)) else { return };
                let Some(t) = ii.obj("m_Trf").and_then(Trf::from) else { return };
                self.instances.push((owner, ii.int("m_symbolId").unwrap_or(-1), t));
                self.inst_strs.push(family_strs(tr, s));
                self.inst_ints.push(family_ints(tr, s));
                self.inst_reals.push(family_reals(tr, s));
                self.inst_ids.push(tr.id);
                self.inst_flips.push((r.int("m_flippedX").unwrap_or(0) != 0, r.int("m_flippedY").unwrap_or(0) != 0));
                self.inst_fams.push(r.int("m_famId").unwrap_or(-1));
                self.inst_vis.push(crate::rvt_annot::Visible::of(tr, s));
                self.inst_drives.push(param_drives(tr, s));
            }
            "FamilyInstance" => {
                // a model family: where it stands (its lines are drawn in plans with it)
                let Some(ii) = r.ptr("m_pInstanceInfo").and_then(|i| tr.objs.get(i)) else { return };
                let Some(t) = ii.obj("m_Trf").and_then(Trf::from) else { return };
                let (sx, sy) = crate::rvt_shape::flip_signs(r.int("m_flippedX").unwrap_or(0) != 0, r.int("m_flippedY").unwrap_or(0) != 0);
                let t = t.after(&Trf::new([sx, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 1.0], [0.0; 3]));
                self.model_insts.insert(tr.id, (ii.int("m_symbolId").unwrap_or(-1), t, family_ints(tr, s)));
                let (strs, reals) = (family_strs(tr, s), family_reals(tr, s));
                if !strs.is_empty() || !reals.is_empty() {
                    self.model_vals.insert(tr.id, (strs, reals));
                }
                let elems = family_elems(tr, s);
                if !elems.is_empty() {
                    self.elem_params.insert(tr.id, elems);
                }
                if let Some(h) = r.int("m_superInstanceId").filter(|h| *h > 0) {
                    self.hosts.insert(tr.id, h);
                }
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
                    attr: r.int("m_attrId").unwrap_or(-1),
                });
            }
            "ViewportAttributes" => {
                // the family drawing its views' titles, whether the title shows, and its line
                if let Some(h) = r.int("m_familyTagId") {
                    self.level_heads.insert(tr.id, h);
                }
                self.vp_types.insert(tr.id, (r.int("m_showLabel").unwrap_or(1) != 0, r.int("m_showExtensionLine").unwrap_or(1) != 0));
            }
            "ScheduleInstance" if owner > 0 => {
                let Some(o) = r.obj("origin_XYZ").and_then(|o| v3(o.fs("m_xyz"))).or_else(|| v3(r.fs("m_origin"))) else { return };
                self.sched_inst.push((owner, o, r.int("m_scheduleId").unwrap_or(-1)));
            }
            "RvtLinkInstance" => {
                self.link_insts.insert(tr.id);
            }
            "ColorFillLegend" if owner > 0 => {
                if let Some(o) = v3(r.fs("m_origin")) {
                    self.cf_legends.push((owner, o, r.int("m_symbolId").unwrap_or(-1), r.int("m_idSchemeCategory").unwrap_or(-1)));
                }
            }
            "ColorFillSymbol" => {
                let attr = |k: &str| r.ptr(k).and_then(|i| tr.objs.get(i)).map_or((-1, false), |a| (a.int("m_fontId").unwrap_or(-1), a.int("m_bBold").unwrap_or(0) != 0));
                let sw = [num(r, "m_swatchWidth").unwrap_or(0.0), num(r, "m_swatchHeight").unwrap_or(0.0)];
                self.cf_types.insert(tr.id, (attr("m_oLineAndTextAttr"), attr("m_oTitleLineAndTextAttr"), sw, r.int("m_bShowTitle").unwrap_or(1) != 0));
            }
            "ColorFillSchema" => {
                // its values not hidden, their colour (COLORREF) and value (a text's; else none)
                let rows = r
                    .list("m_colorFillArr")
                    .iter()
                    .filter(|e| e.int("m_bHidden").unwrap_or(0) == 0)
                    .filter_map(|e| {
                        let c = e.int("m_color")?;
                        let v = e.obj("m_paramStorage").and_then(|p| p.obj("m_str")).and_then(|x| text_of(x, "m_str"))?;
                        Some(([(c & 255) as u8, ((c >> 8) & 255) as u8, ((c >> 16) & 255) as u8], v))
                    })
                    .collect();
                self.cf_schemes.insert(tr.id, (text_of(r, "m_title").unwrap_or_default(), r.int("m_paramId").unwrap_or(-1), r.int("m_areaSchemeId").unwrap_or(-1), rows));
            }
            "RoomElem" => {
                let at = r.fs("m_point").filter(|v| v.len() >= 2).map_or([f64::NAN; 2], |v| [v[0], v[1]]);
                self.spatials.push((tr.id, r.int("m_levelId").unwrap_or(-1), r.int("m_areaSchemeId").unwrap_or(-1), at));
            }
            // a revision of the project: its sequence (the latest on a sheet is its current one)
            "ProjectRevision" => {
                let f = ["m_date", "m_description", "m_issuedBy", "m_issuedTo"].map(|k| text_of(r, k).unwrap_or_default());
                self.revisions.insert(tr.id, (r.int("m_sequenceNumber").unwrap_or(0), f));
            }
            // the value of a family type parameter: a nested family's type, by names
            "NestedFamSymRefElem" => {
                if let (Some(f), Some(n)) = (text_of(r, "m_famName"), text_of(r, "m_symName")) {
                    self.nested_refs.insert(tr.id, (f, n));
                }
            }
            "DBViewType" => {
                let label = match r.get("m_refLabelStr") {
                    Some(G::S(v)) => v.clone(),
                    _ => String::new(),
                };
                let name = r.ptr("m_symbolInfo").and_then(|i| tr.objs.get(i)).and_then(|o| text_of(o, "m_name")).unwrap_or_default();
                self.view_types.insert(tr.id, (r.int("m_sectionAttrId").unwrap_or(-1), r.int("m_calloutAttrId").unwrap_or(-1), label, r.int("m_systemFamilyIdx").unwrap_or(-1), name));
            }
            "SectionAttributes" => {
                self.section_types.insert(tr.id, (r.int("m_sectionHeadFamilyTagId").unwrap_or(-1), r.int("m_sectionTailFamilyTagId").unwrap_or(-1)));
            }
            "CalloutTag" | "CalloutAttributes" => {
                self.callout_tags.insert(tr.id, (num(r, "m_cornerRadius").unwrap_or(0.0), r.int("m_familyTagId").unwrap_or(-1)));
            }
            _ if cls == "Viewer" || s.isa(t, "Viewer") => {
                // a section or a callout: its mark in other views
                if cls == "Section" || s.isa(t, "SectionGraphics") {
                    if let (Some(crop), Some(target)) = (r.obj("m_boundedSpace").and_then(bounded), r.int("m_dbViewId")) {
                        let shape: Vec<Vec<[V3; 2]>> = r
                            .ptrs("m_nonRectangularCropAreaCurvesLoops")
                            .iter()
                            .filter_map(|&i| tr.objs.get(i))
                            .map(|lp| {
                                lp.ptrs("m_curves")
                                    .iter()
                                    .filter_map(|&j| tr.objs.get(j))
                                    .filter_map(|g| {
                                        let (o, d, e) = (v3(g.fs("m_origin"))?, v3(g.fs("m_dirVec"))?, g.fs("m_endParams")?);
                                        (e.len() >= 2).then(|| [std::array::from_fn(|k| o[k] + d[k] * e[0]), std::array::from_fn(|k| o[k] + d[k] * e[1])])
                                    })
                                    .collect::<Vec<[V3; 2]>>()
                            })
                            .filter(|l| !l.is_empty())
                            .collect();
                        let front = r.fs("m_frontCut").filter(|f| f.len() >= 6).map(|f| ([f[0], f[1], f[2]], [f[3], f[4], f[5]]));
                        let heads = r
                            .list("m_viewDataMapForCallout")
                            .iter()
                            .filter_map(|e| {
                                let d = e.obj("second")?;
                                let leader = d.obj("m_leader").filter(|l| l.int("m_bStraight").unwrap_or(1) == 0).and_then(|l| Some((v3(l.fs("m_end"))?, v3(l.fs("m_elbow"))?)));
                                Some((e.int("first")?, v3(d.fs("m_headPoint"))?, leader))
                            })
                            .collect();
                        self.mark_views.insert(tr.id, target);
                        self.marks.push(ViewMark {
                            id: tr.id,
                            target,
                            owner: r.int("m_ownerDBViewId").unwrap_or(-1),
                            parent: r.int("m_parentDBViewId").unwrap_or(-1),
                            reference: r.int("m_bIsViewerReference").unwrap_or(0) != 0,
                            coarser: num(r, "m_coarserScale").unwrap_or(0.0),
                            crop,
                            shape,
                            front,
                            heads,
                            method: r.int("m_sectionCreationMethod").unwrap_or(0),
                            one_view: r.int("m_showInOneViewOnly").unwrap_or(0) != 0,
                        });
                    }
                }
                // Viewer and its kinds (Section, InteriorElevArrow…); a reference section or
                // callout (`m_bIsViewerReference`) only points at its view from another one: its
                // box is its own mark's, not that view's crop
                if r.int("m_bIsViewerReference").unwrap_or(0) != 0 {
                    return;
                }
                let (Some(c), Some(view)) = (r.obj("m_boundedSpace").and_then(bounded), r.int("m_dbViewId")) else {
                    return;
                };
                let mut c = c;
                // annotation crop (paper feet: left, right, top, bottom past the crop)
                let offsets: Option<[f64; 4]> = r.ptr("m_pAnnotationCropOffsets").and_then(|i| tr.objs.get(i)).and_then(|o| o.fs("m_leftRightTopBottomOffsets")).and_then(|f| f.try_into().ok());
                if let Some(o) = offsets.filter(|o| o.iter().all(|x| x.is_finite() && *x >= 0.0)) {
                    c.annot_off = o;
                }
                if r.int("m_bAnnotationCropActive").unwrap_or(0) != 0 {
                    c.annot = offsets;
                }
                if std::env::var("CROP_DEBUG").is_ok_and(|v| v == "all" || v == view.to_string()) {
                    eprintln!("CROP view {view} from {} {} orig {:?} off {:?} active {:?} annotation crop {:?} offsets {:?}", tr.id, cls, c.orig, c.off, c.active, r.int("m_bAnnotationCropActive"), offsets);
                }
                self.crops.insert(view, c);
            }
            "VolumeOfInterest" => {
                if let Some(c) = r.obj("m_boundedSpace").and_then(bounded) {
                    let shown = match r.get("m_viewsWhereExplicitlyVisible") {
                        Some(G::Ids(v)) | Some(G::Is(v)) => v.clone(),
                        _ => Vec::new(),
                    };
                    self.scope_boxes.push((tr.id, c, shown));
                }
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
                // placed in a family document (logo of a title block), or in a view (a picture
                // in a legend): centre and size
                let (Some(w), Some(h), Some(t)) = (num(info, "m_dWidth"), num(info, "m_dHeight"), info.obj("m_Trf").and_then(Trf::from)) else { return };
                let base = t.lin(v3(info.fs("m_baseVec")).unwrap_or([1.0, 0.0, 0.0]));
                if r.int("m_famId").unwrap_or(-1) > 0 {
                    self.fam_images.push((sym, t.apply([0.0, 0.0, 0.0]), base, w, h));
                } else if let Some(owner) = r.int("m_ownerDBViewId").filter(|o| *o > 0) {
                    // (drawn in front of the view's lines and texts: `m_background` set, as
                    // seen in Revit covering the texts under it)
                    self.view_images.push((owner, sym, t.apply([0.0, 0.0, 0.0]), base, w, h, r.int("m_background").unwrap_or(0) != 0));
                }
            }
            "TextNoteAttributes" | "TagNoteAttributes" => {
                if r.obj("m_lineAndTextAttr").and_then(|a| a.int("m_bBold")).unwrap_or(0) != 0 {
                    self.bold_types.insert(tr.id);
                }
                // italic, underlined; its tab stops (`TEXT_TAB_SIZE`, paper feet)
                let flag = |k: &str| r.obj("m_lineAndTextAttr").and_then(|a| a.int(k)).unwrap_or(0) != 0;
                self.type_faces.insert(tr.id, (flag("m_bItalic"), flag("m_bUnderline"), crate::rvt_annot::param_f(tr, TEXT_TAB_SIZE).filter(|v| *v > 1e-6 && *v < 1.0).unwrap_or(0.0)));
                // its colour: its category's (a family's text type: of its family document)
                if let Some(c) = r.obj("m_lineAndTextAttr").and_then(|a| a.int("m_categoryId")) {
                    self.text_styles.insert(tr.id, crate::rvt_annot::scoped(r.int("m_famId").unwrap_or(-1), c));
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
                // its typeface ("Arial", "Arial Narrow")
                if let Some(n) = r.ptr("m_pFont").and_then(|i| tr.objs.get(i)).and_then(|f| text_of(f, "m_name")).filter(|n| !n.trim().is_empty()) {
                    self.font_names.insert(tr.id, n.trim().to_string());
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
                let flip = p.int("m_orientFlag").unwrap_or(1) != 0;
                let up = if v3(p.fs("m_yVec")).is_some_and(|y| y[2] < 0.0) { -1.0 } else { 1.0 };
                let (z0, z1) = (o[2] + up * c[1], o[2] + up * c[3]);
                self.grids.push(Grid { id: tr.id, name: text_of(r, "m_text").unwrap_or_default(), a: at(c[0]), b: at(c[2]), z: (z0.min(z1), z0.max(z1)), attr: r.int("m_attrId").unwrap_or(-1), o, x, flip, up });
            }
            "PlanRegion" if r.int("m_famId").unwrap_or(-1) == -1 => {
                let elev = |k: &str| num(r, k).filter(|v| v.abs() < 1e8);
                let (Some(cut), Some(lo)) = (elev("m_cutPlaneElev"), elev("m_viewDepthPlaneElev").or(elev("m_bottomClipPlaneElev"))) else { return };
                let lo = elev("m_bottomClipPlaneElev").map_or(lo, |b| b.min(lo));
                let Some(lp) = r.ptrs("m_bndyCurvesLoops").first().and_then(|&i| tr.objs.get(i)) else { return };
                let pts: Vec<P2> = lp.ptrs("m_curves").iter().filter_map(|&i| tr.objs.get(i)).flat_map(|c| curve_points(c, s)).map(|p| [p[0], p[1]]).collect();
                if pts.len() >= 3 {
                    // (an unlimited top, stored as 1e9: everything above shows, AC-255's ducts
                    // over the roof)
                    let top = match num(r, "m_topClipPlaneElev") {
                        Some(t) if t >= 1e8 => UNLIMITED,
                        t => t.filter(|v| v.abs() < 1e8).unwrap_or(cut).max(cut),
                    };
                    self.plan_regions.push((r.int("m_ownerDBViewId").unwrap_or(-1), tr.id, (pts, lo.min(cut), lo.max(cut), top)));
                }
            }
            "GridAttributes" => {
                if let Some(h) = r.int("m_familyTagId") {
                    self.level_heads.insert(tr.id, h);
                }
                // which ends of its grids show a bubble, and the style of their lines
                self.grid_bubbles.insert(tr.id, (r.int("m_bubbleAtEnd1").unwrap_or(1) != 0, r.int("m_bubbleAtEnd2").unwrap_or(0) != 0));
                self.grid_elev_bubbles.insert(tr.id, r.int("m_bubbleLocInElev").unwrap_or(2));
                if let Some(c) = r.obj("m_lineAndTextAttr").and_then(|a| a.int("m_categoryId")) {
                    self.grid_lines.insert(tr.id, c);
                }
            }
            "LevelAttributes" | "ReferenceViewerAttributes" => {
                if let Some(h) = r.int("m_familyTagId") {
                    self.level_heads.insert(tr.id, h);
                }
            }
            "ReferenceViewer" if r.int("m_famId").unwrap_or(-1) == -1 => {
                if let (Some(at), Some(dir)) = (v3(r.fs("m_viewerLocation")), v3(r.fs("m_viewerDir"))) {
                    let owner = r.int("m_ownerDBViewId").unwrap_or(-1);
                    self.ref_viewers.push(RefViewer { id: tr.id, owner, at, dir, attr: r.int("m_attrId").unwrap_or(-1), target: r.int("m_targetViewId").unwrap_or(-1) });
                }
            }
            "ParameterFilterElement" => {
                let mut f = Filter::default();
                if let Some(cr) = r.ptr("m_oCategoryRule").and_then(|i| tr.objs.get(i)) {
                    if let Some(G::Ids(v)) | Some(G::Is(v)) = cr.get("m_categories") {
                        f.cats = v.iter().copied().collect();
                    }
                }
                let roots: Vec<usize> = r.ptrs("m_rules").into_iter().chain(r.ptr("m_oRuleCombination")).collect();
                f.unreadable = !roots.iter().all(|&i| filter_rules(tr, s, i, false, &mut f.rules));
                self.filters.insert(tr.id, f);
            }
            "Family" => {
                if let Some(n) = text_of(r, "m_name") {
                    self.fam_names.insert(tr.id, n);
                }
                if let Some(c) = r.int("m_categoryId") {
                    self.fam_cats.insert(tr.id, c);
                }
                if r.int("m_rotateTagWithComponent").unwrap_or(0) != 0 {
                    self.turning_tags.insert(tr.id);
                }
            }
            "ExtentElem" if r.int("m_famId").unwrap_or(-1) == -1 => {
                let view = r.int("m_dbViewId").unwrap_or(-1);
                for e in r.list("m_idExtents") {
                    // a section's ends in this view
                    let sec = e.obj("second").and_then(|o| o.ptr("m_oExtentData")).and_then(|i| tr.objs.get(i)).filter(|o| s.name(o.tag) == "SectionGraphicsViewDataRec");
                    if let (Some(id), Some(d)) = (e.int("first"), sec) {
                        if std::env::var("SECREC_DEBUG").is_ok_and(|v| v == view.to_string()) {
                            eprintln!("SECREC view {view} section {id} {:?}", d.fields);
                        }
                        let two = |k: &str| d.fs(k).filter(|v| v.len() >= 2 && v.iter().all(|x| x.is_finite())).map(|v| [v[0], v[1]]);
                        if let Some(ends) = two("m_horzExtents") {
                            let marks = [d.int("m_end1Mark").unwrap_or(1), d.int("m_end2Mark").unwrap_or(2)];
                            self.section_ends.insert((view, id), SectionEnds { ends, marks, broken: d.int("m_hideLine").unwrap_or(0) != 0, kept: two("m_endVisLengths").unwrap_or([-1.0; 2]) });
                        }
                    }
                    let rec = e.obj("second").and_then(|o| o.ptr("m_oExtentData")).and_then(|i| tr.objs.get(i)).filter(|o| s.name(o.tag) == "DatumPlaneRec");
                    if let (Some(id), Some(d)) = (e.int("first"), rec) {
                        if let (Some(a), Some(b)) = (num(d, "m_freeEnd"), num(d, "m_bubbleEnd")) {
                            if std::env::var("DATUM_DEBUG").is_ok_and(|v| v.split(',').any(|x| x == view.to_string())) {
                                eprintln!("DATUM view {view} datum {id} {:?} leader {:?}", d.fields, d.ptr("m_pLeader").and_then(|i| tr.objs.get(i)).map(|o| (s.name(o.tag), &o.fields)));
                            }
                            if let (Some(a3), Some(b3)) = (num(d, "m_freeEnd3d"), num(d, "m_bubbleEnd3d")) {
                                let elbow = |k: &str| d.ptr(k).and_then(|i| tr.objs.get(i)).and_then(|l| Some([v3(l.fs("m_end"))?, v3(l.fs("m_elbow"))?, v3(l.fs("m_anchor"))?]));
                                let elbows = [elbow("m_pFreeEndLeader"), elbow("m_pLeader")];
                                self.datum_recs.insert((view, id), DatumRec { free: a, bubble: b, free3: a3, bubble3: b3, flags: d.int("m_endBubbleOverrides").unwrap_or(0), elbows });
                            }
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
                    // its plane's extent: corners in the plane's own frame (origin, x and y
                    // directions), as a box in plan
                    let plane = r.ptr("m_pSurface").and_then(|i| tr.objs.get(i));
                    if let Some((c, p)) = plane.and_then(|p| p.obj("m_Envelope")).and_then(|e| e.fs("m_corners")).filter(|c| c.len() == 4).zip(plane) {
                        let (o, xv, yv) = (v3(p.fs("m_origin")).unwrap_or([0.0; 3]), v3(p.fs("m_xVec")).unwrap_or([1.0, 0.0, 0.0]), v3(p.fs("m_yVec")).unwrap_or([0.0, 1.0, 0.0]));
                        self.level_origin.insert(tr.id, o);
                        let at = |u: f64, v: f64| [o[0] + xv[0] * u + yv[0] * v, o[1] + xv[1] * u + yv[1] * v];
                        let pts = [at(c[0], c[1]), at(c[2], c[1]), at(c[0], c[3]), at(c[2], c[3])];
                        let (x0, x1) = pts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, p| (a.0.min(p[0]), a.1.max(p[0])));
                        let (y0, y1) = pts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, p| (a.0.min(p[1]), a.1.max(p[1])));
                        self.level_env.insert(tr.id, [x0, y0, x1, y1]);
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
                        title: text_of(r, "m_viewDescription").unwrap_or_default(),
                        detail: int_param(tr, VIEW_DETAIL_LEVEL).unwrap_or(0),
                        cat_detail: tr.objs.iter().flat_map(|o| o.list("m_detailLevels").iter()).filter_map(|e| Some((e.int("first")?, e.int("second").filter(|l| (1..=3).contains(l))?))).collect(),
                        class: cls.to_string(),
                        // (stored as paper / model, 0.02 at 1:50; read as 1:50 when above one)
                        scale: num(r, "m_scale").filter(|k| *k > 1e-6).map_or(1.0, |k| if k > 1.0 { 1.0 / k } else { k }),
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
                        filters: view_filters(tr, s),
                        elem_looks: elem_overrides(tr, s),
                        regions: tr.objs.iter().filter(|o| s.name(o.tag) == "DBViewPlanViewDepthCutter").flat_map(|o| o.list("m_planRegionFaceTagMap").iter()).filter_map(|e| e.int("first")).filter(|i| *i > 0).collect(),
                        viewer: r.int("m_viewerId").unwrap_or(-1),
                        vtype: r.int("m_dbViewTypeId").unwrap_or(-1),
                        strs: {
                            // a sheet's own: drawn, checked, designed, approved by, its dates
                            // (kept as fields, shown as these built-in parameters)
                            let mut m = strings(tr);
                            for (k, p) in [("m_drawnBy", -1007404), ("m_checkedBy", -1007405), ("m_designedBy", -1007406), ("m_approvedBy", -1007408), ("m_issueDate", -1006322)] {
                                if let Some(v) = text_of(r, k).filter(|v| !v.is_empty()) {
                                    m.entry(p).or_insert(v);
                                }
                            }
                            m
                        },
                        hidden: ids_of(tr, s, "HiddenElementsViewSettings", "m_hiddenElements"),
                        hidden_cats: ids_of(tr, s, "IckyExcludedCategoriesSetPtrWrapper", "m_categoryIds"),
                        ws_set: workset_settings(tr, s),
                        hidden_ws: Default::default(),
                    },
                );
                // its colour schemes by category
                let schemes: Vec<(i64, i64)> = tr.objs.iter().filter(|o| s.name(o.tag) == "ColorFillSchemaSetting").flat_map(|o| o.list("m_categoryToSchema").iter()).filter_map(|e| Some((e.int("first")?, e.int("second")?))).collect();
                if !schemes.is_empty() {
                    self.view_schemes.insert(tr.id, schemes);
                }
                // a sheet's revisions: revision -> its number on this sheet
                let revs: Vec<(i64, String)> = r.list("m_revNumMap").iter().filter_map(|e| Some((e.int("first")?, text_of(e, "second")?))).collect();
                if !revs.is_empty() {
                    self.sheet_revs.insert(tr.id, revs);
                }
                if let Some(data) = r.ptr("m_pData").and_then(|i| tr.objs.get(i)).filter(|o| s.name(o.tag) == "ScheduleData") {
                    // placed on the sheet by its top left corner
                    // drawn at the stored row heights (Revit keeps them, even below the text size)
                    let mut t = Table { top_down: true, ..table(data, tr) };
                    // its appearance
                    if let Some(d) = tr.objs.iter().find(|o| o.get("m_showGridLines").is_some()) {
                        let on = |k: &str, def: bool| d.int(k).map_or(def, |v| v != 0);
                        t.grid = on("m_showGridLines", true);
                        t.outline = on("m_showOutline", true);
                        t.headers = on("m_showHeaders", true);
                        t.spacer = on("m_showInitialSpacer", false);
                        // a keynote legend (`OST_KeynoteTags`), of the sheet it is on or not
                        if r.int("m_categoryId") == Some(KEYNOTE_TAGS) || tr.objs.iter().any(|o| o.int("m_categoryId") == Some(KEYNOTE_TAGS) && o.get("m_bFilterBySheet").is_some()) {
                            t.keynotes = Some(on("m_bFilterBySheet", false));
                        }
                    }
                    // its body: the look of each field's column
                    t.body_looks = tr.objs.iter().filter(|o| s.name(o.tag) == "ScheduleField" && o.int("m_hidden").unwrap_or(0) == 0).filter_map(|f| f.ptr("m_oStyle").and_then(|i| tr.objs.get(i))).map(cell_look).collect();
                    if std::env::var_os("TABLE_DEBUG").is_some() {
                        eprintln!("TABLE {} looks {:?} body {:?} keynotes {:?}", tr.id, t.looks, t.body_looks, t.keynotes);
                        eprintln!("TABLEROWS {} pitch {:.3} mm width {:.1} sections {:?}", tr.id, t.pitch * 304.8, t.width * 304.8, t.sections.iter().map(|s| (s.0, s.2.iter().map(|h| (h * 304.8 * 100.0).round() / 100.0).collect::<Vec<_>>(), s.3.iter().map(|c| (c.0, c.1, c.2.clone())).take(14).collect::<Vec<_>>())).collect::<Vec<_>>());
                    }
                    self.tables.insert(tr.id, t);
                }
            }
            _ => {}
        }
    }

    /// The areas of view `id` filled in their colour scheme's colours, under everything else the
    /// view draws (its fills from `f0` on): each area the face of its level's area boundary
    /// lines around its point (Revit keeps no area outline: it finds them again, as here).
    /// shortcut: areas only (rooms and spaces would need their walls), and an area's islands
    /// are not cut out of it; add both when a sheet needs them.
    fn put_color_fills(&self, out: &mut Sheet2d, id: i64, view: &ViewInfo, mp: &Trf, f0: usize) {
        let Some(schemes) = [id, view.template, view.primary].iter().find_map(|v| self.view_schemes.get(v)) else { return };
        let m = |x: f64| (x * FEET) as f32;
        let segs: Vec<[P2; 2]> = self.ann.area_lines.iter().filter(|l| l.0 == view.level).flat_map(|l| l.1.windows(2).map(|w| [[w[0][0], w[0][1]], [w[1][0], w[1][1]]])).collect();
        let mut fills = Vec::new();
        for (_, sc) in schemes {
            let Some((_, param, area_scheme, values)) = self.cf_schemes.get(sc).filter(|c| c.2 != -1) else { continue };
            // each area's face, and its colour
            let faces: Vec<(Vec<[P2; 2]>, [u8; 3])> = self
                .spatials
                .iter()
                .filter(|e| e.1 == view.level && e.2 == *area_scheme)
                .filter_map(|e| {
                    let face = face_around(&segs, e.3);
                    if std::env::var_os("CF_DEBUG").is_some() {
                        eprintln!("CFAREA view {id} area {} {:?} at {:?} face {:?}", e.0, self.ann.strs.get(&e.0).and_then(|s| s.get(&-1006900)), e.3, face.as_ref().map(|f| (f.len(), f.iter().map(|e| e[0]).fold([f64::MAX; 2], |a, q| if (q[1], q[0]) < (a[1], a[0]) { q } else { a }))));
                    }
                    let face = face?;
                    let c = self.ann.strs.get(&e.0).and_then(|s| s.get(param)).and_then(|v| values.iter().find(|x| &x.1 == v)).map_or([255; 3], |x| x.0);
                    Some((face, c))
                })
                .collect();
            for (face, c) in &faces {
                // (one shared by two areas is not filled: AC-293's ZF13-2 and ZF13-3, redundant)
                let low = |f: &[[P2; 2]]| f.iter().map(|e| e[0]).fold([f64::MAX; 2], |a, q| if (q[1], q[0]) < (a[1], a[0]) { q } else { a });
                if faces.iter().filter(|f| f.0.len() == face.len() && low(&f.0) == low(face)).count() > 1 || *c == [255; 3] {
                    continue;
                }
                let edges: Vec<[P2; 2]> = face.iter().map(|ab| ab.map(|q| { let p = mp.apply([q[0], q[1], 0.0]); [p[0], p[1]] })).collect();
                let tris = crate::rvt_annot::fill(&edges).into_iter().map(|t| t.map(|q| [m(q[0]), m(q[1])])).collect();
                fills.push(Fill2d { tris, color: *c, under: None });
            }
        }
        out.fills.splice(f0..f0, fills);
    }

    /// The colour fill legends placed in view `id`: its colour scheme's title, then a swatch and
    /// the value of each of the scheme's values the rooms, areas or spaces on the view's level
    /// have, in the scheme's order ("Aucune couleur définie" when none has one, FP-282). `mp`
    /// maps model feet to paper feet.
    fn put_color_legends(&self, out: &mut Sheet2d, id: i64, view: &ViewInfo, mp: &Trf) {
        let m = |x: f64| (x * FEET) as f32;
        for &(_, o, ty, cat) in self.cf_legends.iter().filter(|l| l.0 == id || (view.primary > 0 && l.0 == view.primary)) {
            let Some(&(row, title, sw, show_title)) = self.cf_types.get(&ty) else { continue };
            let scheme = [id, view.template, view.primary].iter().find_map(|v| self.view_schemes.get(v)).and_then(|v| v.iter().find(|c| c.0 == cat)).and_then(|c| self.cf_schemes.get(&c.1));
            let Some((name, param, area_scheme, values)) = scheme else { continue };
            let have: crate::det::HashSet<&str> = self
                .spatials
                .iter()
                .filter(|e| e.1 == view.level && (*area_scheme == -1 || e.2 == *area_scheme))
                .filter_map(|e| self.ann.strs.get(&e.0)?.get(param))
                .map(|v| v.as_str())
                .collect();
            let mut rows: Vec<([u8; 3], String)> = values.iter().filter(|v| have.contains(v.1.as_str())).cloned().collect();
            if rows.is_empty() {
                rows.push(([255; 3], if self.french { "Aucune couleur définie" } else { "No colors defined" }.into()));
            }
            let p = mp.apply(o);
            let (x0, y0) = (m(p[0]), m(p[1]));
            if std::env::var_os("CF_DEBUG").is_some() {
                eprintln!("CF view {id} legend at {:.3} {:.3} mm scheme {name:?} rows {}", x0 * 1000.0, y0 * 1000.0, rows.len());
            }
            let text = |out: &mut Sheet2d, x: f32, y: f32, (font, bold): (i64, bool), s: String| {
                let size = self.font_sizes.get(&font).copied().unwrap_or(0.0098);
                let font = self.font_names.get(&font).cloned().unwrap_or_default();
                // (`y` its baseline: its bottom is Arial's descent below it, 434 of 2048 units, and
                // a bold line's own extra)
                let fs = sw_format::font_size(m(size));
                let y = y - fs * 434.0 / 2048.0 - sw_format::line_extra(fs, bold, false).1;
                out.texts.push(Text2d { pos: [x, y], dir: [1.0, 0.0], height: m(size), halign: 0, valign: 2, text: s, bold, font, ..Default::default() });
            };
            // (its origin: the first swatch's top left corner, the title above it)
            // shortcut: offsets measured on AC-293's print (mm: the title's baseline above the
            // first swatch, row pitch, swatch to its text), not derived from the type's sizes;
            // revisit if another legend type shows up
            let mut y = y0;
            if show_title {
                text(out, x0, y + 0.009666, title, name.clone());
            }
            let (w, h) = (m(sw[0]), m(sw[1]));
            for (c, v) in rows {
                let (a, b) = ([x0, y], [x0 + w, y - h]);
                out.fills.push(Fill2d { tris: vec![[a, [b[0], a[1]], b], [a, b, [a[0], b[1]]]], color: c, under: None });
                out.strokes.push(Stroke2d { pts: vec![a, [b[0], a[1]], b, [a[0], b[1]], a], width: 0.000_368_55, color: [0; 3] });
                text(out, x0 + w + 0.00157, y - 0.006482, row, v);
                y -= 0.0133345;
            }
        }
    }

    /// How a text is drawn: bold by its type (its own rich text's bold is [`push_text`]'s), its
    /// colour and its type's size.
    fn look(&self, t: &RawText) -> TextLook {
        let font = self.text_fonts.get(&t.attr);
        let face = self.type_faces.get(&t.attr).copied().unwrap_or_default();
        TextLook {
            bold: self.bold_types.contains(&t.attr),
            color: self.text_styles.get(&t.attr).map_or([0; 3], |c| self.ann.style_color(*c)),
            size: font.and_then(|f| self.font_sizes.get(f)).copied(),
            font: font.and_then(|f| self.font_names.get(f)).cloned().unwrap_or_default(),
            italic: face.0,
            underline: face.1,
            tab: face.2,
        }
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
            // its box: what the table covers (drawn without lines when it shows no grid), and
            // its lines
            let b = self.draw_table(&mut sh, &Table { name, ..tb.clone() }, [0.0, 0.0, 0.0], &[]);
            let mut e = b.map_or([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY], |b| b.map(|x| (x * FEET) as f32));
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
        // (sections and elevations: levels; plans: grids)
        const GRIDS: i64 = -2000220;
        self.views.get(&view).is_some_and(|v| {
            (self.shows_levels(v) && !self.level_ends.is_empty())
                || (v.dir.is_some_and(|d| d[2].abs() > 0.99) && v.class.to_ascii_lowercase().contains("plan") && !v.hidden_cats.contains(&GRIDS) && !self.grids.is_empty())
        })
    }

    /// Level `level`'s ends in view `id` (or the view it depends on, `primary`) along the view's
    /// horizontal (`proj`): its free end and its bubble end, each set in 2D for the view or
    /// following the level's plane, both measured from where the plane's origin stands in the
    /// view; whether each is in 2D; the view's bubble overrides (`DatumRec`). None when the
    /// view keeps no record of it.
    fn level_span(&self, id: i64, primary: i64, level: i64, proj: &dyn Fn(V3) -> P2) -> Option<(f64, f64, bool, bool, i64)> {
        let r = self.datum_recs.get(&(id, level)).or_else(|| self.datum_recs.get(&(primary, level)))?;
        let s = proj(*self.level_origin.get(&level)?)[0];
        let (f2, b2) = (r.flags & 16 != 0, r.flags & 32 != 0);
        Some((s + if f2 { r.free } else { r.free3 }, s + if b2 { r.bubble } else { r.bubble3 }, f2, b2, r.flags))
    }

    /// Extent of a level along a view's horizontal: its plane's corners, else the model's width.
    fn model_span(&self, proj: &dyn Fn(V3) -> P2) -> (f64, f64) {
        let (lo, hi) = self.model_box;
        let far = (f64::INFINITY, f64::NEG_INFINITY);
        // the levels' planes share their extent, some rare one apart
        // (in the levels' order, the first extent kept on a tie: the same every time)
        let mut seen: std::collections::BTreeMap<[i64; 4], (usize, [f64; 4])> = std::collections::BTreeMap::new();
        let mut levels: Vec<(&i64, &[f64; 4])> = self.level_env.iter().filter(|(id, _)| self.level_ends.contains_key(id)).collect();
        levels.sort_by_key(|l| *l.0);
        for (_, c) in levels {
            seen.entry(c.map(|x| x.round() as i64)).or_insert((0, *c)).0 += 1;
        }
        match seen.values().rev().max_by_key(|v| v.0).map(|v| v.1).as_ref() {
            Some(c) => [[c[0], c[1]], [c[2], c[1]], [c[0], c[3]], [c[2], c[3]]].iter().map(|p| proj([p[0], p[1], 0.0])[0]).fold(far, |a, u| (a.0.min(u), a.1.max(u))),
            None => (0..8).map(|k| proj(corner(lo, hi, k))[0]).fold(far, |a, u| (a.0.min(u), a.1.max(u))),
        }
    }

    /// The model's box (metres), for `model_span`.
    pub fn set_model_box(&mut self, lo: [f64; 3], hi: [f64; 3]) {
        self.model_box = (lo.map(|x| x / FEET), hi.map(|x| x / FEET));
    }

    /// Whether plan `view` shows grid `g`: grids not switched off, `g` crossing what the view
    /// keeps (its view range).
    fn grid_shown(&self, view: &ViewInfo, keep: &Option<Clip>, g: &Grid) -> bool {
        let plan = view.dir.is_some_and(|d| d[2].abs() > 0.99) && view.class.to_ascii_lowercase().contains("plan");
        plan && !self.datum_hidden(view, g.id)
            && !keep.is_some_and(|k| {
                let (zl, zh) = (k.orig[2] + k.lo[2] * k.ax[2][2], k.orig[2] + k.hi[2] * k.ax[2][2]);
                g.z.1 < zl.min(zh) || g.z.0 > zl.max(zh)
            })
    }

    /// Where grid `g` crosses the vertical plane through `o` of normal `n` (a section's or an
    /// elevation's): its bottom and top there (model feet); none when it does not run square
    /// to the plane (Revit shows a grid in a section only then) or does not reach it.
    fn grid_cross(&self, g: &Grid, o: V3, n: V3) -> Option<(V3, V3)> {
        let (gx, gy) = (g.b[0] - g.a[0], g.b[1] - g.a[1]);
        let len = gx.hypot(gy);
        let den = gx * n[0] + gy * n[1];
        if len < 1e-9 || den.abs() < 0.9998 * len * n[0].hypot(n[1]) {
            return None;
        }
        let t = ((o[0] - g.a[0]) * n[0] + (o[1] - g.a[1]) * n[1]) / den;
        if !(-1e-6..=1.0 + 1e-6).contains(&t) {
            return None;
        }
        let (x, y) = (g.a[0] + gx * t, g.a[1] + gy * t);
        Some(([x, y, g.z.0.min(g.z.1)], [x, y, g.z.0.max(g.z.1)]))
    }

    /// Whether `view` hides grid `id` (grids switched off, or the grid hidden in the view or in
    /// the view it depends on, or its workset hidden).
    fn datum_hidden(&self, view: &ViewInfo, id: i64) -> bool {
        self.hides(view, id, GRIDS)
    }

    /// Grid `g`'s ends in view `id` (`proj`: model feet to the view plane): its free end and
    /// its bubble end, and whether each is set in 2D for the view (else its 3D end, cut by the
    /// crop).
    fn grid_ends(&self, id: i64, view: &ViewInfo, g: &Grid, proj: &dyn Fn(V3) -> P2) -> (P2, P2, (bool, bool)) {
        let rec = self.datum_recs.get(&(id, g.id)).or_else(|| self.datum_recs.get(&(view.primary, g.id))).copied();
        let at = |t: f64| {
            let u = if g.flip { -t } else { t };
            proj([g.o[0] + g.x[0] * u, g.o[1] + g.x[1] * u, g.o[2]])
        };
        match rec {
            Some(r) => {
                let (f2, b2) = (r.flags & 16 != 0, r.flags & 32 != 0);
                (at(if f2 { r.free } else { r.free3 }), at(if b2 { r.bubble } else { r.bubble3 }), (f2, b2))
            }
            None => (proj(g.b), proj(g.a), (false, false)),
        }
    }

    /// Whether a view shows levels: it looks horizontally (sections, elevations) and their
    /// category is not switched off.
    fn shows_levels(&self, view: &ViewInfo) -> bool {
        view.dir.is_some_and(|d| d[2].abs() < 1e-6) && !view.class.contains("Drafting") && !view.class.contains("Legend") && !view.class.contains("Schedule") && !view.hidden_cats.contains(&LEVELS)
    }

    /// Drafting views and legends drawn alone: their lines, symbols and texts, at the view's
    /// scale, with a margin around them.
    pub fn drawings(&self, ids: &[i64]) -> Vec<Sheet2d> {
        self.drawings_with(ids, &Edges { items: Vec::new() })
    }

    /// Views drawn alone, as Revit shows them (a plan, a section: the model elements they see
    /// by their `edges`, their annotations, datums), at the view's scale, with a margin around
    /// them.
    pub(crate) fn drawings_with(&self, ids: &[i64], edges: &Edges<'_>) -> Vec<Sheet2d> {
        // several views at once (each drawn alone), kept in the order given
        let next = std::sync::atomic::AtomicUsize::new(0);
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 16).min(ids.len().max(1));
        let mut all: Vec<(usize, Sheet2d)> = std::thread::scope(|sc| {
            let work = || {
                let mut got = Vec::new();
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(&id) = ids.get(i) else { break };
                    if let Some(sh) = self.drawing(id, edges) {
                        got.push((i, sh));
                    }
                }
                got
            };
            let hs: Vec<_> = (0..threads).filter_map(|_| std::thread::Builder::new().stack_size(32 << 20).spawn_scoped(sc, work).ok()).collect();
            if hs.is_empty() {
                return work();
            }
            hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
        });
        all.sort_by_key(|x| x.0);
        all.into_iter().map(|x| x.1).collect()
    }

    /// View `id` drawn alone (see `drawings_with`).
    fn drawing(&self, id: i64, edges: &Edges<'_>) -> Option<Sheet2d> {
        {
            let view = self.views.get(&id)?;
            let vp = Vp { owner: -1, offset: [0.0; 3], label: [0.0; 3], label_len: 0.0, scale: view.scale, view: id, attr: -1, number: String::new() };
            if std::env::var("VIEW_DEBUG").is_ok_and(|v| v == id.to_string()) {
                eprintln!("VIEW {id} {:?} scale {} template {} hidden cats {:?} template hidden {:?} hidden elems {}", view.name, view.scale, view.template, view.hidden_cats, self.views.get(&view.template).map(|t| &t.hidden_cats), view.hidden.len());
                for (e, l) in &view.elem_looks {
                    eprintln!("  own look {e}: {l:?}");
                }
            }
            let mut sh = Sheet2d { id, ..Default::default() };
            self.draw_view(&mut sh, view, &vp, edges)?;
            let mut e = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
            let line = sh.lines.iter().flatten().chain(sh.strokes.iter().flat_map(|s| s.pts.iter())).chain(sh.fine.iter().flatten());
            for q in line.chain(sh.fills.iter().flat_map(|f| f.tris.iter().flatten())).chain(sh.texts.iter().map(|t| &t.pos)) {
                e = [e[0].min(q[0]), e[1].min(q[1]), e[2].max(q[0]), e[3].max(q[1])];
            }
            if !e[0].is_finite() {
                return None;
            }
            let m = ((e[2] - e[0]).max(e[3] - e[1]) * 0.04).max(0.005);
            sh.extent = [e[0] - m, e[1] - m, e[2] + m, e[3] + m];
            Some(sh)
        }
    }

    /// The sheets' drawings. `model`: what the placed views show, element id → (box min,
    /// max, metres; rebuilt mesh if any).
    pub fn finish(&self, sheet_ids: &[(i64, String, String)], model: &[(i64, i64, [f64; 3], [f64; 3], Option<&Mesh>)]) -> Vec<Sheet2d> {
        self.finish_with(sheet_ids, model, &|_| {})
    }

    /// Same, telling `progress` the fraction of the sheets drawn.
    pub fn finish_with(&self, sheet_ids: &[(i64, String, String)], model: &[(i64, i64, [f64; 3], [f64; 3], Option<&Mesh>)], progress: &dyn Fn(f32)) -> Vec<Sheet2d> {
        // model edges only when a sheet shows a model view
        let need = self.viewports.iter().any(|v| v.scale > 1e-6 && v.scale < 0.99 && self.views.get(&v.view).is_some_and(|w| w.class != "DBViewSchedule"));
        let te = std::time::Instant::now();
        let edges = Edges::new(if need { model } else { &[] });
        if std::env::var_os("RVT_TIMES").is_some() {
            eprintln!("  edges of {} elements: {:.1}s", edges.items.len(), te.elapsed().as_secs_f64());
        }
        self.finish_edges(sheet_ids, &edges, progress)
    }

    /// The sheets' drawings, the model views they show seeing the model by its `edges`.
    pub(crate) fn finish_edges(&self, sheet_ids: &[(i64, String, String)], edges: &Edges<'_>, progress: &dyn Fn(f32)) -> Vec<Sheet2d> {
        *self.sheet_numbers.lock().unwrap_or_else(|e| e.into_inner()) = sheet_ids.iter().map(|(id, n, _)| (*id, n.clone())).collect();
        // several sheets at once, kept in their order; the progress told by this thread
        let n = sheet_ids.len().max(1) as f32;
        let (next, done) = (std::sync::atomic::AtomicUsize::new(0), std::sync::atomic::AtomicUsize::new(0));
        let work = |report: &dyn Fn(f32)| {
            let mut got = Vec::new();
            loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some((id, number, name)) = sheet_ids.get(i) else { break };
                got.push((i, self.sheet(*id, number, name, edges)));
                report(done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as f32 / n);
            }
            got
        };
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 16).min(sheet_ids.len().max(1));
        let mut all: Vec<(usize, Sheet2d)> = std::thread::scope(|sc| {
            let hs: Vec<_> = (1..threads).filter_map(|_| std::thread::Builder::new().stack_size(32 << 20).spawn_scoped(sc, || work(&|_| {})).ok()).collect();
            let mut all = work(progress);
            for h in hs {
                all.extend(h.join().unwrap_or_default());
            }
            all
        });
        all.sort_by_key(|x| x.0);
        all.into_iter().map(|x| x.1).filter(|s| !s.lines.is_empty() || !s.texts.is_empty() || !s.images.is_empty() || !s.views.is_empty()).collect()
    }

    /// A plan's view range as absolute elevations (feet): what it keeps, from the bottom of
    /// the view depth up to the cut plane (looking down), or from the cut plane up to the
    /// view depth (reflected ceiling plans); and its top clip plane (what MEP elements above
    /// the cut plane are shown up to; the cut plane looking up). Infinite ends are unlimited.
    fn plan_range(&self, v: &ViewInfo) -> Option<(f64, f64, f64)> {
        // the range the view keeps (Revit keeps it with the view, its template's applied);
        // else its template's, relative to the view's own level
        let t = self.views.get(&v.template).filter(|t| t.range_rel.is_some()).filter(|_| v.range_abs[0].is_none());
        if std::env::var_os("SHEET_DEBUG").is_some() {
            if let Some(t) = t {
                eprintln!("  template {} of {}: rel {:?} abs {:?}", t.name, v.name, t.range_rel, t.range_abs);
            }
        }
        let own_abs = v.range_abs;
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
        let top: f64 = if unlimited(1) { f64::INFINITY } else { v.range_abs[1].or_else(|| rel(1).flatten()).unwrap_or(cut).max(cut) };
        let up = v.dir.is_some_and(|d| d[2] < -0.5) || depth.is_some_and(|d| d > cut + 1e-6);
        if std::env::var_os("SHEET_DEBUG").is_some() {
            eprintln!("  range of {}: level {} z {lz:?} above {above:?} cut {cut} depth {depth:?} bottom {bottom:?} own abs {own_abs:?} template {} rel {:?}", v.name, v.level, t.is_some(), v.range_rel);
        }
        if up {
            Some((cut, depth.unwrap_or(f64::INFINITY), cut))
        } else {
            let lo = match (depth, bottom) {
                (Some(d), Some(b)) => d.min(b),
                (Some(d), None) => d,
                _ => f64::NEG_INFINITY,
            };
            Some((lo.min(cut), cut, top))
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
            let range = self.plan_range(v).map(|r| (r.0, r.1)).or_else(|| {
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
        // a dependent view shows what its primary view shows (its categories, its worksets),
        // less the elements hidden in it, its own (`m_hiddenElements` are per view: a riser
        // hidden in a primary view shows in its dependent view, AC-225)
        let base = self.views.get(&view.primary).filter(|_| view.primary > 0).unwrap_or(view);
        [view, base].iter().any(|v| v.hidden_cats.contains(&cat) || (elem != 0 && self.elem_ws.get(&elem).is_some_and(|w| v.hidden_ws.contains(w))))
            || (elem != 0 && (view.hidden.contains(&elem) || self.filter_look(view, elem, cat).hidden))
    }

    /// `filter` (the graphics a view's filters give an element) under the element's own
    /// graphics in the view (or in its primary view), which win.
    fn elem_look(&self, view: &ViewInfo, elem: i64, filter: Look) -> Look {
        let own = view.elem_looks.get(&elem).or_else(|| self.views.get(&view.primary).filter(|_| view.primary > 0).and_then(|p| p.elem_looks.get(&elem)));
        match own {
            Some(e) => Look {
                hidden: filter.hidden,
                line: e.line.or(filter.line),
                pen: e.pen.or(filter.pen),
                pattern: e.pattern.or(filter.pattern),
                fill: e.fill.filter(|f| f.0 > 0).or(filter.fill),
                halftone: e.halftone || filter.halftone,
            },
            None => filter,
        }
    }

    /// An element's own graphics in a view, for its annotations and symbols.
    fn line_over(&self, view: &ViewInfo, elem: i64) -> Option<crate::rvt_annot::LineOver> {
        let e = view.elem_looks.get(&elem).or_else(|| self.views.get(&view.primary).filter(|_| view.primary > 0).and_then(|p| p.elem_looks.get(&elem)))?;
        let tone = |c: [u8; 3]| if e.halftone { halftone(c) } else { c };
        Some(crate::rvt_annot::LineOver { color: e.line.map(tone).or(e.halftone.then_some([128, 128, 128])), pen: e.pen, pattern: e.pattern, fill: e.fill.map(|f| tone(f.1)) })
    }

    /// The graphics a view's filters give an element: the first matching filter in the
    /// view's list sets each property; any matching filter switched off hides it. A dependent
    /// view takes its primary view's filters, a view its template's when the template has any.
    fn filter_look(&self, view: &ViewInfo, elem: i64, cat: i64) -> Look {
        let base = self.views.get(&view.primary).filter(|_| view.primary > 0).unwrap_or(view);
        let uses = match self.views.get(&base.template).filter(|t| base.template > 0 && !t.filters.is_empty()) {
            Some(t) => &t.filters,
            None => &base.filters,
        };
        let mut look = Look::default();
        if std::env::var("RVT_LOOK_DEBUG").is_ok_and(|v| v == elem.to_string()) {
            eprintln!("LOOK {elem} view {} filters {uses:?}", view.name);
            for u in uses {
                if let Some(f) = self.filters.get(&u.filter) {
                    eprintln!("   FILTER {} {:?} matches {} rules {:?}", u.filter, f, self.filter_matches(f, elem, cat), f.rules.iter().map(|r| self.rule_passes(r, elem)).collect::<Vec<_>>());
                }
            }
        }
        for u in uses {
            let Some(f) = self.filters.get(&u.filter) else { continue };
            if !self.filter_matches(f, elem, cat) {
                continue;
            }
            look.hidden |= !u.visible;
            look.line = look.line.or(u.line);
            look.pen = look.pen.or(u.pen);
            look.pattern = look.pattern.or(u.pattern);
            look.fill = look.fill.or(u.fill);
            look.halftone |= u.halftone;
        }
        look
    }

    fn filter_matches(&self, f: &Filter, elem: i64, cat: i64) -> bool {
        !f.unreadable && f.cats.contains(&cat) && f.rules.iter().all(|r| self.rule_passes(r, elem) != r.inverse)
    }

    /// A view type's "Family and Type" as Revit words it: its family's browser name (in the
    /// language the file was saved in), then its name.
    fn view_type_text(&self, vtype: i64) -> Option<String> {
        let (_, _, _, family, name) = self.view_types.get(&vtype)?;
        // shortcut: only floor plans and sections are known, add other families when a file filters on them
        let family = match (*family, self.french) {
            (109, true) => "Plans d'étage",
            (109, false) => "Floor Plans",
            (112, true) => "Coupes",
            (112, false) => "Sections",
            _ => return None,
        };
        Some(format!("{family} ({name})"))
    }

    /// Whether `elem` passes rule `r` (a value the file does not give fails it).
    fn rule_passes(&self, r: &Rule, elem: i64) -> bool {
        let sym = self.ann.inst_type.get(&elem).copied();
        let text = |p: i64| -> Option<String> {
            if p == SYMBOL_FAMILY_NAME {
                return sym.and_then(|s| self.sym_family.get(&s)).and_then(|f| self.fam_names.get(f)).cloned();
            }
            // a view's mark: its view's name and parameters
            if let Some(v) = self.mark_views.get(&elem).and_then(|t| self.views.get(t)) {
                return match p {
                    VIEW_NAME => Some(v.name.clone()),
                    VIEW_TYPE => self.view_type_text(v.vtype),
                    _ => v.strs.get(&p).cloned(),
                };
            }
            self.ann.strs.get(&elem).and_then(|m| m.get(&p)).or_else(|| self.curve_strs.get(&elem).and_then(|m| m.get(&p))).or_else(|| sym.and_then(|s| self.ann.strs.get(&s)).and_then(|m| m.get(&p))).cloned()
        };
        let num = |p: i64| -> Option<f64> {
            if p == WORKSET {
                return self.elem_ws.get(&elem).map(|w| *w as f64);
            }
            text(p).and_then(|t| t.trim().parse().ok())
        };
        let op = r.op.as_str();
        match &r.value {
            RuleValue::Str(v) => {
                let Some(t) = text(r.param) else { return false };
                let (t, v) = (t.to_lowercase(), v.to_lowercase());
                match op {
                    "FilterStringEquals" => t == v,
                    "FilterStringContains" => t.contains(&v),
                    "FilterStringBeginsWith" => t.starts_with(&v),
                    "FilterStringEndsWith" => t.ends_with(&v),
                    "FilterStringGreater" => t > v,
                    "FilterStringGreaterOrEqual" => t >= v,
                    "FilterStringLess" => t < v,
                    "FilterStringLessOrEqual" => t <= v,
                    _ => false,
                }
            }
            RuleValue::Int(_) | RuleValue::Num(_) => {
                let v = match &r.value {
                    RuleValue::Int(i) => *i as f64,
                    RuleValue::Num(x) => *x,
                    _ => unreachable!(),
                };
                let Some(x) = num(r.param) else { return false };
                match op {
                    "FilterNumericEquals" => (x - v).abs() < 1e-6,
                    "FilterNumericGreater" => x > v,
                    "FilterNumericGreaterOrEqual" => x >= v - 1e-6,
                    "FilterNumericLess" => x < v,
                    "FilterNumericLessOrEqual" => x <= v + 1e-6,
                    _ => false,
                }
            }
        }
    }

    /// Colour of an element's MEP system (its system type's projection line colour).
    pub fn system_color(&self, elem: i64) -> Option<[u8; 3]> {
        // its own system, else that of the elements it is joined to (nearest first)
        let mut seen = crate::det::HashSet::from_iter([elem]);
        let mut ring = vec![elem];
        for _ in 0..6 {
            for e in &ring {
                if let Some(c) = self.own_system_color(*e) {
                    return Some(c);
                }
            }
            ring = ring.iter().filter(|e| !self.multi_system.contains(e)).flat_map(|e| self.elem_links.get(e).into_iter().flatten()).copied().filter(|e| seen.insert(*e)).collect();
            if ring.is_empty() {
                break;
            }
        }
        None
    }

    fn own_system_color(&self, elem: i64) -> Option<[u8; 3]> {
        self.system_colors.get(self.system_types.get(self.elem_systems.get(&elem)?)?).copied()
    }

    /// The line weight (pen) and pattern an element's MEP system type sets (its own system,
    /// else that of the elements it is joined to, nearest first).
    fn system_lines(&self, elem: i64) -> (Option<i64>, Option<i64>) {
        let mut seen = crate::det::HashSet::from_iter([elem]);
        let mut ring = vec![elem];
        for _ in 0..6 {
            for e in &ring {
                if let Some(ty) = self.elem_systems.get(e).and_then(|s| self.system_types.get(s)) {
                    return self.system_lines.get(ty).copied().unwrap_or((None, None));
                }
            }
            ring = ring.iter().filter(|e| !self.multi_system.contains(e)).flat_map(|e| self.elem_links.get(e).into_iter().flatten()).copied().filter(|e| seen.insert(*e)).collect();
            if ring.is_empty() {
                break;
            }
        }
        (None, None)
    }

    /// Which family document holds each family's lines (see `family_drawing`).
    pub fn set_family_roots(&mut self, roots: HashMap<i64, i64>) {
        self.fam_roots = roots;
    }

    /// How each family instance's family was fitted to its sizes (its lines follow).
    pub fn set_stretches(&mut self, stretches: HashMap<i64, crate::rvt_shape::Stretch>) {
        self.stretches = stretches;
    }

    /// The lines and filled regions of a symbol's family document (family coordinates).
    /// `ints`: the instance's own values, over its type's.
    fn family_drawing(&self, sy: &Sym, ints: Option<&HashMap<i64, i64>>) -> Option<std::sync::Arc<crate::rvt_annot::Drawn>> {
        self.family_drawing_in(sy, ints, 0)
    }

    /// `family_drawing`, `depth` families deep (the families nested in it drawn with it).
    fn family_drawing_in(&self, sy: &Sym, ints: Option<&HashMap<i64, i64>>, depth: usize) -> Option<std::sync::Arc<crate::rvt_annot::Drawn>> {
        let root = *self.fam_roots.get(&sy.family)?;
        let vals: HashMap<i64, bool> = sy.ints.iter().chain(ints.into_iter().flatten()).filter_map(|(p, v)| Some((self.ann.doc_param(root, *p)?, *v != 0))).collect();
        if std::env::var("FAMDRAW_DEBUG").is_ok_and(|v| v == sy.id.to_string()) {
            let cat = self.fam_cats.get(&sy.family).copied();
            eprintln!("FAMDRAW {} family {} cat {cat:?} style {:?} section line {:?} pens {:?}", sy.id, sy.family, cat.and_then(|c| self.ann.category_style(c)), self.ann.category_style(-2000201), (1..=8).map(|n| (self.ann.pen(n) * 304.8 * 100.0).round() / 100.0).collect::<Vec<_>>());
            eprintln!("FAMDRAW {} root {root} type ints {:?} given {:?} -> vals {vals:?}", sy.id, sy.ints.iter().map(|(p, v)| (*p, *v, self.ann.doc_param(root, *p))).collect::<Vec<_>>(), ints.map(|m| m.iter().map(|(p, v)| (*p, *v, self.ann.doc_param(root, *p))).collect::<Vec<_>>()));
        }
        let mut key: Vec<(i64, bool)> = vals.iter().map(|(a, b)| (*a, *b)).collect();
        key.sort();
        if let Some(d) = self.fam_drawn.lock().unwrap_or_else(|e| e.into_inner()).get(&(root, sy.id, key.clone())) {
            return Some(d.clone());
        }
        let mut d = crate::rvt_annot::Drawn::default();
        // a parametric family whose type moves its reference lines: each point of its drawing
        // moves as the nearest of them across it (a symbol drawn about one moves with it, a
        // line between two stretches)
        let moves = self.ref_line_moves(root, sy);
        let fit = |p: V3| -> P2 {
            if moves.is_empty() {
                return [p[0], p[1]];
            }
            let along = |axis: usize| {
                moves.iter().filter(|m| m.0 == axis).min_by(|a, b| (p[axis] - a.1).abs().total_cmp(&(p[axis] - b.1).abs())).map_or(0.0, |m| m.2)
            };
            [p[0] + along(0), p[1] + along(1)]
        };
        self.ann.draw_with(-root, &fit, 1.0, &vals, &|_| false, &mut d);
        // else when its type moves its lines (its document holds them where its default
        // values put them): the type's own lines, as regenerated for it
        let own: Vec<Vec<V3>> = self.drawn_curves(sy).into_iter().cloned().collect();
        if moves.is_empty() && !own.is_empty() && moved(&d, &own) && !within(&d, &own) {
            let w = self.ann.pen(1);
            d = crate::rvt_annot::Drawn::default();
            d.strokes = own.iter().map(|c| (c.iter().map(|p| [p[0], p[1]]).collect(), w, [0; 3])).collect();
        }
        // the annotation families placed in it, where they stand in it
        for (ix, (_, sym, tf)) in self.nested(root).filter(|_| depth < 4) {
            if self.inst_vis.get(ix).is_some_and(|v| !v.shows(&vals)) {
                continue;
            }
            let Some(n) = self.symbols.get(sym).and_then(|ns| self.family_drawing_in(ns, self.inst_ints.get(ix), depth + 1)) else { continue };
            let at = |p: P2| {
                let q = tf.apply([p[0], p[1], 0.0]);
                [q[0], q[1]]
            };
            d.strokes.extend(n.strokes.iter().map(|(pts, w, c)| (pts.iter().map(|p| at(*p)).collect(), *w, *c)));
            d.fills.extend(n.fills.iter().map(|(tris, c)| (tris.iter().map(|t| t.map(at)).collect(), *c)));
            d.texts.extend(n.texts.iter().map(|(p, dir, h, t)| {
                let (a, b) = (at(*p), at([p[0] + dir[0], p[1] + dir[1]]));
                (a, [b[0] - a[0], b[1] - a[1]], *h, t.clone())
            }));
        }
        let d = std::sync::Arc::new(d);
        self.fam_drawn.lock().unwrap_or_else(|e| e.into_inner()).insert((root, sy.id, key), d.clone());
        Some(d)
    }

    /// A 2D family type's own curves drawn: not its reference lines nor its invisible lines.
    fn drawn_curves<'s>(&self, sy: &'s Sym) -> Vec<&'s Vec<V3>> {
        sy.curves.iter().zip(sy.curve_styles.iter().chain(std::iter::repeat(&-1))).filter(|(_, st)| !self.ann.reference_line_style(**st) && !self.ann.invisible_style(0, **st)).map(|(c, _)| c).collect()
    }

    /// How a 2D family type moves the reference lines of its family document: for each one
    /// across an axis (a vertical line across x, a horizontal one across y), (axis, where it is
    /// in the document, how far the type moves it). Empty when none moves or they do not match.
    fn ref_line_moves(&self, root: i64, sy: &Sym) -> Vec<(usize, f64, f64)> {
        let doc = self.ann.reference_lines(root);
        let mine: Vec<&Vec<V3>> = sy.curves.iter().zip(&sy.curve_styles).filter(|(_, st)| self.ann.reference_line_style(**st)).map(|(c, _)| c).collect();
        if doc.is_empty() || doc.len() != mine.len() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (a, b) in doc.iter().zip(mine) {
            let (a0, a1, b0, b1) = (a[0], a[a.len() - 1], b[0], b[b.len() - 1]);
            // a line across x (vertical) or y (horizontal), the same in the type
            let axis = if (a0[0] - a1[0]).abs() < 1e-9 && (b0[0] - b1[0]).abs() < 1e-9 {
                0
            } else if (a0[1] - a1[1]).abs() < 1e-9 && (b0[1] - b1[1]).abs() < 1e-9 {
                1
            } else {
                continue;
            };
            out.push((axis, a0[axis], b0[axis] - a0[axis]));
        }
        if out.iter().all(|m| m.2.abs() < 1e-9) {
            return Vec::new();
        }
        out
    }

    /// The insulation of a duct or pipe: its thickness (feet) and category.
    fn insulation_of(&self, host: i64) -> Option<(f64, i64)> {
        let by = self.insulated.get_or_init(|| {
            let mut by: HashMap<i64, (f64, i64)> = HashMap::default();
            for &(id, th, cat) in &self.insulations {
                // joined to its duct or pipe by a connector
                for h in self.elem_links.get(&id).into_iter().flatten().filter(|h| self.mep_lines.contains_key(h)) {
                    by.insert(*h, (th, cat));
                }
            }
            by
        });
        by.get(&host).copied()
    }

    /// The category of the instance hosting model instance `id` (a shared family nested in it).
    fn host_category(&self, id: i64) -> Option<i64> {
        let (hsym, _, _) = self.model_insts.get(self.hosts.get(&id)?)?;
        self.sym_family.get(hsym).and_then(|f| self.fam_cats.get(f)).copied()
    }

    /// Whether model instance `id`, a shared family nested in another, shows in `view` at
    /// detail level `detail` as its host's family places it: its host not hidden there (by
    /// category, filter…), one of its places in the host's family showing (its visibility, by
    /// the host's parameters, and its detail levels). Any other shows.
    fn shows_in_host(&self, view: &ViewInfo, id: i64, detail: i64) -> bool {
        let Some(host) = self.hosts.get(&id) else { return true };
        if self.host_category(id).is_some_and(|c| self.hides(view, *host, c)) {
            return false;
        }
        let (Some((hsym, _, hints)), Some((sym, _, _))) = (self.model_insts.get(host), self.model_insts.get(&id)) else { return true };
        let Some(&root) = self.sym_family.get(hsym).and_then(|f| self.fam_roots.get(f)) else { return true };
        // (its family known by name: the project and the host's family keep a copy each)
        let name = |sym: &i64| self.sym_family.get(sym).or_else(|| self.symbols.get(sym).map(|s| &s.family)).and_then(|f| self.fam_names.get(f));
        let Some(fam) = name(sym) else { return true };
        let vals: HashMap<i64, bool> = self.sym_ints.get(hsym).into_iter().flatten().chain(hints.iter()).filter_map(|(p, v)| Some((self.ann.doc_param(root, *p)?, *v != 0))).collect();
        if std::env::var("HOSTED_DEBUG").is_ok_and(|v| v == host.to_string()) {
            eprintln!("HOSTED {id} in {host} ({}): type {sym} family {fam:?}", view.name);
            for (ix, (_, nsym, _)) in self.nested(root).filter(|(_, (_, nsym, _))| name(nsym) == Some(fam)) {
                eprintln!("   placed {ix} type {nsym} shows {:?} detail {:?}", self.inst_vis.get(ix).map(|v| v.shows(&vals)), self.inst_vis.get(ix).map(|v| v.at_detail(detail)));
            }
        }
        let mut placed = self.nested(root).filter(|(_, (_, nsym, _))| name(nsym) == Some(fam)).peekable();
        placed.peek().is_none() || placed.any(|(ix, _)| self.inst_vis.get(ix).is_none_or(|v| v.shows(&vals) && v.at_detail(detail)))
    }

    /// The families placed in family document `root`: (index, (owner, type, placement)).
    fn nested(&self, root: i64) -> impl Iterator<Item = (usize, &(i64, i64, Trf))> {
        let by = self.nested_by.get_or_init(|| {
            let mut by: HashMap<i64, Vec<usize>> = HashMap::default();
            for (ix, f) in self.inst_fams.iter().enumerate().filter(|(_, f)| **f > 0) {
                by.entry(*f).or_default().push(ix);
            }
            by
        });
        by.get(&root).into_iter().flatten().filter_map(|&ix| Some((ix, self.instances.get(ix)?)))
    }

    /// The value a label of instance `ix` (of type `sy`) shows for family parameter `p`: the
    /// instance's text, else its integer, else its type's text, else its type's integer.
    fn label_value(&self, ix: usize, sy: &Sym, t: &RawText, i: usize) -> Option<String> {
        self.label_value_of(&self.given(ix), sy, t, i)
    }

    /// The value a label of an instance (its parameter values `g`, of type `sy`) shows for its
    /// `i`-th parameter: see [`Self::label_value`].
    fn label_value_of(&self, g: &Given, sy: &Sym, t: &RawText, i: usize) -> Option<String> {
        let p = &t.params.get(i)?.0;
        // a text: the instance's (empty when it has none) for an instance parameter, else the type's
        if self.ann.text_params.contains(p) {
            if g.ints.contains_key(p) {
                return Some(g.strs.get(p).cloned().unwrap_or_default());
            }
            return sy.strs.get(p).cloned();
        }
        // a number in its kind's unit (the instance's value, else its type's)
        if let Some(v) = g.reals.get(p).or_else(|| sy.reals.get(p)) {
            if let Some(s) = self.ann.number(*p, *v, t.fmts.get(i).copied().flatten()) {
                return Some(s);
            }
        }
        g.strs.get(p).cloned().or_else(|| g.ints.get(p).map(|v| v.to_string())).or_else(|| sy.strs.get(p).cloned()).or_else(|| sy.ints.get(p).map(|v| v.to_string()))
    }

    /// The parameter values of placed family `ix` as stored.
    fn given(&self, ix: usize) -> Given {
        Given { strs: self.inst_strs.get(ix).cloned().unwrap_or_default(), ints: self.inst_ints.get(ix).cloned().unwrap_or_default(), reals: self.inst_reals.get(ix).cloned().unwrap_or_default() }
    }

    /// The parameter values of family `ix` nested in the family of `host` (type `sy`, family
    /// document `root`, its instance's values `g`): its own, those its host sets taken from the
    /// host (empty when the host has none: the schedule title's numbers, P-101).
    fn nested_given(&self, ix: usize, root: i64, sy: &Sym, g: &Given) -> Given {
        let ty = Given { strs: sy.strs.clone(), ints: sy.ints.clone(), reals: sy.reals.clone() };
        self.nested_given_from(ix, root, &ty, g)
    }

    /// `nested_given` with the host's type values `ty` and instance values `g`.
    fn nested_given_from(&self, ix: usize, root: i64, ty: &Given, g: &Given) -> Given {
        let mut n = self.given(ix);
        let Some(drives) = self.inst_drives.get(ix).filter(|d| !d.is_empty()) else { return n };
        // (the host's values by its family document's parameters: its type's, its instance's over them)
        let doc = |p: &i64| self.ann.doc_param(root, *p);
        let strs: HashMap<i64, &String> = ty.strs.iter().chain(&g.strs).filter_map(|(p, v)| Some((doc(p)?, v))).collect();
        let ints: HashMap<i64, i64> = ty.ints.iter().chain(&g.ints).filter_map(|(p, v)| Some((doc(p)?, *v))).collect();
        let reals: HashMap<i64, f64> = ty.reals.iter().chain(&g.reals).filter_map(|(p, v)| Some((doc(p)?, *v))).collect();
        for (e, h) in drives.iter().filter(|d| d.0 != -1) {
            // (a text the host has none of: empty; a number keeps its value)
            match strs.get(h) {
                Some(v) => n.strs.insert(*e, v.to_string()),
                None if self.ann.text_params.contains(e) => n.strs.insert(*e, String::new()),
                None => n.strs.remove(e),
            };
            if let Some(v) = ints.get(h) {
                n.ints.insert(*e, *v);
            }
            match reals.get(h) {
                Some(v) => n.reals.insert(*e, *v),
                None => n.reals.remove(e),
            };
        }
        n
    }

    /// The type of family `ix` nested in model family document `root` when its host (model
    /// instance `host`) sets it by a family type parameter: the type of the same family named
    /// as the host's value (`NestedFamSymRefElem`) names it, `default` its own family's type
    /// (a sprinkler's GHOST symbol "SYMBOLE - ROUGE" where its family places "SYMBOLE - NOIR").
    fn driven_type(&self, ix: usize, default: i64, root: i64, host: i64) -> Option<i64> {
        if std::env::var("DRIVEN_DEBUG").is_ok_and(|v| v == host.to_string()) {
            let hsym = self.model_insts.get(&host).map(|m| m.0);
            eprintln!("DRIVEN host {host} type {hsym:?} nested {ix} default {default} drives {:?} host elems {:?} type elems {:?} doc params {:?} refs {:?}", self.inst_drives.get(ix), self.elem_params.get(&host), hsym.and_then(|s| self.elem_params.get(&s)), self.elem_params.get(&host).map(|m| m.keys().map(|p| (*p, self.ann.doc_param(root, *p))).collect::<Vec<_>>()), self.elem_params.get(&host).map(|m| m.values().map(|r| self.nested_refs.get(r)).collect::<Vec<_>>()));
        }
        let h = self.inst_drives.get(ix)?.iter().find(|d| d.0 == -1)?.1;
        let (hsym, _, _) = self.model_insts.get(&host)?;
        // (the host's element values, its instance's before its type's)
        let own = self.elem_params.get(&host).into_iter().flatten();
        let r = own.chain(self.elem_params.get(hsym).into_iter().flatten()).find(|(p, _)| self.ann.doc_param(root, **p) == Some(h)).map(|(_, v)| *v)?;
        let (_, name) = self.nested_refs.get(&r)?;
        let by_name = self.syms_by_name.get_or_init(|| self.symbols.values().filter_map(|s| Some(((s.family, self.sym_names.get(&s.id)?.clone()), s.id))).collect());
        by_name.get(&(self.symbols.get(&default)?.family, name.clone())).copied()
    }

    /// The texts and labels of the families nested in `sy`'s family (`g`: its instance's
    /// parameter values), placed by `tf`.
    fn nested_texts(&self, out: &mut Sheet2d, sy: &Sym, g: &Given, tf: &Trf, depth: usize) {
        let Some(&root) = self.fam_roots.get(&sy.family) else { return };
        let vals: HashMap<i64, bool> = sy.ints.iter().chain(&g.ints).filter_map(|(p, v)| Some((self.ann.doc_param(root, *p)?, *v != 0))).collect();
        if std::env::var("NESTED_DEBUG").is_ok_and(|v| v == sy.id.to_string()) {
            eprintln!("NESTED in {} root {root} ints {:?} -> vals {vals:?}", sy.id, sy.ints.iter().chain(&g.ints).map(|(p, v)| (*p, *v, self.ann.doc_param(root, *p))).collect::<Vec<_>>());
            for (ix, (_, sym, _)) in self.nested(root) {
                eprintln!("   nested {ix} type {sym} vis {:?} drives {:?}", self.inst_vis.get(ix), self.inst_drives.get(ix));
            }
        }
        for (ix, (_, sym, ntf)) in self.nested(root).filter(|_| depth < 4) {
            if self.inst_vis.get(ix).is_some_and(|v| !v.shows(&vals)) {
                continue;
            }
            let Some(ns) = self.symbols.get(sym) else { continue };
            let ng = self.nested_given(ix, root, sy, g);
            let t2 = tf.after(ntf);
            // (a text the type keeps already written where it stands: the nested family's, as
            // Revit wrote it for this type; on its line, within its box whatever its alignment,
            // a letter more: a scale bar's "2.5M" written 25 mm from its 23 mm label's origin)
            let cached = |t: &RawText| {
                let p = t2.apply(t.orig);
                let b = t2.lin(t.base);
                let l = b[0].hypot(b[1]).max(1e-12);
                let (bx, by) = (b[0] / l, b[1] / l);
                let k = trf_scale(&t2);
                let h = t.height * k;
                let w = t.width.max(t.wrap) * k + h.max(0.5 / 304.8);
                sy.texts.iter().any(|c| {
                    let q = tf.apply(c.orig);
                    let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
                    (dx * bx + dy * by).abs() <= w && (dy * bx - dx * by).abs() <= h * 0.5
                })
            };
            // (or written anywhere by the type, the same text the same size and alignment: the
            // nested family moved where the type's sizes put it, a schedule title's numbers 315 mm
            // along, P-101)
            let written = |t: &RawText, text: &str| sy.texts.iter().any(|c| c.params.is_empty() && c.text == text && (c.height - t.height).abs() < 1e-9 && c.flags == t.flags);
            for t in ns.texts.iter().filter(|t| !cached(t) && !written(t, &t.text)) {
                push_text(out, t, Some(&t2), trf_scale(&t2), t.text.clone(), self.look(t));
            }
            for t in ns.tags.iter().filter(|t| !cached(t)) {
                let vals: Vec<(usize, String)> = t.params.iter().enumerate().filter_map(|(i, (_, pre, suf, _))| {
                    let v = self.label_value_of(&ng, ns, t, i)?;
                    (!v.is_empty()).then(|| (i, format!("{pre}{v}{suf}")))
                }).collect();
                if std::env::var("NESTED_DEBUG").is_ok_and(|v| v == sy.id.to_string()) {
                    eprintln!("   label {:?} -> {vals:?} (text params {:?}, given {:?})", t.params, t.params.iter().map(|p| self.ann.text_params.contains(&p.0)).collect::<Vec<_>>(), t.params.iter().map(|p| (ng.strs.get(&p.0), ng.ints.get(&p.0), ng.reals.get(&p.0))).collect::<Vec<_>>());
                }
                let text = t.join(&vals);
                if written(t, &text) {
                    continue;
                }
                push_text(out, t, Some(&t2), trf_scale(&t2), text, self.look(t));
            }
            self.nested_texts(out, ns, &ng, &t2, depth + 1);
        }
    }

    /// Lines of a symbol placed by `tf`: those of its family document, else its own.
    fn put_symbol(&self, out: &mut Sheet2d, sy: &Sym, tf: &Trf, ints: Option<&HashMap<i64, i64>>) {
        let at = |p: P2| {
            let q = tf.apply([p[0], p[1], 0.0]);
            [q[0], q[1]]
        };
        let own = self.drawn_curves(sy);
        if let Some(d) = self.family_drawing(sy, ints) {
            if !d.strokes.is_empty() || !d.fills.is_empty() || own.is_empty() {
                put_drawn(out, &d, &at);
                return;
            }
        }
        let w = (self.ann.pen(1) * FEET) as f32;
        for c in own {
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
                out.fills.push(Fill2d { tris: vec![[[m(z[0]), m(z[1])], [m(bx - uy * h), m(by + ux * h)], [m(bx + uy * h), m(by - ux * h)]]], color: [0; 3], under: None });
            }
        }
        let width = if t.width > 0.0 { t.width } else { t.wrap };
        if self.boxed_types.contains(&t.attr) && width > 0.0 {
            let o = p2(tf.apply(t.orig));
            let b = tf.lin(t.base);
            let l = (b[0] * b[0] + b[1] * b[1]).sqrt().max(1e-12);
            let (ux, uy) = (b[0] / l, b[1] / l);
            let along = [0.0, -0.5, -1.0][t.halign() as usize] * width;
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
            // its current revision: its latest by sequence, numbered as on the sheet (AC-XX1…4)
            SHEET_CURRENT_REVISION => return self.sheet_revs.get(&sheet).and_then(|r| r.iter().max_by_key(|(rev, _)| self.revisions.get(rev).map_or(0, |x| x.0))).map(|(_, n)| n.clone()),
            _ => {}
        }
        v.and_then(|v| v.strs.get(&p)).or_else(|| self.project.get(&p)).cloned()
    }

    fn sheet(&self, id: i64, number: &str, name: &str, edges: &Edges<'_>) -> Sheet2d {
        let mut out = Sheet2d { id, ..Default::default() };
        if std::env::var("SHEET_VIEWS").is_ok_and(|v| v == id.to_string()) {
            for vp in self.viewports.iter().filter(|v| v.owner == id) {
                eprintln!("SHEET {id} {number} view {} {:?} scale {}", vp.view, self.views.get(&vp.view).map(|v| v.name.clone()), vp.scale);
            }
        }
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
        let mut insts: Vec<(usize, &(i64, i64, Trf))> = self.instances.iter().enumerate().filter(|i| i.1 .0 == id).collect();
        let area = |i: &(usize, &(i64, i64, Trf))| self.symbols.get(&i.1 .1).and_then(|s| s.outline).map(|o| (o[2] - o[0]) * (o[3] - o[1])).unwrap_or(0.0);
        insts.sort_by(|a, b| area(b).total_cmp(&area(a)));
        for (k, (ix, (_, sym, tf))) in insts.into_iter().enumerate() {
            let Some(sy) = self.symbols.get(sym) else { continue };
            // (one hidden on the sheet, as a view hides an element: a schedule's title, P-101)
            if self.views.get(&id).zip(self.inst_ids.get(ix)).is_some_and(|(v, i)| v.hidden.contains(i)) {
                continue;
            }
            if std::env::var("TB_DEBUG").is_ok_and(|v| v == id.to_string()) {
                eprintln!("TB inst {:?} sym {sym} family {:?} at {:?} ints {:?} nested {:?}", self.inst_ids.get(ix), self.fam_names.get(&sy.family), tf.apply([0.0; 3]), self.inst_ints.get(ix), self.fam_roots.get(&sy.family).map(|r| self.nested(*r).map(|(nix, (_, ns, _))| (nix, *ns, self.symbols.get(ns).and_then(|s| self.fam_names.get(&s.family)).cloned())).collect::<Vec<_>>()));
            }
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
                out.images.push(Image2d { rect: [m(c[0] - hw), m(c[1] - hh), m(c[0] + hw), m(c[1] + hh)], image: img, turns, front: false });
            }
            // the family's own lines, with their style and visibility, else the type's lines
            // (its instance's yes/no parameters showing or hiding parts of it)
            match self.family_drawing(sy, self.inst_ints.get(ix)).filter(|d| !d.strokes.is_empty() || !d.fills.is_empty()) {
                Some(d) => put_drawn(&mut out, &d, &|p| {
                    let q = tf.apply([p[0], p[1], 0.0]);
                    [q[0], q[1]]
                }),
                None => {
                    for c in self.drawn_curves(sy) {
                        out.lines.push(c.iter().map(|p| tf.apply(*p)).map(|q| pt([q[0], q[1]])).collect());
                    }
                }
            }
            for t in &sy.texts {
                push_text(&mut out, t, Some(tf), trf_scale(tf), t.text.clone(), self.look(t));
            }
            self.nested_texts(&mut out, sy, &self.given(ix), tf, 0);
            for t in &sy.tags {
                let mut vals: Vec<(usize, String)> = Vec::new();
                for (i, (p, pre, suf, _)) in t.params.iter().enumerate() {
                    // the instance's own value, its type's, else the sheet's or project's
                    let own = self.inst_strs.get(ix).and_then(|m| m.get(p)).or_else(|| sy.strs.get(p)).cloned();
                    if std::env::var("TB_DEBUG").is_ok_and(|v| v == id.to_string()) {
                        eprintln!("TB label param {p} prefix {pre:?} own {own:?} sheet {:?} inst ints {:?}", self.value(id, number, name, &scale, *p), self.inst_ints.get(ix).and_then(|m| m.get(p)));
                    }
                    let Some(v) = own.or_else(|| self.value(id, number, name, &scale, *p)) else { continue };
                    if v.is_empty() {
                        continue;
                    }
                    vals.push((i, format!("{pre}{v}{suf}")));
                }
                push_text(&mut out, t, Some(tf), trf_scale(tf), t.join(&vals), self.look(t));
            }
            for (pos, tb, turns) in &sy.tables {
                let at = tf.apply(*pos);
                if *turns == 0 {
                    self.draw_table(&mut out, tb, at, &self.revision_rows(id, tb));
                } else {
                    // drawn upright, then turned about where it stands
                    let mut part = Sheet2d::default();
                    self.draw_table(&mut part, tb, at, &self.revision_rows(id, tb));
                    turn_about(&mut part, [(at[0] * FEET) as f32, (at[1] * FEET) as f32], *turns);
                    out.lines.extend(part.lines);
                    out.strokes.extend(part.strokes);
                    out.fills.extend(part.fills);
                    out.texts.extend(part.texts);
                }
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
            if std::env::var_os("TABLE_DEBUG").is_some() {
                eprintln!("TABLE sheet {id} schedule {sch} at {o:?} known {} view {:?} table {:?}", self.tables.contains_key(sch), self.views.get(sch).map(|v| (&v.name, &v.class)), self.tables.get(sch));
            }
            if let Some(tb) = self.tables.get(sch).filter(|t| t.keynotes.is_none()) {
                self.draw_table(&mut out, tb, *o, &[]);
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
                out.images.push(Image2d { rect: [m(r[0]), m(r[1]), m(r[2]), m(r[3])], image: img, turns: 0, front: false });
                rect = Some(r);
            } else if vp.scale > 1e-6 {
                rect = self.draw_view(&mut out, view, vp, edges);
            }
            let Some(r) = rect else { continue };
            out.views.push(Viewport2d { rect: [m(r[0]), m(r[1]), m(r[2]), m(r[3])], view_id: vp.view, name: view.name.clone() });
            // view title: its type's title family at the title's place (its labels: the view's
            // number, sheet, title on sheet and scale), else number, name and scale on a line
            let (shown, line) = self.vp_types.get(&vp.attr).copied().unwrap_or((true, true));
            let title = self.level_heads.get(&vp.attr).and_then(|h| self.symbols.get(h)).filter(|_| shown);
            if std::env::var_os("TITLE_DEBUG").is_some() {
                if let Some(t) = title {
                    for g in t.tags.iter().chain(t.texts.iter()) {
                        eprintln!("  sym label {:?} at {:?} params {:?}", g.text, g.orig, g.params.iter().map(|p| p.0).collect::<Vec<_>>());
                    }
                    for c in &t.curves {
                        eprintln!("  sym curve {:?}", c);
                    }
                }
                eprintln!("TITLE view {} {:?} attr {} shown {shown} line {line} label {:?} len {} sym {:?} curves {:?} root {:?} drawn {:?}", vp.view, view.name, vp.attr, vp.label, vp.label_len, title.map(|t| (t.id, t.family)), title.map(|t| t.curves.len()), title.and_then(|t| self.fam_roots.get(&t.family)), title.and_then(|t| self.family_drawing(t, None)).map(|d| (d.strokes.len(), d.fills.len(), d.texts.len())));
            }
            if let Some(sy) = title {
                let tf = Trf::new([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], [vp.label[0], vp.label[1], 0.0]);
                self.put_symbol(&mut out, sy, &tf, None);
                for t in &sy.texts {
                    push_text(&mut out, t, Some(&tf), 1.0, t.text.clone(), self.look(t));
                }
                // (a drafting view at full size too: "ECHELLE: 1 : 1", P-601/602/608)
                let scale_text = if vp.scale > 1e-6 && vp.scale < 1.0 + 1e-6 { format!("1 : {}", (1.0 / vp.scale).round()) } else { String::new() };
                for t in &sy.tags {
                    if std::env::var_os("SHEET_DEBUG").is_some() {
                        eprintln!("    title {} labels {:?}", vp.view, t.params);
                    }
                    let vals: Vec<(usize, String)> = t
                        .params
                        .iter()
                        .enumerate()
                        .filter_map(|(i, (p, pre, suf, _))| {
                            let v = match *p {
                                VIEWPORT_DETAIL_NUMBER | VIEWER_DETAIL_NUMBER => vp.number.clone(),
                                VIEWPORT_SHEET_NUMBER | VIEWER_SHEET_NUMBER | -1007401 => number.to_string(),
                                // the title on the sheet when it has one, else its name
                                VIEW_NAME | VIEWPORT_VIEW_NAME => if view.title.is_empty() { view.name.clone() } else { view.title.clone() },
                                VIEW_DESCRIPTION => view.title.clone(),
                                VIEWPORT_SCALE | VIEW_SCALE | VIEW_SCALE_PULLDOWN_METRIC | VIEW_SCALE_PULLDOWN_IMPERIAL => scale_text.clone(),
                                _ => view.strs.get(p).cloned()?,
                            };
                            (!v.is_empty()).then(|| (i, format!("{pre}{v}{suf}")))
                        })
                        .collect();
                    push_text(&mut out, t, Some(&tf), 1.0, t.join(&vals), self.look(t));
                }
                if line && vp.label_len > 1e-4 {
                    out.strokes.push(Stroke2d { pts: vec![pt([vp.label[0], vp.label[1]]), pt([vp.label[0] + vp.label_len, vp.label[1]])], width: m(self.ann.pen(1)), color: [0; 3] });
                }
            } else if vp.label_len > 1e-4 && shown && !self.level_heads.get(&vp.attr).is_some_and(|h| *h <= 0) {
                // (a viewport type without a title family shows none: only an unknown type
                // gets this plain title)
                let (l0, l1) = ([vp.label[0], vp.label[1]], [vp.label[0] + vp.label_len, vp.label[1]]);
                out.lines.push(vec![pt(l0), pt(l1)]);
                let h = 0.012; // about 3.5 mm, the usual size of view titles
                let head = if vp.number.is_empty() { view.name.clone() } else { format!("{}  {}", vp.number, view.name) };
                out.texts.push(Text2d { pos: pt([l0[0], l0[1] + h * 0.4]), dir: [1.0, 0.0], height: m(h), halign: 0, valign: 2, text: head, bold: false, wrap: 0.0, color: [0; 3], ..Default::default() });
                if vp.scale > 1e-6 && vp.scale < 0.99 {
                    out.texts.push(Text2d { pos: pt([l0[0], l0[1] - h * 0.4]), dir: [1.0, 0.0], height: m(h * 0.7), halign: 0, valign: 0, text: format!("1 : {}", (1.0 / vp.scale).round()), bold: false, wrap: 0.0, color: [0; 3], ..Default::default() });
                }
            }
        }
        // keynote legends: the keynotes drawn above, by the numbers the sheet gives them
        for (_, o, sch) in self.sched_inst.iter().filter(|s| s.0 == id) {
            if let Some(tb) = self.tables.get(sch).filter(|t| t.keynotes.is_some()) {
                self.draw_table(&mut out, tb, *o, &[]);
            }
        }
        number_keys(&mut out);
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
    /// Returns the box it covers (feet: left, bottom, right, top).
    /// A revision schedule's rows on sheet `sheet`: its revisions by sequence, each column's
    /// field (number on the sheet, date, description, issued by, issued to); none for another
    /// table.
    fn revision_rows(&self, sheet: i64, tb: &Table) -> Vec<Vec<String>> {
        if std::env::var("REV_DEBUG").is_ok() {
            eprintln!("REV sheet {sheet} table {:?} revisions {} fields {:?} sheet revs {:?} sections {:?} headers {} top_down {}", tb.name, tb.revisions, tb.fields, self.sheet_revs.get(&sheet), tb.sections.iter().map(|s| (s.0, s.1.len(), s.2.len())).collect::<Vec<_>>(), tb.headers, tb.top_down);
        }
        let Some(revs) = self.sheet_revs.get(&sheet).filter(|_| tb.revisions) else { return Vec::new() };
        let mut revs: Vec<&(i64, String)> = revs.iter().collect();
        revs.sort_by_key(|(r, _)| self.revisions.get(r).map_or(0, |x| x.0));
        revs.iter()
            .map(|(r, num)| {
                let f = self.revisions.get(r).map(|x| &x.1);
                let field = |i: usize| f.map(|f| f[i].clone()).unwrap_or_default();
                tb.fields
                    .iter()
                    .map(|p| match *p {
                        -1011951 => num.clone(),
                        -1011953 => field(0),
                        -1011952 => field(1),
                        -1011955 => field(2),
                        -1011954 => field(3),
                        _ => String::new(),
                    })
                    .collect()
            })
            .collect()
    }

    fn draw_table(&self, out: &mut Sheet2d, tb: &Table, o: V3, body: &[Vec<String>]) -> Option<[f64; 4]> {
        let m = |x: f64| (x * FEET) as f32;
        let up = !tb.top_down;
        let mut y = o[1];
        let width = if tb.width > 1e-6 { tb.width } else { tb.sections.iter().map(|s| s.1.iter().sum::<f64>()).fold(0.0, f64::max) };
        if width <= 1e-6 {
            return None;
        }
        // its lines: each row's when it shows grid lines, its outline when it shows one
        let rect = |out: &mut Sheet2d, y0: f64, y1: f64| out.lines.push(vec![[m(o[0]), m(y0)], [m(o[0] + width), m(y0)], [m(o[0] + width), m(y1)], [m(o[0]), m(y1)], [m(o[0]), m(y0)]]);
        // a cell text's height (feet): its size is its capitals' height in 96ths of an inch (on
        // Revit's PDF prints a 11.34 one is 12 pt Arial, capitals 3 mm high)
        let caps = |size: f64| size / 96.0 / 12.0;
        // the height Revit gives a row of the body (the rows it computes; the header's keep
        // their stored heights) for a cell's text (`cw` its width): 1.475 times its size (5.9 mm
        // for a line of 12 pt Arial, as for keynote legends), and a line pitch for each line
        // more (wrapped at the cell's width as the viewer wraps it)
        let body_row = |look: Option<&CellLook>, text: &str, cw: f64| -> f64 {
            let Some(l) = look.filter(|l| l.size > 0.0) else { return 0.0 };
            let em = sw_format::font_size((caps(l.size) * 0.3048) as f32);
            let wrap = ((cw - 1.0 / 32.0 / 12.0).max(cw * 0.5) * 1.01 * 0.3048) as f32;
            let n = sw_format::wrapped_rows(text, em, l.bold, wrap, 0.0127);
            let face = (l.bold, l.italic);
            l.size / 72.0 / 12.0 * 1.475 + (n - 1) as f64 * sw_format::line_pitch(em, face, face) as f64 / 0.3048
        };
        // a cell's text: its look's size, weight and alignment (a margin from the side it is
        // aligned to), else most of the row shrunk to fit
        // (a stored cell's text wraps at its cell's width, as Revit wraps it; in a row of a
        // fixed height, `fixed`, in the lines Revit breaks it into, those it has no room for left
        // out, the last one shown ending with "...")
        let put = |out: &mut Sheet2d, text: &str, look: Option<&CellLook>, cx: f64, cw: f64, y0: f64, y1: f64, wrap: bool, fixed: bool| {
            let lines = text.lines().count().max(1) as f64;
            let chars = text.lines().map(|l| l.chars().count()).max().unwrap_or(0).max(1) as f64;
            let h = y1 - y0;
            let (th, bold, align) = match look.filter(|l| l.size > 0.0) {
                Some(l) => (caps(l.size), l.bold, l.halign),
                None => ((h * 0.6 / lines).min(0.0156).min(cw * 0.92 / (chars * 0.75)), false, 1),
            };
            let color = look.map_or([0; 3], |l| l.color);
            let (font, italic, underline) = look.map_or((String::new(), false, false), |l| (l.font.clone(), l.italic, l.underline));
            // turned (a vertical heading): centred in its cell
            let turn = look.map_or(0.0, |l| l.angle).to_radians();
            if turn.abs() > 1e-6 {
                // (at its cell's bottom or top, 1/32" in from it, when it is aligned there: it
                // reads from there, AC-101's vertical headings; else centred)
                let (up, inset) = (turn.sin() > 0.0, 1.0 / 32.0 / 12.0);
                let (y, halign) = match look.map_or(4, |l| l.valign) {
                    8 => (y0 + inset, if up { 0 } else { 2 }),
                    0 => (y1 - inset, if up { 2 } else { 0 }),
                    _ => ((y0 + y1) * 0.5, 1),
                };
                // (wrapped 1/64" short of its cell's height, a word longer than it broken: "RENDEMEN" / "T",
                // "T." / "SOR.", AC-101)
                let wrap = if wrap { m((y1 - y0 - inset * 0.5).max((y1 - y0) * 0.5)) } else { 0.0 };
                out.texts.push(Text2d { pos: [m(cx + cw * 0.5), m(y)], dir: [turn.cos() as f32, turn.sin() as f32], height: m(th), halign, valign: 1, text: text.to_string(), bold, color, font, italic, underline, wrap, break_words: wrap > 0.0, ..Default::default() });
                return;
            }
            // 1/32"
            let margin = 1.0 / 32.0 / 12.0;
            let (x, halign) = match align {
                0 => (cx + margin, 0),
                2 => (cx + cw - margin, 2),
                _ => (cx + cw * 0.5, 1),
            };
            // (wrapped 1/64" short of each side of its cell, as Revit wraps it: measured on its
            // PDF prints of the AC-70x and AC-101 schedules)
            let mut wrap = if wrap { m((cw - margin).max(cw * 0.5)) } else { 0.0 };
            let mut text = text.to_string();
            if fixed && wrap > 0.0 {
                let face = (bold, italic);
                let pitch = sw_format::line_pitch(sw_format::font_size(m(th)), face, face);
                let rows = (m(y1 - y0) / pitch).floor().max(1.0) as usize;
                (text, wrap) = (sw_format::cell_lines(&text, sw_format::text_em(m(th)), bold, wrap, rows), 0.0);
            }
            // at its cell's top, middle or bottom, 1/64" in from it
            let (y, valign) = match look.map_or(4, |l| l.valign) {
                0 => (y1 - margin * 0.5, 0),
                8 => (y0 + margin * 0.5, 2),
                _ => ((y0 + y1) * 0.5, 1),
            };
            out.texts.push(Text2d { pos: [m(x), m(y)], dir: [1.0, 0.0], height: m(th), halign, valign, text, bold, color, font, italic, underline, wrap, ..Default::default() });
        };
        let (mut lo, mut hi) = (y, y);
        // its body rows given (a revision schedule's: the sheet's revisions), past its headers in
        // the way it builds, in the columns of the headers; where they end
        let body_rows = |out: &mut Sheet2d, cols: &[f64], mut y: f64, lo: &mut f64, hi: &mut f64| -> f64 {
            let pitch = tb.pitch.max(0.005);
            for row in body {
                let (y0, y1) = if up { (y, y + pitch) } else { (y - pitch, y) };
                (*lo, *hi) = (lo.min(y0), hi.max(y1));
                if tb.grid {
                    rect(out, y0, y1);
                }
                let mut x = o[0];
                for (k, w) in cols.iter().enumerate() {
                    if k > 0 && tb.grid {
                        out.lines.push(vec![[m(x), m(y0)], [m(x), m(y1)]]);
                    }
                    if let Some(t) = row.get(k).filter(|t| !t.is_empty()) {
                        put(out, t, tb.body_looks.get(k), x, *w, y0, y1, false, false);
                    }
                    x += w;
                }
                y = if up { y1 } else { y0 };
            }
            y
        };
        // bottom of the title (top of the fixed height)
        let mut below_title = y;
        let sections: Vec<(usize, &(bool, Vec<f64>, Vec<f64>, Vec<(usize, usize, String)>))> = if up { tb.sections.iter().enumerate().rev().collect() } else { tb.sections.iter().enumerate().collect() };
        for (si, (title, cols, rows, cells)) in sections {
            // its column headers, unless hidden
            if !*title && !tb.headers {
                continue;
            }
            // with merged cells: each area drawn whole from its first cell (its background,
            // outline, text), its rows and columns placed first
            if tb.merges {
                let n = rows.len();
                // at least a text line high: rows with text, and all rows below the title; as
                // high as their cells' texts need (a cell over one row)
                let mut xs = vec![o[0]];
                for w in cols {
                    xs.push(xs[xs.len() - 1] + w);
                }
                let fits = |r: usize| -> f64 {
                    cells
                        .iter()
                        .filter(|c| !*title && c.0 == r && c.1 < cols.len())
                        .filter(|c| tb.spans.get(&(si, r, c.1)).is_none_or(|s| s.1 == 0))
                        .map(|c| {
                            let k1 = (c.1 + tb.spans.get(&(si, r, c.1)).map_or(0, |s| s.0)).min(cols.len() - 1);
                            body_row(tb.looks.get(&(si, r, c.1)), &c.2, xs[k1 + 1] - xs[c.1])
                        })
                        .fold(0.0, f64::max)
                };
                let heights: Vec<f64> = (0..n).map(|r| if *title && !cells.iter().any(|c| c.0 == r) { rows[r] } else { rows[r].max(tb.pitch).max(fits(r)) }.max(0.005)).collect();
                let (mut tops, mut bots) = (vec![0.0; n], vec![0.0; n]);
                if up {
                    for r in (0..n).rev() {
                        (bots[r], tops[r]) = (y, y + heights[r]);
                        y = tops[r];
                    }
                } else {
                    for r in 0..n {
                        (tops[r], bots[r]) = (y, y - heights[r]);
                        y = bots[r];
                    }
                }
                if n > 0 {
                    lo = lo.min(bots.iter().cloned().fold(f64::INFINITY, f64::min));
                    hi = hi.max(tops.iter().cloned().fold(f64::NEG_INFINITY, f64::max));
                }
                for r in 0..n {
                    for k in 0..cols.len() {
                        if tb.covered.contains(&(si, r, k)) {
                            continue;
                        }
                        let (right, down) = tb.spans.get(&(si, r, k)).copied().unwrap_or((0, 0));
                        let (k1, r1) = ((k + right).min(cols.len() - 1), (r + down).min(n - 1));
                        let (x0, x1, y0, y1) = (xs[k], xs[k1 + 1], bots[r1], tops[r]);
                        let look = tb.looks.get(&(si, r, k));
                        if let Some(bg) = look.and_then(|l| l.background) {
                            let q = |x: f64, y: f64| [m(x), m(y)];
                            out.fills.push(Fill2d { tris: vec![[q(x0, y0), q(x1, y0), q(x1, y1)], [q(x0, y0), q(x1, y1), q(x0, y1)]], color: bg, under: None });
                        }
                        if tb.grid {
                            out.lines.push(vec![[m(x0), m(y0)], [m(x1), m(y0)], [m(x1), m(y1)], [m(x0), m(y1)], [m(x0), m(y0)]]);
                        }
                        let text = cells.iter().find(|c| c.0 == r && c.1 == k).map(|c| c.2.clone()).or_else(|| (*title && r == 0 && k == 0 && !cells.iter().any(|c| c.0 == 0) && !tb.name.is_empty()).then(|| tb.name.clone()));
                        if let Some(text) = text {
                            put(out, &text, look, x0, x1 - x0, y0, y1, true, *title);
                        }
                    }
                }
                if *title {
                    below_title = y;
                } else {
                    y = body_rows(out, cols, y, &mut lo, &mut hi);
                }
                continue;
            }
            let rows_order: Vec<usize> = if up { (0..rows.len()).rev().collect() } else { (0..rows.len()).collect() };
            for r in rows_order {
                // at least a text line high: rows with text, and all rows below the title; as
                // high as their cells' texts need
                let texted = cells.iter().any(|c| c.0 == r);
                let lone = cols.len() <= 1 || cells.iter().filter(|c| c.0 == r).count() <= 1;
                let fits = cells.iter().filter(|c| !*title && c.0 == r).map(|c| body_row(tb.looks.get(&(si, r, c.1)), &c.2, if lone { width } else { cols.get(c.1).copied().unwrap_or(width) })).fold(0.0, f64::max);
                let h = if *title && !texted { rows[r] } else { rows[r].max(tb.pitch).max(fits) }.max(0.005);
                let (y0, y1) = if up { (y, y + h) } else { (y - h, y) };
                (lo, hi) = (lo.min(y0), hi.max(y1));
                // its background (a title cell's colour, over the whole row it spans)
                if let Some(bg) = (0..cols.len()).find_map(|k| tb.looks.get(&(si, r, k)).and_then(|l| l.background)) {
                    let q = |x: f64, y: f64| [m(x), m(y)];
                    out.fills.push(Fill2d { tris: vec![[q(o[0], y0), q(o[0] + width, y0), q(o[0] + width, y1)], [q(o[0], y0), q(o[0] + width, y1), q(o[0], y1)]], color: bg, under: None });
                }
                if tb.grid {
                    rect(out, y0, y1);
                }
                let mut row_cells: Vec<(usize, usize, String)> = cells.iter().filter(|c| c.0 == r).cloned().collect();
                if *title && row_cells.is_empty() && !tb.name.is_empty() {
                    row_cells.push((r, 0, tb.name.clone()));
                }
                let multi = cols.len() > 1 && row_cells.len() > 1;
                let mut x = o[0];
                for (k, w) in cols.iter().enumerate() {
                    if multi && k > 0 && tb.grid {
                        out.lines.push(vec![[m(x), m(y0)], [m(x), m(y1)]]);
                    }
                    if let Some(c) = row_cells.iter().find(|c| c.1 == k) {
                        let (cx, cw) = if multi { (x, *w) } else { (o[0], width) };
                        put(out, &c.2, tb.looks.get(&(si, r, k)), cx, cw, y0, y1, true, *title);
                    }
                    x += w;
                }
                y = if up { y1 } else { y0 };
            }
            if *title {
                below_title = y;
            }
            if !*title {
                y = body_rows(out, cols, y, &mut lo, &mut hi);
            }
        }
        // a keynote legend: its keynotes (those of the sheet when it lists only them), in the
        // columns of its headers, a blank row first when it has one
        if let (Some(by_sheet), false) = (tb.keynotes, up) {
            // (of the sheet: the keynotes drawn on it, as Revit lists them; not the sheets their
            // tags last saved, which can be out of date)
            let mut order = key_order(out, false);
            let shown = key_order(out, true);
            let mut rows: Vec<(&str, &str)> = self.ann.keynotes.iter().filter(|k| !by_sheet || shown.contains(&k.0)).map(|k| (k.0.as_str(), k.1.as_str())).collect();
            rows.sort();
            rows.dedup_by(|a, b| a.0 == b.0);
            // their numbers: those of the keynotes drawn on the sheet, then the others by key
            for (k, _) in &rows {
                if !order.iter().any(|o| o == k) {
                    order.push(k.to_string());
                }
            }
            let number = |k: &str| order.iter().position(|o| o == k).unwrap_or(usize::MAX);
            rows.sort_by_key(|r| number(r.0));
            let rows: Vec<(String, &str)> = rows.into_iter().map(|(k, t)| ((number(k) + 1).to_string(), t)).collect();
            let cols = tb.sections.iter().rev().find(|s| !s.0).map(|s| s.1.clone()).unwrap_or_else(|| vec![width]);
            // rows of about 1.5 times its text (measured: 5.9 mm for 11.3 pt)
            let size = tb.body_looks.iter().map(|l| l.size).fold(0.0, f64::max);
            let pitch = if size > 0.0 { size / 72.0 / 12.0 * 1.475 } else { tb.pitch.max(0.005) };
            if tb.spacer && !rows.is_empty() {
                y -= pitch;
            }
            for (key, text) in &rows {
                let (key, text) = (key.as_str(), *text);
                // (as high as its text wrapped at its column's width needs: "… (HORS LOT) -" /
                // "ARCHITECTE.", P-212)
                let tall = cols.get(1).map_or(0.0, |w| body_row(tb.body_looks.get(1), text, *w));
                let (y0, y1) = (y - pitch.max(tall), y);
                lo = lo.min(y0);
                if tb.grid {
                    rect(out, y0, y1);
                }
                let mut x = o[0];
                for (k, w) in cols.iter().enumerate() {
                    if k > 0 && tb.grid {
                        out.lines.push(vec![[m(x), m(y0)], [m(x), m(y1)]]);
                    }
                    let t = if k == 0 { key } else if k == 1 { text } else { "" };
                    if !t.is_empty() {
                        // (its number level with the text's first line: "3", P-212)
                        put(out, t, tb.body_looks.get(k), x, *w, if k == 1 { y0 } else { y1 - pitch }, y1, k == 1, false);
                    }
                    x += w;
                }
                y = y0;
            }
        }
        if tb.outline && !tb.grid {
            rect(out, lo, hi);
        }
        let bounds = |lo: f64| Some([o[0], lo.min(y), o[0] + width, hi]);
        // fixed height: empty body rows down to its bottom, with the columns of the headers
        let (Some(fixed), false) = (tb.fixed, up) else { return bounds(lo) };
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
        Some([o[0], lo.min(y), o[0] + width, hi])
    }

    /// A model view on the sheet: the edges of the elements it sees, clipped by its crop
    /// region, at the viewport's scale. Returns the frame (feet, paper).
    fn draw_view(&self, out: &mut Sheet2d, view: &ViewInfo, vp: &Vp, edges: &Edges<'_>) -> Option<[f64; 4]> {
        let f0 = out.fills.len();
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
        // plan regions of the view: (outline in plan, bottom, cut, top), absolute feet
        let regions: Vec<&(Vec<P2>, f64, f64, f64)> = self.plan_regions.iter().filter(|r| mine(r.0) || view.regions.contains(&r.1)).map(|r| &r.2).collect();
        // the top clip plane (absolute feet): MEP elements above the cut plane shown up to it
        let view_top = if view.dir.is_some_and(|d| d[2].abs() > 0.99) { self.plan_range(view).map(|r| r.2) } else { None };
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
        let mut seg_colors: Vec<Option<SegStyle>> = Vec::new();
        let mut tris: Vec<[V3; 3]> = Vec::new();
        // the cut plane (plans: the cut height; sections and elevations: their line), on the
        // side of the kept box towards the viewer, in clip coordinates
        let cut_z = keep.filter(|_| !view.class.to_ascii_lowercase().contains("3d")).and_then(|k| {
            let z = if dot(k.ax[2], nz) > 0.0 { k.hi[2] } else { k.lo[2] };
            z.is_finite().then_some(z)
        });
        let mut cuts: Vec<[V3; 2]> = Vec::new();
        // (each cut line's colour: an MEP element's system's, else black)
        let mut cut_colors: Vec<[u8; 3]> = Vec::new();
        // the cut pieces of each hatched element
        let mut hatch_runs: Vec<std::ops::Range<usize>> = Vec::new();
        // faces filled by a view filter's projection fill: (triangle, pattern, colour)
        // (and the weight of its pattern's lines: the element's projection lines')
        let mut fill_tris: Vec<([V3; 3], i64, [u8; 3], f64)> = Vec::new();
        // solid fills of the 2D families placed in model families (view coordinates, colour)
        let mut sym_fills: Vec<([V3; 3], [u8; 3])> = Vec::new();
        // the texts of the annotations nested in model families: (type, its place in the model)
        let mut nested_notes: Vec<(i64, Trf, Given)> = Vec::new();
        // the elements drawn as one line (no faces: they hide nothing)
        let mut line_only: crate::det::HashSet<u32> = Default::default();
        // the model elements drawn (their tags are shown only then)
        let mut drawn: crate::det::HashSet<i64> = Default::default();
        // (the elements drawn that left lines or fills in the view)
        let mut inked: crate::det::HashSet<i64> = Default::default();
        // (the elements within its range, up to its top clip plane, drawn or not: their tags show)
        let mut in_range: crate::det::HashSet<i64> = Default::default();
        let mut range_info: HashMap<i64, String> = HashMap::default();
        let model_cats: HashMap<i64, i64> = edges.items.iter().map(|e| (e.0, e.1)).collect();
        // drafting views and legends draw no model, only their own lines, texts and symbols
        let drafting = view.class.contains("Drafting") || view.class.contains("Legend");
        // the detail level: its template's, its own, its primary view's (fine by default)
        let detail = [self.views.get(&view.template), Some(view), self.views.get(&view.primary)].into_iter().flatten().map(|v| v.detail).find(|d| (1..=3).contains(d)).unwrap_or(3);
        let plan_view = view.dir.is_some_and(|d| d[2].abs() > 0.99);
        // (a view on a sheet drawn whole; one drawn alone, a working view of the whole model,
        // only up to `MAX_SEGMENTS`)
        let max_segments = if vp.owner > 0 { usize::MAX } else { MAX_SEGMENTS };
        // (debugging: what each element added, to name those in a region of the sheet)
        let mut owners: Vec<(i64, i64, usize, usize, usize, usize, usize, usize)> = Vec::new();
        // whose each segment and each face is (index in `who`)
        let (mut seg_owner, mut tri_owner, mut who): (Vec<u32>, Vec<u32>, Vec<i64>) = (Vec::new(), Vec::new(), Vec::new());
        // a category's level: the one the view (its template, its primary) sets it to, else
        // the view's
        let detail_of = |cat: i64| [self.views.get(&view.template), Some(view), self.views.get(&view.primary)].into_iter().flatten().find_map(|v| v.cat_detail.iter().find(|c| c.0 == cat).map(|c| c.1)).unwrap_or(detail);
        for (id, cat, lo, hi, levels, mesh) in edges.items.iter().filter(|_| !drafting) {
            let detail = detail_of(*cat);
            let (list, smooth) = (&levels[(detail - 1).clamp(0, 2) as usize].0, &levels[(detail - 1).clamp(0, 2) as usize].1);
            let me = who.len().saturating_sub(1) as u32;
            seg_owner.resize(segs.len(), me);
            tri_owner.resize(tris.len(), me);
            who.push(*cat);
            let marks = (segs.len(), cuts.len(), fill_tris.len());
            let sym0 = sym_fills.len();
            // inside a plan region: its own view range
            // (Revit shows the part of an element inside a region with the region's range, the
            // rest with the view's: a region it reaches that shows it, else the view's range when
            // it reaches past the regions, else the region holding it; AC-211, AC-225)
            let region = match (&keep, regions.is_empty()) {
                (Some(_), false) => {
                    let (a, b) = (ft(*lo), ft(*hi));
                    let c = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
                    let corners = [[a[0], a[1]], [b[0], a[1]], [b[0], b[1]], [a[0], b[1]]];
                    let reaches = |r: &&&(Vec<P2>, f64, f64, f64)| inside_poly(&r.0, c) || corners.iter().any(|p| inside_poly(&r.0, *p)) || r.0.iter().any(|p| p[0] >= a[0] && p[0] <= b[0] && p[1] >= a[1] && p[1] <= b[1]);
                    let top_of = |r: &(Vec<P2>, f64, f64, f64)| if ABOVE_CUT.contains(cat) { r.3 } else { r.2 };
                    let past = corners.iter().any(|p| !regions.iter().any(|r| inside_poly(&r.0, *p)));
                    regions.iter().filter(reaches).find(|r| a[2] <= top_of(r) && b[2] >= r.1).or_else(|| if past { None } else { regions.iter().find(|r| inside_poly(&r.0, c)) })
                }
                _ => None,
            };
            let keep_e = match (&keep, region) {
                (Some(k), Some(r)) => {
                    let s = k.ax[2][2].signum();
                    let (a, b) = ((r.1 - k.orig[2]) * s, (r.2 - k.orig[2]) * s);
                    Some(Clip { orig: k.orig, ax: k.ax, lo: [k.lo[0], k.lo[1], a.min(b)], hi: [k.hi[0], k.hi[1], a.max(b)] })
                }
                _ => keep,
            };
            // ducts, pipes and the like above the cut plane: shown up to the top clip plane
            let top = region.map(|r| r.3).or(view_top);
            let to_top = |k: Clip, top: f64| {
                let c = (top - k.orig[2]) * k.ax[2][2].signum();
                if dot(k.ax[2], nz) < 0.0 { Clip { lo: [k.lo[0], k.lo[1], k.lo[2].min(c)], ..k } } else { Clip { hi: [k.hi[0], k.hi[1], k.hi[2].max(c)], ..k } }
            };
            let range = match (keep_e, top) {
                (Some(k), Some(top)) if top.is_finite() => Some(to_top(k, top)),
                (k, _) => k,
            };
            // (a plumbing fixture by its origin: P-222's floor drain "SS 03", standing below the
            // plan's range and reaching into it, keeps no tag)
            let origin_out = plan_view && *cat == PLUMBING_FIXTURES && range.zip(self.model_insts.get(id)).is_some_and(|(k, (_, t, _))| {
                let c = k.coords(t.apply([0.0; 3]));
                c[2] < k.lo[2] || c[2] > k.hi[2]
            });
            if !self.hides(view, *id, *cat) && !range.is_some_and(|k| k.misses(ft(*lo), ft(*hi))) && !origin_out {
                in_range.insert(*id);
            }
            if let Some(k) = range.filter(|_| std::env::var_os("TAG_DEBUG").is_some()) {
                let (a, b) = k.box_range(ft(*lo), ft(*hi));
                range_info.insert(*id, format!("cat {cat} box {:?}..{:?} range {:?}..{:?} hidden {}", a.map(|v| (v * 100.0).round() / 100.0), b.map(|v| (v * 100.0).round() / 100.0), k.lo.map(|v| (v * 100.0).round() / 100.0), k.hi.map(|v| (v * 100.0).round() / 100.0), self.hides(view, *id, *cat)));
            }
            if std::env::var("RANGE_DEBUG").is_ok_and(|v| v == id.to_string()) {
                let c = ft([(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, 0.0]);
                eprintln!("RANGEC {id} view {} centre {:.2},{:.2} box {:.2},{:.2}..{:.2},{:.2} corners in region {:?}", view.name, c[0], c[1], ft(*lo)[0], ft(*lo)[1], ft(*hi)[0], ft(*hi)[1], region.map(|r| [[ft(*lo)[0], ft(*lo)[1]], [ft(*hi)[0], ft(*lo)[1]], [ft(*hi)[0], ft(*hi)[1]], [ft(*lo)[0], ft(*hi)[1]]].map(|p| inside_poly(&r.0, p))));
                for r in &regions {
                    let b = r.0.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]);
                    eprintln!("  REGION {} pts {} box {:.2},{:.2}..{:.2},{:.2} z {:.2} {:.2} {:.2}", view.name, r.0.len(), b[0], b[1], b[2], b[3], r.1, r.2, r.3);
                }
                eprintln!("RANGE {id} view {} regions {} region {:?} top {top:?} range z {:?} box z {:.2}..{:.2} misses {:?}", view.name, regions.len(), region.map(|r| (r.1, r.2, r.3)), range.map(|k| (k.orig[2], k.lo[2], k.hi[2], k.ax[2])), ft(*lo)[2], ft(*hi)[2], range.map(|k| k.misses(ft(*lo), ft(*hi))));
            }
            let keep_e = match (keep_e, top) {
                (Some(_), Some(top)) if (ABOVE_CUT.contains(cat) || self.host_category(*id).is_some_and(|c| ABOVE_CUT.contains(&c))) && top.is_finite() => range,
                (k, _) => k,
            };
            if self.hides(view, *id, *cat) || keep_e.is_some_and(|k| k.misses(ft(*lo), ft(*hi))) || !self.shows_in_host(view, *id, detail) {
                continue;
            }
            // in a plan, the view depth picks the elements shown but cuts none of them: one
            // reaching below it is drawn whole (an elbow turning down through it)
            let keep_e = if plan_view && std::env::var_os("RVT_DEPTH_CLIP").is_none() { keep_e.map(|k| Clip { hi: [k.hi[0], k.hi[1], if dot(k.ax[2], nz) < 0.0 { f64::INFINITY } else { k.hi[2] }], lo: [k.lo[0], k.lo[1], if dot(k.ax[2], nz) > 0.0 { f64::NEG_INFINITY } else { k.lo[2] }], ..k }) } else { keep_e };
            drawn.insert(*id);
            // the view's filters: line colour, fill, halftone (its own graphics in the view first)
            let look = self.elem_look(view, *id, self.filter_look(view, *id, *cat));
            // its lines: its category's style (pen at the view's scale, colour, pattern), the
            // colour changed by a filter or its MEP system, halftone
            // (a family's solid in its subcategory's style, the project's for that name)
            let style = self.ann.category_style(*cat);
            // (an equipment too, when joined to one system only)
            let sys_on = SYSTEM_COLORED.contains(cat) || (*cat == MECHANICAL_EQUIPMENT && self.elem_systems.contains_key(id));
            let sys = self.system_color(*id).filter(|_| sys_on);
            // (and its weight and pattern)
            let (sys_pen, sys_pattern) = if sys_on { self.system_lines(*id) } else { (None, None) };
            let look_of = |st: Option<(i64, [u8; 3], i64)>| -> Option<SegStyle> {
                look.line
                    .or(sys)
                    .or(st.map(|s| s.1))
                    .map(|c| if look.halftone { halftone(c) } else { c })
                    .or(look.halftone.then_some([128, 128, 128]))
                    .map(|c| (look.pen.or(sys_pen).or(st.map(|s| s.0)).map_or(self.ann.pen(1), |p| self.ann.model_pen(p, vp.scale)), c, look.pattern.or(sys_pattern).or(st.map(|s| s.2)).unwrap_or(-3000010)))
            };
            let edge_look = |sub: i64| look_of(if sub != -1 { self.ann.subcategory_style(sub).or(style) } else { style });
            if std::env::var("RVT_LOOK_DEBUG").is_ok_and(|v| v == id.to_string()) {
                let subs: std::collections::BTreeSet<i64> = list.iter().map(|e| e.1).collect();
                eprintln!("EDGES {id} cat {cat} style {style:?} subs {:?} mesh cats {:?}", subs.iter().map(|s| (*s, self.ann.subcategory_style(*s), edge_look(*s))).collect::<Vec<_>>(), mesh.map(|m| m.cats.clone()));
                for e in list.iter() {
                    let f = |p: V3| format!("({:.3} {:.3} {:.3})", p[0], p[1], p[2]);
                    eprintln!("  edge {} {}", f(e.0[0]), f(e.0[1]));
                }
                if let Some(m) = mesh {
                    eprintln!("  mesh {} pos {} tris", m.pos.len(), m.tri.len());
                }
            }
            let line_col = edge_look(-1);
            if let (Some((pat, col)), true) = (look.fill, view.dir.is_some_and(|d| d[2].abs() > 0.99)) {
                let col = if look.halftone { halftone(col) } else { col };
                // the faces seen from above
                // (the weight of its solid's subcategory, else its category's)
                let fw = mesh.and_then(|m| m.cats.first()).and_then(|c| edge_look(c.1)).or(line_col).map_or(self.ann.pen(1), |l| l.0);
                match mesh {
                    Some(m) => {
                        for t in &m.tri {
                            let q = t.map(|i| ft(m.pos.get(i as usize).copied().unwrap_or_default().map(|x| x as f64)));
                            let (u, v) = (sub(q[1], q[0]), sub(q[2], q[0]));
                            let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
                            if dot(n, nz) > 1e-12 && fill_tris.len() < 200_000 {
                                fill_tris.push((q.map(to_view), pat, col, fw));
                            }
                        }
                    }
                    None => {
                        let (a, b) = (ft(*lo), ft(*hi));
                        let c = |x: f64, y: f64| to_view([x, y, b[2]]);
                        fill_tris.push(([c(a[0], a[1]), c(b[0], a[1]), c(b[0], b[1])], pat, col, fw));
                        fill_tris.push(([c(a[0], a[1]), c(b[0], b[1]), c(a[0], b[1])], pat, col, fw));
                    }
                }
            }
            // a duct or pipe seen from above: its outline (two sides and its ends), as Revit
            // draws it, rather than the edges of its faceted mesh
            let outline = self.mep_lines.get(id).zip(self.mep_widths.get(id)).filter(|_| view.dir.is_some_and(|d| d[2].abs() > 0.99) && keep_e.is_some());
            let around = |(a, b): &(V3, V3), r: f64| -> Vec<[V3; 2]> {
                {
                    let (pa, pb) = (to_view(*a), to_view(*b));
                    let (dx, dy) = (pb[0] - pa[0], pb[1] - pa[1]);
                    let l = (dx * dx + dy * dy).sqrt();
                    if l <= 1e-6_f64.max(dot(sub(*b, *a), sub(*b, *a)).sqrt() * 0.01) {
                        // seen end on (within a hundredth of its length): a circle (a rectangular
                        // duct's section is drawn by `section`)
                        (0..24).map(|i| {
                            let (t0, t1) = (i as f64 / 24.0 * std::f64::consts::TAU, (i + 1) as f64 / 24.0 * std::f64::consts::TAU);
                            [[pa[0] + r * t0.cos(), pa[1] + r * t0.sin(), pa[2]], [pa[0] + r * t1.cos(), pa[1] + r * t1.sin(), pa[2]]]
                        }).collect()
                    } else {
                        let (nx, ny) = (-dy / l * r, dx / l * r);
                        let c = |p: V3, s: f64| [p[0] + nx * s, p[1] + ny * s, p[2]];
                        vec![[c(pa, 1.0), c(pb, 1.0)], [c(pa, -1.0), c(pb, -1.0)], [c(pa, 1.0), c(pa, -1.0)], [c(pb, 1.0), c(pb, -1.0)]]
                    }
                }
            };
            // (a pipe at coarse and medium, a duct at coarse: one line, its axis)
            let single = matches!(*cat, PIPES | FLEX_PIPES) && detail <= 2 || matches!(*cat, DUCTS | FLEX_DUCTS) && detail <= 1;
            if single {
                line_only.insert(who.len().saturating_sub(1) as u32);
            }
            // (a rectangular duct seen end on: its section, its width across and its height along
            // its up, corners in view coordinates, in turn)
            let rect = self.duct_sections.get(id).filter(|_| !self.mep_size_texts.get(id).is_some_and(|t| t.contains(['ø', 'Ø', '⌀'])));
            // (`grow`: past each side, for its insulation)
            let section_at = |grow: f64| outline.and_then(|((a, b), w)| {
                let (h, up, _) = rect?;
                let (pa, pb) = (to_view(*a), to_view(*b));
                let d = sub(*b, *a);
                let l = dot(d, d).sqrt().max(1e-12);
                // (end on: seen within a hundredth of its length)
                if (pb[0] - pa[0]).hypot(pb[1] - pa[1]) > 1e-6_f64.max(l * 0.01) {
                    return None;
                }
                let n = [d[0] / l, d[1] / l, d[2] / l];
                let u = sub(*up, n.map(|x| x * dot(*up, n)));
                let lu = dot(u, u).sqrt();
                if lu < 1e-9 {
                    return None;
                }
                let u = u.map(|x| x / lu);
                let side = [u[1] * n[2] - u[2] * n[1], u[2] * n[0] - u[0] * n[2], u[0] * n[1] - u[1] * n[0]];
                Some([(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(i, j): (f64, f64)| to_view(std::array::from_fn(|k| a[k] + side[k] * i * (w * 0.5 + grow) + u[k] * j * (h * 0.5 + grow)))))
            });
            let section = section_at(0.0);
            let outline_segs: Vec<[V3; 2]> = outline.map_or_else(Vec::new, |((a, b), w)| match section {
                _ if single => vec![[to_view(*a), to_view(*b)]],
                Some(c) => (0..4).map(|i| [c[i], c[(i + 1) % 4]]).collect(),
                None => around(&(*a, *b), w * 0.5),
            });
            for sg in &outline_segs {
                segs.push(*sg);
                seg_colors.push(line_col);
            }
            // a duct's centre line (its axis, in the centre line subcategory's style, its
            // colour); seen end on, its rise or drop symbol: a round duct's circle crossed by a
            // diagonal ("\" drawn top down, "/" bottom up), a pipe drawn as one line a broken
            // circle with a tick towards the pipe it turns into (1.5 mm on paper), as Revit
            // draws them in plan; on the element's top, never hidden by its own faces
            if let Some(((a, b), w)) = outline {
                let (pa, pb) = (to_view(*a), to_view(*b));
                // (seen along it unless within a hundredth of its length, as `section`)
                let along = ((pb[0] - pa[0]).powi(2) + (pb[1] - pa[1]).powi(2)).sqrt() > 1e-6_f64.max(dot(sub(*b, *a), sub(*b, *a)).sqrt() * 0.01);
                // (the element's side nearest the viewer)
                let top = to_view(ft(*lo))[2].max(to_view(ft(*hi))[2]);
                let lift = |p: V3| -> V3 {
                    let q = to_view(p);
                    [q[0], q[1], top]
                };
                if *cat == DUCTS && along && !single && !self.hides(view, 0, DUCT_CENTER_LINE) {
                    segs.push([lift(*a), lift(*b)]);
                    seg_colors.push(look_of(self.ann.category_style(DUCT_CENTER_LINE)));
                }
                // a rectangular one: the diagonal of its section and the half under it filled,
                // its right angle at the bottom, on the right when its air flows up ("/"), on the
                // left when it flows down ("\\")
                if let (Some(c), true) = (section.filter(|_| *cat == DUCTS && !single), !self.hides(view, 0, DUCT_RISE_DROP)) {
                    let rd = look_of(self.ann.category_style(DUCT_RISE_DROP));
                    let flow = rect.map_or(0, |r| r.2);
                    let up = (b[2] - a[2]) * if flow == 0 { 1.0 } else { flow as f64 } > 0.0;
                    let c = c.map(|q| [q[0], q[1], top]);
                    let bottom = (0..4).min_by(|&i, &j| (c[i][1] + c[(i + 1) % 4][1]).total_cmp(&(c[j][1] + c[(j + 1) % 4][1]))).unwrap_or(0);
                    let (i, j) = (bottom, (bottom + 1) % 4);
                    let (bl, br) = if c[i][0] <= c[j][0] { (i, j) } else { (j, i) };
                    let other = |k: usize, not: usize| if (k + 1) % 4 != not { (k + 1) % 4 } else { (k + 3) % 4 };
                    let (tl, tr) = (other(bl, br), other(br, bl));
                    let (diag, tri) = if up { ([c[bl], c[tr]], [c[bl], c[br], c[tr]]) } else { ([c[tl], c[br]], [c[tl], c[bl], c[br]]) };
                    segs.push(diag);
                    seg_colors.push(rd);
                    // (filled in the rise/drop style's own colour: black, whatever the system's)
                    let fill = self.ann.category_style(DUCT_RISE_DROP).map_or([0; 3], |st| st.1);
                    sym_fills.push((tri, if look.halftone { halftone(fill) } else { fill }));
                } else if *cat == DUCTS && !along && !self.hides(view, 0, DUCT_RISE_DROP) {
                    let r = w * 0.5 * std::f64::consts::FRAC_1_SQRT_2;
                    let (dx, dy) = if a[2] > b[2] { (-r, r) } else { (r, r) };
                    segs.push([lift([a[0] + dx, a[1] + dy, a[2]]), lift([a[0] - dx, a[1] - dy, a[2]])]);
                    seg_colors.push(look_of(self.ann.category_style(DUCT_RISE_DROP)));
                }
                if *cat == PIPES && single && !along && !self.hides(view, 0, PIPE_RISE_DROP) && vp.scale > 1e-9 {
                    let rr = 0.0015 / vp.scale / FEET;
                    // towards the pipe it turns into: the nearest end of a pipe drawn along
                    let near = self
                        .mep_lines
                        .iter()
                        .filter(|(o, (p, q))| **o != *id && ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt() > 1e-3 && (p[2] - q[2]).abs() < 1e-3)
                        .flat_map(|(_, (p, q))| [(*p, *q), (*q, *p)])
                        .map(|(n, f)| ((n[0] - a[0]).hypot(n[1] - a[1]), n, f))
                        .filter(|c| c.0 < 2.0)
                        .min_by(|x, y| x.0.total_cmp(&y.0));
                    let dir = near.map_or(std::f64::consts::FRAC_PI_2, |(_, n, f)| (f[1] - n[1]).atan2(f[0] - n[0]));
                    let (t0, t1) = (dir + std::f64::consts::FRAC_PI_4, dir + std::f64::consts::TAU - std::f64::consts::FRAC_PI_4);
                    let at = |t: f64| lift([a[0] + rr * t.cos(), a[1] + rr * t.sin(), a[2]]);
                    let look_p = look_of(self.ann.category_style(PIPE_RISE_DROP));
                    let n = 18;
                    for i in 0..n {
                        let (u, v) = (t0 + (t1 - t0) * i as f64 / n as f64, t0 + (t1 - t0) * (i + 1) as f64 / n as f64);
                        segs.push([at(u), at(v)]);
                        seg_colors.push(look_p);
                    }
                    segs.push([lift(*a), at(dir)]);
                    seg_colors.push(look_p);
                }
            }
            // its insulation: the outline grown by its thickness, in its category's style and
            // the duct's colour (its two sides, the ends meeting the fittings' own; seen end on,
            // all round)
            if let (Some((ends, w)), Some((th, icat)), false) = (outline, self.insulation_of(*id), single) {
                let ist = self.ann.category_style(icat);
                let look_i = line_col.map(|(_, c, _)| (ist.map_or(self.ann.pen(1), |s| self.ann.model_pen(s.0, vp.scale)), c, ist.map_or(-3000010, |s| s.2)));
                // (seen end on: all round, or its section grown)
                let ring: Vec<[V3; 2]> = match section_at(th) {
                    Some(c) => (0..4).map(|i| [c[i], c[(i + 1) % 4]]).collect(),
                    None => {
                        let all = around(ends, w * 0.5 + th);
                        let n = if all.len() > 4 { all.len() } else { 2 };
                        all.into_iter().take(n).collect()
                    }
                };
                for sg in ring {
                    segs.push(sg);
                    seg_colors.push(look_i);
                }
            }
            // its sharp edges, and its silhouette: the smooth edges between a facet turned to
            // the viewer and one turned away
            let silhouette: Vec<([V3; 2], i64)> = smooth.iter().filter(|(_, a, b, _)| dot(*a, nz) * dot(*b, nz) <= 0.0).map(|(e, _, _, sub)| (*e, *sub)).collect();
            for (e, sub) in list.iter().chain(silhouette.iter()).filter(|_| outline_segs.is_empty()) {
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
                    seg_colors.push(if *sub == -1 { line_col } else { edge_look(*sub) });
                }
            }
            // the lines of its family drawn in its plan (an equipment's maintenance zone…), in
            // their style, as the view's detail level and the family's parameters show them
            if let (Some((sym, t, ints)), true) = (self.model_insts.get(id), plan_view) {
                if let Some(&root) = self.sym_family.get(sym).and_then(|f| self.fam_roots.get(f)) {
                    let vals: HashMap<i64, bool> = self.sym_ints.get(sym).into_iter().flatten().chain(ints.iter()).filter_map(|(p, v)| Some((self.ann.doc_param(root, *p)?, *v != 0))).collect();
                    // a line drawn twice (two outlines sharing a side) is drawn once, as Revit
                    // does (else the dashes of the two fill each other's gaps)
                    let mut seen: crate::det::HashSet<[i64; 6]> = Default::default();
                    let key = |p: V3, q: V3| {
                        let r = |p: V3| p.map(|x| (x * 1e4).round() as i64);
                        let (a, b) = (r(p), r(q));
                        let (a, b) = if a <= b { (a, b) } else { (b, a) };
                        [a[0], a[1], a[2], b[0], b[1], b[2]]
                    };
                    let dbg = std::env::var("RVT_FL_DEBUG").is_ok_and(|v| v == id.to_string());
                    if dbg {
                        if let Some(k) = &keep_e {
                            eprintln!("  clip orig {:?} z axis {:?} lo {:?} hi {:?}", k.orig, k.ax[2], k.lo, k.hi);
                            let zs: Vec<f64> = self.ann.family_lines(root).iter().map(|l| (t.apply(l.0[0])[2] * 1000.0).round() / 1000.0).collect();
                            eprintln!("  world z {:?}", zs.iter().fold(Vec::<f64>::new(), |mut v, z| { if !v.contains(z) { v.push(*z) } v }));
                        }
                        let t = self.views.get(&view.template);
                        eprintln!("FL {id} sym {sym} root {root} detail {detail} view {} template {} ({:?} hidden cats) own hidden cats {} vals {vals:?}", view.name, view.template, t.map(|t| t.hidden_cats.len()), view.hidden_cats.len());
                    }
                    for (pts, ls, fev, vis) in self.ann.family_lines(root) {
                        if dbg {
                            let w = |p: V3| { let q = t.apply(self.stretches.get(id).map_or(p, |s| s.apply(p))); [(q[0] * 304.8).round() / 1000.0, (q[1] * 304.8).round() / 1000.0, (q[2] * 304.8).round() / 1000.0] };
                            eprintln!("  world m {:?}..{:?}", w(pts[0]), w(pts[pts.len() - 1]));
                            let sub = self.ann.family_line_subcategory(root, ls);
                            eprintln!("  line style {ls} -> {:?} sub {sub:?} in template {:?} hidden {:?} fev {fev:#x} vis {vis:?} shows {} z {:?}", self.ann.family_line_style(root, ls), sub.map(|c| self.views.get(&view.template).is_some_and(|t| t.hidden_cats.contains(&c))), sub.map(|c| self.hides(view, 0, c)), vis.shows(&vals), pts.iter().map(|p| (p[2] * 1000.0).round() / 1000.0).collect::<Vec<_>>());
                        }
                        // flat in its frame (the lines of its elevations are not seen from above)
                        if !vis.shows(&vals) || (fev != -1 && fev & (1 << (12 + detail)) == 0) || pts.iter().any(|p| (p[2] - pts[0][2]).abs() > 1e-6) {
                            continue;
                        }
                        let st = self.ann.family_line_style(root, ls).or(style);
                        if st.is_some_and(|s| s.2 == -1) {
                            continue; // invisible lines
                        }
                        let look_l = look_of(st);
                        // a symbolic line (no view of its own, `m_famElemVisibility` without
                        // view bits) is the element's plan symbol: drawn on its top
                        let symbolic = fev != -1 && fev & 0x3e == 0;
                        let top = ft(*hi)[2];
                        for w in pts.windows(2) {
                            if !seen.insert(key(w[0], w[1])) {
                                continue;
                            }
                            let fit = |p: V3| self.stretches.get(id).map_or(p, |s| s.apply(p));
                            let lift = |p: V3| if symbolic { [p[0], p[1], top] } else { p };
                            let (p, q) = (lift(t.apply(fit(w[0]))), lift(t.apply(fit(w[1]))));
                            let (p, q) = match &keep_e {
                                Some(k) => {
                                    let (mut a, mut b) = (k.coords(p), k.coords(q));
                                    // (a symbol stays within the view's depth)
                                    if symbolic {
                                        for c in [&mut a, &mut b] {
                                            *c = depth_within(k, *c);
                                        }
                                    }
                                    let Some((a, b)) = k.segment(a, b) else { continue };
                                    (back(k, a), back(k, b))
                                }
                                None => (p, q),
                            };
                            let (a, b) = (to_view(p), to_view(q));
                            if (a[0] - b[0]).abs() + (a[1] - b[1]).abs() > 1e-4 {
                                segs.push([a, b]);
                                seg_colors.push(look_l);
                            }
                        }
                    }
                    // the 2D families placed in it (a terminal's flow arrow): their drawing where
                    // they stand in it, moved with the plane they are on as its type moves it,
                    // on its top, in its look (its system's colour), their fills solid
                    let top = ft(*hi)[2];
                    for (ix, (_, nsym, ntf)) in self.nested(root) {
                        if self.inst_vis.get(ix).is_some_and(|v| !v.shows(&vals) || !v.at_detail(detail)) {
                            continue;
                        }
                        // (its type, when the element's family type parameter sets it)
                        let typed = self.driven_type(ix, *nsym, root, *id).unwrap_or(*nsym);
                        let nsym = &typed;
                        // (only one lying flat in the plan, the view showing its category: detail
                        // items, or generic annotations, these at their size on paper; flat once
                        // placed, exactly: a sprinkler tilted 3° on its sloped pipe shows no GHOST
                        // symbol, FP-113)
                        let annotation = self.symbols.get(nsym).and_then(|ns| self.fam_cats.get(&ns.family)) == Some(&GENERIC_ANNOTATIONS);
                        let pz = t.lin(ntf.lin([0.0, 0.0, 1.0]));
                        if pz[2].abs() < (1.0 - 1e-6) * dot(pz, pz).sqrt() || self.hides(view, 0, if annotation { GENERIC_ANNOTATIONS } else { DETAIL_ITEMS }) {
                            continue;
                        }
                        let paper = if annotation && vp.scale > 1e-9 { 1.0 / vp.scale } else { 1.0 };
                        let sized = ntf.after(&Trf::new([paper, 0.0, 0.0, 0.0, paper, 0.0, 0.0, 0.0, paper], [0.0; 3]));
                        let ntf = &sized;
                        let Some(d) = self.symbols.get(nsym).and_then(|ns| self.family_drawing(ns, self.inst_ints.get(ix))) else { continue };
                        let o = ntf.apply([0.0; 3]);
                        let shift = self.stretches.get(id).map_or([0.0; 3], |s| sub(s.apply(o), o));
                        let place = |p: P2| -> V3 {
                            let q = ntf.apply([p[0], p[1], 0.0]);
                            let w = t.apply([q[0] + shift[0], q[1] + shift[1], q[2] + shift[2]]);
                            [w[0], w[1], top]
                        };
                        if annotation {
                            // (with the values its host sets: a grille's airflow "0 m3/h", AC-113)
                            let (hs, hr) = self.model_vals.get(id).cloned().unwrap_or_default();
                            let host = Given { strs: hs, ints: self.model_insts.get(id).map(|m| m.2.clone()).unwrap_or_default(), reals: hr };
                            let ty = Given { ints: self.model_insts.get(id).and_then(|m| self.sym_ints.get(&m.0)).cloned().unwrap_or_default(), ..Default::default() };
                            let ng = self.nested_given_from(ix, root, &ty, &host);
                            nested_notes.push((*nsym, t.after(&Trf::new([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], shift)).after(ntf), ng));
                        }
                        // (within the view's depth)
                        let at = |p: V3| -> V3 {
                            match &keep_e {
                                Some(k) => {
                                    depth_within(k, k.coords(p))
                                }
                                None => p,
                            }
                        };
                        let look_n = look_of(self.ann.category_style(DETAIL_ITEMS).or(style));
                        if std::env::var("RVT_FL_DEBUG").is_ok_and(|v| v == id.to_string()) {
                            eprintln!("  nested {nsym} at {o:?} shift {shift:?} strokes {} fills {} look {look_n:?} drives {:?} own {:?} host ints {:?} host elems {:?} tags {:?}", d.strokes.len(), d.fills.len(), self.inst_drives.get(ix), self.given(ix), self.model_insts.get(id).map(|m| m.2.iter().map(|(p, v)| (*p, *v, self.ann.doc_param(root, *p))).collect::<Vec<_>>()), self.elem_params.get(id), self.symbols.get(nsym).map(|s| s.tags.iter().map(|t| t.params.clone()).collect::<Vec<_>>()));
                        }
                        for (pts, sw, sc) in &d.strokes {
                            // (an annotation in its own lines' look: a GHOST symbol's red circle;
                            // a detail item in the element's: a terminal's arrow in its system's colour)
                            let look_s = if annotation { Some((*sw, *sc, -3000010)) } else { look_n };
                            for w in pts.windows(2) {
                                let (p, q) = (at(place(w[0])), at(place(w[1])));
                                let (p, q) = match &keep_e {
                                    Some(k) => {
                                        let Some((a, b)) = k.segment(p, q) else { continue };
                                        (back(k, a), back(k, b))
                                    }
                                    None => (p, q),
                                };
                                let (a, b) = (to_view(p), to_view(q));
                                if (a[0] - b[0]).abs() + (a[1] - b[1]).abs() > 1e-4 {
                                    segs.push([a, b]);
                                    seg_colors.push(look_s);
                                }
                            }
                        }
                        // (an annotation shows its lines and texts, not its fills: a GHOST symbol's
                        // solid filled region is not on Revit's prints, FP-113)
                        for (tris3, c) in d.fills.iter().filter(|_| !annotation) {
                            let col = look_n.map_or(*c, |l| l.1);
                            for tri in tris3 {
                                let q = tri.map(|p| at(place(p)));
                                // (whole in the crop)
                                if keep_e.is_some_and(|k| q.iter().any(|c| (0..2).any(|i| c[i] < k.lo[i] || c[i] > k.hi[i]))) {
                                    continue;
                                }
                                let q = q.map(|c| keep_e.as_ref().map_or(c, |k| back(k, c)));
                                sym_fills.push((q.map(to_view), col));
                            }
                        }
                    }
                }
            }
            owners.push((*id, *cat, marks.0, segs.len(), marks.1, cuts.len(), marks.2, fill_tris.len()));
            if (segs.len(), cuts.len(), fill_tris.len(), sym_fills.len()) != (marks.0, marks.1, marks.2, sym0) {
                inked.insert(*id);
            }
            // the faces that hide edges: the rebuilt solid, else the element's box
            // (a duct or pipe drawn as one line hides nothing)
            if hlr_on && tris.len() < MAX_FACES && !single {
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
                        for (ti, t) in m.tri.iter().enumerate() {
                            if !shown_at(tri_vis(m, ti), detail) {
                                continue;
                            }
                            add(t.map(|i| ft(m.pos.get(i as usize).copied().unwrap_or_default().map(|x| x as f64))));
                        }
                        // the element's section at the cut plane, drawn heavier
                        let cut_e = keep_e.as_ref().filter(|_| cut_z.is_some()).map(|k| if dot(k.ax[2], nz) > 0.0 { k.hi[2] } else { k.lo[2] }).filter(|z| z.is_finite());
                        if let (Some(k), Some(z)) = (keep_e.as_ref(), cut_e) {
                            let first = cuts.len();
                            if !NOT_CUT.contains(cat) && cuts.len() < max_segments {
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
                            // (an MEP element's cut in its system's colour, as its projection)
                            cut_colors.resize(cuts.len(), sys.map_or([0; 3], |c| if look.halftone { halftone(c) } else { c }));
                            // cut walls, floors, roofs and columns: their section hatched
                            if (HATCHED.contains(cat) || m.cut_hatched) && cuts.len() > first {
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
            if segs.len() > max_segments {
                break;
            }
        }
        // the view's annotations (not cut by the crop)
        // (less those hidden in the view or in its primary view)
        let mut ann = crate::rvt_annot::Drawn::default();
        let primary_hidden = self.views.get(&view.primary).map(|p| &p.hidden);
        let hide = |e: i64| view.hidden.contains(&e) || primary_hidden.is_some_and(|h| h.contains(&e));
        let over = |e: i64| self.line_over(view, e);
        self.ann.draw_hiding(vp.view, &proj, vp.scale, &hide, &over, &mut ann);
        if view.primary > 0 {
            self.ann.draw_hiding(view.primary, &proj, vp.scale, &hide, &over, &mut ann);
        }
        if uv_box.is_none() {
            // no crop: the extents of what is seen
            let mut e = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
            let texts = self.texts.iter().filter(|t| mine(t.0)).map(|t| proj(t.1.orig));
            let placed = self.instances.iter().filter(|i| mine(i.0)).map(|i| proj(i.2.apply([0.0; 3])));
            let lv = self.level_ends.iter().filter(|_| self.shows_levels(view)).flat_map(|(id, _)| {
                let z = self.level_z.get(id).copied().unwrap_or(0.0);
                let v = proj([0.0, 0.0, z])[1];
                let (u0, u1) = self.level_span(vp.view, view.primary, *id, &proj).map_or_else(|| self.model_span(&proj), |e| (e.0, e.1));
                [[u0, v], [u1, v]]
            });
            // its grids whole too (their ends as the view sets them): nothing cuts them
            let gr = self.grids.iter().filter(|g| self.grid_shown(view, &keep, g)).flat_map(|g| {
                let (a, b, _) = self.grid_ends(vp.view, view, g, &proj);
                [a, b]
            });
            let upright = view.dir.is_some_and(|d| d[2].abs() < 1e-6) && !view.hidden_cats.contains(&GRIDS);
            let gs = self.grids.iter().filter(|g| upright && !self.datum_hidden(view, g.id)).filter_map(|g| self.grid_cross(g, o, nz)).flat_map(|(a, b)| [proj(a), proj(b)]);
            let gr = gr.chain(gs);
            for p in segs.iter().flatten().map(|p| [p[0], p[1]]).chain(ann.strokes.iter().flat_map(|s| s.0.iter().copied())).chain(texts).chain(placed).chain(lv).chain(gr) {
                e = [e[0].min(p[0]), e[1].min(p[1]), e[2].max(p[0]), e[3].max(p[1])];
            }
            if !e[0].is_finite() {
                return None;
            }
            uv_box = Some(e);
        }
        let ub = uv_box?;
        // datums (levels, grids) whose ends follow the model are cut past a cropped view's crop:
        // 1" on paper (its annotation crop's offset when smaller) and 1/128" further, as Revit
        // draws them (P-212's annotation crop reaches 1.8" and 2.7" past it, its grid heads 1")
        let dub = match crop.filter(|_| cropped && vp.scale > 1e-9) {
            // (left, right, top, bottom offsets)
            Some(c) => {
                let d = |i: usize| (c.annot_off[i].min(1.0 / 12.0) + DATUM_PAST_ANNOT) / vp.scale;
                [ub[0] - d(0), ub[1] - d(3), ub[2] + d(1), ub[3] + d(2)]
            }
            None => ub,
        };
        // on a sheet a view puts its origin (its crop's, active or not) at the viewport offset;
        // a view drawn alone is centred on what it shows
        // (legends and drafting views put their origin there too: their title then sits under
        // their lower left corner, as in Revit)
        let legend = view.class.contains("Legend") || view.class.contains("Drafting");
        let (cu, cv) = if cropped || legend || vp.owner > 0 { (0.0, 0.0) } else { ((ub[0] + ub[2]) * 0.5, (ub[1] + ub[3]) * 0.5) };
        let k = vp.scale;
        if std::env::var_os("SHEET_DEBUG").is_some() {
            eprintln!("  view {} {:?} {} crop {:?} ub {:?} offset {:?} label {:?} len {} k {}", vp.view, view.name, view.class, crop.map(|c| (c.orig, c.off, c.active)), ub, vp.offset, vp.label, vp.label_len, k);
        }
        let to_paper = |p: P2| [vp.offset[0] + (p[0] - cu) * k, vp.offset[1] + (p[1] - cv) * k];
        // RVT_PICK=x0,y0,x1,y1,left,top: the elements drawing in that rectangle of the sheet
        // (millimetres from its top left corner; left and top: the sheet's extent, metres)
        if let Ok(v) = std::env::var("RVT_PICK") {
            let v: Vec<f64> = v.split(',').filter_map(|x| x.parse().ok()).collect();
            if v.len() == 6 {
                let inside = |p: P2| {
                    let q = to_paper(p);
                    let (x, y) = ((q[0] * FEET - v[4]) * 1000.0, (v[5] - q[1] * FEET) * 1000.0);
                    x >= v[0] && x <= v[2] && y >= v[1] && y <= v[3]
                };
                // a segment crossing the rectangle: one of 9 points along it inside
                let crosses = |s: &[V3; 2]| (0..=8).any(|i| {
                    let t = i as f64 / 8.0;
                    inside([s[0][0] + (s[1][0] - s[0][0]) * t, s[0][1] + (s[1][1] - s[0][1]) * t])
                });
                for &(id, cat, s0, s1, c0, c1, f0, f1) in &owners {
                    let (ns, nc, nf) = (segs[s0..s1].iter().filter(|s| crosses(s)).count(), cuts[c0..c1].iter().filter(|s| crosses(s)).count(), fill_tris[f0..f1].iter().filter(|t| inside([t.0[0][0], t.0[0][1]])).count());
                    if ns + nc + nf > 0 {
                        eprintln!("PICK view {} elem {id} cat {cat} segs {ns} cuts {nc} fills {nf}", view.name);
                    }
                }
            }
        }
        // FAM_DEBUG=<view id>:<part of a family name>: that family's model instances, where they
        // stand on the sheet (mm from its top left corner) and whether the view draws them
        if let Some((v, part)) = std::env::var("FAM_DEBUG").ok().as_deref().and_then(|d| d.split_once(':')).map(|(v, p)| (v.to_string(), p.to_string())) {
            if v == vp.view.to_string() {
                let e = out.extent;
                let fam = |sym: &i64| self.sym_family.get(sym).and_then(|f| self.fam_names.get(f)).cloned().unwrap_or_default();
                for (id, (sym, t, ints)) in self.model_insts.iter().filter(|(_, (sym, _, _))| fam(sym).contains(part.as_str())) {
                    let p = t.apply([0.0; 3]);
                    let q = to_paper(proj(p));
                    let item = edges.items.iter().find(|i| i.0 == *id);
                    eprintln!("FAM {id} {:?} type {sym} at {:.0},{:.0} mm z {:.3} drawn {} box z {:?} ints {ints:?}", fam(sym), (q[0] * FEET - e[0] as f64) * 1000.0, (e[3] as f64 - q[1] * FEET) * 1000.0, p[2], drawn.contains(id), item.map(|i| (ft(i.2)[2], ft(i.3)[2])));
                }
            }
        }
        // annotation crop: the annotations wholly outside it are not drawn
        let acrop = crop.and_then(|c| c.annot).filter(|_| cropped && k > 0.0).map(|a| {
            let (p, q) = (to_paper([ub[0], ub[1]]), to_paper([ub[2], ub[3]]));
            [p[0] - a[0], p[1] - a[3], q[0] + a[1], q[1] + a[2]]
        });
        let inside = |p: P2| acrop.map_or(true, |r| p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3]);
        let mcrop = acrop.map(|_| {
            let (p, q) = (to_paper([ub[0], ub[1]]), to_paper([ub[2], ub[3]]));
            [p[0], p[1], q[0], q[1]]
        });
        let in_model_crop = |p: P2| mcrop.map_or(true, |r| p[0] >= r[0].min(r[2]) && p[0] <= r[2].max(r[0]) && p[1] >= r[1].min(r[3]) && p[1] <= r[3].max(r[1]));
        let mut ann = ann;
        if acrop.is_some() && std::env::var_os("SHEET_DEBUG").is_some() {
            eprintln!("    annotation crop {acrop:?}");
        }
        if let Some(r) = acrop {
            // lines are cut at the annotation crop (a dependent view shows its part of the
            // primary view's dimensions and lines), the rest kept when it lies inside
            let from = |q: P2| [(q[0] - vp.offset[0]) / k + cu, (q[1] - vp.offset[1]) / k + cv];
            let (a, b) = (from([r[0], r[1]]), from([r[2], r[3]]));
            let box_v = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
            // dimensions and spots: whole when entirely inside, else left out; a chain of
            // dimensions crossing the crop is cut at it, its values inside kept (as Revit shows
            // the primary view's chains in its dependent views)
            let n = (ann.strokes.len(), ann.fills.len(), ann.texts.len());
            let mut ends: Vec<(usize, usize, usize)> = ann.wholes.iter().skip(1).copied().collect();
            ends.push(n);
            let (mut ks, mut kf, mut kt) = (vec![true; n.0], vec![true; n.1], vec![true; n.2]);
            let mut whole_s = vec![false; n.0];
            for (st, en) in ann.wholes.iter().zip(&ends) {
                let ok = ann.strokes[st.0..en.0].iter().all(|s| s.0.iter().all(|p| inside(to_paper(*p))))
                    && ann.texts[st.2..en.2].iter().all(|t| inside(to_paper(t.0)));
                let texts_in = ann.texts[st.2..en.2].iter().filter(|t| inside(to_paper(t.0))).count();
                if !ok && en.2 - st.2 >= 2 && texts_in > 0 {
                    // (its lines cut with the others below; a value shown when its text and its
                    // segment are inside)
                    for (i, t) in (st.2..en.2).zip(&ann.texts[st.2..en.2]) {
                        let span = ann.spans.binary_search_by_key(&i, |s| s.0).ok().map(|k| ann.spans[k].1);
                        kt[i] = inside(to_paper(t.0)) && span.is_none_or(|s| s.iter().all(|p| inside(to_paper(*p))));
                    }
                    for (i, f) in (st.1..en.1).zip(&ann.fills[st.1..en.1]) {
                        kf[i] = f.0.iter().flatten().all(|p| inside(to_paper(*p)));
                    }
                    continue;
                }
                for i in st.0..en.0 {
                    ks[i] = ok;
                    whole_s[i] = true;
                }
                kf[st.1..en.1].iter_mut().for_each(|x| *x = ok);
                kt[st.2..en.2].iter_mut().for_each(|x| *x = ok);
            }
            let strokes: Vec<_> = std::mem::take(&mut ann.strokes).into_iter().enumerate().filter(|(i, _)| ks[*i]).collect();
            let mut keep_whole = Vec::new();
            let mut rest = Vec::new();
            for (i, st) in strokes {
                if whole_s[i] { keep_whole.push(st) } else { rest.push(st) }
            }
            let mut fi = 0;
            ann.fills.retain(|_| { fi += 1; kf[fi - 1] });
            let mut ti = 0;
            ann.texts.retain(|_| { ti += 1; kt[ti - 1] });
            let mut cut = keep_whole;
            for (pts, w, c) in rest {
                let mut cur: Vec<P2> = Vec::new();
                for seg in pts.windows(2) {
                    match clip(seg[0], seg[1], box_v) {
                        Some((p, q)) => {
                            if cur.last().is_none_or(|l| (l[0] - p[0]).abs() + (l[1] - p[1]).abs() > 1e-9) {
                                if cur.len() >= 2 {
                                    cut.push((std::mem::take(&mut cur), w, c));
                                }
                                cur = vec![p];
                            }
                            cur.push(q);
                        }
                        None => {
                            if cur.len() >= 2 {
                                cut.push((std::mem::take(&mut cur), w, c));
                            }
                            cur.clear();
                        }
                    }
                }
                if cur.len() >= 2 {
                    cut.push((cur, w, c));
                }
            }
            ann.strokes = cut;
            ann.fills.retain(|f| f.0.iter().flatten().all(|p| inside(to_paper(*p))));
            ann.texts.retain(|t| inside(to_paper(t.0)));
        }
        let m = |x: f64| (x * FEET) as f32;
        let last = who.len().saturating_sub(1) as u32;
        seg_owner.resize(segs.len(), last);
        tri_owner.resize(tris.len(), last);
        // a line lying on a nearer one of an element drawn as one line (a pipe over another):
        // Revit draws the nearer one only there
        if !line_only.is_empty() {
            let left = uncovered(&segs, &seg_owner, &|o| line_only.contains(&o), 1e-3);
            let (mut s2, mut c2, mut o2) = (Vec::with_capacity(segs.len()), Vec::with_capacity(segs.len()), Vec::with_capacity(segs.len()));
            for (k, parts) in left.iter().enumerate() {
                let [a, b] = segs[k];
                let at = |t: f64| -> V3 { std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t) };
                for &(t0, t1) in parts {
                    s2.push([at(t0), at(t1)]);
                    c2.push(seg_colors[k]);
                    o2.push(seg_owner[k]);
                }
            }
            (segs, seg_colors, seg_owner) = (s2, c2, o2);
        }
        // hidden lines removed: the faces in a depth buffer over the frame, then the edges
        // keep the parts in front of them
        let hlr = if hlr_on && !tris.is_empty() { crate::rvt_hlr::Hlr::new(ub, HLR_PIXELS) } else { None };
        let mut hlr = hlr;
        if let Some(h) = hlr.as_mut() {
            for (t, o) in tris.iter().zip(&tri_owner) {
                h.tri_of(t[0], t[1], t[2], *o);
            }
        }
        // ducts and pipes hidden by other ducts and pipes: drawn in the pattern of hidden lines,
        // in their own weight and colour (Mechanical settings, "Draw MEP hidden lines": taken
        // as on, its setting not being read)
        let mep_dash = Some(self.ann.category_style(HIDDEN_LINES).map_or(HIDDEN_PATTERN, |s| s.2)).filter(|p| self.ann.line_pattern(*p).is_some());
        let is_mep = |o: u32| who.get(o as usize).is_some_and(|c| MEP_HIDDEN_LINES.contains(c));
        // the faces of each element (for what it hides of itself)
        let mut faces_of: HashMap<u32, Vec<usize>> = HashMap::default();
        if mep_dash.is_some() {
            for (k, o) in tri_owner.iter().enumerate() {
                if is_mep(*o) {
                    faces_of.entry(*o).or_default().push(k);
                }
            }
        }
        let mut own_bufs: HashMap<u32, Option<crate::rvt_hlr::Hlr>> = HashMap::default();
        let tol = hlr.as_ref().map_or(0.0, |h| 0.02 + h.pixel() * 0.25);
        let mut shown = 0usize;
        // the dashed pieces by look, drawn once gathered: a line made of several pieces (an
        // edge split where faces meet, hidden in places) keeps one pattern along it
        let mut dashed: std::collections::BTreeMap<(u64, [u8; 3], i64), Vec<(P2, P2)>> = std::collections::BTreeMap::new();
        for ((s, look), o) in segs.iter().zip(&seg_colors).zip(&seg_owner) {
            let parts: Vec<(V3, V3, bool)> = match (&hlr, mep_dash) {
                (Some(h), Some(_)) if is_mep(*o) => {
                    let mut parts = h.parts(s[0], s[1], tol, *o, &is_mep);
                    // what another hides is drawn only where the element would show it
                    if parts.iter().any(|p| p.2) {
                        let own = own_bufs.entry(*o).or_insert_with(|| {
                            let ks = faces_of.get(o)?;
                            let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
                            for t in ks.iter().flat_map(|&k| tris[k].iter()) {
                                lo = [lo[0].min(t[0]), lo[1].min(t[1])];
                                hi = [hi[0].max(t[0]), hi[1].max(t[1])];
                            }
                            let px = h.pixel();
                            let mut b = crate::rvt_hlr::Hlr::new([lo[0] - px, lo[1] - px, hi[0] + px, hi[1] + px], (((hi[0] - lo[0]).max(hi[1] - lo[1]) / px).ceil() as usize + 4).min(4096))?;
                            for &k in ks {
                                b.tri_of(tris[k][0], tris[k][1], tris[k][2], *o);
                            }
                            Some(b)
                        });
                        if let Some(own) = own.as_ref() {
                            parts = parts.into_iter().flat_map(|(a, b, hid)| if hid { own.visible(a, b, tol).into_iter().map(|(a, b)| (a, b, true)).collect() } else { vec![(a, b, false)] }).collect();
                        }
                    }
                    parts
                }
                (Some(h), _) => h.visible(s[0], s[1], tol).into_iter().map(|(a, b)| (a, b, false)).collect(),
                (None, _) => vec![(s[0], s[1], false)],
            };
            for (a, b, hidden) in parts {
                if let Some((a, b)) = clip([a[0], a[1]], [b[0], b[1]], ub) {
                    let (a, b) = (to_paper(a), to_paper(b));
                    // (hidden under another duct or pipe: its weight and colour, dashed)
                    let look = match (look, hidden, mep_dash) {
                        (Some((w, c, _)), true, Some(d)) => Some((*w, *c, d)),
                        (None, true, Some(d)) => Some((self.ann.pen(1), [0; 3], d)),
                        _ => *look,
                    };
                    match &look {
                        Some((w, c, pat)) => match self.ann.line_pattern(*pat) {
                            Some(_) => dashed.entry((w.to_bits(), *c, *pat)).or_default().push((a, b)),
                            None => out.strokes.push(Stroke2d { pts: vec![[m(a[0]), m(a[1])], [m(b[0]), m(b[1])]], width: m(*w), color: *c }),
                        },
                        None => out.fine.push([[m(a[0]), m(a[1])], [m(b[0]), m(b[1])]]),
                    }
                    shown += 1;
                }
            }
        }
        for ((w, c, pat), pieces) in dashed {
            let Some(dash) = self.ann.line_pattern(pat) else { continue };
            for (line, phase) in chains(collinear_runs(pieces)) {
                for piece in crate::rvt_annot::dashes(&line, dash, phase) {
                    out.strokes.push(Stroke2d { pts: piece.iter().map(|q| [m(q[0]), m(q[1])]).collect(), width: m(f64::from_bits(w)), color: c });
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
        // projection fills of the view's filters: pattern lines, or a solid fill
        let pb = {
            let (a, b) = (to_paper([ub[0], ub[1]]), to_paper([ub[2], ub[3]]));
            [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
        };
        for (t, pat, col, fw) in &fill_tris {
            let p: Vec<P2> = t.iter().map(|q| to_paper([q[0], q[1]])).collect();
            match self.ann.pattern_lines(*pat, k) {
                Some(lines) if !lines.is_empty() => {
                    let edges = [[p[0], p[1]], [p[1], p[2]], [p[2], p[0]]];
                    for (angle, gap) in lines {
                        for (a, b) in hatch_at(&edges, gap.max(0.0015), angle) {
                            if let Some((a, b)) = clip(a, b, pb) {
                                out.strokes.push(Stroke2d { pts: vec![[m(a[0]), m(a[1])], [m(b[0]), m(b[1])]], width: m(*fw), color: *col });
                            }
                        }
                    }
                }
                Some(_) => out.fills.push(Fill2d { tris: vec![[[m(p[0][0]), m(p[0][1])], [m(p[1][0]), m(p[1][1])], [m(p[2][0]), m(p[2][1])]]], color: *col, under: None }),
                None => {}
            }
        }
        for (t, col) in &sym_fills {
            let p: Vec<P2> = t.iter().map(|q| to_paper([q[0], q[1]])).collect();
            out.fills.push(Fill2d { tris: vec![[[m(p[0][0]), m(p[0][1])], [m(p[1][0]), m(p[1][1])], [m(p[2][0]), m(p[2][1])]]], color: *col, under: None });
        }
        // cut lines: on the front plane, never hidden
        let cut_w = m(CUT_PEN);
        for (i, s) in cuts.iter().enumerate() {
            if let Some((a, b)) = clip([s[0][0], s[0][1]], [s[1][0], s[1][1]], ub) {
                let (a, b) = (to_paper(a), to_paper(b));
                out.strokes.push(Stroke2d { pts: vec![[m(a[0]), m(a[1])], [m(b[0]), m(b[1])]], width: cut_w, color: cut_colors.get(i).copied().unwrap_or([0; 3]) });
            }
        }
        if std::env::var_os("SHEET_DEBUG").is_some() {
            eprintln!("    cut pieces {} at {cut_z:?}", cuts.len());
            eprintln!("    view {} {:?}: edges {} faces {} drawn {} shown pieces {shown} nz {nz:?} bz {:?} keep {:?}", vp.view, view.name, segs.len(), tris.len(), hlr.as_ref().map_or(0, |h| h.faces), crop.map(|c| c.bz), keep.map(|k| (k.orig, k.ax[2], k.lo[2], k.hi[2])));
        }
        put_drawn(out, &ann, &to_paper);
        // model → paper, for the view's texts and tags
        let mp = Trf::new([k * bx[0], k * by[0], 0.0, k * bx[1], k * by[1], 0.0, k * bx[2], k * by[2], 0.0], [vp.offset[0] - k * (dot(o, bx) + cu), vp.offset[1] - k * (dot(o, by) + cv), 0.0]);
        // the texts of the annotations nested in the model families it shows
        for (nsym, tf, ng) in &nested_notes {
            let Some(ns) = self.symbols.get(nsym) else { continue };
            let at = mp.after(tf);
            // (cut by the view's crop, as the model is)
            let out_of_crop = |t: &RawText| {
                let q = proj(tf.apply(t.orig));
                cropped && (q[0] < ub[0] || q[0] > ub[2] || q[1] < ub[1] || q[1] > ub[3])
            };
            for t in ns.texts.iter().filter(|t| !out_of_crop(t)) {
                push_text(out, t, Some(&at), 1.0, t.text.clone(), self.look(t));
            }
            // its labels, with the values its host sets
            for t in ns.tags.iter().filter(|t| !out_of_crop(t)) {
                let vals: Vec<(usize, String)> = t.params.iter().enumerate().filter_map(|(i, (_, pre, suf, _))| {
                    let v = self.label_value_of(ng, ns, t, i)?;
                    (!v.is_empty()).then(|| (i, format!("{pre}{v}{suf}")))
                }).collect();
                push_text(out, t, Some(&at), 1.0, t.join(&vals), self.look(t));
            }
        }
        // pictures placed in the view, at the view's scale
        for &(_, isym, c, base, w, h, front) in self.view_images.iter().filter(|i| mine(i.0)) {
            let Some((img, _, _)) = self.image(isym) else { continue };
            let (c, b) = (mp.apply(c), mp.lin(base));
            if !inside([c[0], c[1]]) {
                continue;
            }
            let turns = ((b[1].atan2(b[0]) / std::f64::consts::FRAC_PI_2).round().rem_euclid(4.0)) as u8;
            let (hw, hh) = if turns % 2 == 0 { (w * k * 0.5, h * k * 0.5) } else { (h * k * 0.5, w * k * 0.5) };
            out.images.push(Image2d { rect: [m(c[0] - hw), m(c[1] - hh), m(c[0] + hw), m(c[1] + hh)], image: img, turns, front: front });
        }
        for (_, t) in self.texts.iter().filter(|t| mine(t.0)) {
            if !inside([mp.apply(t.orig)[0], mp.apply(t.orig)[1]]) {
                continue;
            }
            push_text(out, t, Some(&mp), 1.0, t.text.clone(), self.look(t));
            self.put_text_marks(out, t, &mp);
        }
        self.put_color_legends(out, vp.view, view, &mp);
        self.put_color_fills(out, vp.view, view, &mp, f0);
        // 2D families placed in the view (detail components, symbols)
        for (k, (_, sym, tf)) in self.instances.iter().enumerate().filter(|(_, i)| mine(i.0)) {
            if std::env::var("SYM_DEBUG").is_ok_and(|v| v == vp.view.to_string()) {
                eprintln!("SYM0 inst {:?} type {sym} hidden {} inside {}", self.inst_ids.get(k), self.inst_ids.get(k).is_some_and(|id| view.hidden.contains(id)), { let o = mp.apply(tf.apply([0.0; 3])); inside([o[0], o[1]]) });
            }
            if self.inst_ids.get(k).is_some_and(|id| view.hidden.contains(id)) {
                continue;
            }
            // outside the view's annotation crop (a dependent view, a cropped plan): not shown
            let o = mp.apply(tf.apply([0.0; 3]));
            if !inside([o[0], o[1]]) {
                continue;
            }
            if std::env::var("SYM_DEBUG").is_ok_and(|v| v == vp.view.to_string()) {
                let d = self.symbols.get(sym).and_then(|sy| self.family_drawing(sy, self.inst_ints.get(k)));
                eprintln!("SYM inst {:?} type {sym} known {} family {:?} root {:?} drawn {:?} at {:?}", self.inst_ids.get(k), self.symbols.contains_key(sym), self.symbols.get(sym).map(|s| s.family), self.symbols.get(sym).and_then(|s| self.fam_roots.get(&s.family)), d.map(|d| (d.strokes.len(), d.fills.len(), d.texts.len())), o);
            }
            let Some(sy) = self.symbols.get(sym) else { continue };
            let flip = self.inst_flips.get(k).copied().unwrap_or_default();
            // (annotation families: `tf` already holds the view's scale)
            let at = mp.after(tf);
            let pl = at.after(&Trf::new([if flip.0 { -1.0 } else { 1.0 }, 0.0, 0.0, 0.0, if flip.1 { -1.0 } else { 1.0 }, 0.0, 0.0, 0.0, 1.0], [0.0; 3]));
            // the drawing flips, its texts stay in place and readable
            // (in its own graphics in the view, when it has any)
            let (s0, f0) = (out.strokes.len(), out.fills.len());
            self.put_symbol(out, sy, &pl, self.inst_ints.get(k));
            if let Some(o) = self.inst_ids.get(k).and_then(|id| self.line_over(view, *id)) {
                for st in &mut out.strokes[s0..] {
                    st.color = o.color.unwrap_or(st.color);
                }
                for f in &mut out.fills[f0..] {
                    f.color = o.fill.unwrap_or(f.color);
                }
            }
            for t in &sy.texts {
                push_text(out, t, Some(&at), 1.0, t.text.clone(), self.look(t));
                self.put_text_marks(out, t, &at);
            }
            // labels: the instance's text parameters, else its type's, else the family's sample
            for t in &sy.tags {
                let vals: Vec<(usize, String)> = t
                    .params
                    .iter()
                    .enumerate()
                    .filter_map(|(i, (p, pre, suf, sample))| {
                        let v = self.label_value(k, sy, t, i).unwrap_or_else(|| sample.clone());
                        if std::env::var("LABEL_DEBUG").is_ok_and(|d| d == vp.view.to_string()) {
                            eprintln!("LABEL inst {:?} type {sym} param {p} value {v:?} from str {:?} int {:?} type str {:?} type int {:?} sample {sample:?}", self.inst_ids.get(k), self.inst_strs.get(k).and_then(|m| m.get(p)), self.inst_ints.get(k).and_then(|m| m.get(p)), sy.strs.get(p), sy.ints.get(p));
                        }
                        (!v.is_empty()).then(|| (i, format!("{pre}{v}{suf}")))
                    })
                    .collect();
                push_text(out, t, Some(&at), 1.0, t.join(&vals), self.look(t));
            }
        }
        // grids, in plan views: their line cut to the view, their bubble at each end
        for g in self.grids.iter().filter(|g| self.grid_shown(view, &keep, g)) {
            // its ends in this view: free end (end 1) to bubble end (end 2); an end set in 2D
            // for the view stands where it is, an end in 3D stops at the crop
            let (fa, fb, two) = self.grid_ends(vp.view, view, g, &proj);
            if std::env::var("GRID_DEBUG").is_ok_and(|v| v.split(',').any(|x| x == vp.view.to_string())) {
                eprintln!("GRID view {} {} {:?}: free {fa:?} bubble {fb:?} 2d {two:?} crosses crop {} annotation crop {} (crop {ub:?} annotation {dub:?})", vp.view, g.id, g.name, clip(fa, fb, ub).is_some(), clip(fa, fb, dub).is_some());
            }
            // (shown when it crosses the crop itself, its model ends then reaching past it; with
            // both ends set in 2D for the view, when it crosses the annotation crop: C2 on P-153)
            if clip(fa, fb, if two.0 && two.1 { dub } else { ub }).is_none() {
                continue;
            }
            let Some((ca, cb)) = clip(fa, fb, dub) else { continue };
            let (a, b) = (if two.0 { fa } else { ca }, if two.1 { fb } else { cb });
            // p: the free end, q: the bubble end
            let (p, q) = (to_paper(a), to_paper(b));
            let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-6 {
                continue;
            }
            let (ux, uy) = (dx / len, dy / len);
            // the elbows the view gives its heads (a head moved aside so that it does not
            // overlap the next: P-153 C and C2): the line leaves the grid where the elbow starts,
            // turns, and the head stands at its other end
            let rec = self.datum_recs.get(&(vp.view, g.id)).or_else(|| self.datum_recs.get(&(view.primary, g.id))).copied();
            let elbow = |i: usize| rec.and_then(|r| r.elbows[i]).map(|e| e.map(|v| to_paper(proj(v))));
            let mut line = match elbow(0) {
                Some(e) => vec![e[2], e[1], e[0]],
                None => vec![p],
            };
            line.extend(elbow(1).map_or(vec![q], |e| e.to_vec()));
            let (p, q) = (line[0], line[line.len() - 1]);
            // in the line style of its type (pen, colour, pattern), else a black dash-dot
            let styled = self.grid_lines.get(&g.attr).and_then(|c| self.ann.category_line(*c, &line));
            let pieces = styled.unwrap_or_else(|| {
                let mut v = Vec::new();
                for s in line.windows(2) {
                    let (sx, sy) = (s[1][0] - s[0][0], s[1][1] - s[0][1]);
                    let sl = sx.hypot(sy);
                    let mut t = 0.0;
                    while t < sl {
                        for (l, draw) in [(0.04, true), (0.01, false), (0.004, true), (0.01, false)] {
                            let e = (t + l).min(sl);
                            if draw && e > t {
                                v.push((vec![[s[0][0] + sx / sl * t, s[0][1] + sy / sl * t], [s[0][0] + sx / sl * e, s[0][1] + sy / sl * e]], self.ann.pen(1), [0; 3]));
                            }
                            t = e;
                        }
                    }
                }
                v
            });
            for (pts, w, c) in pieces {
                out.strokes.push(Stroke2d { pts: pts.iter().map(|q| [m(q[0]), m(q[1])]).collect(), width: m(w), color: c });
            }
            if let Some(sy) = self.level_heads.get(&g.attr).and_then(|h| self.symbols.get(h)) {
                // the type's bubbles at end 1 (free end) and end 2 (bubble end), unless the view
                // shows or hides one itself
                let (t1, t2) = self.grid_bubbles.get(&g.attr).copied().unwrap_or((true, true));
                let fl = rec.map_or(0, |r| r.flags);
                let show = |bits: i64, dflt: bool| match bits & 3 {
                    1 => true,
                    2 => false,
                    _ => dflt,
                };
                let (b1, b2) = (show(fl, t1), show(fl >> 2, t2));
                for (end, dir) in [(p, [-ux, -uy]), (q, [ux, uy])].into_iter().zip([b1, b2]).filter(|e| e.1).map(|e| e.0) {
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
        // grids in sections and elevations: where their plane crosses the view's, between the
        // ends the view sets (in 2D, else their plane's bottom and top cut past the crop: both
        // measured up from their plane's origin); their bubbles where their type puts them,
        // unless the view says otherwise for that end
        let upright = view.dir.is_some_and(|d| d[2].abs() < 1e-6) && !view.class.contains("Drafting") && !view.hidden_cats.contains(&GRIDS);
        for g in self.grids.iter().filter(|g| upright && !self.datum_hidden(view, g.id)) {
            let Some((lo3, hi3)) = self.grid_cross(g, o, nz) else { continue };
            let (fa, fb) = (proj(lo3), proj(hi3));
            if clip(fa, fb, ub).is_none() {
                continue;
            }
            let rec = self.datum_recs.get(&(vp.view, g.id)).or_else(|| self.datum_recs.get(&(view.primary, g.id)));
            // (free end, bubble end: height, set in 2D, the view's bubble override)
            let (f, b) = match rec {
                Some(r) => {
                    let s = proj(g.o)[1];
                    let (f2, b2) = (r.flags & 16 != 0, r.flags & 32 != 0);
                    ((s + g.up * if f2 { r.free } else { r.free3 }, f2, r.flags & 3), (s + g.up * if b2 { r.bubble } else { r.bubble3 }, b2, r.flags >> 2 & 3))
                }
                None => ((fa[1], false, 0), (fb[1], false, 0)),
            };
            let cut = |e: (f64, bool, i64)| if e.1 { e } else { (e.0.clamp(dub[1], dub[3]), e.1, e.2) };
            let (f, b) = (cut(f), cut(b));
            let (lo, hi) = if f.0 <= b.0 { (f, b) } else { (b, f) };
            let (p, q) = (to_paper([fa[0], lo.0]), to_paper([fa[0], hi.0]));
            let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-6 {
                continue;
            }
            let styled = self.grid_lines.get(&g.attr).and_then(|c| self.ann.category_line(*c, &[p, q])).unwrap_or_else(|| vec![(vec![p, q], self.ann.pen(1), [0; 3])]);
            for (pts, w, c) in styled {
                out.strokes.push(Stroke2d { pts: pts.iter().map(|q| [m(q[0]), m(q[1])]).collect(), width: m(w), color: c });
            }
            if let Some(sy) = self.level_heads.get(&g.attr).and_then(|h| self.symbols.get(h)) {
                let at = self.grid_elev_bubbles.get(&g.attr).copied().unwrap_or(2);
                let (ux, uy) = (dx / len, dy / len);
                let shown = |typed: bool, over: i64| over == 1 || (typed && over != 2);
                for (end, dir) in [(p, [-ux, -uy]), (q, [ux, uy])].into_iter().zip([shown(at & 1 != 0, lo.2), shown(at & 2 != 0, hi.2)]).filter(|e| e.1).map(|e| e.0) {
                    let r = sy.outline.map_or(0.0, |o| (o[3] - o[1]).abs() * 0.5);
                    let tf = Trf::new([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], [end[0] + dir[0] * r, end[1] + dir[1] * r, 0.0]);
                    self.put_symbol(out, sy, &tf, None);
                    for t in &sy.tags {
                        push_text(out, t, Some(&tf), 1.0, g.name.clone(), self.look(t));
                    }
                }
            }
        }
        // scope boxes, in plan views whose cut plane crosses them: their outline in the style
        // of their category (Revit's green dashes)
        const SCOPE_BOXES: i64 = -2006000;
        let plan = view.dir.is_some_and(|d| d[2].abs() > 0.99) && view.class.to_ascii_lowercase().contains("plan");
        let cut = if plan && !view.hidden_cats.contains(&SCOPE_BOXES) { self.plan_range(view).map(|r| r.1) } else { None };
        // (shown unless hidden: its category, itself, its workset)
        for (bid, c, shown) in self.scope_boxes.iter().filter(|b| !self.hides(view, b.0, SCOPE_BOXES)) {
            let Some(z) = cut else { break };
            let at = |u: f64, v: f64, w: f64| -> V3 { std::array::from_fn(|i| c.orig[i] + c.bx[i] * u + c.by[i] * v + c.bz[i] * w) };
            let (z0, z1) = (at(0.0, 0.0, c.off[5].min(c.off[4]))[2], at(0.0, 0.0, c.off[5].max(c.off[4]))[2]);
            if !(shown.contains(&vp.view) || (z >= z0.min(z1) - 1e-6 && z <= z0.max(z1) + 1e-6)) {
                continue;
            }
            let corners: Vec<P2> = [(c.off[1], c.off[3]), (c.off[0], c.off[3]), (c.off[0], c.off[2]), (c.off[1], c.off[2]), (c.off[1], c.off[3])].iter().map(|&(u, v)| proj(at(u, v, 0.0))).collect();
            // each side, cut by the crop
            for w2 in corners.windows(2) {
                let Some((a, b)) = (if cropped { clip(w2[0], w2[1], ub) } else { Some((w2[0], w2[1])) }) else { continue };
                let side = [to_paper(a), to_paper(b)];
                let pieces = self.ann.category_line_at(SCOPE_BOXES, &side, vp.scale).unwrap_or_else(|| vec![(side.to_vec(), self.ann.pen(1), [0, 128, 0])]);
                for (pts, w, col) in pieces {
                    out.strokes.push(Stroke2d { pts: pts.iter().map(|q| [m(q[0]), m(q[1])]).collect(), width: m(w), color: col });
                }
            }
            let _ = bid;
        }
        // levels: their line and head; on a sheet, cut to what the view shows
        for (id, (p0, _)) in self.level_ends.iter().filter(|_| self.shows_levels(view)) {
            let (Some(z), Some(name)) = (self.level_z.get(id), self.level_names.get(id)) else { continue };
            // (hidden in the view, by its workset or a filter)
            if self.hides(view, *id, LEVELS) {
                continue;
            }
            let a = proj([p0[0], p0[1], *z]);
            // (no record: the level's plane, its bubble at its right end)
            let (mut uf, mut ubb, f2, b2, flags) = self.level_span(vp.view, view.primary, *id, &proj).unwrap_or_else(|| {
                let (a, b) = self.model_span(&proj);
                (a, b, false, false, 0)
            });
            if std::env::var("LEVEL_DEBUG").is_ok_and(|v| v == vp.view.to_string()) {
                eprintln!("LEVEL {id} {name} free {uf} bubble {ubb} 2d {f2} {b2} flags {flags} ub {ub:?} dub {dub:?} v {}", a[1]);
            }
            // its heads: at its bubble end (unless the view hides it), at its free end when the
            // view shows one there (`m_endBubbleOverrides`: 1 shown, 2 hidden)
            let (show_f, show_b) = (flags & 3 == 1, flags >> 2 & 3 != 2);
            // (on a sheet or cropped: within the crop's height, its ends following its plane cut
            // past the crop)
            if vp.owner != -1 || cropped {
                if a[1] < ub[1] || a[1] > ub[3] {
                    continue;
                }
                if !f2 {
                    uf = uf.clamp(dub[0], dub[2]);
                }
                if !b2 {
                    ubb = ubb.clamp(dub[0], dub[2]);
                }
            }
            let (u0, u1) = (uf.min(ubb), uf.max(ubb));
            if u1 - u0 < 1e-6 {
                continue;
            }
            let q = to_paper([u1, a[1]]);
            // the elbows the view gives its heads (P-402): the line leaves the level where the
            // elbow starts, turns, and the head stands at its other end
            let rec = self.datum_recs.get(&(vp.view, *id)).or_else(|| self.datum_recs.get(&(view.primary, *id))).copied();
            let elbow = |i: usize| rec.and_then(|r| r.elbows[i]).map(|e| e.map(|v| to_paper(proj(v))));
            let (hf, hb) = (elbow(0).map_or(to_paper([uf, a[1]]), |e| e[2]), elbow(1).map_or(to_paper([ubb, a[1]]), |e| e[2]));
            // (from its left end, the dashes starting there)
            let mut line = match elbow(0) {
                Some(e) => vec![e[2], e[1], e[0]],
                None => vec![hf],
            };
            line.extend(elbow(1).map_or(vec![hb], |e| e.to_vec()));
            if ubb < uf {
                line.reverse();
            }
            let w = m(self.ann.pen(1));
            let (dash, gap) = (0.04, 0.02);
            for s in line.windows(2) {
                let (sx, sy) = (s[1][0] - s[0][0], s[1][1] - s[0][1]);
                let sl = sx.hypot(sy);
                let mut t = 0.0;
                while t < sl {
                    let e = (t + dash).min(sl);
                    out.strokes.push(Stroke2d { pts: vec![[m(s[0][0] + sx / sl * t), m(s[0][1] + sy / sl * t)], [m(s[0][0] + sx / sl * e), m(s[0][1] + sy / sl * e)]], width: w, color: [0; 3] });
                    t = e + gap;
                }
            }
            let head = self.level_types.get(id).and_then(|t| self.level_heads.get(t)).and_then(|h| self.symbols.get(h));
            // (one at its left end mirrored)
            let ends = [(ubb, show_b, hb), (uf, show_f, hf)];
            match head {
                Some(sy) => for (u, _, at) in ends.iter().filter(|e| e.1) {
                    let at = *at;
                    let x = if *u < (u0 + u1) * 0.5 { -1.0 } else { 1.0 };
                    let tf = Trf::new([x, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], [at[0], at[1], 0.0]);
                    self.put_symbol(out, sy, &tf, None);
                    for t in &sy.texts {
                        push_text(out, t, Some(&tf), 1.0, t.text.clone(), self.look(t));
                    }
                    for t in &sy.tags {
                        // labels of a level head: its name, its elevation
                        let vals: Vec<(usize, String)> = t.params.iter().enumerate().map(|(i, (p, pre, suf, _))| (i, format!("{pre}{}{suf}", if *p == LEVEL_ELEV { self.ann.length(*z) } else { name.clone() }))).collect();
                        let text = t.join(&vals);
                        push_text(out, t, Some(&tf), 1.0, text, self.look(t));
                    }
                }
                None => {
                    let h = 0.0078;
                    out.texts.push(Text2d { pos: [m(q[0]), m(q[1] + h * 0.5)], dir: [1.0, 0.0], height: m(h), halign: 2, valign: 0, text: name.clone(), bold: false, wrap: 0.0, color: [0; 3], ..Default::default() });
                }
            }
        }
        let tag_debug = std::env::var_os("TAG_DEBUG").is_some();
        for tg in self.ann.tags.iter().filter(|t| mine(t.owner) && !view.hidden.contains(&t.id)) {
            // a tag of a model element the view hides, or out of its range, is not shown either
            // (one within its range keeps its tag, drawn or not: above the cut plane)
            // (nor, in a plan, one of a fixture or a component the view draws nothing of: P-403's
            // flush valves "R 04" and mirrors "MI 01"; a duct's or pipe's kept, as one seen end on
            // or past the cut plane may draw nothing and keep its tag)
            let blank = plan_view && drawn.contains(&tg.target) && !inked.contains(&tg.target) && !model_cats.get(&tg.target).is_some_and(|c| ABOVE_CUT.contains(c));
            if model_cats.contains_key(&tg.target) && (!in_range.contains(&tg.target) || blank) {
                if tag_debug {
                    let h = mp.apply(tg.head);
                    eprintln!("TAG {} view {} target {} not drawn (inked {}), head at {:.4},{:.4} m (paper) {}", tg.id, vp.view, tg.target, inked.contains(&tg.target), h[0] * FEET, h[1] * FEET, range_info.get(&tg.target).map_or("", |s| s.as_str()));
                }
                continue;
            }
            // a tag of a duct's or pipe's insulation: shown with its insulation (its category
            // shown, its duct or pipe in the view's range: "SOUFFLAGE DEPUIS CTA 02" not on
            // AC-213)
            if let Some(&(_, _, icat)) = self.insulations.iter().find(|i| i.0 == tg.target) {
                let host = self.elem_links.get(&tg.target).into_iter().flatten().find(|h| self.mep_lines.contains_key(h));
                if self.hides(view, tg.target, icat) || host.is_some_and(|h| self.hides(view, *h, model_cats.get(h).copied().unwrap_or(0)) || (model_cats.contains_key(h) && !in_range.contains(h))) {
                    if tag_debug {
                        eprintln!("TAG {} view {} insulation {} hidden or out of range (host {host:?})", tg.id, vp.view, tg.target);
                    }
                    continue;
                }
            }
            let Some(sy) = self.symbols.get(&tg.symbol) else {
                if tag_debug {
                    eprintln!("TAG {} view {} no symbol {}", tg.id, vp.view, tg.symbol);
                }
                continue;
            };
            // its own category switched off in the view (room tags…), or the tag hidden
            if self.fam_cats.get(&sy.family).is_some_and(|c| self.hides(view, tg.id, *c)) {
                if tag_debug {
                    eprintln!("TAG {} view {} category hidden", tg.id, vp.view);
                }
                continue;
            }
            let h = mp.apply(tg.head);
            // a tag whose leader folds onto its head sits on its duct or pipe: not shown where
            // the view's crop cuts that away, though inside the annotation crop (AC-154's)
            let folded = tg.leader.is_some_and(|(e, f)| e == [0.0; 2] && f == [0.0; 2]);
            if !inside([h[0], h[1]]) || (folded && !in_model_crop([h[0], h[1]])) {
                if tag_debug {
                    eprintln!("TAG {} view {} head outside", tg.id, vp.view);
                }
                continue;
            }
            // a tag of a family turned with its component follows the tagged duct or pipe,
            // kept readable (a tag with a leader stays as placed, unless its leader is folded
            // onto its head along a duct or pipe running in the view's plane: AC-153's, AC-211's)
            let angle = match self.mep_dirs.get(&tg.target).filter(|_| self.turning_tags.contains(&sy.family) && (tg.leader.is_none() || folded)) {
                Some(d) => {
                    let p = mp.lin(*d);
                    let mut a = p[1].atan2(p[0]);
                    if a > std::f64::consts::FRAC_PI_2 + 1e-6 {
                        a -= std::f64::consts::PI;
                    } else if a <= -std::f64::consts::FRAC_PI_2 + 1e-6 {
                        a += std::f64::consts::PI;
                    }
                    // (one within half a degree of the vertical reads upwards, as Revit's: AC-253's
                    // "AE - Ø250"; AC-254's a degree off reads downwards)
                    if (a.abs() - std::f64::consts::FRAC_PI_2).abs() < 0.5f64.to_radians() {
                        a = std::f64::consts::FRAC_PI_2;
                    }
                    if p[0].hypot(p[1]) > 1e-9 && (tg.leader.is_none() || d[2].abs() < 0.5) { a } else { tg.angle }
                }
                None => tg.angle,
            };
            if tag_debug {
                eprintln!("TAGANG {} view {} turning {} leader {:?} dir {:?} angle {:.1} stored {:.3}", tg.id, vp.view, self.turning_tags.contains(&sy.family), tg.leader, self.mep_dirs.get(&tg.target), angle.to_degrees(), tg.angle);
            }
            let (c, s) = (angle.cos(), angle.sin());
            let tf = Trf::new([c, s, 0.0, -s, c, 0.0, 0.0, 0.0, 1.0], [h[0], h[1], 0.0]);
            // shown only when wholly inside the annotation crop, as Revit does
            if sy.outline.is_some_and(|o| [[o[0], o[1]], [o[2], o[1]], [o[2], o[3]], [o[0], o[3]]].iter().any(|q| {
                let p = tf.apply([q[0], q[1], 0.0]);
                !inside([p[0], p[1]])
            })) {
                if tag_debug {
                    eprintln!("TAG {} view {} outline outside", tg.id, vp.view);
                }
                continue;
            }
            // a tag of an element of a linked model: orphaned without the link, Revit does not
            // show it when the link is not loaded (a keynote's still takes its number)
            if self.link_insts.contains(&tg.target) {
                if tag_debug {
                    eprintln!("TAG {} view {} linked target {} family {} cat {:?}", tg.id, vp.view, tg.target, sy.family, self.fam_cats.get(&sy.family));
                }
                if let Some(k) = self.ann.keys.get(&tg.id) {
                    out.texts.push(Text2d { text: format!("{KEY_MARK}{:020}{KEY_MARK}{k}", tg.id), ..Default::default() });
                }
                continue;
            }
            if tag_debug {
                eprintln!("TAG {} view {} drawn target {} (cat {:?} in range {} hides {:?}) symbol {} family {} at {:.1},{:.1} mm labels {:?}", tg.id, vp.view, tg.target, model_cats.get(&tg.target), in_range.contains(&tg.target), model_cats.get(&tg.target).map(|c| self.hides(view, tg.target, *c)), tg.symbol, sy.family, h[0] * 304.8, h[1] * 304.8, sy.tags.iter().map(|t| t.params.iter().map(|p| p.0).collect::<Vec<_>>()).collect::<Vec<_>>());
            }
            let texts0 = out.texts.len();
            self.put_symbol(out, sy, &tf, None);
            for t in &sy.texts {
                push_text(out, t, Some(&tf), 1.0, t.text.clone(), self.look(t));
            }
            // labels: the tagged element's text parameters, else its type's
            let ty = self.ann.inst_type.get(&tg.target);
            for t in &sy.tags {
                let vals: Vec<(usize, String)> = t.params.iter().enumerate().filter_map(|(i, (p, pre, suf, _))| {
                    // a keynote shows its key; the others the tagged element's values
                    let computed = match *p {
                        // the key's last part, as numbered on the sheet ("AC-251.02" -> "2")
                        // numbered by sheet once all its keynotes are known (see `number_keys`)
                        KEYNOTE_NUMBER => self.ann.keys.get(&tg.id).map(|k| format!("{KEY_MARK}{:020}{KEY_MARK}{k}", tg.id)),
                        SYSTEM_ABBREVIATION => self.elem_systems.get(&tg.target).and_then(|sy| self.system_types.get(sy)).and_then(|ty| self.system_abbr.get(ty)).cloned(),
                        // written as the project writes duct sizes ("1 300x700", "Ø355")
                        // (in a section, a rectangular duct running across it level reads its
                        // height first: AC-404's "AR - 300x550", AC-406's "AR - 450x700")
                        MEP_SIZE => {
                            let across = match (view.dir, self.mep_dirs.get(&tg.target), self.mep_sizes.get(&tg.target)) {
                                (Some(v), Some(d), Some((_, Some(_)))) => v[2].abs() < 1e-6 && d[2].abs() < 0.5 && (d[0] * v[0] + d[1] * v[1]).abs() < 0.5,
                                _ => false,
                            };
                            let text = self.mep_size_texts.get(&tg.target).cloned().or_else(|| self.mep_sizes.get(&tg.target).map(|&(w, h)| match h {
                                Some(h) => format!("{}x{}", self.ann.size_mm(w), self.ann.size_mm(h)),
                                None => format!("Ø{}", self.ann.size_mm(w)),
                            }));
                            match text.as_deref().and_then(|t| t.split_once('x')).filter(|_| across) {
                                Some((w, h)) => Some(format!("{}x{}", h.trim(), w.trim())),
                                None => text,
                            }
                        }
                        PIPE_DIAMETER => self.mep_diams.get(&tg.target).cloned(),
                        TERMINAL_FLOW => self.elem_flows.get(&tg.target).and_then(|v| self.ann.air_flow(*v, t.fmts.get(i).copied().flatten())),
                        CABLE_TRAY_WIDTH => self.mep_sizes.get(&tg.target).map(|&(w, _)| self.ann.size_mm(w)),
                        ROOM_AREA => self.ann.enclosure(tg.target).map(|e| match e {
                            crate::rvt_annot::Enclosure::Area(a) => self.ann.area(a),
                            crate::rvt_annot::Enclosure::Open => if self.french { "Non fermée" } else { "Not Enclosed" }.into(),
                            crate::rvt_annot::Enclosure::Redundant => if self.french { "Espace superflu" } else { "Redundant Space" }.into(),
                        }),
                        _ => None,
                    };
                    let v = computed
                        .or_else(|| self.ann.strs.get(&tg.target).and_then(|x| x.get(p)).cloned())
                        // (a duct's, pipe's or cable tray's own: a tray's service type, "CFO")
                        .or_else(|| self.curve_strs.get(&tg.target).and_then(|x| x.get(p)).cloned())
                        .or_else(|| ty.and_then(|y| self.ann.strs.get(y)).and_then(|x| x.get(p)).cloned())?;
                    (!v.is_empty()).then(|| (i, format!("{pre}{v}{suf}")))
                }).collect();
                push_text(out, t, Some(&tf), 1.0, t.join(&vals), self.look(t));
            }
            if let Some((elbow, end)) = tg.leader {
                let k = vp.scale;
                let pt = |o: [f64; 2]| [m(h[0] + o[0] * k), m(h[1] + o[1] * k)];
                // (from its texts' side facing its elbow, level with its head, as Revit attaches
                // it: past "DN22" on P-213, not through it)
                let first = pt(elbow.map(|_| 0.0));
                let toward = if elbow[0].hypot(elbow[1]) > 1e-9 { pt(elbow) } else { pt(end) };
                let attach = texts_box(&out.texts[texts0..]).filter(|b| toward[0] < b[0] || toward[0] > b[2]).map_or(first, |b| [if toward[0] < b[0] { b[0] } else { b[2] }, first[1]]);
                let mut pts = vec![attach];
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
        }
        // view references: their symbol at their point, along their direction, labelled with
        // the sheet showing the view they point to
        let numbers = self.sheet_numbers.lock().map(|n| n.clone()).unwrap_or_default();
        // (never one pointing to the view itself)
        for rv in self.ref_viewers.iter().filter(|r| mine(r.owner) && r.target != vp.view && !view.hidden.contains(&r.id) && !self.views.get(&view.primary).is_some_and(|p| p.hidden.contains(&r.id))) {
            let Some(sy) = self.level_heads.get(&rv.attr).and_then(|h| self.symbols.get(h)) else { continue };
            let h = mp.apply(rv.at);
            if !inside([h[0], h[1]]) {
                continue;
            }
            let d = mp.lin(rv.dir);
            let angle = if d[0].hypot(d[1]) > 1e-9 { d[1].atan2(d[0]) } else { 0.0 };
            if std::env::var_os("REF_DEBUG").is_some() {
                eprintln!("REF {} view {} dir {:?} angle {:.1} texts {:?} tags {:?}", rv.id, view.name, rv.dir, angle.to_degrees(), sy.texts.iter().map(|t| (t.text.clone(), t.base, t.orig)).collect::<Vec<_>>(), sy.tags.iter().map(|t| (t.base, t.orig, t.flags)).collect::<Vec<_>>());
            }
            let (c, s) = (angle.cos(), angle.sin());
            let tf = Trf::new([c, s, 0.0, -s, c, 0.0, 0.0, 0.0, 1.0], [h[0], h[1], 0.0]);
            self.put_symbol(out, sy, &tf, None);
            for t in &sy.texts {
                push_text(out, t, Some(&tf), 1.0, t.text.clone(), self.look(t));
            }
            let sheet = self.viewports.iter().find(|v| v.view == rv.target).map(|v| v.owner);
            let target = self.views.get(&rv.target);
            for t in &sy.tags {
                if std::env::var_os("SHEET_DEBUG").is_some() {
                    eprintln!("    reference {} -> {} labels {:?}", rv.id, rv.target, t.params);
                }
                let vals: Vec<(usize, String)> = t
                    .params
                    .iter()
                    .enumerate()
                    .filter_map(|(i, (p, pre, suf, _))| {
                        let v = match *p {
                            VIEWER_SHEET_NUMBER => sheet.and_then(|sh| numbers.get(&sh)).cloned(),
                            VIEWER_DETAIL_NUMBER => self.viewports.iter().find(|v| v.view == rv.target).map(|v| v.number.clone()),
                            VIEW_NAME => target.map(|t| t.name.clone()),
                            _ => target.and_then(|t| t.strs.get(p)).cloned(),
                        }?;
                        (!v.is_empty()).then(|| (i, format!("{pre}{v}{suf}")))
                    })
                    .collect();
                push_text(out, t, Some(&tf), 1.0, t.join(&vals), self.look(t));
            }
        }
        // callouts drawn in this view (or in the view it depends on): their boundary (their
        // sketch, else their crop's rectangle) with rounded corners, a leader from it to their
        // head, their head naming the view (its detail and sheet numbers)
        self.draw_callouts(out, view, vp, &proj, &to_paper, &inside, &mine, &numbers);
        // the sections it shows: their line, their head naming their view
        let crop_box = cropped.then(|| {
            let (a, b) = (to_paper([ub[0], ub[1]]), to_paper([ub[2], ub[3]]));
            [a[0], a[1], b[0], b[1]]
        });
        self.draw_sections(out, view, vp, &proj, &to_paper, &inside, crop_box, (o, nz, dub, crop), &mine, &numbers);
        // SECTION_DEBUG=<view>: the sections seen in it, their front cut and crop ends on the
        // sheet (mm from its top left corner)
        if std::env::var("SECTION_DEBUG").is_ok_and(|v| v == vp.view.to_string()) {
            let e = out.extent;
            let mm = |p: P2| {
                let q = to_paper(p);
                [((q[0] * FEET - e[0] as f64) * 1000.0 * 10.0).round() / 10.0, ((e[3] as f64 - q[1] * FEET) * 1000.0 * 10.0).round() / 10.0]
            };
            for mk in self.marks.iter().filter(|mk| mk.crop.bz[2].abs() < 0.01 && mk.target != vp.view) {
                let c = &mk.crop;
                let at = |u: f64| -> V3 { std::array::from_fn(|i| c.orig[i] + c.bx[i] * u) };
                eprintln!("SECTIONMARK {} -> {} {:?} front {:?} crop u {:?} bz {:?} raw front {:?} orig {:?} by {:?} off {:?}", mk.id, mk.target, self.views.get(&mk.target).map(|t| t.name.clone()), mk.front.map(|(a, b)| (mm(proj(a)), mm(proj(b)))), (mm(proj(at(c.off[1]))), mm(proj(at(c.off[0])))), c.bz, mk.front, c.orig, c.by, c.off);
            }
        }
        let a = to_paper([ub[0], ub[1]]);
        let b = to_paper([ub[2], ub[3]]);
        Some([a[0], a[1], b[0], b[1]])
    }

    /// The sections a view shows (see `draw_callouts` for the arguments; `crop_box` its crop on
    /// paper when cropped, `host` its plane, annotation crop and crop box):
    /// - a plan, those whose box its cut plane crosses, along their front cut;
    /// - a section or an elevation, those whose box reaches its own (its depth included),
    ///   upright where their plane crosses its plane;
    ///
    /// whose line crosses its crop (drawn in it only, when shown there only), between the ends the
    /// view keeps for them (else their crop's width, or height), their line in the "Section Line"
    /// style (only its ends when the view breaks it), their type's head and tail at the ends the
    /// view puts them: the family's x along the line, inwards, its y the way the section looks;
    /// the head's circle and its labels upright, naming their view (its detail and sheet numbers,
    /// "-" and "---" while it is on no sheet).
    #[allow(clippy::too_many_arguments)]
    fn draw_sections(&self, out: &mut Sheet2d, view: &ViewInfo, vp: &Vp, proj: &dyn Fn(V3) -> P2, to_paper: &dyn Fn(P2) -> P2, inside: &dyn Fn(P2) -> bool, crop_box: Option<[f64; 4]>, host: (V3, V3, [f64; 4], Option<&Crop>), mine: &dyn Fn(i64) -> bool, numbers: &HashMap<i64, String>) {
        const VIEWERS: i64 = -2000278;
        const SECTIONS: i64 = -2000200;
        // ("Section Line": a section's line, whatever its type)
        const SECTION_LINE: i64 = -2000201;
        let m = |x: f64| (x * FEET) as f32;
        let plan = view.dir.is_some_and(|d| d[2].abs() > 0.99);
        // (sections and elevations show the sections crossing them, upright)
        let upright = view.dir.is_some_and(|d| d[2].abs() < 1e-6) && !view.class.contains("Drafting") && !view.class.contains("Legend");
        if !(plan || upright) || view.hidden_cats.contains(&VIEWERS) || view.hidden_cats.contains(&SECTIONS) {
            return;
        }
        let cut = if plan {
            let Some(r) = self.plan_range(view) else { return };
            r.1
        } else {
            0.0
        };
        // the view's plane (a point, its normal), its annotation crop (view feet), its crop box
        let (o, n, dub, host_box) = host;
        let k = view.scale.max(1e-9);
        for mk in self.marks.iter().filter(|mk| mk.method == 201 && mk.target != vp.view && mk.crop.bz[2].abs() < 0.01) {
            let Some((a, b)) = mk.front else { continue };
            // (hidden in this view: its own list, as a model element)
            if view.hidden.contains(&mk.id) || (mk.coarser > 0.0 && view.scale < mk.coarser - 1e-9) {
                continue;
            }
            if mk.one_view && !mine(mk.parent) {
                continue;
            }
            // (the view's filters on sections, read on the section's view)
            if self.hides(view, mk.id, SECTIONS) {
                continue;
            }
            // its box's height across the plan's cut plane
            let c = &mk.crop;
            let (z0, z1) = (c.orig[2] + c.by[2] * c.off[3], c.orig[2] + c.by[2] * c.off[2]);
            if plan && (cut < z0.min(z1) - 1e-6 || cut > z0.max(z1) + 1e-6) {
                continue;
            }
            let Some(target) = self.views.get(&mk.target) else { continue };
            let (head, tail) = self.view_types.get(&target.vtype).and_then(|t| self.section_types.get(&t.0)).copied().unwrap_or((-1, -1));
            // its ends in this view (or in the view it depends on): along its front cut
            let d = sub(b, a);
            let len = dot(d, d).sqrt();
            if len < 1e-9 {
                continue;
            }
            let dir = [d[0] / len, d[1] / len, d[2] / len];
            let along = |u: f64| (0..3).map(|i| (c.orig[i] + c.bx[i] * u - a[i]) * dir[i]).sum::<f64>();
            let (t0, t1) = (along(c.off[1]), along(c.off[0]));
            // where it stands: in a plan along its front cut (none kept: its crop's width); in a
            // section on the line where its box crosses the view's plane, measured from its front
            // cut's height, its end 1 the upper one (none kept: its box's height, an end past the
            // view's crop at its annotation crop)
            let (base, default) = if plan {
                (a, [t0.min(t1), t0.max(t1)])
            } else {
                let den = dot(dir, n);
                if den.abs() < 1e-6 {
                    continue;
                }
                let s = dot(sub(o, a), n) / den;
                // (shown when its box reaches the view's, the view's depth included: its far
                // clip along its box's z)
                let (s0, s1) = host_box.map_or((s, s), |h| {
                    let at = |d: f64| dot(sub(std::array::from_fn(|i| h.orig[i] + h.bz[i] * d), a), n) / den;
                    let (sa, sb) = (at(h.off[5]), at(h.off[4]));
                    (sa.min(sb), sa.max(sb))
                });
                if s1 < t0.min(t1) - 1e-6 || s0 > t0.max(t1) + 1e-6 {
                    continue;
                }
                let base = [a[0] + dir[0] * s, a[1] + dir[1] * s, a[2]];
                let v = |t: f64| proj([base[0], base[1], base[2] - t])[1];
                let dv = v(1.0) - v(0.0);
                if dv.abs() < 1e-9 {
                    continue;
                }
                // (an end past the view's crop goes to its annotation crop on that side: the
                // datums' limit, less their 1/128")
                let edge = DATUM_PAST_ANNOT / k;
                let snap = |t: f64| match crop_box.map(|b| (to_paper([0.0, v(t)])[1], b)) {
                    Some((q, b)) if q > b[3] + 1e-9 => t + (dub[3] - edge - v(t)) / dv,
                    Some((q, b)) if q < b[1] - 1e-9 => t + (dub[1] + edge - v(t)) / dv,
                    _ => t,
                };
                (base, [snap(a[2] - z0.max(z1)), snap(a[2] - z0.min(z1))])
            };
            let ends = self.section_ends.get(&(vp.view, mk.id)).or_else(|| self.section_ends.get(&(view.primary, mk.id))).copied().unwrap_or(SectionEnds { ends: default, marks: [1, 2], broken: false, kept: [-1.0; 2] });
            let step = if plan { dir } else { [0.0, 0.0, if ends.ends[0] > ends.ends[1] { 1.0 } else { -1.0 }] };
            let at = |t: f64| -> P2 { to_paper(proj(std::array::from_fn(|i| base[i] + step[i] * t))) };
            if !plan && std::env::var_os("SECSEC_DEBUG").is_some() {
                let e = out.extent;
                let mm = |q: P2| [((q[0] * FEET - e[0] as f64) * 1000.0 * 10.0).round() / 10.0, ((e[3] as f64 - q[1] * FEET) * 1000.0 * 10.0).round() / 10.0];
                eprintln!("SECSEC host {} mark {} marks {:?} rec {} dir {:?} bz {:?} a {:?} ends {:?} default {:?} up {} end1 {:?} end2 {:?}", vp.view, mk.id, ends.marks, self.section_ends.contains_key(&(vp.view, mk.id)), dir.map(|x| (x * 1000.0).round() / 1000.0), c.bz.map(|x| (x * 1000.0).round() / 1000.0), a, ends.ends, default, step[2], mm(at(ends.ends[0])), mm(at(ends.ends[1])));
            }
            let (p0, p1) = (at(ends.ends[0]), at(ends.ends[1]));
            // (a cropped view shows the sections crossing its crop)
            if (!inside(p0) && !inside(p1)) || crop_box.is_some_and(|r| clip(p0, p1, r).is_none()) {
                continue;
            }
            if std::env::var("SECTION_DEBUG").is_ok_and(|v| v == vp.view.to_string()) {
                eprintln!("DRAWN {} {:?} z {z0:.2}..{z1:.2} cut {cut:.2} ends {:?} crop t {t0:.3}..{t1:.3} len {len:.3} in {} {}", mk.id, target.name, ends, inside(p0), inside(p1));
            }
            // its line: whole, or its ends only
            let span = (p1[0] - p0[0]).hypot(p1[1] - p0[1]);
            if span < 1e-9 {
                continue;
            }
            let u = [(p1[0] - p0[0]) / span, (p1[1] - p0[1]) / span];
            let pieces: Vec<[P2; 2]> = if ends.broken {
                // (kept lengths on paper; a third of the line each when the view does not say)
                let kept = |l: f64| if l > 0.0 { (l * k).min(span * 0.5) } else { span / 3.0 };
                let (l0, l1) = (kept(ends.kept[0]), kept(ends.kept[1]));
                vec![[p0, [p0[0] + u[0] * l0, p0[1] + u[1] * l0]], [[p1[0] - u[0] * l1, p1[1] - u[1] * l1], p1]]
            } else {
                vec![[p0, p1]]
            };
            for pts in pieces {
                let lines = self.ann.category_line(SECTION_LINE, &pts).unwrap_or_else(|| vec![(pts.to_vec(), self.ann.pen(5), [0; 3])]);
                for (pts, w, col) in lines {
                    out.strokes.push(Stroke2d { pts: pts.iter().map(|q| [m(q[0]), m(q[1])]).collect(), width: m(w), color: col });
                }
            }
            // the way it looks, on paper (its box's z: the head's arrow, the family's +y)
            let lk = {
                let (q, o) = (to_paper(proj(c.bz)), to_paper(proj([0.0; 3])));
                let q = [q[0] - o[0], q[1] - o[1]];
                let l = q[0].hypot(q[1]).max(1e-12);
                [q[0] / l, q[1] / l]
            };
            let sheet = self.viewports.iter().find(|v| v.view == mk.target).map(|v| v.owner);
            let label = self.view_types.get(&target.vtype).map(|t| t.2.clone()).unwrap_or_default();
            for (end, inward, mark) in [(p0, u, ends.marks[0]), (p1, [-u[0], -u[1]], ends.marks[1])] {
                let sym = match mark {
                    1 => head,
                    2 => tail,
                    _ => continue,
                };
                let Some(sy) = self.symbols.get(&sym) else { continue };
                let tf = Trf::new([inward[0], inward[1], 0.0, lk[0], lk[1], 0.0, 0.0, 0.0, 1.0], [end[0], end[1], 0.0]);
                self.put_symbol(out, sy, &tf, None);
                // (its circle's contents upright: its texts and labels about the circle's centre)
                let pivot = closed_centre(sy, self.family_drawing(sy, None).as_deref());
                let pc = tf.apply([pivot[0], pivot[1], 0.0]);
                if std::env::var("SECTION_DEBUG").is_ok_and(|v| v == vp.view.to_string()) {
                    let mm = |p: &[f64]| format!("({:.1},{:.1})", p[0] * 304.8, p[1] * 304.8);
                    let closed: Vec<String> = self.family_drawing(sy, None).iter().flat_map(|d| d.strokes.iter()).map(|c| format!("{}pts {}..{} closed {}", c.0.len(), mm(&c.0[0]), mm(c.0.last().unwrap()), c.0.first().zip(c.0.last()).is_some_and(|(p, q)| (p[0] - q[0]).hypot(p[1] - q[1]) < 1e-9))).collect();
                    eprintln!("HEAD {} mark {mark} sym {sym} pivot {} inward {inward:?} look {lk:?} curves {closed:?} texts {:?} tags {:?}", mk.id, mm(&pivot), sy.texts.iter().map(|t| mm(&t.orig)).collect::<Vec<_>>(), sy.tags.iter().map(|t| (mm(&t.orig), t.params.iter().map(|p| p.0).collect::<Vec<_>>())).collect::<Vec<_>>());
                }
                let up = Trf::new([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], [pc[0] - pivot[0], pc[1] - pivot[1], 0.0]);
                for t in &sy.texts {
                    push_text(out, t, Some(&up), 1.0, t.text.clone(), self.look(t));
                }
                for t in &sy.tags {
                    let vals: Vec<(usize, String)> = t
                        .params
                        .iter()
                        .enumerate()
                        .filter_map(|(i, (p, pre, suf, _))| {
                            let v = match *p {
                                // (a view on no sheet: its numbers as dashes)
                                VIEWER_SHEET_NUMBER => Some(sheet.and_then(|sh| numbers.get(&sh)).cloned().unwrap_or_else(|| "---".into())),
                                VIEWER_DETAIL_NUMBER => Some(self.viewports.iter().find(|v| v.view == mk.target).map(|v| v.number.clone()).unwrap_or_else(|| "-".into())),
                                VIEWER_REFERENCE_LABEL => mk.reference.then(|| label.clone()),
                                VIEW_NAME => Some(target.name.clone()),
                                _ => target.strs.get(p).cloned(),
                            }?;
                            (!v.is_empty()).then(|| (i, format!("{pre}{v}{suf}")))
                        })
                        .collect();
                    push_text(out, t, Some(&up), 1.0, t.join(&vals), self.look(t));
                }
            }
        }
    }

    /// The callouts `view` shows (see `draw_view`): `proj` maps model feet to its plane,
    /// `to_paper` its plane to the paper (feet), `inside` whether a paper point is inside its
    /// annotation crop, `mine` whether a view's annotations are its own; `numbers` the sheets'
    /// numbers.
    #[allow(clippy::too_many_arguments)]
    fn draw_callouts(&self, out: &mut Sheet2d, view: &ViewInfo, vp: &Vp, proj: &dyn Fn(V3) -> P2, to_paper: &dyn Fn(P2) -> P2, inside: &dyn Fn(P2) -> bool, mine: &dyn Fn(i64) -> bool, numbers: &HashMap<i64, String>) {
        const VIEWERS: i64 = -2000278;
        const CALLOUTS: i64 = -2000537;
        const CALLOUT_BOUNDARY: i64 = -2000539;
        const CALLOUT_LEADER: i64 = -2000544;
        let m = |x: f64| (x * FEET) as f32;
        let plan = view.dir.is_some_and(|d| d[2].abs() > 0.99);
        if !plan || view.hidden_cats.contains(&VIEWERS) || view.hidden_cats.contains(&CALLOUTS) {
            return;
        }
        let k = view.scale.max(1e-9);
        for mk in self.marks.iter().filter(|mk| mk.crop.bz[2].abs() > 0.99 && (mine(mk.parent) || mine(mk.owner)) && mk.target != vp.view) {
            // hidden here, or at this scale
            if view.hidden.contains(&mk.id) || self.views.get(&view.primary).is_some_and(|p| p.hidden.contains(&mk.id)) || (mk.coarser > 0.0 && view.scale < mk.coarser - 1e-9) {
                continue;
            }
            let Some(target) = self.views.get(&mk.target) else { continue };
            let (radius, head) = self.view_types.get(&target.vtype).and_then(|t| self.callout_tags.get(&t.1)).copied().unwrap_or((0.0, -1));
            // its outline in this view's plane: the sketch's lines chained, else the crop
            let c = &mk.crop;
            let at = |u: f64, v: f64| -> V3 { std::array::from_fn(|i| c.orig[i] + c.bx[i] * u + c.by[i] * v) };
            let loops: Vec<Vec<P2>> = if mk.shape.is_empty() {
                vec![[(c.off[1], c.off[3]), (c.off[0], c.off[3]), (c.off[0], c.off[2]), (c.off[1], c.off[2])].iter().map(|&(u, v)| proj(at(u, v))).collect()]
            } else {
                mk.shape.iter().map(|l| l.iter().map(|s| proj(s[0])).collect()).collect()
            };
            let outline: Vec<Vec<P2>> = loops.iter().filter(|l| l.len() >= 3).map(|lp| rounded(lp, radius / k)).collect();
            // shown whole or not at all (by the annotation crop)
            if outline.is_empty() || outline.iter().flatten().any(|p| !inside(to_paper(*p))) {
                continue;
            }
            for curve in loops.iter().filter(|l| l.len() >= 3).flat_map(|lp| rounded_curves(lp, radius / k)) {
                let pts: Vec<P2> = curve.iter().map(|p| to_paper(*p)).collect();
                let pieces = self.ann.category_line(CALLOUT_BOUNDARY, &pts).unwrap_or_else(|| vec![(pts.clone(), self.ann.pen(5), [0; 3])]);
                for (pts, w, col) in pieces {
                    out.strokes.push(Stroke2d { pts: pts.iter().map(|q| [m(q[0]), m(q[1])]).collect(), width: m(w), color: col });
                }
            }
            // its head in this view (else in the view it depends on), the leader to it from the
            // nearest point of the boundary
            let Some((h, leader)) = mk.heads.iter().find(|h| h.0 == vp.view).or_else(|| mk.heads.iter().find(|h| h.0 == view.primary)).or(mk.heads.first()).map(|h| (proj(h.1), h.2)) else { continue };
            // (its leader: from the boundary's point nearest its elbow by its elbow, else
            // straight from the boundary's point nearest the head: P-111's leaving the
            // boundary square to it)
            let pts: Vec<P2> = match leader {
                Some((end, elbow)) => {
                    let e = proj(elbow);
                    vec![to_paper(nearest_on(&outline, e).unwrap_or(proj(end))), to_paper(e), to_paper(h)]
                }
                None => nearest_on(&outline, h).map(|e| vec![to_paper(e), to_paper(h)]).unwrap_or_default(),
            };
            if std::env::var_os("CALLOUT_DEBUG").is_some() {
                let e = out.extent;
                let mm = |q: &P2| [((q[0] * FEET - e[0] as f64) * 1000.0 * 10.0).round() / 10.0, ((e[3] as f64 - q[1] * FEET) * 1000.0 * 10.0).round() / 10.0];
                eprintln!("CALLOUT {} view {} primary {} leader {:?} head {:?} heads {:?}", mk.id, vp.view, view.primary, pts.iter().map(mm).collect::<Vec<_>>(), mm(&to_paper(h)), mk.heads.iter().map(|x| (x.0, mm(&to_paper(proj(x.1))), x.2.is_some())).collect::<Vec<_>>());
            }
            if pts.len() >= 2 {
                let pieces = self.ann.category_line(CALLOUT_LEADER, &pts).unwrap_or_else(|| vec![(pts.clone(), self.ann.pen(4), [0; 3])]);
                for (pts, w, col) in pieces {
                    out.strokes.push(Stroke2d { pts: pts.iter().map(|q| [m(q[0]), m(q[1])]).collect(), width: m(w), color: col });
                }
            }
            let Some(sy) = self.symbols.get(&head) else { continue };
            // the head touches the leader by its side facing it, level with its origin; reached
            // from above or below, by its top or bottom, its middle there (P-211's)
            let q = to_paper(h);
            let prev = if pts.len() >= 2 { pts[pts.len() - 2] } else { q };
            let upright = (prev[1] - q[1]).abs() > (prev[0] - q[0]).abs();
            let (dx, dy) = (q[0] - prev[0], q[1] - prev[1]);
            let dl = dx.hypot(dy);
            let slanted = dl > 1e-9 && dx.abs().min(dy.abs()) > 0.2 * dl;
            let at = match drawn_box(sy, self.family_drawing(sy, None).as_deref()) {
                // reached aslant: the leader aimed at its middle, touching it there (AC-232's)
                Some(b) if slanted => {
                    let r = (b[2] - b[0]).min(b[3] - b[1]) * 0.5;
                    [(b[0] + b[2]) * 0.5 - r * dx / dl, (b[1] + b[3]) * 0.5 - r * dy / dl]
                }
                Some(b) if upright => [(b[0] + b[2]) * 0.5, if prev[1] > q[1] { b[3] } else { b[1] }],
                Some(b) => [if prev[0] <= q[0] { b[0] } else { b[2] }, 0.0],
                None => [0.0; 2],
            };
            let tf = Trf::new([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], [q[0] - at[0], q[1] - at[1], 0.0]);
            self.put_symbol(out, sy, &tf, None);
            for t in &sy.texts {
                push_text(out, t, Some(&tf), 1.0, t.text.clone(), self.look(t));
            }
            let sheet = self.viewports.iter().find(|v| v.view == mk.target).map(|v| v.owner);
            let label = self.view_types.get(&target.vtype).map(|t| t.2.clone()).unwrap_or_default();
            for t in &sy.tags {
                if std::env::var_os("SHEET_DEBUG").is_some() {
                    eprintln!("    callout {} -> {} labels {:?}", mk.id, mk.target, t.params);
                }
                let vals: Vec<(usize, String)> = t
                    .params
                    .iter()
                    .enumerate()
                    .filter_map(|(i, (p, pre, suf, _))| {
                        let v = match *p {
                            VIEWER_SHEET_NUMBER => sheet.and_then(|sh| numbers.get(&sh)).cloned(),
                            VIEWER_DETAIL_NUMBER => self.viewports.iter().find(|v| v.view == mk.target).map(|v| v.number.clone()),
                            VIEWER_REFERENCE_LABEL => mk.reference.then(|| label.clone()),
                            VIEW_NAME => Some(target.name.clone()),
                            _ => target.strs.get(p).cloned(),
                        }?;
                        (!v.is_empty()).then(|| (i, format!("{pre}{v}{suf}")))
                    })
                    .collect();
                push_text(out, t, Some(&tf), 1.0, t.join(&vals), self.look(t));
            }
        }
    }
}

/// The box on paper (metres: left, bottom, right, top) of horizontal texts `ts` as drawn,
/// each line's width measured in Arial; none when one is turned or there are none.
fn texts_box(ts: &[Text2d]) -> Option<[f32; 4]> {
    let mut b = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    for t in ts.iter().filter(|t| !t.text.trim().is_empty()) {
        if t.dir[1].abs() > 1e-3 || t.dir[0] <= 0.0 {
            return None;
        }
        let fs = sw_format::font_size(t.height);
        let mut w = t.text.split('\n').map(|l| sw_format::arial_width(l.trim_end(), fs, t.bold)).fold(0.0f32, f32::max);
        if t.wrap > 0.0 {
            w = w.min(t.wrap * sw_format::WRAP_SLACK);
        }
        let x0 = t.pos[0] - [0.0, 0.5, 1.0][t.halign.min(2) as usize] * w * sw_format::align_scale(t.height);
        b = [b[0].min(x0), b[1].min(t.pos[1] - t.height), b[2].max(x0 + w), b[3].max(t.pos[1] + t.height)];
    }
    b[0].is_finite().then_some(b)
}

/// The box of a family's lines (its own and its family drawing's), in its coordinates.
fn drawn_box(sy: &Sym, drawn: Option<&crate::rvt_annot::Drawn>) -> Option<[f64; 4]> {
    let own = sy.curves.iter().flatten().map(|p| [p[0], p[1]]);
    let theirs = drawn.into_iter().flat_map(|d| d.strokes.iter().flat_map(|s| s.0.iter().copied()));
    let b = own.chain(theirs).fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]);
    b[0].is_finite().then_some(b)
}

/// The centre of a family's curved lines (a section head's circle, often two arcs), its own or
/// its family drawing's, in its coordinates; its origin when it has none.
fn closed_centre(sy: &Sym, drawn: Option<&crate::rvt_annot::Drawn>) -> P2 {
    let own = sy.curves.iter().map(|c| c.iter().map(|p| [p[0], p[1]]).collect::<Vec<P2>>());
    let theirs = drawn.into_iter().flat_map(|d| d.strokes.iter().map(|s| s.0.clone()));
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for c in own.chain(theirs).filter(|c| c.len() > 2) {
        for p in c {
            b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
        }
    }
    if b[0].is_finite() {
        return [(b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5];
    }
    // (no closed curve: the middle of its lines along its line, on the line; a "▷" head drawn as
    // three single lines, its "IOU" and "Coupe 37" upright on the section line, AC-XX2)
    let mut x = [f64::INFINITY, f64::NEG_INFINITY];
    for p in sy.curves.iter().flatten().map(|p| p[0]).chain(drawn.into_iter().flat_map(|d| d.strokes.iter().flat_map(|s| s.0.iter().map(|p| p[0])))) {
        x = [x[0].min(p), x[1].max(p)];
    }
    if x[0].is_finite() { [(x[0] + x[1]) * 0.5, 0.0] } else { [0.0, 0.0] }
}

/// The drawing `d` turned about `c` (paper metres) by `turns` quarter turns counter-clockwise.
fn turn_about(d: &mut Sheet2d, c: [f32; 2], turns: i64) {
    let (cs, sn) = match turns.rem_euclid(4) {
        1 => (0.0f32, 1.0f32),
        2 => (-1.0, 0.0),
        3 => (0.0, -1.0),
        _ => return,
    };
    let at = |p: [f32; 2]| [c[0] + (p[0] - c[0]) * cs - (p[1] - c[1]) * sn, c[1] + (p[0] - c[0]) * sn + (p[1] - c[1]) * cs];
    let dir = |v: [f32; 2]| [v[0] * cs - v[1] * sn, v[0] * sn + v[1] * cs];
    for l in &mut d.lines {
        for p in l.iter_mut() {
            *p = at(*p);
        }
    }
    for s in &mut d.strokes {
        for p in s.pts.iter_mut() {
            *p = at(*p);
        }
    }
    for f in &mut d.fills {
        for t in f.tris.iter_mut() {
            *t = t.map(at);
        }
    }
    for t in &mut d.texts {
        t.pos = at(t.pos);
        t.dir = dir(t.dir);
    }
}

/// Polygon `pts` (closed) with each corner rounded by an arc of radius `r` (as far as its
/// sides allow), as a polyline.
fn rounded(pts: &[P2], r: f64) -> Vec<P2> {
    rounded_corners(pts, r).concat()
}

/// The curves of polygon `pts` rounded as [`rounded`] does, each drawn on its own as Revit
/// draws a callout's (its line pattern starting again on each): its sides and corner arcs.
fn rounded_curves(pts: &[P2], r: f64) -> Vec<Vec<P2>> {
    let corners = rounded_corners(pts, r);
    let n = corners.len();
    let mut out = Vec::new();
    for i in 0..n {
        let (c, next) = (&corners[i], &corners[(i + 1) % n]);
        if c.len() > 1 {
            out.push(c.clone());
        }
        out.push(vec![*c.last().unwrap(), next[0]]);
    }
    out
}

/// Each corner of polygon `pts` rounded by radius `r`: its arc's points, else the corner.
fn rounded_corners(pts: &[P2], r: f64) -> Vec<Vec<P2>> {
    let n = pts.len();
    if r <= 1e-9 || n < 3 {
        return pts.iter().map(|p| vec![*p]).collect();
    }
    let mut out = Vec::new();
    for i in 0..n {
        let (a, p, b) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
        let (u, w) = ([a[0] - p[0], a[1] - p[1]], [b[0] - p[0], b[1] - p[1]]);
        let (lu, lw) = (u[0].hypot(u[1]), w[0].hypot(w[1]));
        if lu < 1e-9 || lw < 1e-9 {
            out.push(vec![p]);
            continue;
        }
        let (u, w) = ([u[0] / lu, u[1] / lu], [w[0] / lw, w[1] / lw]);
        let cos = (u[0] * w[0] + u[1] * w[1]).clamp(-1.0, 1.0);
        let half = cos.acos() * 0.5;
        // straight on: no corner
        if half > std::f64::consts::FRAC_PI_2 - 1e-6 || half < 1e-6 {
            out.push(vec![p]);
            continue;
        }
        // where the arc meets each side, its centre on the bisector
        let t = (r / half.tan()).min(lu * 0.5).min(lw * 0.5);
        let rr = t * half.tan();
        let (s, e) = ([p[0] + u[0] * t, p[1] + u[1] * t], [p[0] + w[0] * t, p[1] + w[1] * t]);
        let bis = [u[0] + w[0], u[1] + w[1]];
        let lb = bis[0].hypot(bis[1]).max(1e-12);
        let d = rr / half.sin();
        let c = [p[0] + bis[0] / lb * d, p[1] + bis[1] / lb * d];
        let (a0, a1) = ((s[1] - c[1]).atan2(s[0] - c[0]), (e[1] - c[1]).atan2(e[0] - c[0]));
        let mut sweep = a1 - a0;
        while sweep > std::f64::consts::PI {
            sweep -= std::f64::consts::TAU;
        }
        while sweep < -std::f64::consts::PI {
            sweep += std::f64::consts::TAU;
        }
        let steps = 8;
        out.push((0..=steps).map(|j| a0 + sweep * j as f64 / steps as f64).map(|ang| [c[0] + rr * ang.cos(), c[1] + rr * ang.sin()]).collect());
    }
    out
}

/// The point of the closed polylines `rings` nearest `p`.
fn nearest_on(rings: &[Vec<P2>], p: P2) -> Option<P2> {
    let mut best: Option<(f64, P2)> = None;
    for ring in rings {
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l2 = dx * dx + dy * dy;
            let t = if l2 > 1e-18 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let q = [a[0] + dx * t, a[1] + dy * t];
            let d = (q[0] - p[0]).hypot(q[1] - p[1]);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, q));
            }
        }
    }
    best.map(|b| b.1)
}

/// Length scale of a transform in its plane.
fn trf_scale(tf: &Trf) -> f64 {
    let x = tf.lin([1.0, 0.0, 0.0]);
    (x[0] * x[0] + x[1] * x[1]).sqrt()
}

/// A text placed by `tf` (paper feet out), its height scaled by `hk`.
/// How a text is written: bold, colour, size its type gives (feet), typeface, italic,
/// underlined, tab stops (paper feet).
#[derive(Clone, Debug, Default)]
struct TextLook {
    bold: bool,
    color: [u8; 3],
    size: Option<f64>,
    font: String,
    italic: bool,
    underline: bool,
    tab: f64,
}

/// `segs` with each loose end (meeting no other line's) carried on to where it meets the
/// nearest line (as far as `reach` past its end, and past that line's end as far), and that
/// line carried on to meet it.
fn extend_ends(segs: &[[P2; 2]], reach: f64) -> Vec<[P2; 2]> {
    const TOL: f64 = 3e-3;
    let near = |a: P2, b: P2| (a[0] - b[0]).abs() < TOL && (a[1] - b[1]).abs() < TOL;
    let cross = |a: P2, b: P2| a[0] * b[1] - a[1] * b[0];
    let mut out = segs.to_vec();
    let mut loose: Vec<(usize, P2)> = Vec::new();
    for (i, s) in segs.iter().enumerate() {
        for (end, other) in [(s[1], s[0]), (s[0], s[1])] {
            // (an end on another line, at its end or along it, is joined already)
            let on = |t: &[P2; 2]| {
                let (d, w) = ([t[1][0] - t[0][0], t[1][1] - t[0][1]], [end[0] - t[0][0], end[1] - t[0][1]]);
                let l = d[0] * d[0] + d[1] * d[1];
                let u = (w[0] * d[0] + w[1] * d[1]) / l.max(1e-18);
                (-1e-9..=1.0 + 1e-9).contains(&u) && cross(d, w).abs() / l.sqrt().max(1e-9) < TOL
            };
            if segs.iter().enumerate().any(|(j, t)| j != i && (near(t[0], end) || near(t[1], end) || on(t))) {
                continue;
            }
            let len = ((end[0] - other[0]).powi(2) + (end[1] - other[1]).powi(2)).sqrt();
            if len < 1e-9 {
                continue;
            }
            let d = [(end[0] - other[0]) / len, (end[1] - other[1]) / len];
            // the nearest line ahead (its own length grown by `reach` each way)
            let hit = segs
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .filter_map(|(_, t)| {
                    let q = [t[1][0] - t[0][0], t[1][1] - t[0][1]];
                    let ql = (q[0] * q[0] + q[1] * q[1]).sqrt();
                    let den = cross(d, q);
                    if den.abs() < 1e-9 * ql || ql < 1e-9 {
                        return None;
                    }
                    let w = [t[0][0] - end[0], t[0][1] - end[1]];
                    let (k, u) = (cross(w, q) / den, cross(w, d) / den);
                    (k > 1e-9 && k <= reach && u * ql >= -reach && (u - 1.0) * ql <= reach).then_some((k, t, u))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((k, t, u)) = hit {
                let x = [end[0] + d[0] * k, end[1] + d[1] * k];
                out.push([end, x]);
                if u < 0.0 {
                    out.push([t[0], x]);
                } else if u > 1.0 {
                    out.push([t[1], x]);
                }
            } else {
                loose.push((i, end));
            }
        }
    }
    // loose ends nothing ahead meets (a line broken in two, AC-294's ZF1, or stopping short
    // beside an arc's end): joined to the nearest other line's end within reach
    for &(i, a) in &loose {
        let near = segs.iter().enumerate().filter(|(j, _)| *j != i).flat_map(|(_, t)| *t).map(|b| (((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt(), b)).filter(|(l, _)| *l > TOL && *l <= reach).min_by(|x, y| x.0.total_cmp(&y.0));
        if let Some((_, b)) = near {
            out.push([a, b]);
        }
    }
    out
}

/// The face of the lines `segs` around point `p`: the edges of the smallest loop they make
/// that holds it (split where they cross or touch), or none when no loop does.
fn face_around(segs: &[[P2; 2]], p: P2) -> Option<Vec<[P2; 2]>> {
    const TOL: f64 = 3e-3;
    let cross = |a: P2, b: P2| a[0] * b[1] - a[1] * b[0];
    let sub = |a: P2, b: P2| [a[0] - b[0], a[1] - b[1]];
    // a line whose end meets no other is drawn to where it meets the nearest line within a
    // foot, that line drawn there too (Revit joins its boundary lines' ends: AC-293's ZF12
    // closes at a corner its two lines stop 14 cm short of)
    let segs = &extend_ends(segs, 1.0);
    // each line split where another crosses or touches it
    let mut cuts: Vec<Vec<f64>> = vec![vec![0.0, 1.0]; segs.len()];
    for i in 0..segs.len() {
        for j in i + 1..segs.len() {
            let (a, r, b, q) = (segs[i][0], sub(segs[i][1], segs[i][0]), segs[j][0], sub(segs[j][1], segs[j][0]));
            let d = cross(r, q);
            if d.abs() < 1e-12 {
                continue;
            }
            let (t, u) = (cross(sub(b, a), q) / d, cross(sub(b, a), r) / d);
            let (et, eu) = (TOL / cross(r, r).sqrt().max(1e-12), TOL / cross(q, q).sqrt().max(1e-12));
            if t > -et && t < 1.0 + et && u > -eu && u < 1.0 + eu {
                cuts[i].push(t.clamp(0.0, 1.0));
                cuts[j].push(u.clamp(0.0, 1.0));
            }
        }
    }
    // their pieces between shared points
    let mut pts: Vec<P2> = Vec::new();
    let mut at = |q: P2| -> usize {
        if let Some(k) = pts.iter().position(|v| (v[0] - q[0]).abs() < TOL && (v[1] - q[1]).abs() < TOL) {
            return k;
        }
        pts.push(q);
        pts.len() - 1
    };
    let mut edges: Vec<(usize, usize)> = Vec::new();
    for (s, c) in segs.iter().zip(cuts.iter_mut()) {
        c.sort_by(f64::total_cmp);
        let ks: Vec<usize> = c.iter().map(|&t| at([s[0][0] + (s[1][0] - s[0][0]) * t, s[0][1] + (s[1][1] - s[0][1]) * t])).collect();
        for w in ks.windows(2) {
            if w[0] != w[1] && !edges.iter().any(|e| *e == (w[0], w[1]) || *e == (w[1], w[0])) {
                edges.push((w[0], w[1]));
            }
        }
    }
    // around each point its neighbours, counter-clockwise
    let mut around: Vec<Vec<usize>> = vec![Vec::new(); pts.len()];
    for &(a, b) in &edges {
        around[a].push(b);
        around[b].push(a);
    }
    let ang = |a: usize, b: usize| (pts[b][1] - pts[a][1]).atan2(pts[b][0] - pts[a][0]);
    for (a, n) in around.iter_mut().enumerate() {
        n.sort_by(|x, y| ang(a, *x).total_cmp(&ang(a, *y)));
    }
    // the nearest line right of the point, taken with the point on its left
    let y = p[1] + 1e-7;
    let (a, b) = edges
        .iter()
        .filter_map(|&(a, b)| {
            let (pa, pb) = (pts[a], pts[b]);
            if (pa[1] - y) * (pb[1] - y) >= 0.0 {
                return None;
            }
            let x = pa[0] + (pb[0] - pa[0]) * (y - pa[1]) / (pb[1] - pa[1]);
            (x > p[0]).then_some((x, a, b))
        })
        .min_by(|u, v| u.0.total_cmp(&v.0))
        .map(|(_, a, b)| if cross(sub(pts[b], pts[a]), sub(p, pts[a])) > 0.0 { (a, b) } else { (b, a) })?;
    // round its face, turning left as far as it goes at each point
    let (mut u, mut v) = (a, b);
    let mut face = Vec::new();
    let mut area = 0.0;
    for _ in 0..=edges.len() * 2 {
        face.push([pts[u], pts[v]]);
        area += cross(pts[u], pts[v]);
        let n = &around[v];
        let k = n.iter().position(|&x| x == u)?;
        let w = n[(k + n.len() - 1) % n.len()];
        (u, v) = (v, w);
        if (u, v) == (a, b) {
            // (counter-clockwise: an inside; clockwise, the point is outside every loop)
            return (area > 0.0).then_some(face);
        }
    }
    None
}

fn push_text(out: &mut Sheet2d, t: &RawText, tf: Option<&Trf>, hk: f64, text: String, lk: TextLook) {
    let TextLook { bold, color, size: typed, .. } = lk.clone();
    if text.trim().is_empty() {
        return;
    }
    if let Some(q) = std::env::var_os("TEXT_DEBUG") {
        if text.contains(q.to_string_lossy().as_ref()) {
            eprintln!("PUSH {text:?} attr {} raw {:?} h {:.4} color {color:?} flags {:#x} base {:?} params {:?} orig {:?} wrap {:.4} width {:.4} leaders {:?}", t.attr, t.text, t.height, t.flags, t.base, t.params, t.orig, t.wrap, t.width, t.leaders);
        }
    }
    let m = |x: f64| (x * FEET) as f32;
    // (mirrored: its up direction turned to its other side)
    let (o, b, mirrored) = match tf {
        Some(tf) => {
            let (b, u) = (tf.lin(t.base), tf.lin([-t.base[1], t.base[0], 0.0]));
            (tf.apply(t.orig), b, b[0] * u[1] - b[1] * u[0] < 0.0)
        }
        None => (t.orig, t.base, false),
    };
    let l = (b[0] * b[0] + b[1] * b[1]).sqrt().max(1e-9);
    // alignment: top / middle / bottom flags 512 / 1024 / 2048
    let h = t.halign();
    let v = if t.flags & 2048 != 0 {
        2
    } else if t.flags & 1024 != 0 {
        1
    } else {
        0
    };
    // the type's size; else what the text box holds
    let size = typed.unwrap_or_else(|| text_size(t.height, t.wrap, &text)) * hk;
    if std::env::var_os("WRAP_DEBUG").is_some() {
        eprintln!("WRAP wrap {:.7} true {:.7} size {:.7} bold {bold} font {:?} params {} nowrap {} {text:?}", t.wrap * hk, t.width * hk, size, lk.font, t.params.len(), t.wrap <= 0.0);
    }
    // a text stays readable: mirrored, it stays upright on the other side of its line; a text, or
    // a label kept readable (flag 0x2000), turned past a quarter turn either way (to -90° included)
    // is turned half a turn about where it stands (on Revit's prints, a sprinkler symbol's text
    // at 110° reads at -70°); another keeps its angle (a view reference's "VOIR PLAN …" at
    // -90°, AC-225); one its own document already turned (its alignment flags and low bits
    // say different sides) is not turned again (AC-607's valve "GTB", "T1" read downwards,
    // where AC-604's "GT" at -90° reads upwards)
    let v = if mirrored { 2 - v } else { v };
    let ang = b[1].atan2(b[0]);
    let turned = t.flags & 0x1c0 != 0 && (t.flags & 3) as u8 != h;
    let readable = (t.params.is_empty() || t.flags & 0x2000 != 0) && !turned;
    let over = readable && (ang > std::f64::consts::FRAC_PI_2 + 1e-4 || ang <= -std::f64::consts::FRAC_PI_2 + 1e-4);
    // (a text in a box of its own width turned about the box's middle: its alignment kept,
    // where its box's other side was: P-608's "TUYAUTERIE - 07" left-aligned at the left of
    // its box, its leader leaving the box's right side)
    let (o, b, h, v) = match (over, t.wrap > 0.0 && t.params.is_empty()) {
        (true, true) => {
            let k = t.wrap * hk * (1.0 - h.min(2) as f64) / l;
            ([o[0] + b[0] * k, o[1] + b[1] * k, o[2]], [-b[0], -b[1], b[2]], h, 2 - v)
        }
        (true, false) => (o, [-b[0], -b[1], b[2]], 2 - h.min(2), 2 - v),
        _ => (o, b, h, v),
    };
    // (its rich text's parts when it is the text itself, not a label's values)
    let spans = if text == t.text { t.spans.clone() } else { Vec::new() };
    // (bold by its type or when its rich text mostly is, unless its parts say which are, whatever
    // its type: AC-100's notes, bold French lines and italic English ones spaced by their own
    // faces; its legend's "\b0\i SHEET ON WHICH…" under a bold type, printed italic only)
    let bold = spans.is_empty() && (bold || t.bold);
    out.texts.push(Text2d {
        pos: [m(o[0]), m(o[1])],
        dir: [(b[0] / l) as f32, (b[1] / l) as f32],
        height: m(size),
        halign: h.min(2),
        valign: v,
        text,
        bold,
        wrap: m(t.wrap * hk),
        color,
        font: lk.font,
        italic: lk.italic,
        underline: lk.underline,
        spans,
        tab: m(lk.tab * hk),
        // (a text note's words break; a label's run past)
        break_words: t.params.is_empty(),
    });
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

/// Whether the lines drawn from a family document (nearly all of them) lie on its type's own
/// lines: some of the type's lines may be hidden, none moved.
fn within(d: &crate::rvt_annot::Drawn, curves: &[Vec<V3>]) -> bool {
    let near = |p: P2| {
        curves.iter().any(|c| {
            c.windows(2).any(|w| {
                let (a, b) = ([w[0][0], w[0][1]], [w[1][0], w[1][1]]);
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let l2 = dx * dx + dy * dy;
                let t = if l2 > 1e-18 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
                let (ex, ey) = (a[0] + dx * t - p[0], a[1] + dy * t - p[1]);
                ex * ex + ey * ey < 0.003 * 0.003
            })
        })
    };
    let pts: Vec<P2> = d.strokes.iter().flat_map(|s| s.0.iter().copied()).collect();
    let off = pts.iter().filter(|p| !near(**p)).count();
    off * 10 <= pts.len()
}

/// Whether most of a type's own (regenerated) lines are not where its family document draws
/// them: its parameters move them (a parametric family, drawn at its default values).
fn moved(d: &crate::rvt_annot::Drawn, curves: &[Vec<V3>]) -> bool {
    let near = |p: P2| {
        d.strokes.iter().any(|(pts, _, _)| {
            pts.windows(2).any(|w| {
                let (a, b) = (w[0], w[1]);
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let l2 = dx * dx + dy * dy;
                let t = if l2 > 1e-18 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
                let (ex, ey) = (a[0] + dx * t - p[0], a[1] + dy * t - p[1]);
                ex * ex + ey * ey < 0.003 * 0.003
            })
        })
    };
    let mids: Vec<P2> = curves.iter().flat_map(|c| c.windows(2).map(|w| [(w[0][0] + w[1][0]) * 0.5, (w[0][1] + w[1][1]) * 0.5])).collect();
    let found = mids.iter().filter(|p| near(**p)).count();
    !mids.is_empty() && found * 2 < mids.len()
}

/// Annotation geometry (`Drawn`, view plane) put on the paper by `to_paper`.
fn put_drawn(out: &mut Sheet2d, d: &crate::rvt_annot::Drawn, to_paper: &dyn Fn(P2) -> P2) {
    let m = |x: f64| (x * FEET) as f32;
    let pt = |p: P2| {
        let q = to_paper(p);
        [m(q[0]), m(q[1])]
    };
    let s0 = out.strokes.len();
    for (pts, w, c) in &d.strokes {
        out.strokes.push(Stroke2d { pts: pts.iter().map(|p| pt(*p)).collect(), width: m(*w), color: *c });
    }
    for (tris, c) in &d.fills {
        out.fills.push(Fill2d { tris: tris.iter().map(|t| [pt(t[0]), pt(t[1]), pt(t[2])]).collect(), color: *c, under: None });
    }
    for (at, tris) in &d.masks {
        out.fills.push(Fill2d { tris: tris.iter().map(|t| [pt(t[0]), pt(t[1]), pt(t[2])]).collect(), color: [255; 3], under: Some(s0 + at) });
    }
    for (p, dir, h, text) in &d.texts {
        let (a, b) = (to_paper(*p), to_paper([p[0] + dir[0], p[1] + dir[1]]));
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = (dx * dx + dy * dy).sqrt().max(1e-12);
        out.texts.push(Text2d { pos: [m(a[0]), m(a[1])], dir: [(dx / l) as f32, (dy / l) as f32], height: m(*h), halign: 1, valign: 2, text: text.clone(), bold: false, wrap: 0.0, color: [0; 3], ..Default::default() });
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
    hatch_at(edges, gap, std::f64::consts::FRAC_PI_4)
}

/// Lines at `angle` (radians) `gap` apart inside the closed outline `edges` (even-odd).
fn hatch_at(edges: &[[P2; 2]], gap: f64, angle: f64) -> Vec<(P2, P2)> {
    let (c, s) = (angle.cos(), angle.sin());
    // rotated frame: u along the hatch, v across it
    let to = |p: P2| [p[0] * c + p[1] * s, p[1] * c - p[0] * s];
    let back = |u: f64, v: f64| [u * c - v * s, u * s + v * c];
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

/// Most model edges drawn in a view drawn alone (one on a sheet is drawn whole).
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
/// Colour of a graphics override (`m_color`; 0x1000000 and up: not overridden).
fn override_color(o: &Obj) -> Option<[u8; 3]> {
    o.int("m_color").filter(|c| (0..1 << 24).contains(c)).map(|c| [(c & 255) as u8, ((c >> 8) & 255) as u8, ((c >> 16) & 255) as u8])
}

/// The elements a view draws with graphics of their own: `AdHocOverrides`, element id ->
/// index in its settings (`m_oaSettings` -> `AdHocGraphicSettings` -> `OverrideGraphicSettings`).
fn elem_overrides(tr: &Tree, s: &Schema) -> HashMap<i64, Look> {
    let at = |i: usize| tr.objs.get(i);
    let Some(ah) = tr.objs.iter().find(|o| s.name(o.tag) == "AdHocOverrides") else { return HashMap::default() };
    let looks: Vec<Look> = ah
        .ptrs("m_oaSettings")
        .iter()
        .map(|&i| {
            let g = at(i).and_then(|a| a.ptr("m_pOverrideGraphicSettings")).and_then(at);
            let sub = |k: &str| g.and_then(|g| g.ptr(k)).and_then(at);
            let fill_on = g.and_then(|g| g.int("m_bProjFillPatternVisible")).unwrap_or(1) != 0;
            let fill_color = sub("m_oProjFillColor").and_then(override_color);
            let pattern = sub("m_oProjFillPattern").and_then(|p| p.int("m_fillPatternId")).filter(|p| *p > 0 && fill_on);
            Look {
                hidden: false,
                line: sub("m_oProjPenColor").and_then(override_color),
                pen: sub("m_oProjPenNumber").and_then(|p| p.int("m_penNumber")).filter(|n| (1..=16).contains(n)),
                pattern: sub("m_oProjLinePattern").and_then(|p| p.int("m_linePatternId")).filter(|id| *id != 0 && *id != -1),
                // (a fill colour alone keeps the element's own pattern: solid when it has none)
                fill: match (pattern, fill_color) {
                    (Some(p), c) => Some((p, c.unwrap_or([0; 3]))),
                    (None, Some(c)) => Some((-1, c)),
                    (None, None) => None,
                },
                halftone: g.and_then(|g| g.int("m_bHalftone")).unwrap_or(0) != 0,
            }
        })
        .collect();
    ah.list("m_elementMap").iter().filter_map(|e| Some((e.int("first")?, looks.get(usize::try_from(e.int("second")?).ok()?)?.clone()))).collect()
}

/// A view's filters and what each does there (`FilterOverrides` -> `FilterGraphicSettings`).
fn view_filters(tr: &Tree, s: &Schema) -> Vec<FilterUse> {
    let at = |i: usize| tr.objs.get(i);
    tr.objs
        .iter()
        .filter(|o| s.name(o.tag) == "FilterGraphicSettings")
        .filter_map(|o| {
            let g = o.ptr("m_pOverrideGraphicSettings").and_then(at);
            let sub = |k: &str| g.and_then(|g| g.ptr(k)).and_then(at);
            let fill_on = g.and_then(|g| g.int("m_bProjFillPatternVisible")).unwrap_or(1) != 0;
            let pattern = sub("m_oProjFillPattern").and_then(|p| p.int("m_fillPatternId")).filter(|p| *p > 0 && fill_on);
            Some(FilterUse {
                filter: o.int("m_filterElemId")?,
                visible: o.int("m_bVisibilityFlag").unwrap_or(1) != 0,
                line: sub("m_oProjPenColor").and_then(override_color),
                pen: sub("m_oProjPenNumber").and_then(|p| p.int("m_penNumber")).filter(|n| (1..=16).contains(n)),
                pattern: sub("m_oProjLinePattern").and_then(|p| p.int("m_linePatternId")).filter(|id| *id != 0 && *id != -1),
                fill: pattern.map(|p| (p, sub("m_oProjFillColor").and_then(override_color).unwrap_or([0; 3]))),
                halftone: g.and_then(|g| g.int("m_bHalftone")).unwrap_or(0) != 0,
            })
        })
        .collect()
}

/// A filter's rules (`m_rules`, or `m_oRuleCombination` of AND-ed parameter filters).
fn filter_rules(tr: &Tree, s: &Schema, i: usize, inverse: bool, out: &mut Vec<Rule>) -> bool {
    let Some(o) = tr.objs.get(i) else { return false };
    match s.name(o.tag) {
        "LogicalAndFilterData" => o.ptrs("m_filters").iter().all(|&j| filter_rules(tr, s, j, inverse, out)),
        "ElementParameterFilterData" => {
            let inv = inverse != (o.int("m_inverted").unwrap_or(0) != 0);
            o.ptrs("m_rules").iter().all(|&j| filter_rules(tr, s, j, inv, out))
        }
        "FilterInverseRule" => o.ptr("m_oInnerRule").is_some_and(|j| filter_rules(tr, s, j, !inverse, out)),
        n if n.starts_with("Filter") && n.ends_with("Rule") && n != "FilterCategoryRule" => {
            let Some(param) = o.ptr("m_oValueProvider").and_then(|j| tr.objs.get(j)).and_then(|p| p.int("m_parameter")) else { return false };
            let Some(op) = o.ptr("m_oEvaluator").and_then(|j| tr.objs.get(j)).map(|e| s.name(e.tag).to_string()) else { return false };
            let value = match (o.get("m_ruleString"), o.get("m_ruleValue")) {
                (Some(G::S(v)), _) => RuleValue::Str(v.clone()),
                (_, Some(G::I(v))) => RuleValue::Int(*v),
                (_, Some(G::F(v))) => RuleValue::Num(*v),
                _ => return false,
            };
            out.push(Rule { param, op, value, inverse });
            true
        }
        _ => false,
    }
}

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
/// The parameters of a family document's element that the family's own parameters set
/// (`FamilyParametrizedElemParamsCell`): (its parameter, the family's), its visibility apart.
fn param_drives(tr: &Tree, s: &Schema) -> Vec<(i64, i64)> {
    let cells = tr.objs.iter().filter(|o| s.name(o.tag) == "FamilyParametrizedElemParamsCell");
    cells.flat_map(|o| o.list("m_paramDrivenData").iter()).filter_map(|d| Some((d.int("m_elemPropId")?, d.int("m_famParamId")?))).filter(|(e, _)| *e != crate::rvt_annot::VISIBLE_PARAM).collect()
}

/// The element values of a family instance's or type's own parameters (`FamilyParams`
/// `m_elemId`): parameter -> element.
fn family_elems(tr: &Tree, s: &Schema) -> HashMap<i64, i64> {
    tr.objs.iter().filter(|o| s.name(o.tag) == "FamilyParams").flat_map(|o| o.list("m_params").iter()).filter_map(|p| Some((p.int("m_paramId")?, p.int("m_elemId").filter(|e| *e > 0)?))).collect()
}

/// A placed family's parameter values: texts, yes/no and integers, numbers.
#[derive(Debug, Clone, Default)]
struct Given {
    strs: HashMap<i64, String>,
    ints: HashMap<i64, i64>,
    reals: HashMap<i64, f64>,
}

fn family_ints(tr: &Tree, s: &Schema) -> HashMap<i64, i64> {
    tr.objs.iter().filter(|o| s.name(o.tag) == "FamilyParams").flat_map(|o| o.list("m_params").iter()).filter_map(|p| Some((p.int("m_paramId")?, p.int("m_int")?))).collect()
}

/// The real values of a family element's parameters (`m_value`; a number's, a length's…).
fn family_reals(tr: &Tree, s: &Schema) -> HashMap<i64, f64> {
    tr.objs.iter().filter(|o| s.name(o.tag) == "FamilyParams").flat_map(|o| o.list("m_params").iter()).filter_map(|p| Some((p.int("m_paramId")?, num(p, "m_value")?))).collect()
}

fn ids_of(tr: &Tree, s: &Schema, class: &str, field: &str) -> crate::det::HashSet<i64> {
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
pub(crate) struct Edges<'a> {
    /// element, category, box, its sharp edges (with their subcategory) and its smooth edges
    /// with the normals of their two faces (drawn where they outline it: its silhouette from
    /// the view) at each detail level (coarse, medium, fine: the solids shown there), its mesh
    items: Vec<(i64, i64, [f64; 3], [f64; 3], [std::sync::Arc<(Vec<([V3; 2], i64)>, Vec<Smooth>)>; 3], Option<&'a Mesh>)>,
}

/// Whether a family solid of visibility `vis` (`m_famElemVisibility`, -1: everywhere) shows
/// at detail level `d` (1 coarse, 2 medium, 3 fine).
fn shown_at(vis: i64, d: i64) -> bool {
    vis == -1 || vis & (1 << (12 + d)) != 0
}

/// The visibility of triangle `t` of a mesh (its runs, -1 when none is known).
fn tri_vis(m: &Mesh, t: usize) -> i64 {
    match m.vis.binary_search_by_key(&(t as u32), |r| r.0) {
        Ok(i) => m.vis[i].1,
        Err(0) => -1,
        Err(i) => m.vis[i - 1].1,
    }
}

/// How a model edge is drawn: width on paper (feet), colour, line pattern.
type SegStyle = (f64, [u8; 3], i64);

/// An edge between two faces of a curved surface: its ends, the faces' normals and its
/// subcategory.
type Smooth = ([V3; 2], V3, V3, i64);

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
    /// The edges of the model elements (id, category, box, rebuilt mesh if any).
    pub(crate) fn new(model: &[(i64, i64, [f64; 3], [f64; 3], Option<&'a Mesh>)]) -> Edges<'a> {
        let items = model
            .iter()
            .map(|(id, cat, lo, hi, mesh)| {
                let mesh = mesh.filter(|m| !m.tri.is_empty());
                let levels = match mesh {
                    // the same solids at every level: one set of edges
                    Some(m) if m.vis.iter().all(|r| (1..=3).all(|d| shown_at(r.1, d))) => {
                        let e = std::sync::Arc::new(feature_edges(m));
                        [e.clone(), e.clone(), e]
                    }
                    // else the edges of the solids each level shows
                    Some(m) => [1, 2, 3].map(|d| std::sync::Arc::new(feature_edges_where(m, &|t| shown_at(tri_vis(m, t), d)))),
                    // (a sprinkler's box holds its plan symbol at the view's scale, not its
                    // small head: none, FP-113, where Revit shows only the GHOST symbol)
                    None if *cat == SPRINKLERS => {
                        let e = std::sync::Arc::new((Vec::new(), Vec::new()));
                        [e.clone(), e.clone(), e]
                    }
                    None => {
                        let e = std::sync::Arc::new(([(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)].iter().map(|&(a, b)| ([corner(*lo, *hi, a), corner(*lo, *hi, b)], -1)).collect(), Vec::new()));
                        [e.clone(), e.clone(), e]
                    }
                };
                (*id, *cat, *lo, *hi, levels, mesh)
            })
            .collect();
        Edges { items }
    }
}

/// Edges of a mesh where its faces meet at an angle (or on its border), as Revit draws a
/// solid's edges; and the edges inside its curved surfaces (facets meeting nearly flat), with
/// their faces' normals, which show only as the silhouette.
fn feature_edges(m: &Mesh) -> (Vec<([V3; 2], i64)>, Vec<Smooth>) {
    feature_edges_where(m, &|_| true)
}

/// `feature_edges` of the triangles `keep` keeps (by index).
fn feature_edges_where(m: &Mesh, keep: &dyn Fn(usize) -> bool) -> (Vec<([V3; 2], i64)>, Vec<Smooth>) {
    let q = |p: [f32; 3]| [(p[0] * 2000.0).round() as i64, (p[1] * 2000.0).round() as i64, (p[2] * 2000.0).round() as i64];
    let mut by: std::collections::BTreeMap<([i64; 3], [i64; 3]), (Vec<[f64; 3]>, [V3; 2], i64)> = std::collections::BTreeMap::new();
    for (ti, t) in m.tri.iter().enumerate().filter(|(ti, _)| keep(*ti)) {
        // its subcategory (its solid's, in its family)
        let sub = match m.cats.binary_search_by_key(&(ti as u32), |r| r.0) {
            Ok(i) => m.cats[i].1,
            Err(0) => -1,
            Err(i) => m.cats[i - 1].1,
        };
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
            let e = by.entry(key).or_insert_with(|| (Vec::new(), [a.map(|x| x as f64), b.map(|x| x as f64)], sub));
            e.0.push(n);
        }
    }
    let (mut sharp, mut smooth) = (Vec::new(), Vec::new());
    // the edges of one face only: where faces meet in T (the strips of a cap), each edge of a
    // face meets several of the next face in its plane
    let mut open: Vec<([V3; 2], V3, i64)> = Vec::new();
    for (ns, e, sub) in by.into_values() {
        if ns.len() == 1 {
            open.push((e, ns[0], sub));
        } else if ns.len() != 2 || dot(ns[0], ns[1]) < SHARP {
            sharp.push((e, sub));
        } else if dot(ns[0], ns[1]) < 0.9999 {
            smooth.push((e, ns[0], ns[1], sub));
        }
    }
    // such an edge is one only where no face of its plane lies on its other side
    // (the edges on one line together: its direction and its point nearest the origin)
    let line_key = |e: &[V3; 2]| -> Option<[i64; 6]> {
        let d = [e[1][0] - e[0][0], e[1][1] - e[0][1], e[1][2] - e[0][2]];
        let l = dot(d, d).sqrt();
        if l < 1e-9 {
            return None;
        }
        let mut u = [d[0] / l, d[1] / l, d[2] / l];
        let k = if u[0].abs() > 1e-9 { u[0] } else if u[1].abs() > 1e-9 { u[1] } else { u[2] };
        if k < 0.0 {
            u = [-u[0], -u[1], -u[2]];
        }
        let t = dot(e[0], u);
        let c = [e[0][0] - u[0] * t, e[0][1] - u[1] * t, e[0][2] - u[2] * t];
        let q = |x: f64, s: f64| (x * s).round() as i64;
        Some([q(u[0], 1e3), q(u[1], 1e3), q(u[2], 1e3), q(c[0], 1e3), q(c[1], 1e3), q(c[2], 1e3)])
    };
    let mut by_line: HashMap<[i64; 6], Vec<usize>> = HashMap::default();
    for (i, (e, _, _)) in open.iter().enumerate() {
        if let Some(k) = line_key(e) {
            by_line.entry(k).or_default().push(i);
        }
    }
    for (i, (e, n, sub)) in open.iter().enumerate() {
        let same: &[usize] = line_key(e).and_then(|k| by_line.get(&k)).map_or(&[], |v| v.as_slice());
        let (a, b) = (e[0], e[1]);
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let l2 = dot(d, d);
        if l2 < 1e-12 {
            continue;
        }
        let on = |p: V3| {
            let w = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
            let c = [w[1] * d[2] - w[2] * d[1], w[2] * d[0] - w[0] * d[2], w[0] * d[1] - w[1] * d[0]];
            dot(c, c) / l2 < 1e-8
        };
        let t = |p: V3| dot([p[0] - a[0], p[1] - a[1], p[2] - a[2]], d) / l2;
        let mut cov: Vec<(f64, f64)> = same
            .iter()
            .map(|&j| (j, &open[j]))
            .filter(|(j, (f, m, _))| *j != i && dot(*n, *m) > 0.9999 && on(f[0]) && on(f[1]))
            .map(|(_, (f, _, _))| {
                let (t0, t1) = (t(f[0]), t(f[1]));
                (t0.min(t1), t0.max(t1))
            })
            .collect();
        cov.sort_by(|x, y| x.0.total_cmp(&y.0));
        let at_t = |t: f64| [a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t];
        let mut at = 0.0f64;
        for (c0, c1) in cov {
            if c0 > at + 1e-9 {
                sharp.push(([at_t(at), at_t(c0.min(1.0))], *sub));
            }
            at = at.max(c1);
            if at >= 1.0 {
                break;
            }
        }
        if at < 1.0 - 1e-9 {
            sharp.push(([at_t(at), at_t(1.0)], *sub));
        }
    }
    (sharp, smooth)
}

/// Segments on one line joined where they touch or overlap, each run with where it starts
/// along its line from the first point of that line drawn (the phase of a dash pattern laid
/// once along the whole line).
fn collinear_runs(pieces: Vec<(P2, P2)>) -> Vec<(P2, P2, f64)> {
    // each piece on its line: direction (angle in [0, pi)), offset, from-to along it
    let mut lines: std::collections::BTreeMap<(i64, i64), Vec<(f64, f64, [f64; 2], f64)>> = std::collections::BTreeMap::new();
    let mut out = Vec::new();
    for (a, b) in pieces {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = (dx * dx + dy * dy).sqrt();
        if l < 1e-12 {
            continue;
        }
        let (mut ux, mut uy) = (dx / l, dy / l);
        if ux < -1e-12 || (ux.abs() <= 1e-12 && uy < 0.0) {
            ux = -ux;
            uy = -uy;
        }
        let off = -uy * a[0] + ux * a[1];
        let ang = uy.atan2(ux);
        let key = ((ang * 1e5).round() as i64, (off * 1e5).round() as i64);
        let (t0, t1) = (a[0] * ux + a[1] * uy, b[0] * ux + b[1] * uy);
        lines.entry(key).or_default().push((t0.min(t1), t0.max(t1), [ux, uy], off));
    }
    for (_, mut v) in lines {
        v.sort_by(|x, y| x.0.total_cmp(&y.0));
        let origin = v[0].0;
        let (mut s, mut e, u, off) = v[0];
        let at = |t: f64, u: [f64; 2], off: f64| [u[0] * t - u[1] * off, u[1] * t + u[0] * off];
        for &(t0, t1, _, _) in &v[1..] {
            if t0 <= e + 1e-7 {
                e = e.max(t1);
            } else {
                out.push((at(s, u, off), at(e, u, off), s - origin));
                (s, e) = (t0, t1);
            }
        }
        out.push((at(s, u, off), at(e, u, off), s - origin));
    }
    out
}

/// Pieces joined end to end into lines (a curve's chords one after the other), each with the
/// phase of its first piece.
fn chains(runs: Vec<(P2, P2, f64)>) -> Vec<(Vec<P2>, f64)> {
    let key = |p: P2| ((p[0] * 1e6).round() as i64, (p[1] * 1e6).round() as i64);
    // the pieces at each end point
    let mut at: HashMap<(i64, i64), Vec<usize>> = HashMap::default();
    for (k, r) in runs.iter().enumerate() {
        at.entry(key(r.0)).or_default().push(k);
        at.entry(key(r.1)).or_default().push(k);
    }
    let mut used = vec![false; runs.len()];
    let mut out = Vec::new();
    // start where a line ends (an end point of one piece only), then the rest (loops)
    let mut order: Vec<usize> = (0..runs.len()).filter(|&k| at[&key(runs[k].0)].len() != 2 || at[&key(runs[k].1)].len() != 2).collect();
    order.extend(0..runs.len());
    for k in order {
        if used[k] {
            continue;
        }
        used[k] = true;
        let (a, b, phase) = runs[k];
        // from the end shared with no other piece, when there is one
        let (a, b) = if at[&key(a)].len() == 2 && at[&key(b)].len() != 2 { (b, a) } else { (a, b) };
        let mut line = vec![a, b];
        loop {
            let end = *line.last().unwrap();
            let next = at.get(&key(end)).and_then(|v| v.iter().copied().find(|&j| !used[j] && v.len() == 2));
            let Some(j) = next else { break };
            used[j] = true;
            let (p, q, _) = runs[j];
            line.push(if key(p) == key(end) { q } else { p });
        }
        out.push((line, phase));
    }
    out
}

/// Faces meeting at more than about 37° form an edge; less, a curved surface (facets of
/// round ducts, cones and transitions).
const SHARP: f64 = 0.8;

#[cfg(test)]
mod tests {
    #[test]
    fn an_area_is_the_face_of_its_boundary_lines() {
        // two rooms side by side (0..10 and 10..20 by 0..10), the middle line stopping 0.3
        // short of the top, and a stray line inside the left one
        let segs = [
            [[0.0, 0.0], [20.0, 0.0]],
            [[20.0, 0.0], [20.0, 10.0]],
            [[20.0, 10.0], [0.0, 10.0]],
            [[0.0, 10.0], [0.0, 0.0]],
            [[10.0, 0.0], [10.0, 9.7]],
            [[2.0, 2.0], [4.0, 4.0]],
        ];
        let area = |f: &[[[f64; 2]; 2]]| f.iter().map(|e| e[0][0] * e[1][1] - e[1][0] * e[0][1]).sum::<f64>() * 0.5;
        let left = super::face_around(&segs, [5.0, 5.0]).expect("left room");
        assert!((area(&left) - 100.0).abs() < 1e-6, "{}", area(&left));
        let right = super::face_around(&segs, [15.0, 5.0]).expect("right room");
        assert!((area(&right) - 100.0).abs() < 1e-6, "{}", area(&right));
        // outside every loop
        assert!(super::face_around(&segs, [30.0, 5.0]).is_none());
    }

    use super::*;

    #[test]
    fn line_under_a_nearer_line_not_drawn() {
        // two pipes stacked: the lower one is drawn only beyond the upper one's ends
        let segs = [[[0.0, 0.0, 1.0], [10.0, 0.0, 1.0]], [[-2.0, 0.0, 0.0], [12.0, 0.0, 0.0]], [[0.0, 1.0, 0.0], [10.0, 1.0, 0.0]]];
        let left = uncovered(&segs, &[0, 1, 2], &|o| o != 2, 1e-3);
        assert_eq!(left[0], vec![(0.0, 1.0)]);
        assert_eq!(left[1].len(), 2);
        assert!((left[1][0].1 - 2.0 / 14.0).abs() < 1e-9 && (left[1][1].0 - 12.0 / 14.0).abs() < 1e-9);
        assert_eq!(left[2], vec![(0.0, 1.0)]);
        // the same depth: both drawn
        let same = [[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]], [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]]];
        assert_eq!(uncovered(&same, &[0, 1], &|_| true, 1e-3), vec![vec![(0.0, 1.0)], vec![(0.0, 1.0)]]);
    }

    #[test]
    fn dashed_pieces_joined() {
        // two pieces of one line, touching, and a third further on: two runs, the third's
        // pattern going on from the line's start
        let runs = collinear_runs(vec![([0.0, 0.0], [1.0, 0.0]), ([2.0, 0.0], [1.0, 0.0]), ([3.0, 0.0], [4.0, 0.0])]);
        assert_eq!(runs.len(), 2);
        let far = runs.iter().find(|r| r.0[0] > 2.5).unwrap();
        assert!((far.2 - 3.0).abs() < 1e-9);
        // chords of a curve chained into one line
        let pts: Vec<P2> = (0..=4).map(|i| [i as f64, (i * i) as f64]).collect();
        let pieces: Vec<(P2, P2, f64)> = pts.windows(2).map(|w| (w[0], w[1], 0.0)).collect();
        let lines = chains(pieces);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].0.len(), 5);
    }

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
        let (t, b, _) = rtf_parse(r);
        assert_eq!((t, b), ("Base vie \u{2013} Piste\n77\u{a0}777".to_string(), true));
        assert!(!rtf_parse(br"{\rtf1 a \b b\b0  cde\par}").1);
        // the last \par ends the last paragraph; an empty one before it stays a line
        assert_eq!(rtf_text(br"{\rtf1 A \par}"), "A");
        assert_eq!(rtf_text(br"{\rtf1 A. \par\par}"), "A. \n");
    }

    #[test]
    fn rtf_parts_written_their_own_way() {
        // "- " plain, "EAU" underlined, " =" plain, a tab, "PARTOUT" bold italic
        let (t, _, s) = rtf_parse(br"{\rtf1\ansi - {\ul EAU}\ulnone  =\tab\b\i PARTOUT\par}");
        assert_eq!(t, "- EAU =\tPARTOUT");
        let look: Vec<(usize, bool, bool, bool)> = s.iter().map(|s| (s.len, s.bold, s.italic, s.underline)).collect();
        assert_eq!(look, vec![(2, false, false, false), (3, false, false, true), (3, false, false, false), (7, true, true, false)]);
    }

    #[test]
    fn rtf_capitals_and_superscripts() {
        // (as Revit prints them: ASCII letters in capitals, the accented ones kept)
        let (t, _, _) = rtf_parse(br"{\rtf1\ansi\pard\b TUYAUTERIE 05\par \b0\caps Cuivre R\'e9gulateur\caps0  x\par}");
        assert_eq!(t, "TUYAUTERIE 05\nCUIVRE R\u{e9}GULATEUR x");
        // "22 040 m" "3" "/h": the 3 a superscript part of its own
        let (t, _, s) = rtf_parse(br"{\rtf1\ansi\pard\cf1\fs24 22 040 m\super 3\nosupersub /h\par}");
        assert_eq!(t, "22 040 m3/h");
        let parts: Vec<(usize, i8)> = s.iter().map(|s| (s.len, s.script)).collect();
        assert_eq!(parts, vec![(8, 0), (1, 1), (2, 0)]);
    }

    #[test]
    fn section_head_circle_from_two_arcs() {
        // a head family's drawing: its circle as two arcs (centre (-6, 0), radius 6), its
        // arrow's straight sides, nothing of its own
        let arc = |from: f64| (0..=12).map(|i| from + std::f64::consts::PI * i as f64 / 12.0).map(|a| [-6.0 + 6.0 * a.cos(), 6.0 * a.sin()]).collect::<Vec<P2>>();
        let mut d = crate::rvt_annot::Drawn::default();
        d.strokes.push((arc(0.0), 0.1, [0; 3]));
        d.strokes.push((arc(std::f64::consts::PI), 0.1, [0; 3]));
        d.strokes.push((vec![[-14.5, 0.0], [-6.0, 8.5]], 0.1, [0; 3]));
        let sy = Sym::default();
        let c = closed_centre(&sy, Some(&d));
        assert!((c[0] + 6.0).abs() < 1e-9 && c[1].abs() < 1e-9, "{c:?}");
        let b = drawn_box(&sy, Some(&d)).unwrap();
        assert!((b[0] + 14.5).abs() < 1e-9 && (b[2] - 0.0).abs() < 1e-9 && (b[3] - 8.5).abs() < 1e-9, "{b:?}");
        assert!(drawn_box(&sy, None).is_none());
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
        let (sharp, smooth) = feature_edges(&m);
        assert_eq!(sharp.len(), 12);
        assert!(sharp.iter().all(|e| e.1 == -1));
        // the diagonals of its faces join coplanar triangles: neither edges nor silhouettes
        assert!(smooth.is_empty());
    }
}
