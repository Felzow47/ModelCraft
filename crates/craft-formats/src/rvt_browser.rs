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
pub(crate) struct Ty {
    pub(crate) b0: u8,
    pub(crate) b1: u8,
    pub(crate) n: u32,
    pub(crate) tag: u16,
    /// Compound value types (`0x0d`): the element type (`b0`, count) given after the field.
    pub(crate) e0: u8,
    pub(crate) en: u32,
}

#[derive(Debug)]
pub(crate) struct Class {
    pub(crate) name: String,
    pub(crate) parent: u16,
    pub(crate) fields: Vec<(String, Ty)>,
}

/// The class table of `/Formats/Latest`.
pub struct Schema {
    pub(crate) classes: HashMap<u16, Class>,
    by_name: HashMap<String, u16>,
    /// Flattened field list of each class, base class first.
    pub(crate) chains: HashMap<u16, Vec<(String, Ty)>>,
    pub(crate) id_tag: u16,
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

    pub fn tags_list(&self) -> Vec<u16> {
        self.classes.keys().copied().collect()
    }
    /// Classes whose name contains `sub`: (tag, name, parent name, own field names).
    pub fn find(&self, sub: &str) -> Vec<(u16, String, String, Vec<String>)> {
        let mut v: Vec<_> = self.classes.iter().filter(|(_, c)| c.name.contains(sub)).map(|(t, c)| (*t, c.name.clone(), self.name(c.parent).to_string(), c.fields.iter().map(|f| f.0.clone()).collect())).collect();
        v.sort();
        v
    }
    /// Full field chain of a class (base first) as `(name, "b0 b1 n tag-name")`, for probes.
    pub fn describe(&self, tag: u16) -> Vec<(String, String)> {
        self.chains.get(&tag).map(|c| c.iter().map(|(n, t)| (n.clone(), format!("{:02x} {:02x} {} [{:02x}x{}] {}", t.b0, t.b1, t.n, t.e0, t.en, if t.tag != 0 { self.name(t.tag) } else { "" }))).collect()).unwrap_or_default()
    }
    pub fn tag_of(&self, name: &str) -> Option<u16> {
        self.by_name.get(name).copied()
    }
    pub fn name(&self, tag: u16) -> &str {
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
    let mut ty = Ty { b0, b1, n: 0, tag: 0, e0: 7, en: 3 };
    // `1x`: fixed array of n (`10` values, `13` raw pointers…)
    if b1 & 0xf0 == 0x10 {
        ty.n = u32_at(d, p)?;
        p += 4;
    }
    // `0d` (XYZ-like): `[u32 len][name][element type]`, e.g. `01 00 00 00 20` + `07 10 00 00 03 00 00 00`
    if b0 == 0x0d && u32_at(d, p) == Some(1) && d.get(p + 4) == Some(&0x20) {
        let (e, mut q) = parse_type(d, p + 5, depth + 1)?;
        // an element of class type: its tag is written once more after it
        if e.b0 == 0x0e && e.tag != 0 && u16_at(d, q) == Some(e.tag) {
            q += 2;
        }
        ty.e0 = e.b0;
        ty.en = if e.b1 == 0x10 { e.n } else { 1 };
        if e.b0 == 0x0e {
            ty.tag = e.tag;
        }
        p = q;
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

pub(crate) fn prim_size(b0: u8) -> Option<usize> {
    Some(match b0 {
        9 => 16,
        7 | 6 | 0x0b => 8,
        4 | 5 => 4,
        3 => 2,
        1 | 2 => 1,
        _ => return None,
    })
}

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
    /// Explicit local object ids already queued (`[u32 id][u16 tag]` pointers).
    seen: HashSet<u32>,
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
            match self.val(*ty, p, out, name, depth) {
                Some(q) => p = q,
                None => {
                    if std::env::var_os("RVT_BROWSER_DEBUG").is_some() {
                        eprintln!("fail {} {name} {:02x} {:02x} {}", s.name(tag), ty.b0, ty.b1, ty.tag);
                    }
                    return None;
                }
            }
        }
        Some(p)
    }

    fn val(&mut self, ty: Ty, p: usize, out: &mut Map<'s>, name: &'s str, depth: u32) -> Option<usize> {
        let d = self.d;
        if ty.b0 == 0x0e {
            return match ty.b1 {
                // owned pointer: null (0), or `[u32][u16 class tag]`: ff ff ff ff (content
                // deferred) or an explicit local object id (deferred on first mention, a
                // reference afterwards; 1 and 2 are the document and the element itself)
                1 | 2 => {
                    let v = u32_at(d, p)?;
                    if v == 0 {
                        return Some(p + 4);
                    }
                    let t = u16_at(d, p + 4)?;
                    if !self.s.chains.contains_key(&t) {
                        return None;
                    }
                    if v == 0xffff_ffff || (v > 2 && v < 0x100_0000 && self.seen.insert(v)) {
                        self.pending.push(t);
                        Some(p + 6)
                    } else if v < 0x100_0000 {
                        Some(p + 6)
                    } else {
                        None
                    }
                }
                3 => Some(p + 4),
                0x13 => Some(p + 4 * ty.n as usize),
                0x53 => Some(p + 4 + 4 * u32_at(d, p)? as usize),
                0x11 | 0x12 => {
                    let mut p = p;
                    for _ in 0..ty.n {
                        p = self.val(Ty { b0: 0x0e, b1: 1, n: 0, tag: 0, e0: 7, en: 3 }, p, out, name, depth)?;
                    }
                    Some(p)
                }
                // fixed array of by-value objects
                0x10 => {
                    let mut p = p;
                    for _ in 0..ty.n.min(64) {
                        if ty.tag == self.s.id_tag {
                            p += self.w;
                        } else {
                            let mut o = Map::new();
                            p = self.fields(ty.tag, p, &mut o, depth + 1)?;
                        }
                    }
                    Some(p)
                }
                0x51 | 0x52 => {
                    let n = u32_at(d, p)?;
                    if n > 100_000 {
                        return None;
                    }
                    let mut p = p + 4;
                    for _ in 0..n {
                        p = self.val(Ty { b0: 0x0e, b1: 1, n: 0, tag: 0, e0: 7, en: 3 }, p, out, name, depth)?;
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
            9 => 16,
            // XYZ (or another compound of `en` elements of type `e0`; ids: `en` ids)
            0x0d if ty.e0 == 0x0e => self.w * ty.en.max(1) as usize,
            0x0d => prim_size(ty.e0)? * ty.en.max(1) as usize,
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
        self.seen.clear();
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
    let mut dec = Dec { s, d, w, pending: Vec::new(), seen: HashSet::new() };
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

/// A linked Revit model (`RvtLinkSymbol`), its state at the last save (`TransmissionData`) and
/// the names of its placed instances (`RvtLinkInstance`).
#[derive(Debug, Clone, Default)]
pub struct Link {
    pub id: i64,
    pub name: String,
    /// `LastSavedLoadState` of `TransmissionData` ("Loaded", "Unloaded", "NotFound"…), empty
    /// when not listed.
    pub state: String,
    pub path_name: String,
    pub instances: Vec<String>,
}

/// A loadable family (`FamilySurrogate`): its name and built-in category id.
#[derive(Debug, Clone, Default)]
pub struct Fam {
    pub surrogate: i64,
    pub family: i64,
    pub name: String,
    pub category: i64,
}

/// A group type (`ElementGroupType`): detail when it belongs to a view type, attached detail
/// groups point to their model group.
#[derive(Debug, Clone, Default)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub detail: bool,
    pub attached_to: i64,
}

/// What the browser is built from.
#[derive(Debug, Default)]
pub struct Model {
    pub families: Vec<Fam>,
    /// Family types (`FamSymSurrogate`) as (family surrogate id, name).
    pub family_types: Vec<(i64, String)>,
    pub in_place: HashSet<i64>,
    pub groups: Vec<Group>,
    pub links: Vec<Link>,
    /// Link instances as (symbol id, name) until they are attached to their link.
    pub link_instances: Vec<(i64, String)>,
    pub views: Vec<View>,
    pub types: HashMap<i64, ViewType>,
    pub phases: HashMap<i64, String>,
    pub orgs: HashMap<i64, Org>,
    /// Current organization for views, sheets, schedules.
    pub current: [Option<i64>; 3],
    /// Starting view (`InitialViewSettings.m_initialViewId`, "Vue de démarrage"); `None` when
    /// the project has none (Revit then reopens the last active views).
    pub initial_view: Option<i64>,
}

impl Model {
    fn absorb(&mut self, s: &Schema, elems: &[Elem]) {
        for e in elems {
            if int(&e.f, "m_famId").unwrap_or(-1) != -1 {
                continue;
            }
            let cls = s.name(e.tag);
            if cls == "FamilySurrogate" {
                if !self.families.iter().any(|f| f.surrogate == e.id) {
                    self.families.push(Fam {
                        surrogate: e.id,
                        family: int(&e.f, "m_elemId").unwrap_or(-1),
                        name: text(&e.f, "m_name").unwrap_or_default(),
                        category: int(&e.f, "m_categoryId").unwrap_or(-1),
                    });
                }
            } else if cls == "FamSymSurrogate" {
                if let Some(f) = int(&e.f, "m_famSurrogateId") {
                    self.family_types.push((f, text(&e.f, "m_name").unwrap_or_default()));
                }
            } else if cls == "Family" {
                if int(&e.f, "m_inPlace") == Some(1) {
                    self.in_place.insert(e.id);
                }
            } else if cls == "ElementGroupType" {
                if !self.groups.iter().any(|g| g.id == e.id) {
                    let attached = int(&e.f, "m_attachedParentId").unwrap_or(-1);
                    self.groups.push(Group {
                        id: e.id,
                        name: e.deferred("SymbolInfo").and_then(|m| text(m, "m_name")).unwrap_or_default(),
                        detail: int(&e.f, "m_viewType").unwrap_or(0) != 0 || attached > 0,
                        attached_to: if attached > 0 { attached } else { -1 },
                    });
                }
            } else if cls == "RvtLinkSymbol" {
                if int(&e.f, "m_parentSymbolId").unwrap_or(-1) == -1 && !self.links.iter().any(|l| l.id == e.id) {
                    let name = e.deferred("SymbolInfo").and_then(|m| text(m, "m_name")).unwrap_or_default();
                    self.links.push(Link { id: e.id, name, ..Default::default() });
                }
            } else if cls == "RvtLinkInstance" {
                let sym = e.deferred("RvtLinkInstInfo").or_else(|| e.deferred("InstanceInfo")).and_then(|m| int(m, "m_symbolId")).unwrap_or(-1);
                if int(&e.f, "m_parentInstanceId").unwrap_or(-1) == -1 {
                    self.link_instances.push((sym, text(&e.f, "m_name").unwrap_or_default()));
                }
            } else if s.isa(e.tag, "DBViewType") {
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
            } else if cls == "InitialViewSettings" {
                if let Some(v) = int(&e.f, "m_initialViewId").filter(|&v| v > 0) {
                    self.initial_view = Some(v);
                }
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
        let mut dec = Dec { s, d: g, w, pending: Vec::new(), seen: HashSet::new() };
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
    read_model_with(cf, names, max_partition, &mut |_, _, _| {})
}

/// [`read_model`], also handing every inflated partition (schema, data, id width) to `each`
/// (one inflation for several readers).
pub fn read_model_with<F: Read + std::io::Seek>(cf: &mut cfb::CompoundFile<F>, names: &[(String, u64)], max_partition: u64, each: &mut dyn FnMut(&Schema, &[u8], usize)) -> Option<Model> {
    let fmt = rvt_model::read_stream(cf, "/Formats/Latest", 64 << 20, 64 << 20, |_| true)?;
    if fmt.verified == 0 {
        return None;
    }
    let schema = Schema::parse(&fmt.data);
    drop(fmt);
    let mut want = vec![false; 65536];
    for &t in schema.classes.keys() {
        if ["DBView", "DBViewType", "BrowserOrganization", "ProjectPhase", "RvtLinkSymbol", "RvtLinkInstance", "FamilySurrogate", "FamSymSurrogate", "Family", "ElementGroupType", "InitialViewSettings"].iter().any(|b| schema.isa(t, b)) {
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
        drop(elems);
        each(&schema, &inf.data, w);
    }
    if width == 0 {
        return None;
    }
    if let Some(g) = rvt_model::read_stream(cf, "/Global/Latest", 256 << 20, 256 << 20, |_| true) {
        m.tracking(&schema, &g.data, width);
    }
    let mut td = Vec::new();
    if cf.entry("/TransmissionData").is_ok_and(|e| e.len() < (16 << 20)) {
        if let Ok(mut st) = cf.open_stream("/TransmissionData") {
            let _ = st.read_to_end(&mut td);
        }
    }
    m.attach_links(&td);
    Some(m)
}

/// `TransmissionData`: a u32 then a UTF-16 XML document with one `<ExternalFileReference>` per
/// external file (`ElementId`, `ExternalFileReferenceType` "Revit Link", `LastSavedPath`,
/// `LastSavedLoadState`…).
pub fn transmission_refs(b: &[u8]) -> Vec<HashMap<String, String>> {
    let u: Vec<u16> = b.get(4..).unwrap_or(&[]).chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let x = String::from_utf16_lossy(&u);
    let mut out = Vec::new();
    for block in x.split("<ExternalFileReference>").skip(1) {
        let block = block.split("</ExternalFileReference>").next().unwrap_or("");
        let mut m = HashMap::new();
        let mut rest = block;
        while let Some(a) = rest.find('<') {
            let Some(b) = rest[a..].find('>') else { break };
            let tag = &rest[a + 1..a + b];
            rest = &rest[a + b + 1..];
            if tag.starts_with('/') || tag.ends_with('/') {
                continue;
            }
            if let Some(end) = rest.find(&format!("</{tag}>")) {
                let v = rest[..end].replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'");
                m.insert(tag.to_string(), v);
                rest = &rest[end + tag.len() + 3..];
            }
        }
        out.push(m);
    }
    out
}

impl Model {
    fn attach_links(&mut self, td: &[u8]) {
        let refs = transmission_refs(td);
        for r in refs.iter().filter(|r| r.get("ExternalFileReferenceType").is_some_and(|t| t.contains("Revit"))) {
            let id: i64 = r.get("ElementId").and_then(|v| v.trim().parse().ok()).unwrap_or(i64::MIN);
            let path = r.get("LastSavedPath").or(r.get("DesiredPath")).cloned().unwrap_or_default();
            let file = path.rsplit(['\\', '/']).next().unwrap_or("").to_string();
            let state = r.get("LastSavedLoadState").cloned().unwrap_or_default();
            if let Some(l) = self.links.iter_mut().find(|l| l.id == id) {
                l.state = state;
                l.path_name = file;
            } else {
                self.links.push(Link { id, name: String::new(), state, path_name: file, instances: Vec::new() });
            }
        }
        for (sym, name) in std::mem::take(&mut self.link_instances) {
            if let Some(l) = self.links.iter_mut().find(|l| l.id == sym) {
                l.instances.push(name);
            }
        }
    }
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
    ("Liens Revit", "Revit Links"),
    ("Familles", "Families"),
    ("Groupes", "Groups"),
    ("Détail", "Detail"),
    ("Modèle", "Model"),
    ("Autres familles", "Other Families"),
    ("Symboles d'annotation", "Annotation Symbols"),
    ("Murs", "Walls"),
    ("Fenêtres", "Windows"),
    ("Portes", "Doors"),
    ("Sols", "Floors"),
    ("Toits", "Roofs"),
    ("Plafonds", "Ceilings"),
    ("Mobilier", "Furniture"),
    ("Poteaux", "Columns"),
    ("Escaliers", "Stairs"),
    ("Garde-corps", "Railings"),
    ("Balustres", "Balusters"),
    ("Modèles génériques", "Generic Models"),
    ("Panneaux de murs-rideaux", "Curtain Panels"),
    ("Meneaux de murs-rideaux", "Curtain Wall Mullions"),
    ("Rampes", "Ramps"),
    ("Cartouches", "Title Blocks"),
    ("Supports", "Supports"),
    ("Meubles de rangement", "Casework"),
    ("Equipement électrique", "Electrical Equipment"),
    ("Installations électriques", "Electrical Fixtures"),
    ("Systèmes de mobilier", "Furniture Systems"),
    ("Luminaires", "Lighting Fixtures"),
    ("Equipement de génie climatique", "Mechanical Equipment"),
    ("Installations de plomberie", "Plumbing Fixtures"),
    ("Parking", "Parking"),
    ("Site", "Site"),
    ("Fondations", "Structural Foundations"),
    ("Ossature", "Structural Framing"),
    ("Poteaux porteurs", "Structural Columns"),
    ("Equipement spécialisé", "Specialty Equipment"),
    ("Plantes", "Planting"),
    ("Environnement", "Entourage"),
    ("Composants de détail", "Detail Items"),
    ("Profils", "Profiles"),
    ("Volume", "Mass"),
    ("Raccords de gaine", "Duct Fittings"),
    ("Bouches d'aération", "Air Terminals"),
    ("Accessoires de gaine", "Duct Accessories"),
    ("Raccords de canalisation", "Pipe Fittings"),
    ("Accessoires de canalisation", "Pipe Accessories"),
    ("Extrémités de garde-corps", "Railing Terminations"),
    ("Equipement de restauration", "Food Service Equipment"),
    ("Circulation verticale", "Vertical Circulation"),
    ("Profils de division", "Division Profiles"),
    ("Formes d'armature", "Rebar Shapes"),
    ("Assemblages structurels", "Structural Connections"),
    ("Coupleurs", "Rebar Couplers"),
    ("Aménagements paysagers", "Hardscape"),
    ("Raidisseurs structurels", "Structural Stiffeners"),
    ("Equipement de plomberie", "Plumbing Equipment"),
    ("Treillis", "Structural Trusses"),
    ("Appareils téléphoniques", "Telephone Devices"),
    ("Appareils d'appel infirmière", "Nurse Call Devices"),
    ("Appareils de sécurité", "Security Devices"),
    ("Appareils de communication", "Communication Devices"),
    ("Appareils de données", "Data Devices"),
    ("Dispositifs d'alarme incendie", "Fire Alarm Devices"),
    ("Appareils d'éclairage", "Lighting Devices"),
    ("Sprinklers", "Sprinklers"),
    ("Raccords de chemin de câbles", "Cable Tray Fittings"),
    ("Raccords de conduit", "Conduit Fittings"),
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
        out.push(BrowserNode { label: self.label.clone(), depth, kind: self.kind.unwrap_or(K::Other), id: if self.id > 0 { self.id } else { -1 }, start: false });
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
    families(m, &mut out);
    groups(m, &mut out);
    links(m, &mut out);
    // the starting view ("Vue de démarrage"): Revit opens the project on it
    if let Some(sv) = m.initial_view {
        for r in out.iter_mut().filter(|r| r.id == sv) {
            r.start = true;
        }
    }
    out
}

/// Built-in category of a loadable family, French plural as in the French Revit browser.
/// Built-in annotation categories (tags, heads, symbols, tick marks, view titles,
/// boundary conditions), sorted ascending. Ids from the public Revit API BuiltInCategory list.
const ANNOTATION_CATEGORIES: &[i64] = &[
    -2009663, -2009656, -2009655, -2009654, -2009653, -2009652, -2009651, -2009650, -2009649, -2009608,
    -2009064, -2009063, -2009061, -2009059, -2009058, -2009057, -2009056, -2009055, -2009040, -2009033,
    -2009028, -2009023, -2009022, -2009021, -2009020, -2009011, -2009010, -2009005, -2008236, -2008235,
    -2008233, -2008229, -2008213, -2008209, -2008204, -2008194, -2008155, -2008154, -2008153, -2008133,
    -2008131, -2008129, -2008127, -2008115, -2008100, -2008088, -2008086, -2008084, -2008082, -2008080,
    -2008078, -2008076, -2008074, -2008061, -2008060, -2008057, -2008056, -2008048, -2008047, -2008038,
    -2008017, -2008014, -2008004, -2008003, -2006282, -2006281, -2006279, -2006278, -2006276, -2006273,
    -2006266, -2006264, -2006243, -2006230, -2006226, -2006222, -2006220, -2006211, -2006210, -2006209,
    -2006208, -2006178, -2006177, -2006176, -2006175, -2006174, -2006173, -2006172, -2006171, -2006170,
    -2006100, -2006080, -2006045, -2006040, -2006020, -2005307, -2005306, -2005305, -2005304, -2005303,
    -2005302, -2005301, -2005255, -2005254, -2005253, -2005252, -2005251, -2005250, -2005130, -2005111,
    -2005110, -2005102, -2005101, -2005100, -2005033, -2005032, -2005031, -2005030, -2005029, -2005028,
    -2005027, -2005026, -2005025, -2005024, -2005023, -2005022, -2005021, -2005020, -2005019, -2005018,
    -2005017, -2005016, -2005015, -2005014, -2005013, -2005012, -2005011, -2005010, -2005009, -2005008,
    -2005007, -2005006, -2005004, -2005003, -2005002, -2005001, -2005000, -2003410, -2003405, -2001355,
    -2001335, -2001269, -2001267, -2001221, -2001116, -2001114, -2001111, -2001110, -2001103, -2001102,
    -2001094, -2001078, -2001074, -2001073, -2001072, -2001071, -2001070, -2001069, -2001068, -2001067,
    -2001066, -2001065, -2001064, -2001063, -2001062, -2001061, -2001057, -2001054, -2001051, -2001048,
    -2001045, -2001042, -2001038, -2001015, -2001007, -2000989, -2000988, -2000986, -2000984, -2000958,
    -2000957, -2000956, -2000955, -2000943, -2000942, -2000941, -2000940, -2000930, -2000917, -2000834,
    -2000538, -2000515, -2000485, -2000480, -2000460, -2000450, -2000400, -2000270, -2000268, -2000266,
    -2000197, -2000150, -2000133,
];

/// Annotation families (tags, heads, generic annotations…) share "Symboles d'annotation".
pub fn category_label(id: i64) -> &'static str {
    match id {
        -2000011 => "Murs",
        -2000014 => "Fenêtres",
        -2000023 => "Portes",
        -2000032 => "Sols",
        -2000035 => "Toits",
        -2000038 => "Plafonds",
        -2000080 => "Mobilier",
        -2000100 => "Poteaux",
        -2000120 => "Escaliers",
        -2000126 => "Garde-corps",
        -2000127 => "Balustres",
        -2000151 => "Modèles génériques",
        -2000170 => "Panneaux de murs-rideaux",
        -2000171 => "Meneaux de murs-rideaux",
        -2000180 => "Rampes",
        -2000280 => "Cartouches",
        -2000948 => "Supports",
        -2001000 => "Meubles de rangement",
        -2001040 => "Equipement électrique",
        -2001060 => "Installations électriques",
        -2001100 => "Systèmes de mobilier",
        -2001120 => "Luminaires",
        -2001140 => "Equipement de génie climatique",
        -2001160 => "Installations de plomberie",
        -2001180 => "Parking",
        -2001260 => "Site",
        -2001300 => "Fondations",
        -2001320 => "Ossature",
        -2001330 => "Poteaux porteurs",
        -2001350 => "Equipement spécialisé",
        -2001360 => "Plantes",
        -2001370 => "Environnement",
        -2002000 => "Composants de détail",
        -2003000 => "Profils",
        -2003400 => "Volume",
        -2008010 => "Raccords de gaine",
        -2008013 => "Bouches d'aération",
        -2008016 => "Accessoires de gaine",
        -2008049 => "Raccords de canalisation",
        -2008055 => "Accessoires de canalisation",
        -2008075 => "Appareils téléphoniques",
        -2008077 => "Appareils d'appel infirmière",
        -2008079 => "Appareils de sécurité",
        -2008081 => "Appareils de communication",
        -2008083 => "Appareils de données",
        -2008085 => "Dispositifs d'alarme incendie",
        -2008087 => "Appareils d'éclairage",
        -2008099 => "Sprinklers",
        -2008126 => "Raccords de chemin de câbles",
        -2008128 => "Raccords de conduit",
        -2000949 => "Extrémités de garde-corps",
        -2001043 => "Equipement de restauration",
        -2001052 => "Circulation verticale",
        -2008165 => "Profils de division",
        -2009013 => "Formes d'armature",
        -2009030 => "Assemblages structurels",
        -2009060 => "Coupleurs",
        -2001036 => "Aménagements paysagers",
        -2001354 => "Raidisseurs structurels",
        -2008234 => "Equipement de plomberie",
        -2009600 => "Treillis",
        _ if ANNOTATION_CATEGORIES.binary_search(&id).is_ok() => "Symboles d'annotation",
        _ => "Autres familles",
    }
}

/// "Familles": category > loadable family > types (in-place families are not listed).
fn families(m: &Model, out: &mut Vec<BrowserNode>) {
    // "Autres familles" (categories not in the table) last
    let mut cats: std::collections::BTreeMap<(bool, Vec<(u8, u64, String)>), (&str, Vec<&Fam>)> = Default::default();
    for f in m.families.iter().filter(|f| !m.in_place.contains(&f.family) && !f.name.is_empty()) {
        let l = category_label(f.category);
        cats.entry((l == "Autres familles", sort_key(l))).or_insert((l, Vec::new())).1.push(f);
    }
    if cats.is_empty() {
        return;
    }
    out.push(BrowserNode { label: "Familles".into(), depth: 0, kind: K::Families, id: -1, start: false });
    for (_, (label, mut fams)) in cats {
        out.push(BrowserNode { label: label.into(), depth: 1, kind: K::Folder, id: -1, start: false });
        fams.sort_by_key(|f| sort_key(&f.name));
        // the same family can have several surrogates: one row, types merged
        fams.dedup_by(|a, b| a.name == b.name);
        for f in fams {
            out.push(BrowserNode { label: f.name.clone(), depth: 2, kind: K::Family, id: -1, start: false });
            let ids: Vec<i64> = m.families.iter().filter(|g| g.name == f.name && category_label(g.category) == label).map(|g| g.surrogate).collect();
            let mut types: Vec<&str> = m.family_types.iter().filter(|(s, n)| ids.contains(s) && !n.is_empty()).map(|(_, n)| n.as_str()).collect();
            types.sort_by_key(|n| sort_key(n));
            types.dedup();
            for t in types {
                out.push(BrowserNode { label: t.into(), depth: 3, kind: K::FamilyType, id: -1, start: false });
            }
        }
    }
}

fn sorted_groups(mut v: Vec<&Group>) -> Vec<&Group> {
    v.sort_by_key(|g| sort_key(&g.name));
    v
}

/// "Groupes": "Détail" and "Modèle" (attached detail groups under their model group).
fn groups(m: &Model, out: &mut Vec<BrowserNode>) {
    if m.views.is_empty() {
        return;
    }
    let named: Vec<&Group> = m.groups.iter().filter(|g| !g.name.is_empty()).collect();
    out.push(BrowserNode { label: "Groupes".into(), depth: 0, kind: K::Groups, id: -1, start: false });
    out.push(BrowserNode { label: "Détail".into(), depth: 1, kind: K::Folder, id: -1, start: false });
    for g in sorted_groups(named.iter().copied().filter(|g| g.detail && !named.iter().any(|p| p.id == g.attached_to)).collect()) {
        out.push(BrowserNode { label: g.name.clone(), depth: 2, kind: K::Group, id: -1, start: false });
    }
    out.push(BrowserNode { label: "Modèle".into(), depth: 1, kind: K::Folder, id: -1, start: false });
    for g in sorted_groups(named.iter().copied().filter(|g| !g.detail).collect()) {
        out.push(BrowserNode { label: g.name.clone(), depth: 2, kind: K::Group, id: -1, start: false });
        for a in sorted_groups(named.iter().copied().filter(|a| a.attached_to == g.id).collect()) {
            out.push(BrowserNode { label: a.name.clone(), depth: 3, kind: K::Group, id: -1, start: false });
        }
    }
}

/// "Liens Revit": each linked model (red when it was not loaded at the last save) with its
/// placed instances below.
fn links(m: &Model, out: &mut Vec<BrowserNode>) {
    let mut items: Vec<(String, &Link)> = m
        .links
        .iter()
        .map(|l| (if !l.name.trim().is_empty() { l.name.clone() } else if !l.path_name.is_empty() { l.path_name.clone() } else { EMPTY.into() }, l))
        .collect();
    if items.is_empty() {
        return;
    }
    items.sort_by_key(|(n, _)| sort_key(n));
    out.push(BrowserNode { label: "Liens Revit".into(), depth: 0, kind: K::Links, id: -1, start: false });
    for (name, l) in items {
        let loaded = l.state.is_empty() || l.state.eq_ignore_ascii_case("Loaded");
        out.push(BrowserNode { label: name.clone(), depth: 1, kind: if loaded { K::Link } else { K::LinkUnloaded }, id: -1, start: false });
        let mut inst: Vec<String> = l.instances.iter().map(|i| if i.trim().is_empty() { name.clone() } else { format!("{name} : {i}") }).collect();
        inst.sort_by_key(|n| sort_key(n));
        for i in inst {
            out.push(BrowserNode { label: i, depth: 2, kind: K::LinkInstance, id: -1, start: false });
        }
    }
}


#[cfg(test)]
mod tests {
    #[test]
    fn annotation_categories_sorted() {
        assert!(super::ANNOTATION_CATEGORIES.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(super::category_label(-2008047), "Symboles d'annotation");
        assert_eq!(super::category_label(-2008055), "Accessoires de canalisation");
        assert_eq!(super::category_label(-2001043), "Equipement de restauration");
    }

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
    fn transmission_data_links() {
        let x = "<?xml version=\"1.0\"?><TransmissionData><ExternalFileReference><ElementId>42</ElementId><ExternalFileReferenceType>Revit Link</ExternalFileReferenceType><LastSavedPath>..\\B\\Lien A.rvt</LastSavedPath><LastSavedLoadState>NotFound</LastSavedLoadState></ExternalFileReference><ExternalFileReference><ElementId>7</ElementId><ExternalFileReferenceType>Keynote Table</ExternalFileReferenceType></ExternalFileReference></TransmissionData>";
        let mut b = (x.encode_utf16().count() as u32).to_le_bytes().to_vec();
        b.extend(x.encode_utf16().flat_map(|c| c.to_le_bytes()));
        assert_eq!(transmission_refs(&b).len(), 2);
        let mut m = Model { links: vec![Link { id: 42, name: "Lien A.rvt".into(), ..Default::default() }], link_instances: vec![(42, "1".into())], ..Default::default() };
        m.attach_links(&b);
        let mut out = Vec::new();
        links(&m, &mut out);
        assert_eq!(out.iter().map(|r| (r.label.as_str(), r.depth, r.kind)).collect::<Vec<_>>(), [("Liens Revit", 0, K::Links), ("Lien A.rvt", 1, K::LinkUnloaded), ("Lien A.rvt : 1", 2, K::LinkInstance)]);
        // no link: no section
        let mut out = Vec::new();
        links(&Model::default(), &mut out);
        assert!(out.is_empty());
    }

    /// Families, groups and links of the public samples (rst_2025 links the rac sample, not
    /// loaded when it was saved).
    #[test]
    fn sample_families_groups_links() {
        let Some(m) = model("rac_2018.rvt") else { return };
        let l = lines(&build(&m));
        let at = |s: &str| l.iter().position(|x| x == s).unwrap_or_else(|| panic!("{s} missing"));
        let f = at("Familles");
        assert!(at("  Installations de plomberie") > f);
        let shower = at("    M_Shower Stall-2D");
        assert_eq!(&l[shower + 1..shower + 3], ["      M_Shower Stall-2D", "      small"]);
        assert_eq!(l.iter().filter(|x| *x == "    M_Pile-Steel Pipe").count(), 1);
        let g = at("Groupes");
        assert_eq!(&l[g..g + 7], ["Groupes", "  Détail", "  Modèle", "    Array Group 1", "    Array Group 2", "    Array Group 3", "    Array Group 4"]);
        let Some(m) = model("rst_2025.rvt") else { return };
        let rows = build(&m);
        let k = rows.iter().position(|r| r.kind == K::Links).unwrap();
        assert_eq!(rows[k + 1].kind, K::LinkUnloaded);
        assert_eq!(rows[k + 2].label, format!("{} : 4", rows[k + 1].label));
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
