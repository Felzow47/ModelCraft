//! Revit "Navigateur de projet" (Project Browser), rebuilt read-only from the model database.
//!
//! Clean-room, from byte observations only (see docs/rvt-rea-notes.md). `/Formats/Latest`
//! describes every serialized class (name, parent, typed fields); the partitions hold the
//! elements written field by field in that order. We decode the few classes the browser needs:
//! the views (`DBView` and subclasses), their types (`DBViewType`: system family and type
//! name), the phases, the browser organizations (folders, filters) and, in `/Global/Latest`, the
//! `BrowserOrganizationTracking` that says which organization is current for views, sheets and
//! schedules. The tree then follows Revit's rules: one folder level per organization folder
//! parameter, views sorted by name, dependent views under their primary view, sheets shown as
//! "NUM - Nom".

use crate::rvt_model;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use sw_format::{BrowserKind as K, BrowserNode};

// ---------------------------------------------------------------- schema

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ty {
    b0: u8,
    b1: u8,
    n: u32,
    tag: u16,
}

#[derive(Debug)]
struct Class {
    name: String,
    parent: u16,
    fields: Vec<(String, Ty)>,
}

/// The class table of `/Formats/Latest`.
pub struct Schema {
    classes: HashMap<u16, Class>,
    by_name: HashMap<String, u16>,
    /// Flattened field list of each class, base class first.
    chains: HashMap<u16, Vec<(String, Ty)>>,
    id_tag: u16,
}

fn u16_at(d: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(p..p + 2)?.try_into().ok()?))
}
fn u32_at(d: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(p..p + 4)?.try_into().ok()?))
}
fn ident(b: &[u8]) -> bool {
    b.first().is_some_and(|c| c.is_ascii_alphabetic()) && b.iter().all(|&c| c.is_ascii_alphanumeric() || b"_:<>, ".contains(&c))
}

impl Schema {
    /// Class numbering: classes are numbered in order of appearance, except that an explicit
    /// `[tag|0x8000] 00 00` just before a name resets the counter (inline definitions).
    fn tags(d: &[u8]) -> Vec<(u16, usize)> {
        let mut out = Vec::new();
        let mut cnt: u32 = 0;
        let mut i = 0;
        while i + 5 < d.len() {
            let l = d[i] as usize;
            if (3..=0x7f).contains(&l) && d[i + 1] == 0 && i + 2 + l <= d.len() && ident(&d[i + 2..i + 2 + l]) && !d[i + 2..].starts_with(b"m_") {
                let mut t = cnt;
                if i >= 4 && d[i - 2] == 0 && d[i - 1] == 0 {
                    let pre = u16::from_le_bytes([d[i - 4], d[i - 3]]) as u32;
                    if pre & 0x8000 != 0 && (cnt..=cnt + 64).contains(&(pre & 0x7fff)) {
                        t = pre & 0x7fff;
                    }
                }
                cnt = t + 1;
                out.push((t as u16, i));
            }
            i += 1;
        }
        out
    }

    pub fn parse(d: &[u8]) -> Schema {
        let mut s = Schema { classes: HashMap::new(), by_name: HashMap::new(), chains: HashMap::new(), id_tag: 20 };
        for (t, i) in Self::tags(d) {
            if s.classes.contains_key(&t) {
                continue;
            }
            if let Some((c, _)) = parse_def(d, i, 0) {
                s.by_name.entry(c.name.clone()).or_insert(t);
                s.classes.insert(t, c);
            }
        }
        if let Some(&t) = s.by_name.get("ElementId") {
            s.id_tag = t;
        }
        let tags: Vec<u16> = s.classes.keys().copied().collect();
        for t in tags {
            let mut chain = Vec::new();
            let mut c = t;
            let mut seen = HashSet::new();
            while let Some(cl) = s.classes.get(&c) {
                if !seen.insert(c) {
                    break;
                }
                chain.push(c);
                if cl.parent == 0 || cl.parent == c {
                    break;
                }
                c = cl.parent;
            }
            let fields = chain.iter().rev().flat_map(|c| s.classes[c].fields.iter().cloned()).collect();
            s.chains.insert(t, fields);
        }
        s
    }

    fn tag(&self, name: &str) -> Option<u16> {
        self.by_name.get(name).copied()
    }

    /// Does class `tag` derive from (or equal) `base`?
    pub fn isa(&self, tag: u16, base: &str) -> bool {
        let mut c = tag;
        for _ in 0..64 {
            let Some(cl) = self.classes.get(&c) else { return false };
            if cl.name == base {
                return true;
            }
            if cl.parent == 0 || cl.parent == c {
                return false;
            }
            c = cl.parent;
        }
        false
    }

    fn name(&self, tag: u16) -> &str {
        self.classes.get(&tag).map(|c| c.name.as_str()).unwrap_or("")
    }
}

/// `[u16 len][name][u16 parent](00 00 inline parent)[u32][u32 n][fields][u32 ng][ng GUIDs]`
fn parse_def(d: &[u8], i: usize, depth: u32) -> Option<(Class, usize)> {
    if depth > 16 {
        return None;
    }
    let l = u16_at(d, i)? as usize;
    let name = std::str::from_utf8(d.get(i + 2..i + 2 + l)?).ok()?.to_string();
    let mut p = i + 2 + l;
    let mut parent = u16_at(d, p)?;
    p += 2;
    if parent & 0x8000 != 0 {
        p = parse_def(d, p + 2, depth + 1)?.1;
        parent &= 0x7fff;
    }
    p += 4;
    let nf = u32_at(d, p)?;
    p += 4;
    if nf > 500 {
        return None;
    }
    let mut fields = Vec::with_capacity(nf as usize);
    for _ in 0..nf {
        let fl = u32_at(d, p)? as usize;
        if fl > 256 {
            return None;
        }
        let fname = std::str::from_utf8(d.get(p + 4..p + 4 + fl)?).ok()?.to_string();
        p += 4 + fl;
        let (ty, q) = parse_type(d, p, depth)?;
        p = q;
        fields.push((fname, ty));
    }
    let ng = u32_at(d, p)?;
    if ng > 64 {
        return None;
    }
    p += 4 + 16 * ng as usize;
    (p <= d.len()).then_some((Class { name, parent, fields }, p))
}

fn parse_type(d: &[u8], mut p: usize, depth: u32) -> Option<(Ty, usize)> {
    let (b0, b1) = (*d.get(p)?, *d.get(p + 1)?);
    p += 4;
    let mut ty = Ty { b0, b1, n: 0, tag: 0 };
    if b1 == 0x10 {
        ty.n = u32_at(d, p)?;
        p += 4;
    }
    if b0 == 0x0e && matches!(b1, 0x00 | 0x50 | 0x10) {
        let mut t = u16_at(d, p)?;
        p += 2;
        if t & 0x8000 != 0 {
            p = parse_def(d, p + 2, depth + 1)?.1;
            t &= 0x7fff;
        }
        ty.tag = t;
    }
    Some((ty, p))
}

// ---------------------------------------------------------------- decoder

/// A decoded value. Field names borrow the schema.
#[derive(Debug, Clone)]
pub enum V<'s> {
    I(i64),
    F(f64),
    S(String),
    Ids(Vec<i64>),
    O(Map<'s>),
    L(Vec<Map<'s>>),
}
pub type Map<'s> = HashMap<&'s str, V<'s>>;

fn int(m: &Map, k: &str) -> Option<i64> {
    match m.get(k)? {
        V::I(v) => Some(*v),
        _ => None,
    }
}
fn text(m: &Map, k: &str) -> Option<String> {
    match m.get(k)? {
        V::S(v) => Some(v.clone()),
        V::O(o) => o.values().find_map(|v| if let V::S(s) = v { Some(s.clone()) } else { None }),
        _ => None,
    }
}
fn list<'a, 's>(m: &'a Map<'s>, k: &str) -> &'a [Map<'s>] {
    match m.get(k) {
        Some(V::L(l)) => l,
        _ => &[],
    }
}
fn ids(m: &Map, k: &str) -> Vec<i64> {
    match m.get(k) {
        Some(V::Ids(v)) => v.clone(),
        Some(V::I(v)) => vec![*v],
        _ => vec![],
    }
}

struct Dec<'a, 's> {
    s: &'s Schema,
    d: &'a [u8],
    w: usize,
    pending: Vec<u16>,
}

/// Most deferred objects decoded after an element's body (the parameter sets come first).
const MAX_DEFERRED: usize = 24;

impl<'a, 's> Dec<'a, 's> {
    fn id(&self, p: usize) -> Option<i64> {
        let b = self.d.get(p..p + self.w)?;
        Some(if self.w == 8 { i64::from_le_bytes(b.try_into().ok()?) } else { i32::from_le_bytes(b.try_into().ok()?) as i64 })
    }

    fn fields(&mut self, tag: u16, mut p: usize, out: &mut Map<'s>, depth: u32) -> Option<usize> {
        if depth > 24 {
            return None;
        }
        let s = self.s;
        for (name, ty) in s.chains.get(&tag)? {
            p = self.val(*ty, p, out, name, depth)?;
        }
        Some(p)
    }

    fn val(&mut self, ty: Ty, p: usize, out: &mut Map<'s>, name: &'s str, depth: u32) -> Option<usize> {
        let d = self.d;
        if ty.b0 == 0x0e {
            return match ty.b1 {
                // owned pointer: ff ff ff ff + class tag (content deferred), null, or back-reference
                1 | 2 => {
                    let v = u32_at(d, p)?;
                    if v == 0xffff_ffff {
                        self.pending.push(u16_at(d, p + 4)?);
                        Some(p + 6)
                    } else if v < 0x10000 {
                        Some(p + 4)
                    } else {
                        None
                    }
                }
                3 => Some(p + 4),
                0x51 => {
                    let n = u32_at(d, p)?;
                    if n > 100_000 {
                        return None;
                    }
                    let mut p = p + 4;
                    for _ in 0..n {
                        p = self.val(Ty { b0: 0x0e, b1: 1, n: 0, tag: 0 }, p, out, name, depth)?;
                    }
                    Some(p)
                }
                0x50 => {
                    let n = u32_at(d, p)? as usize;
                    let mut p = p + 4;
                    if n == 0 {
                        return Some(p);
                    }
                    if ty.tag == self.s.id_tag {
                        if n > 10_000_000 / self.w {
                            return None;
                        }
                        let v: Option<Vec<i64>> = (0..n).map(|i| self.id(p + i * self.w)).collect();
                        out.insert(name, V::Ids(v?));
                        return Some(p + n * self.w);
                    }
                    if n > 1_000_000 {
                        return None;
                    }
                    let mut l = Vec::new();
                    for _ in 0..n {
                        let mut e = Map::new();
                        p = self.fields(ty.tag, p, &mut e, depth + 1)?;
                        if l.len() < 4000 {
                            l.push(e);
                        }
                    }
                    out.insert(name, V::L(l));
                    Some(p)
                }
                0 => {
                    if ty.tag == self.s.id_tag {
                        out.insert(name, V::I(self.id(p)?));
                        return Some(p + self.w);
                    }
                    let mut o = Map::new();
                    let p = self.fields(ty.tag, p, &mut o, depth + 1)?;
                    out.insert(name, V::O(o));
                    Some(p)
                }
                _ => None,
            };
        }
        if ty.b0 == 8 && ty.b1 == 0x60 {
            let n = u32_at(d, p)? as usize;
            if n > 4096 {
                return None;
            }
            let b = d.get(p + 4..p + 4 + 2 * n)?;
            let u: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            out.insert(name, V::S(String::from_utf16_lossy(&u)));
            return Some(p + 4 + 2 * n);
        }
        let sz = match ty.b0 {
            7 | 6 | 0x0b => 8,
            4 | 5 => 4,
            3 => 2,
            1 | 2 => 1,
            _ => return None,
        };
        match ty.b1 {
            0 => {
                let b = d.get(p..p + sz)?;
                let v = match ty.b0 {
                    7 => Some(V::F(f64::from_le_bytes(b.try_into().ok()?))),
                    6 => Some(V::I(i64::from_le_bytes(b.try_into().ok()?))),
                    4 => Some(V::I(i32::from_le_bytes(b.try_into().ok()?) as i64)),
                    1 => Some(V::I(b[0] as i64)),
                    _ => None,
                };
                if let Some(v) = v {
                    out.insert(name, v);
                }
                Some(p + sz)
            }
            0x10 => {
                let e = p.checked_add(sz.checked_mul(ty.n as usize)?)?;
                (e <= d.len()).then_some(e)
            }
            0x50 => {
                let n = u32_at(d, p)? as usize;
                let e = p + 4 + n.checked_mul(sz)?;
                (e <= d.len()).then_some(e)
            }
            _ => None,
        }
    }

    /// Decodes an element body at `p` (class `tag`), then its deferred pointer contents in FIFO
    /// order until the first one that does not decode.
    fn obj(&mut self, tag: u16, p: usize) -> Option<Elem<'s>> {
        self.pending.clear();
        let mut f = Map::new();
        let mut p = self.fields(tag, p, &mut f, 0)?;
        let mut def = Vec::new();
        let mut i = 0;
        while i < self.pending.len() && i < MAX_DEFERRED {
            let t = self.pending[i];
            i += 1;
            let mut sub = Map::new();
            match self.fields(t, p, &mut sub, 0) {
                Some(q) => {
                    p = q;
                    def.push((self.s.name(t), sub));
                }
                None => break,
            }
        }
        Some(Elem { tag, id: int(&f, "m_id").unwrap_or(i64::MIN), f, def })
    }
}

/// A decoded element: its own fields and the deferred objects (class name, fields).
#[derive(Debug)]
pub struct Elem<'s> {
    pub tag: u16,
    pub id: i64,
    pub f: Map<'s>,
    pub def: Vec<(&'s str, Map<'s>)>,
}

impl<'s> Elem<'s> {
    fn deferred(&self, class: &str) -> Option<&Map<'s>> {
        self.def.iter().find(|(n, _)| *n == class).map(|(_, m)| m)
    }
    /// `(paramId, value)` pairs of a `ParamValueSet*` deferred object.
    fn params(&self, class: &str) -> &[Map<'s>] {
        self.deferred(class).map(|m| list(m, "m_paramSet")).unwrap_or(&[])
    }
}

/// Every element of a class in `want` in a partition: header `[id][u32][u32 size][u16 tag]`,
/// body right after the tag; kept when its decoded `m_id` equals the header id.
pub fn scan<'s>(s: &'s Schema, d: &[u8], w: usize, want: &[bool]) -> Vec<Elem<'s>> {
    let mut dec = Dec { s, d, w, pending: Vec::new() };
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    if d.len() < 16 + w {
        return out;
    }
    for at in (8 + w)..d.len() - 6 {
        let t = u16::from_le_bytes([d[at], d[at + 1]]);
        if !want[t as usize] {
            continue;
        }
        let v = u32::from_le_bytes([d[at + 2], d[at + 3], d[at + 4], d[at + 5]]);
        if v != 0 && v != 0xffff_ffff {
            continue;
        }
        let Some(eid) = dec.id(at - 8 - w) else { continue };
        let size = u32::from_le_bytes([d[at - 4], d[at - 3], d[at - 2], d[at - 1]]);
        if !(17..0x40_0000).contains(&size) || eid <= 0 {
            continue;
        }
        if let Some(e) = dec.obj(t, at + 2) {
            if e.id == eid && seen.insert(eid) {
                out.push(e);
            }
        }
    }
    out
}

// ---------------------------------------------------------------- model

#[derive(Debug, Clone, Default)]
pub struct View {
    pub id: i64,
    pub class: String,
    pub name: String,
    pub type_id: i64,
    pub template: bool,
    pub family_owned: bool,
    pub owner_view: i64,
    pub primary: i64,
    pub on_sheet: bool,
    pub section_type: i64,
    pub sheet_number: String,
    pub ints: HashMap<i64, i64>,
    pub elem_ids: HashMap<i64, i64>,
    pub strs: HashMap<i64, String>,
    pub dbls: HashMap<i64, f64>,
}

#[derive(Debug, Clone, Default)]
pub struct ViewType {
    pub family: i64,
    pub name: String,
}

#[derive(Debug, Clone, Default)]
pub struct Org {
    pub id: i64,
    pub name: String,
    pub kind: i64,
    pub default: bool,
    /// (paramId, relation, string value, int value, element id value)
    pub filters: Vec<(i64, i64, String, i64, i64)>,
    /// (paramId, number of leading characters, 0 = all)
    pub folders: Vec<(i64, usize)>,
    pub sort_asc: bool,
}

/// What the browser is built from.
#[derive(Debug, Default)]
pub struct Model {
    pub views: Vec<View>,
    pub types: HashMap<i64, ViewType>,
    pub phases: HashMap<i64, String>,
    pub orgs: HashMap<i64, Org>,
    /// Current organization for views, sheets, schedules.
    pub current: [Option<i64>; 3],
}

impl Model {
    fn absorb(&mut self, s: &Schema, elems: &[Elem]) {
        for e in elems {
            if int(&e.f, "m_famId").unwrap_or(-1) != -1 {
                continue;
            }
            let cls = s.name(e.tag);
            if s.isa(e.tag, "DBViewType") {
                let name = e.deferred("SymbolInfo").and_then(|m| text(m, "m_name")).unwrap_or_default();
                self.types.insert(e.id, ViewType { family: int(&e.f, "m_systemFamilyIdx").unwrap_or(-1), name });
            } else if s.isa(e.tag, "BrowserOrganization") {
                let name = e.deferred("SymbolInfo").and_then(|m| text(m, "m_name")).unwrap_or_default();
                let filters = list(&e.f, "m_filters")
                    .iter()
                    .map(|f| {
                        let v = match f.get("m_value") {
                            Some(V::O(o)) => o.clone(),
                            _ => Map::new(),
                        };
                        (int(f, "m_paramId").unwrap_or(-1), int(f, "m_relation").unwrap_or(0), text(&v, "m_str").unwrap_or_default(), int(&v, "m_int").unwrap_or(0), int(&v, "m_elemId").unwrap_or(-1))
                    })
                    .collect();
                let folders = list(&e.f, "m_folderDefinitions").iter().map(|f| (int(f, "m_paramId").unwrap_or(-1), int(f, "m_numCharsToUse").unwrap_or(0).max(0) as usize)).collect();
                self.orgs.insert(e.id, Org { id: e.id, name, kind: int(&e.f, "m_type").unwrap_or(-1), default: int(&e.f, "m_bDefault") == Some(1), filters, folders, sort_asc: int(&e.f, "m_bSortOrderAsc") != Some(0) });
            } else if cls == "ProjectPhase" {
                self.phases.insert(e.id, text(&e.f, "m_name").unwrap_or_default());
            } else if s.isa(e.tag, "DBView") {
                let mut v = View {
                    id: e.id,
                    class: cls.to_string(),
                    name: text(&e.f, "m_viewName").unwrap_or_default(),
                    type_id: int(&e.f, "m_dbViewTypeId").unwrap_or(-1),
                    template: int(&e.f, "m_isTemplate") == Some(1),
                    family_owned: false,
                    owner_view: int(&e.f, "m_ownerDBViewId").unwrap_or(-1),
                    primary: int(&e.f, "m_primaryViewId").unwrap_or(-1),
                    on_sheet: ids(&e.f, "m_sheetViewportElemIds").iter().chain(ids(&e.f, "m_sheetViewportElemId").iter()).any(|&i| i != -1),
                    section_type: int(&e.f, "m_sectionViewType").unwrap_or(-1),
                    sheet_number: text(&e.f, "m_sheetNumber").unwrap_or_default(),
                    ..Default::default()
                };
                for p in e.params("ParamValueSetInt") {
                    v.ints.insert(int(p, "m_paramId").unwrap_or(0), int(p, "m_value").unwrap_or(0));
                }
                for p in e.params("ParamValueSetElementId") {
                    v.elem_ids.insert(int(p, "m_paramId").unwrap_or(0), int(p, "m_value").unwrap_or(-1));
                }
                for p in e.params("ParamValueSetAString") {
                    v.strs.insert(int(p, "m_paramId").unwrap_or(0), text(p, "m_value").unwrap_or_default());
                }
                for p in e.params("ParamValueSetDouble") {
                    if let Some(V::F(x)) = p.get("m_value") {
                        v.dbls.insert(int(p, "m_paramId").unwrap_or(0), *x);
                    }
                }
                if !self.views.iter().any(|o| o.id == v.id) {
                    self.views.push(v);
                }
            }
        }
    }

    /// `BrowserOrganizationTracking` in `/Global/Latest`: the object whose id set lists the
    /// organizations found in the partitions.
    fn tracking(&mut self, s: &Schema, g: &[u8], w: usize) {
        let Some(tag) = s.tag("BrowserOrganizationTracking") else { return };
        let mut dec = Dec { s, d: g, w, pending: Vec::new() };
        for p in 0..g.len().saturating_sub(8) {
            if u32_at(g, p) != Some(1) {
                continue;
            }
            let n = u32_at(g, p + 4).unwrap_or(0) as usize;
            if n == 0 || n > 100_000 {
                continue;
            }
            let mut f = Map::new();
            if dec.fields(tag, p, &mut f, 0).is_none() {
                continue;
            }
            let set = ids(&f, "m_elemIdSet");
            let cur = [int(&f, "m_currentBrOrgViews"), int(&f, "m_currentBrOrgSheets"), int(&f, "m_currentBrOrgSchedules")];
            if set.is_empty() || cur.iter().any(|c| c.is_none()) {
                continue;
            }
            let known = cur.iter().flatten().filter(|c| self.orgs.contains_key(c)).count();
            if known == 0 && !self.orgs.is_empty() {
                continue;
            }
            self.current = cur.map(|c| c.filter(|c| self.orgs.contains_key(c)));
            return;
        }
    }
}

/// Reads the browser model of a Revit file (`None` when the schema is unreadable).
pub fn read_model<F: Read + std::io::Seek>(cf: &mut cfb::CompoundFile<F>, names: &[(String, u64)], max_partition: u64) -> Option<Model> {
    let fmt = rvt_model::read_stream(cf, "/Formats/Latest", 64 << 20, 64 << 20, |_| true)?;
    if fmt.verified == 0 {
        return None;
    }
    let schema = Schema::parse(&fmt.data);
    drop(fmt);
    let mut want = vec![false; 65536];
    for &t in schema.classes.keys() {
        if ["DBView", "DBViewType", "BrowserOrganization", "ProjectPhase"].iter().any(|b| schema.isa(t, b)) {
            want[t as usize] = true;
        }
    }
    let mut m = Model::default();
    let mut width = 0;
    for (n, _) in names.iter().filter(|(n, _)| n.starts_with("Partitions/")) {
        let Some(inf) = rvt_model::read_stream(cf, &format!("/{n}"), max_partition, rvt_model::MAX_INFLATED, |_| true) else { continue };
        let w = rvt_model::walk_chain(&inf.data).id_width;
        let w = if w == 0 { 4 } else { w };
        width = width.max(w);
        let elems = scan(&schema, &inf.data, w, &want);
        m.absorb(&schema, &elems);
    }
    if width == 0 {
        return None;
    }
    if let Some(g) = rvt_model::read_stream(cf, "/Global/Latest", 256 << 20, 256 << 20, |_| true) {
        m.tracking(&schema, &g.data, width);
    }
    Some(m)
}

// ---------------------------------------------------------------- tree

/// Revit system families of view types (`m_systemFamilyIdx`): (kind, FR plural, FR singular,
/// EN singular).
fn family(idx: i64) -> Option<(K, &'static str, &'static str, &'static str)> {
    Some(match idx {
        0x66 => (K::View3d, "Vues 3D", "Vue 3D", "3D View"),
        0x67 => (K::View3d, "Visites virtuelles", "Visite virtuelle", "Walkthrough"),
        0x68 => (K::Rendering, "Rendus", "Rendu", "Rendering"),
        0x69 => (K::Schedule, "Nomenclatures/Quantités", "Nomenclature", "Schedule"),
        0x6a => (K::Other, "Rapports de coûts", "Rapport de coûts", "Cost Report"),
        0x6b => (K::Sheet, "Feuilles", "Feuille", "Sheet"),
        0x6c => (K::Drafting, "Vues de dessin", "Vue de dessin", "Drafting View"),
        0x6d => (K::Plan, "Plans d'étage", "Plan d'étage", "Floor Plan"),
        0x6e => (K::Plan, "Plans de surface", "Plan de surface", "Area Plan"),
        0x6f => (K::CeilingPlan, "Plans de plafond", "Plan de plafond", "Ceiling Plan"),
        0x70 => (K::Section, "Coupes", "Coupe", "Section"),
        0x71 => (K::Detail, "Vues de détail", "Vue de détail", "Detail View"),
        0x72 => (K::Elevation, "Élévations", "Élévation", "Elevation"),
        0x73 => (K::Other, "Rapports de charges", "Rapport de charges", "Loads Report"),
        0x74 => (K::Other, "Rapports de perte de charge", "Rapport de perte de charge", "Pressure Loss Report"),
        0x75 => (K::Legend, "Légendes", "Légende", "Legend"),
        0x76 => (K::Schedule, "Nomenclatures de tableaux", "Nomenclature de tableau", "Panel Schedule"),
        0x77 => (K::Schedule, "Nomenclatures de poteaux graphiques", "Nomenclature de poteaux graphique", "Graphical Column Schedule"),
        0x78 => (K::Plan, "Plans de structure", "Plan de structure", "Structural Plan"),
        _ => return None,
    })
}

/// Revit's English labels for the French section and folder names, longest first (the viewer
/// shows them when ModelCraft runs in English).
pub const LABELS_EN: &[(&str, &str)] = &[
    ("Nomenclatures de poteaux graphiques", "Graphical Column Schedules"),
    ("Rapports de perte de charge", "Pressure Loss Reports"),
    ("Nomenclatures de tableaux", "Panel Schedules"),
    ("Nomenclatures/Quantités", "Schedules/Quantities"),
    ("Rapports de charges", "Loads Reports"),
    ("Rapports de coûts", "Cost Reports"),
    ("Visites virtuelles", "Walkthroughs"),
    ("Plans de structure", "Structural Plans"),
    ("Plans de plafond", "Ceiling Plans"),
    ("Plans de surface", "Area Plans"),
    ("Vues de dessin", "Drafting Views"),
    ("Vues de détail", "Detail Views"),
    ("Plans d'étage", "Floor Plans"),
    ("Élévations", "Elevations"),
    ("Légendes", "Legends"),
    ("Feuilles", "Sheets"),
    ("Vues 3D", "3D Views"),
    ("Coupes", "Sections"),
    ("Rendus", "Renderings"),
    ("Vues", "Views"),
    ("(pas sur des feuilles)", "(not on sheets)"),
    ("(préfixe de feuille)", "(Sheet Prefix)"),
    ("(date d'émission)", "(Issue Date)"),
    ("(type/discipline)", "(Type/Discipline)"),
    ("(discipline)", "(Discipline)"),
    ("(dessiné par)", "(Drawn By)"),
    ("(phase)", "(Phase)"),
    ("(tout)", "(all)"),
    ("Architecture", "Architectural"),
    ("Structure", "Structural"),
    ("Mécanique", "Mechanical"),
    ("Électricité", "Electrical"),
    ("Plomberie", "Plumbing"),
];

/// A section or folder label in English (view names are left as they are).
pub fn label_en(label: &str) -> String {
    let mut s = label.to_string();
    let mut done = false;
    for (fr, en) in LABELS_EN {
        if !fr.starts_with('(') && !done && (s == *fr || s.starts_with(&format!("{fr} ("))) {
            s = format!("{en}{}", &s[fr.len()..]);
            done = true;
        } else if fr.starts_with('(') && s.ends_with(fr) {
            s = format!("{}{en}", &s[..s.len() - fr.len()]);
        }
    }
    s
}

/// Built-in parameter ids seen in browser organizations.
const P_FAMILY_TYPE: i64 = -1012106;
const P_PHASE: i64 = -1012102;
const P_DISCIPLINE: i64 = -1005163;
const P_VIEW_NAME: i64 = -1005112;
const P_SHEET_NUMBER: i64 = -1007401;
const P_SHEET_NAME: i64 = -1007400;
const P_VIEW_SHEET_NUMBER: i64 = -1005207;
const EMPTY: &str = "???";

/// Organization names of the stock templates, in Revit's French labels.
fn org_label(name: &str) -> String {
    match name {
        "" | "all" | "All" => "tout".into(),
        "not on sheets" => "pas sur des feuilles".into(),
        "Type/Discipline" => "type/discipline".into(),
        "Discipline" => "discipline".into(),
        "Phase" => "phase".into(),
        "Sheet Prefix" => "préfixe de feuille".into(),
        "Issue Date" => "date d'émission".into(),
        "Drawn By" => "dessiné par".into(),
        n => n.into(),
    }
}

fn discipline(v: i64) -> Option<&'static str> {
    Some(match v {
        1 => "Architecture",
        2 => "Structure",
        4 => "Mécanique",
        8 => "Électricité",
        16 => "Plomberie",
        4095 => "Coordination",
        _ => return None,
    })
}

/// Accent-free, case-free, digit-aware sort key ("Niveau 2" < "Niveau 10").
fn sort_key(s: &str) -> Vec<(u8, u64, String)> {
    let mut out = Vec::new();
    let mut num = String::new();
    let mut txt = String::new();
    let flush_txt = |t: &mut String, out: &mut Vec<(u8, u64, String)>| {
        if !t.is_empty() {
            out.push((1, 0, std::mem::take(t)));
        }
    };
    for c in s.chars() {
        if c.is_ascii_digit() {
            flush_txt(&mut txt, &mut out);
            num.push(c);
            continue;
        }
        if !num.is_empty() {
            out.push((0, num.parse().unwrap_or(u64::MAX), String::new()));
            num.clear();
        }
        let base = match c {
            'à' | 'â' | 'ä' | 'á' | 'À' | 'Â' | 'Ä' | 'Á' => 'a',
            'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'e',
            'î' | 'ï' | 'Î' | 'Ï' => 'i',
            'ô' | 'ö' | 'Ô' | 'Ö' => 'o',
            'ù' | 'û' | 'ü' | 'Ù' | 'Û' | 'Ü' => 'u',
            'ç' | 'Ç' => 'c',
            c => c,
        };
        txt.extend(base.to_lowercase());
    }
    flush_txt(&mut txt, &mut out);
    if !num.is_empty() {
        out.push((0, num.parse().unwrap_or(u64::MAX), String::new()));
    }
    out
}

#[derive(Debug, Default)]
struct Node {
    label: String,
    kind: Option<K>,
    id: i64,
    key: Vec<(u8, u64, String)>,
    folders: Vec<Node>,
    leaves: Vec<Node>,
}

impl Node {
    fn folder<'a>(&'a mut self, label: &str) -> &'a mut Node {
        if let Some(i) = self.folders.iter().position(|f| f.label == label) {
            return &mut self.folders[i];
        }
        self.folders.push(Node { label: label.to_string(), kind: Some(K::Folder), id: -1, key: sort_key(label), ..Default::default() });
        self.folders.last_mut().unwrap()
    }
    fn find(&mut self, id: i64) -> Option<&mut Node> {
        if self.id == id && id != -1 {
            return Some(self);
        }
        for c in self.folders.iter_mut().chain(self.leaves.iter_mut()) {
            if let Some(n) = c.find(id) {
                return Some(n);
            }
        }
        None
    }
    fn sort(&mut self, asc: bool) {
        self.folders.sort_by(|a, b| a.key.cmp(&b.key));
        self.leaves.sort_by(|a, b| if asc { a.key.cmp(&b.key) } else { b.key.cmp(&a.key) });
        for c in self.folders.iter_mut().chain(self.leaves.iter_mut()) {
            c.sort(asc);
        }
    }
    fn flatten(&self, depth: u16, out: &mut Vec<BrowserNode>) {
        out.push(BrowserNode { label: self.label.clone(), depth, kind: self.kind.unwrap_or(K::Other) });
        // Revit lists the sub-folders before the views of a folder
        for c in self.folders.iter().chain(self.leaves.iter()) {
            c.flatten(depth + 1, out);
        }
    }
}

struct Ctx<'m> {
    m: &'m Model,
    /// Type used for a view without a type (old elevations): first type of the family.
    fallback: HashMap<i64, i64>,
}

impl<'m> Ctx<'m> {
    fn new(m: &'m Model) -> Self {
        let mut fallback: HashMap<i64, i64> = HashMap::new();
        for (&id, t) in &m.types {
            let e = fallback.entry(t.family).or_insert(id);
            if id < *e {
                *e = id;
            }
        }
        Ctx { m, fallback }
    }

    fn view_type(&self, v: &View) -> Option<(i64, &'m ViewType)> {
        if let Some(t) = self.m.types.get(&v.type_id) {
            return Some((v.type_id, t));
        }
        let fam = match (v.class.as_str(), v.section_type) {
            ("DBViewSection", 2) => 0x72,
            ("DBViewSection", 3) => 0x71,
            ("DBViewSection", _) => 0x70,
            ("DBViewLegend", _) => 0x75,
            ("DBViewSchedule", _) => 0x69,
            ("DBView3d", _) => 0x66,
            ("DBViewRendering", _) => 0x68,
            _ => return None,
        };
        let id = *self.fallback.get(&fam)?;
        Some((id, &self.m.types[&id]))
    }

    fn family_idx(&self, v: &View) -> i64 {
        self.view_type(v).map(|(_, t)| t.family).unwrap_or(match v.class.as_str() {
            "DBViewSchedule" => 0x69,
            "DBViewLegend" => 0x75,
            "DBViewRendering" => 0x68,
            "DBView3d" => 0x66,
            "DBViewDrafting" => 0x6c,
            "DBViewSection" => 0x70,
            _ => -1,
        })
    }

    /// "Plans d'étage" or "Coupes (Wall Section)": family plural, plus the type name when it is
    /// not the family's default name.
    fn family_type_label(&self, v: &View) -> String {
        let fi = self.family_idx(v);
        let Some((_, plural, fr, en)) = family(fi) else { return EMPTY.into() };
        let tname = self.view_type(v).map(|(_, t)| t.name.trim().to_string()).unwrap_or_default();
        if tname.is_empty() || tname.eq_ignore_ascii_case(fr) || tname.eq_ignore_ascii_case(en) || tname.eq_ignore_ascii_case(plural) {
            plural.into()
        } else {
            format!("{plural} ({tname})")
        }
    }

    fn param(&self, v: &View, p: i64) -> String {
        let s = match p {
            P_FAMILY_TYPE => return self.family_type_label(v),
            P_VIEW_NAME => v.name.clone(),
            P_SHEET_NUMBER => v.sheet_number.clone(),
            P_SHEET_NAME => v.name.clone(),
            P_DISCIPLINE => v.ints.get(&p).and_then(|&d| discipline(d)).unwrap_or("").to_string(),
            P_PHASE => v.elem_ids.get(&p).and_then(|i| self.m.phases.get(i)).cloned().unwrap_or_default(),
            P_VIEW_SHEET_NUMBER => if v.on_sheet { "•".into() } else { String::new() },
            _ => {
                if let Some(s) = v.strs.get(&p) {
                    s.clone()
                } else if let Some(i) = v.ints.get(&p) {
                    i.to_string()
                } else if let Some(i) = v.elem_ids.get(&p) {
                    self.m.phases.get(i).cloned().unwrap_or_else(|| if *i == -1 { String::new() } else { i.to_string() })
                } else if let Some(x) = v.dbls.get(&p) {
                    format!("{x}")
                } else {
                    String::new()
                }
            }
        };
        s
    }

    /// Organization filters: equality tests (relation 0 = equals, 1 = does not equal); other
    /// relations are not interpreted (the view is kept).
    fn passes(&self, v: &View, org: &Org) -> bool {
        org.filters.iter().all(|(p, rel, s, i, e)| {
            let have = self.param(v, *p);
            let want = if !s.is_empty() || (*i == 0 && *e == -1) { s.clone() } else if *e != -1 { self.m.phases.get(e).cloned().unwrap_or(e.to_string()) } else { i.to_string() };
            match rel {
                0 => have == want,
                1 => have != want,
                _ => true,
            }
        })
    }

    fn folder_path(&self, v: &View, org: Option<&Org>) -> Vec<String> {
        org.map(|o| o.folders.as_slice())
            .unwrap_or(&[])
            .iter()
            .map(|&(p, n)| {
                let mut s = self.param(v, p);
                if n > 0 {
                    s = s.chars().take(n).collect();
                }
                if s.trim().is_empty() {
                    EMPTY.into()
                } else {
                    s
                }
            })
            .collect()
    }

    /// One section: its views placed in their folders, dependent views under their primary view.
    fn section(&self, label: String, kind: K, items: &[&View], org: Option<&Org>, leaf: impl Fn(&View) -> (String, K)) -> Node {
        let mut root = Node { label, kind: Some(kind), id: -1, ..Default::default() };
        let items: Vec<&View> = items.iter().copied().filter(|v| org.is_none_or(|o| self.passes(v, o))).collect();
        let ids: HashSet<i64> = items.iter().map(|v| v.id).collect();
        let (deps, prims): (Vec<&View>, Vec<&View>) = items.iter().partition(|v| v.primary != -1 && ids.contains(&v.primary));
        for v in prims {
            let mut n = &mut root;
            for f in self.folder_path(v, org) {
                n = n.folder(&f);
            }
            let (l, k) = leaf(v);
            n.leaves.push(Node { key: sort_key(&l), label: l, kind: Some(k), id: v.id, ..Default::default() });
        }
        for v in deps {
            let (l, k) = leaf(v);
            if let Some(p) = root.find(v.primary) {
                p.leaves.push(Node { key: sort_key(&l), label: l, kind: Some(k), id: v.id, ..Default::default() });
            }
        }
        root.sort(org.is_none_or(|o| o.sort_asc));
        root
    }
}

/// The browser rows: Vues, Légendes, Nomenclatures/Quantités, Feuilles.
pub fn build(m: &Model) -> Vec<BrowserNode> {
    let c = Ctx::new(m);
    let mut views = Vec::new();
    let mut legends = Vec::new();
    let mut schedules = Vec::new();
    let mut sheets = Vec::new();
    for v in &m.views {
        if v.template || v.family_owned || v.owner_view != -1 || v.name.is_empty() || v.name == EMPTY || matches!(v.class.as_str(), "DBViewProject" | "RbsDbViewSystemNavigator") {
            continue;
        }
        match c.family_idx(v) {
            0x75 => legends.push(v),
            0x69 | 0x76 => schedules.push(v),
            0x6b => sheets.push(v),
            -1 => {}
            _ => views.push(v),
        }
    }
    let org = |k: usize, kind: i64| -> Option<&Org> { m.current[k].and_then(|i| m.orgs.get(&i)).or_else(|| m.orgs.values().filter(|o| o.kind == kind && o.default).min_by_key(|o| o.id)) };
    let (ov, osh, osc) = (org(0, 0), org(1, 1), org(2, 3));
    let title = |base: &str, o: Option<&Org>| format!("{base} ({})", org_label(o.map(|o| o.name.as_str()).unwrap_or("")));
    let mut out = Vec::new();
    let view_leaf = |v: &View| (v.name.clone(), family(c.family_idx(v)).map(|f| f.0).unwrap_or(K::Other));
    let stock = Org { kind: 0, default: true, folders: vec![(P_FAMILY_TYPE, 0)], sort_asc: true, ..Default::default() };
    c.section(title("Vues", ov), K::Views, &views, ov.or(Some(&stock)), view_leaf).flatten(0, &mut out);
    if !legends.is_empty() {
        c.section("Légendes".into(), K::Legends, &legends, None, |v| (v.name.clone(), K::Legend)).flatten(0, &mut out);
    }
    if !schedules.is_empty() {
        c.section(title("Nomenclatures/Quantités", osc), K::Schedules, &schedules, osc, |v| (v.name.clone(), K::Schedule)).flatten(0, &mut out);
    }
    let sheet_leaf = |v: &View| (if v.sheet_number.is_empty() { v.name.clone() } else { format!("{} - {}", v.sheet_number, v.name) }, K::Sheet);
    c.section(title("Feuilles", osh), K::Sheets, &sheets, osh, sheet_leaf).flatten(0, &mut out);
    out
}


#[cfg(test)]
mod tests {
    use super::*;

    fn model(name: &str) -> Option<Model> {
        let dir = std::env::var("RVT_SAMPLES").unwrap_or_else(|_| "/workspace/rvt-samples".into());
        let p = std::path::Path::new(&dir).join("ad").join(name);
        let f = std::fs::File::open(p).ok()?;
        let mut cf = cfb::CompoundFile::open(std::io::BufReader::new(f)).ok()?;
        let names: Vec<(String, u64)> = cf.walk().filter(|e| e.is_stream()).map(|e| (e.path().to_string_lossy().replace('\\', "/").trim_start_matches('/').to_string(), e.len())).collect();
        read_model(&mut cf, &names, 2 << 30)
    }

    fn lines(rows: &[BrowserNode]) -> Vec<String> {
        rows.iter().map(|r| format!("{}{}", "  ".repeat(r.depth as usize), r.label)).collect()
    }

    #[test]
    fn english_labels() {
        assert_eq!(label_en("Vues (tout)"), "Views (all)");
        assert_eq!(label_en("Coupes (Wall Section)"), "Sections (Wall Section)");
        assert_eq!(label_en("Nomenclatures/Quantités (tout)"), "Schedules/Quantities (all)");
        assert_eq!(label_en("Vues 3D"), "3D Views");
        assert_eq!(label_en("Coupes de bâtiment"), "Coupes de bâtiment");
    }

    #[test]
    fn sort_is_natural_and_accent_free() {
        let mut v = vec!["Niveau 10", "niveau 2", "Élévations", "Coupes", "Vues 3D"];
        v.sort_by_key(|s| sort_key(s));
        assert_eq!(v, ["Coupes", "Élévations", "niveau 2", "Niveau 10", "Vues 3D"]);
    }

    #[test]
    fn sample_project_browser() {
        let Some(mut m) = model("rac_2018.rvt") else { return };
        assert_eq!(m.current, [Some(24945), Some(24946), Some(1088448)]);
        let l = lines(&build(&m));
        for want in ["Vues (tout)", "  Plans d'étage", "    Level 1", "  Coupes (Wall Section)", "  Élévations (Building Elevation)", "Nomenclatures/Quantités (tout)", "  Planting Schedule", "Feuilles (tout)", "  A001 - Title Sheet"] {
            assert!(l.iter().any(|x| x == want), "{want} missing in {l:#?}");
        }
        // templates, revision schedules and system views are not listed
        assert!(!l.iter().any(|x| x.contains("Revision Schedule") || x.contains("???")), "{l:#?}");
        // other organizations
        m.current = [Some(26013), Some(26017), None];
        let l = lines(&build(&m));
        assert!(l.contains(&"Vues (discipline)".to_string()) && l.contains(&"  Architecture".to_string()) && l.contains(&"    Plans d'étage".to_string()), "{l:#?}");
        assert!(l.contains(&"Feuilles (préfixe de feuille)".to_string()) && l.contains(&"  A".to_string()) && l.contains(&"    A101 - Site Plan".to_string()), "{l:#?}");
        m.current = [Some(24947), None, None];
        let l = lines(&build(&m));
        assert!(l.contains(&"Vues (pas sur des feuilles)".to_string()) && l.contains(&"    {3D}".to_string()) && !l.contains(&"    Level 1".to_string()), "{l:#?}");
        m.current = [Some(26012), None, None];
        let l = lines(&build(&m));
        assert!(l.contains(&"  Working Drawings".to_string()), "{l:#?}");
    }

    #[test]
    fn samples_all_versions() {
        for f in ["rac_2021.rvt", "rst_2018.rvt", "rst_2023.rvt", "rst_2025.rvt"] {
            let Some(m) = model(f) else { continue };
            assert!(m.current.iter().all(|c| c.is_some()), "{f}");
            let l = lines(&build(&m));
            assert!(l.contains(&"  Élévations (Building Elevation)".to_string()), "{f}: {l:#?}");
            assert!(l.iter().filter(|x| x.starts_with("  ") && x.contains(" - ") && !x.starts_with("    ")).count() >= 6, "{f}: {l:#?}");
        }
    }
}
