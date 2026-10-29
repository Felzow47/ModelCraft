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
use sw_format::{Image2d, Rgba, Sheet2d, Text2d, ViewFrame, Viewport2d};

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
        return String::from_utf8_lossy(b).trim_end_matches('\0').to_string();
    }
    let mut out = String::new();
    let mut stack: Vec<(bool, i64)> = Vec::new();
    let (mut skip, mut uc) = (false, 1i64);
    let mut pending_skip = 0i64;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b'{' => {
                stack.push((skip, uc));
                i += 1;
            }
            b'}' => {
                if let Some((s, u)) = stack.pop() {
                    skip = s;
                    uc = u;
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
    out.trim_end_matches(['\n', ' ', '\0']).to_string()
}

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
    /// Plain text, or for a label the parameters it shows: (param id, prefix, suffix, sample).
    text: String,
    params: Vec<(i64, String, String, String)>,
}

fn raw_text(o: &Obj, tr: &Tree) -> Option<RawText> {
    let orig = v3(o.fs("m_orig"))?;
    let base = v3(o.fs("m_baseVec")).unwrap_or([1.0, 0.0, 0.0]);
    let height = num(o, "m_height").filter(|h| *h > 0.0 && *h < 10.0)?;
    let flags = o.int("m_flags").unwrap_or(0);
    let text = o
        .ptr("m_oRtfData")
        .or_else(|| o.ptr("m_oSavedRtfData"))
        .and_then(|i| tr.objs.get(i))
        .and_then(|r| match r.get("m_arrRtfStream") {
            Some(G::Is(v)) => Some(rtf_text(&v.iter().map(|&x| x as u8).collect::<Vec<_>>())),
            _ => None,
        })
        .unwrap_or_default();
    let params = o
        .list("m_paramDataFormattingArr")
        .iter()
        .map(|p| (p.int("m_paramId").unwrap_or(-1), text_of(p, "m_prefix").unwrap_or_default(), text_of(p, "m_suffix").unwrap_or_default(), text_of(p, "m_sampleText").unwrap_or_default()))
        .collect();
    Some(RawText { orig, base, height, flags, text, params })
}

/// A schedule's stored table: title / header sections (rows heights, column widths, cells).
#[derive(Clone, Debug, Default)]
struct Table {
    width: f64,
    /// Sections shown, top to bottom: (column widths, row heights, cells (row, col, text)).
    sections: Vec<(Vec<f64>, Vec<f64>, Vec<(usize, usize, String)>)>,
}

fn table(data: &Obj, tr: &Tree) -> Table {
    let mut t = Table { width: num(data, "m_WidthInFeet").unwrap_or(0.0), sections: vec![] };
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
            t.sections.push((cols, rows, cells));
        }
    }
    t
}

#[derive(Default, Debug)]
struct Sym {
    curves: Vec<Vec<V3>>,
    texts: Vec<RawText>,
    tags: Vec<RawText>,
    /// Schedules drawn by the symbol (revision schedule of a title block): (position, table).
    tables: Vec<(V3, Table)>,
    outline: Option<[f64; 4]>,
}

#[derive(Debug, Clone)]
struct Crop {
    orig: V3,
    bx: V3,
    by: V3,
    bz: V3,
    off: [f64; 6],
    active: [bool; 6],
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
    strs: HashMap<i64, String>,
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

/// Collects the sheet content, partition by partition (see [`Sheets::take`]).
#[derive(Default)]
pub struct Sheets {
    symbols: HashMap<i64, Sym>,
    /// (owner view, symbol, transform)
    instances: Vec<(i64, i64, Trf)>,
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
    holders: HashMap<i64, Vec<u8>>,
    holder_bytes: usize,
    project: HashMap<i64, String>,
    levels: Vec<f64>,
}

const CLASSES: [&str; 11] = ["FamilySymbol", "FamilyInstance", "Viewport", "TextNote", "ScheduleInstance", "Viewer", "ImageSymbol", "ImageInstance", "ImageHolder", "ProjectInfo", "Level"];
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
        for c in CLASSES {
            if let Some(t) = s.tag_of(c) {
                want[t as usize] = true;
            }
        }
        for &t in s.classes.keys() {
            if s.isa(t, "DBView") {
                want[t as usize] = true;
            }
        }
        want
    }

    pub fn take(&mut self, s: &Schema, t: u16, tr: &Tree) {
        let r = &tr.root;
        let owner = r.int("m_ownerDBViewId").unwrap_or(-1);
        let cls = s.name(t);
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
                        let Some(data) = d.ptr("m_oScheduleData").and_then(|i| tr.objs.get(i)) else { continue };
                        tables.push((pos, table(data, tr)));
                    }
                }
                if curves.is_empty() && texts.is_empty() && tags.is_empty() {
                    return;
                }
                self.symbols.insert(tr.id, Sym { curves, texts, tags, tables, outline });
            }
            "FamilyInstance" if owner > 0 => {
                let Some(ii) = r.ptr("m_pInstanceInfo").and_then(|i| tr.objs.get(i)) else { return };
                let Some(t) = ii.obj("m_Trf").and_then(Trf::from) else { return };
                self.instances.push((owner, ii.int("m_symbolId").unwrap_or(-1), t));
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
            "Viewer" => {
                let Some(b) = r.obj("m_boundedSpace") else { return };
                let (Some(orig), Some(m), Some(off)) = (v3(b.fs("m_orig")), b.fs("m_basis"), b.fs("m_boundOffset")) else { return };
                if m.len() != 9 || off.len() != 6 {
                    return;
                }
                let act: Vec<i64> = match b.get("m_boundActive") {
                    Some(G::Is(v)) => v.clone(),
                    _ => vec![0; 6],
                };
                let Some(view) = r.int("m_dbViewId") else { return };
                self.crops.insert(
                    view,
                    Crop { orig, bx: [m[0], m[1], m[2]], by: [m[3], m[4], m[5]], bz: [m[6], m[7], m[8]], off: off.try_into().unwrap(), active: std::array::from_fn(|k| act.get(k).copied().unwrap_or(0) != 0) },
                );
            }
            "ImageSymbol" => {
                if let (Some(h), Some(w), Some(hh)) = (r.int("m_idImageHolder"), num(r, "m_widthInFeet"), num(r, "m_heightInFeet")) {
                    self.image_syms.insert(tr.id, (h, w, hh));
                }
            }
            "ImageInstance" => {
                if let Some(sym) = r.ptr("m_pImageInfo").and_then(|i| tr.objs.get(i)).and_then(|o| o.int("m_symbolId")) {
                    self.image_inst.insert(tr.id, sym);
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
            "ProjectInfo" => self.project.extend(strings(tr)),
            "Level" => {
                if let Some(z) = r.ptr("m_pSurface").and_then(|i| tr.objs.get(i)).and_then(|p| v3(p.fs("m_origin"))) {
                    self.levels.push(z[2]);
                }
            }
            _ if s.isa(t, "DBView") => {
                let cut = tr.objs.iter().find(|o| s.name(o.tag) == "PlanViewRange").and_then(|o| o.fs("m_offsets")).and_then(|v| v.first().copied()).filter(|v| v.is_finite() && *v > 0.3); // 0: not the cut plane (seen on upper levels)
                self.views.insert(
                    tr.id,
                    ViewInfo {
                        name: text_of(r, "m_viewName").unwrap_or_default(),
                        class: cls.to_string(),
                        horz: v3(r.fs("m_horzDir")),
                        vert: v3(r.fs("m_vertDir")),
                        dir: v3(r.fs("m_viewDir")),
                        image: r.int("m_imageInstanceId").unwrap_or(-1),
                        cut,
                        strs: strings(tr),
                    },
                );
                if let Some(data) = r.ptr("m_pData").and_then(|i| tr.objs.get(i)).filter(|o| s.name(o.tag) == "ScheduleData") {
                    self.tables.insert(tr.id, table(data, tr));
                }
            }
            _ => {}
        }
    }

    fn image(&self, sym: i64) -> Option<(std::sync::Arc<Rgba>, f64, f64)> {
        let &(h, w, hh) = self.image_syms.get(&sym)?;
        let b = self.holders.get(&h)?;
        let img = image::load_from_memory(b).ok()?.to_rgba8();
        Some((std::sync::Arc::new(Rgba { width: img.width(), height: img.height(), pixels: img.into_raw() }), w, hh))
    }

    /// The sheets' drawings. `model`: what the placed views show, element id → (box min,
    /// max, metres; rebuilt mesh if any).
    pub fn finish(&self, sheet_ids: &[(i64, String, String)], model: &[(i64, [f64; 3], [f64; 3], Option<&Mesh>)]) -> Vec<Sheet2d> {
        // model edges only when a sheet shows a model view
        let need = self.viewports.iter().any(|v| v.scale > 1e-6 && v.scale < 0.99 && self.views.get(&v.view).is_some_and(|w| w.class != "DBViewSchedule"));
        let edges = Edges::new(if need { model } else { &[] });
        sheet_ids.iter().map(|(id, number, name)| self.sheet(*id, number, name, &edges)).filter(|s| !s.lines.is_empty() || !s.texts.is_empty() || !s.images.is_empty() || !s.views.is_empty()).collect()
    }

    /// How the views look at the model: direction and up of each view, and for the cropped
    /// plans, sections and elevations the region they show (crop region × depth clip, or the
    /// level up to the cut plane for plans), clamped to the model's box `lo`..`hi` (metres).
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
            let (Some(dir), Some(up)) = (v.dir.and_then(unit), v.vert.and_then(unit)) else { continue };
            let mut region = None;
            let three_d = v.class.contains("3D") || v.class.contains("Perspective");
            if let Some(c) = self.crops.get(&id).filter(|c| !three_d && c.active[0] && c.active[1] && c.active[2] && c.active[3]) {
                let at = |u: f64, w: f64, d: f64| [c.orig[0] + c.bx[0] * u + c.by[0] * w + c.bz[0] * d, c.orig[1] + c.bx[1] * u + c.by[1] * w + c.bz[1] * d, c.orig[2] + c.bx[2] * u + c.by[2] * w + c.bz[2] * d];
                // depth along bz: the model's extent, narrowed by the far / near clips
                let mut dr = (f64::INFINITY, f64::NEG_INFINITY);
                for k in 0..8 {
                    let p = [if k & 1 == 0 { flo[0] } else { fhi[0] }, if k & 2 == 0 { flo[1] } else { fhi[1] }, if k & 4 == 0 { flo[2] } else { fhi[2] }];
                    let d = dot(sub(p, c.orig), c.bz);
                    dr = (dr.0.min(d), dr.1.max(d));
                }
                let plan = v.class.contains("Plan") && c.bz[2] < -0.9;
                if plan {
                    // level below the view origin up to the cut plane
                    let lv = self.levels.iter().copied().filter(|z| *z <= c.orig[2] + 0.01).fold(f64::NEG_INFINITY, f64::max);
                    let lv = if lv.is_finite() && c.orig[2] - lv < 20.0 { lv } else { c.orig[2] };
                    let top = lv + v.cut.unwrap_or(4.0);
                    // bz points down: depth = orig.z - z
                    dr = (dr.0.max(c.orig[2] - top), dr.1.min(c.orig[2] - lv + 0.05));
                } else {
                    if c.active[4] {
                        dr.1 = dr.1.min(c.off[4]);
                    }
                    if c.active[5] {
                        dr.0 = dr.0.max(c.off[5]);
                    }
                }
                if dr.0 < dr.1 {
                    let (mut a, mut b) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
                    for k in 0..8 {
                        let p = at(if k & 1 == 0 { c.off[1] } else { c.off[0] }, if k & 2 == 0 { c.off[3] } else { c.off[2] }, if k & 4 == 0 { dr.0 } else { dr.1 });
                        for i in 0..3 {
                            a[i] = a[i].min(p[i]);
                            b[i] = b[i].max(p[i]);
                        }
                    }
                    // clamped to the model, kept only when something is left
                    let a: V3 = std::array::from_fn(|i| a[i].max(flo[i]) * FEET);
                    let b: V3 = std::array::from_fn(|i| b[i].min(fhi[i]) * FEET);
                    if (0..3).all(|i| b[i] > a[i] - 1e-6) && (0..3).any(|i| b[i] - a[i] > 0.01) {
                        region = Some((f32s(a), f32s(b)));
                    }
                }
            }
            out.push(ViewFrame { id, dir: f32s(dir), up: f32s(up), region });
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

    fn sheet(&self, id: i64, number: &str, name: &str, edges: &Edges) -> Sheet2d {
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
        let push_text = |out: &mut Sheet2d, t: &RawText, tf: Option<&Trf>, text: String| {
            if text.trim().is_empty() {
                return;
            }
            let (o, b) = match tf {
                Some(tf) => (tf.apply(t.orig), tf.lin(t.base)),
                None => (t.orig, t.base),
            };
            let l = (b[0] * b[0] + b[1] * b[1]).sqrt().max(1e-9);
            let k = tf.map(|tf| (tf.lin([1.0, 0.0, 0.0])[0].powi(2) + tf.lin([1.0, 0.0, 0.0])[1].powi(2)).sqrt()).unwrap_or(1.0);
            let h = (t.flags & 3) as u8;
            let v = match (t.flags >> 6) & 7 {
                2 => 1,
                4 => 2,
                _ => 0,
            };
            out.texts.push(Text2d { pos: pt([o[0], o[1]]), dir: [(b[0] / l) as f32, (b[1] / l) as f32], height: m(t.height * k / LINE_BOX / text.lines().count().max(1) as f64), halign: h.min(2), valign: v, text });
        };
        // families on the sheet (title block first: the largest)
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
            for c in &sy.curves {
                out.lines.push(c.iter().map(|p| tf.apply(*p)).map(|q| pt([q[0], q[1]])).collect());
            }
            for t in &sy.texts {
                push_text(&mut out, t, Some(tf), t.text.clone());
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
                push_text(&mut out, t, Some(tf), s);
            }
            for (pos, tb) in &sy.tables {
                // in a title block the schedule grows upwards from its position
                self.draw_table(&mut out, tb, tf.apply(*pos), true);
            }
        }
        for (_, t) in self.texts.iter().filter(|t| t.0 == id) {
            push_text(&mut out, t, None, t.text.clone());
        }
        for (_, o, sch) in self.sched_inst.iter().filter(|s| s.0 == id) {
            if let Some(tb) = self.tables.get(sch) {
                self.draw_table(&mut out, tb, *o, false);
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
                out.images.push(Image2d { rect: [m(r[0]), m(r[1]), m(r[2]), m(r[3])], image: img });
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
                out.texts.push(Text2d { pos: pt([l0[0], l0[1] + h * 0.4]), dir: [1.0, 0.0], height: m(h), halign: 0, valign: 2, text: head });
                if vp.scale > 1e-6 && vp.scale < 0.99 {
                    out.texts.push(Text2d { pos: pt([l0[0], l0[1] - h * 0.4]), dir: [1.0, 0.0], height: m(h * 0.7), halign: 0, valign: 0, text: format!("1 : {}", (1.0 / vp.scale).round()) });
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

    /// A schedule's stored rows (title, headers): grid and cell texts. `up`: rows stacked
    /// upwards from `o` (title block revision schedules), else downwards (top left corner).
    fn draw_table(&self, out: &mut Sheet2d, tb: &Table, o: V3, up: bool) {
        let m = |x: f64| (x * FEET) as f32;
        let mut y = o[1];
        let width = if tb.width > 1e-6 { tb.width } else { tb.sections.iter().map(|s| s.0.iter().sum::<f64>()).fold(0.0, f64::max) };
        if width <= 1e-6 {
            return;
        }
        let sections: Vec<_> = if up { tb.sections.iter().rev().collect() } else { tb.sections.iter().collect() };
        for (cols, rows, cells) in sections {
            let rows_order: Vec<usize> = if up { (0..rows.len()).rev().collect() } else { (0..rows.len()).collect() };
            for r in rows_order {
                let h = rows[r].max(0.005);
                let (y0, y1) = if up { (y, y + h) } else { (y - h, y) };
                out.lines.push(vec![[m(o[0]), m(y0)], [m(o[0] + width), m(y0)], [m(o[0] + width), m(y1)], [m(o[0]), m(y1)], [m(o[0]), m(y0)]]);
                let row_cells: Vec<&(usize, usize, String)> = cells.iter().filter(|c| c.0 == r).collect();
                let multi = cols.len() > 1 && row_cells.len() > 1;
                let mut x = o[0];
                for (k, w) in cols.iter().enumerate() {
                    if multi && k > 0 {
                        out.lines.push(vec![[m(x), m(y0)], [m(x), m(y1)]]);
                    }
                    if let Some(c) = row_cells.iter().find(|c| c.1 == k) {
                        let (cx, cw) = if multi { (x, *w) } else { (o[0], width) };
                        out.texts.push(Text2d { pos: [m(cx + cw * 0.5), m((y0 + y1) * 0.5)], dir: [1.0, 0.0], height: m((h * 0.45).min(0.0156)), halign: 1, valign: 1, text: c.2.clone() });
                    }
                    x += w;
                }
                y = if up { y1 } else { y0 };
            }
        }
    }

    /// A model view on the sheet: the edges of the elements it sees, clipped by its crop
    /// region, at the viewport's scale. Returns the frame (feet, paper).
    fn draw_view(&self, out: &mut Sheet2d, view: &ViewInfo, vp: &Vp, edges: &Edges) -> Option<[f64; 4]> {
        let crop = self.crops.get(&vp.view);
        // view axes: the crop basis, else the view directions
        let (o, bx, by, bz) = match crop {
            Some(c) => (c.orig, c.bx, c.by, c.bz),
            None => (
                [0.0; 3],
                view.horz?,
                view.vert?,
                view.dir.map(|d| [-d[0], -d[1], -d[2]])?,
            ),
        };
        let plan = view.class.contains("Plan") && bz[2] < -0.9;
        // depth range (feet along bz from o) and crop (u, v)
        let mut depth = (f64::NEG_INFINITY, f64::INFINITY);
        let mut z_range = (f64::NEG_INFINITY, f64::INFINITY);
        if plan {
            // level below the view origin, up to the cut plane
            let lv = self.levels.iter().copied().filter(|z| *z <= o[2] + 0.01).fold(f64::NEG_INFINITY, f64::max);
            let lv = if lv.is_finite() && o[2] - lv < 20.0 { lv } else { o[2] };
            z_range = (lv - 0.05, lv + view.cut.unwrap_or(4.0));
        } else if let Some(c) = crop {
            if c.active[4] {
                depth.1 = c.off[4];
            }
            if c.active[5] {
                depth.0 = c.off[5].min(depth.1);
            }
        }
        let mut uv_box = crop.filter(|c| c.active[0] && c.active[1] && c.active[2] && c.active[3]).map(|c| [c.off[1], c.off[3], c.off[0], c.off[2]]);
        let mut segs: Vec<[P2; 2]> = Vec::new();
        let proj = |p: V3| -> (P2, f64) {
            let d = sub([p[0] / FEET, p[1] / FEET, p[2] / FEET], o);
            ([dot(d, bx), dot(d, by)], dot(d, bz))
        };
        for (lo, hi, list) in edges.items.iter() {
            if plan && (lo[2] / FEET > z_range.1 || hi[2] / FEET < z_range.0) {
                continue;
            }
            if !plan && (depth.0.is_finite() || depth.1.is_finite()) {
                // box depth range along bz
                let corners = (0..8).map(|k| proj([if k & 1 == 0 { lo[0] } else { hi[0] }, if k & 2 == 0 { lo[1] } else { hi[1] }, if k & 4 == 0 { lo[2] } else { hi[2] }]).1);
                let (dmin, dmax) = corners.fold((f64::INFINITY, f64::NEG_INFINITY), |a, d| (a.0.min(d), a.1.max(d)));
                if dmin > depth.1 || dmax < depth.0 {
                    continue;
                }
            }
            for e in list {
                let (a, b) = (proj(e[0]).0, proj(e[1]).0);
                if (a[0] - b[0]).abs() + (a[1] - b[1]).abs() > 1e-4 {
                    segs.push([a, b]);
                }
            }
            if segs.len() > MAX_SEGMENTS {
                break;
            }
        }
        if uv_box.is_none() {
            // no crop: the extents of what is seen
            let mut e = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
            for s in &segs {
                for p in s {
                    e = [e[0].min(p[0]), e[1].min(p[1]), e[2].max(p[0]), e[3].max(p[1])];
                }
            }
            if !e[0].is_finite() {
                return None;
            }
            uv_box = Some(e);
        }
        let ub = uv_box?;
        // a cropped view puts its origin (the crop's) at the viewport offset; without a crop
        // the centre of what is seen is used
        let (cu, cv) = if crop.is_some() { (0.0, 0.0) } else { ((ub[0] + ub[2]) * 0.5, (ub[1] + ub[3]) * 0.5) };
        let k = vp.scale;
        if std::env::var_os("SHEET_DEBUG").is_some() {
            eprintln!("  view {} {:?} crop {:?} ub {:?} offset {:?} label {:?} len {} k {}", vp.view, view.name, crop.map(|c| (c.orig, c.off, c.active)), ub, vp.offset, vp.label, vp.label_len, k);
        }
        let to_paper = |p: P2| [vp.offset[0] + (p[0] - cu) * k, vp.offset[1] + (p[1] - cv) * k];
        let m = |x: f64| (x * FEET) as f32;
        for s in segs {
            if let Some((a, b)) = clip(s[0], s[1], ub) {
                let (a, b) = (to_paper(a), to_paper(b));
                out.fine.push([[m(a[0]), m(a[1])], [m(b[0]), m(b[1])]]);
            }
        }
        let a = to_paper([ub[0], ub[1]]);
        let b = to_paper([ub[2], ub[3]]);
        Some([a[0], a[1], b[0], b[1]])
    }
}

/// Most model edges drawn in one view.
const MAX_SEGMENTS: usize = 60_000;

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

/// The model's drawable edges (metres): feature edges of the rebuilt meshes, box edges of
/// the others, with each element's box.
struct Edges {
    items: Vec<([f64; 3], [f64; 3], Vec<[V3; 2]>)>,
}

impl Edges {
    fn new(model: &[(i64, [f64; 3], [f64; 3], Option<&Mesh>)]) -> Edges {
        let items = model
            .iter()
            .map(|(_, lo, hi, mesh)| {
                let list = match mesh {
                    Some(m) if !m.tri.is_empty() => feature_edges(m),
                    _ => {
                        let c = |k: usize| [if k & 1 == 0 { lo[0] } else { hi[0] }, if k & 2 == 0 { lo[1] } else { hi[1] }, if k & 4 == 0 { lo[2] } else { hi[2] }];
                        [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)].iter().map(|&(a, b)| [c(a), c(b)]).collect()
                    }
                };
                (*lo, *hi, list)
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
    fn rtf_plain() {
        let r = br"{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\fnil Arial;}}{\colortbl ;\red0\green0\blue0;}\viewkind4\uc1\pard\cf1\f0\fs20 Caf\'e9 \u8364?\par Ligne 2\par}";
        assert_eq!(rtf_text(r), "Café €\nLigne 2");
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
        let m = Mesh { pos, tri };
        assert_eq!(feature_edges(&m).len(), 12);
    }
}
