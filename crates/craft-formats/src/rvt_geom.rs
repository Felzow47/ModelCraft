//! Revit element objects decoded generically from the schema, with their owned pointers
//! (deferred contents) resolved into a tree. Base for reading the stored geometry (`GRep`,
//! `GPolyMesh`, `GBRep` faces and loops, curves, transforms).
//!
//! Clean-room, from byte observations only (see docs/rvt-rea-notes.md).

use crate::rvt_browser::{prim_size, Schema, Ty};

/// A decoded value.
#[derive(Debug, Clone)]
pub enum G {
    I(i64),
    F(f64),
    S(String),
    /// Doubles (fixed or variable arrays of `07`, compounds of doubles such as XYZ).
    Fs(Vec<f64>),
    /// Integers (arrays of `04`/`05`/`06`).
    Is(Vec<i64>),
    Ids(Vec<i64>),
    O(Obj),
    L(Vec<Obj>),
    /// Owned pointer: index into [`Tree::objs`] (resolved after the body), or `None` (null).
    P(Option<usize>),
    Ps(Vec<Option<usize>>),
    /// Raw pointer / back-reference.
    R(u32),
    Raw(Vec<u8>),
}

#[derive(Debug, Clone, Default)]
pub struct Obj {
    pub tag: u16,
    pub fields: Vec<(String, G)>,
}

impl Obj {
    pub fn get(&self, k: &str) -> Option<&G> {
        self.fields.iter().find(|(n, _)| n == k).map(|(_, v)| v)
    }
    pub fn int(&self, k: &str) -> Option<i64> {
        match self.get(k)? {
            G::I(v) => Some(*v),
            _ => None,
        }
    }
    pub fn float(&self, k: &str) -> Option<f64> {
        match self.get(k)? {
            G::F(v) => Some(*v),
            _ => None,
        }
    }
    pub fn fs(&self, k: &str) -> Option<&[f64]> {
        match self.get(k)? {
            G::Fs(v) => Some(v),
            _ => None,
        }
    }
    pub fn ptr(&self, k: &str) -> Option<usize> {
        match self.get(k)? {
            G::P(v) => *v,
            _ => None,
        }
    }
    pub fn ptrs(&self, k: &str) -> Vec<usize> {
        match self.get(k) {
            Some(G::Ps(v)) => v.iter().flatten().copied().collect(),
            Some(G::P(Some(v))) => vec![*v],
            _ => vec![],
        }
    }
    pub fn obj(&self, k: &str) -> Option<&Obj> {
        match self.get(k)? {
            G::O(o) => Some(o),
            _ => None,
        }
    }
    pub fn list(&self, k: &str) -> &[Obj] {
        match self.get(k) {
            Some(G::L(l)) => l,
            _ => &[],
        }
    }
}

/// An element and the objects its owned pointers lead to (in file order).
#[derive(Debug, Default)]
pub struct Tree {
    pub id: i64,
    pub root: Obj,
    pub objs: Vec<Obj>,
    /// Pending pointers that did not decode (tag), if any: the tree is then partial.
    pub failed: Option<u16>,
    pub start: usize,
    pub end: usize,
    /// Explicit local ids of the deferred objects (raw pointers `R(id)` refer to them).
    pub local: std::collections::HashMap<u32, usize>,
}

pub struct Dec<'a> {
    pub s: &'a Schema,
    pub d: &'a [u8],
    pub w: usize,
    pending: Vec<(u16, usize)>,
    /// Local object ids already queued (explicit-id pointers).
    seen: std::collections::HashMap<u32, usize>,
    pub max_objs: usize,
    pub debug: bool,
}

fn u16_at(d: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(p..p + 2)?.try_into().ok()?))
}
fn u32_at(d: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(p..p + 4)?.try_into().ok()?))
}

impl<'a> Dec<'a> {
    pub fn new(s: &'a Schema, d: &'a [u8], w: usize) -> Self {
        Dec { s, d, w, pending: Vec::new(), seen: Default::default(), max_objs: 200_000, debug: std::env::var_os("RVT_GEOM_DEBUG").is_some() }
    }

    fn id(&self, p: usize) -> Option<i64> {
        let b = self.d.get(p..p + self.w)?;
        Some(if self.w == 8 { i64::from_le_bytes(b.try_into().ok()?) } else { i32::from_le_bytes(b.try_into().ok()?) as i64 })
    }

    fn prim(&self, b0: u8, p: usize) -> Option<G> {
        let d = self.d;
        Some(match b0 {
            7 => G::F(f64::from_le_bytes(d.get(p..p + 8)?.try_into().ok()?)),
            6 | 0x0b => G::I(i64::from_le_bytes(d.get(p..p + 8)?.try_into().ok()?)),
            4 => G::I(i32::from_le_bytes(d.get(p..p + 4)?.try_into().ok()?) as i64),
            5 => G::I(u32_at(d, p)? as i64),
            3 => G::I(u16_at(d, p)? as i64),
            1 | 2 => G::I(*d.get(p)? as i64),
            9 => G::Raw(d.get(p..p + 16)?.to_vec()),
            _ => return None,
        })
    }

    /// A run of `n` primitives of type `b0` as doubles or integers.
    fn run(&self, b0: u8, p: usize, n: usize) -> Option<(G, usize)> {
        let sz = prim_size(b0)?;
        let e = p.checked_add(n.checked_mul(sz)?)?;
        let b = self.d.get(p..e)?;
        let g = match b0 {
            7 => G::Fs(b.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect()),
            4 => G::Is(b.chunks_exact(4).map(|c| i32::from_le_bytes(c.try_into().unwrap()) as i64).collect()),
            5 => G::Is(b.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap()) as i64).collect()),
            6 | 0x0b => G::Is(b.chunks_exact(8).map(|c| i64::from_le_bytes(c.try_into().unwrap())).collect()),
            3 => G::Is(b.chunks_exact(2).map(|c| u16::from_le_bytes(c.try_into().unwrap()) as i64).collect()),
            _ => G::Is(b.iter().map(|&c| c as i64).collect()),
        };
        Some((g, e))
    }

    pub fn fields(&mut self, tag: u16, mut p: usize, out: &mut Obj, depth: u32) -> Option<usize> {
        if depth > 32 {
            return None;
        }
        out.tag = tag;
        let s = self.s;
        for (name, ty) in s.chains.get(&tag)? {
            match self.val(*ty, p, depth) {
                Some((g, q)) => {
                    p = q;
                    if let Some(g) = g {
                        out.fields.push((name.clone(), g));
                    }
                }
                None => {
                    if self.debug {
                        eprintln!("  fail {}.{name} {:02x} {:02x} at {p:#x}", s.name(tag), ty.b0, ty.b1);
                    }
                    return None;
                }
            }
        }
        Some(p)
    }

    fn ptr(&mut self, p: usize) -> Option<(G, usize)> {
        let v = u32_at(self.d, p)?;
        if v == 0 {
            return Some((G::P(None), p + 4));
        }
        let t = u16_at(self.d, p + 4)?;
        if !self.s.chains.contains_key(&t) {
            return None;
        }
        if v == 0xffff_ffff {
            let idx = self.pending.len();
            self.pending.push((t, p));
            Some((G::P(Some(idx)), p + 6))
        } else if v < 0x100_0000 {
            // Explicit local id: the first mention queues the body (FIFO), later ones are refs.
            if let Some(&idx) = self.seen.get(&v) {
                return Some((G::P(Some(idx)), p + 6));
            }
            if v <= 2 {
                return Some((G::R(v), p + 6));
            }
            let idx = self.pending.len();
            self.seen.insert(v, idx);
            self.pending.push((t, p));
            Some((G::P(Some(idx)), p + 6))
        } else {
            None
        }
    }

    fn val(&mut self, ty: Ty, p: usize, depth: u32) -> Option<(Option<G>, usize)> {
        let d = self.d;
        if ty.b0 == 0x0e {
            return match ty.b1 {
                1 | 2 => self.ptr(p).map(|(g, q)| (Some(g), q)),
                3 => Some((Some(G::R(u32_at(d, p)?)), p + 4)),
                // fixed / variable arrays of raw pointers
                0x13 => {
                    let n = ty.n as usize;
                    let v: Option<Vec<i64>> = (0..n).map(|i| u32_at(d, p + 4 * i).map(|x| x as i64)).collect();
                    Some((Some(G::Is(v?)), p + 4 * n))
                }
                0x53 => {
                    let n = u32_at(d, p)? as usize;
                    if n > 2_000_000 {
                        return None;
                    }
                    let v: Option<Vec<i64>> = (0..n).map(|i| u32_at(d, p + 4 + 4 * i).map(|x| x as i64)).collect();
                    Some((Some(G::Is(v?)), p + 4 + 4 * n))
                }
                0x11 | 0x12 => {
                    let mut p = p;
                    let mut v = Vec::new();
                    for _ in 0..ty.n.min(4096) {
                        let (g, q) = self.ptr(p)?;
                        p = q;
                        v.push(match g {
                            G::P(x) => x,
                            _ => None,
                        });
                    }
                    Some((Some(G::Ps(v)), p))
                }
                0x10 => {
                    let mut p = p;
                    let mut l = Vec::new();
                    for _ in 0..ty.n.min(4096) {
                        if ty.tag == self.s.id_tag {
                            p += self.w;
                        } else {
                            let mut o = Obj::default();
                            p = self.fields(ty.tag, p, &mut o, depth + 1)?;
                            l.push(o);
                        }
                    }
                    Some((Some(G::L(l)), p))
                }
                0x51 | 0x52 => {
                    let n = u32_at(d, p)? as usize;
                    if n > 2_000_000 {
                        return None;
                    }
                    let mut p = p + 4;
                    let mut v = Vec::with_capacity(n.min(4096));
                    for _ in 0..n {
                        let (g, q) = self.ptr(p)?;
                        p = q;
                        v.push(match g {
                            G::P(x) => x,
                            _ => None,
                        });
                    }
                    Some((Some(G::Ps(v)), p))
                }
                0x50 => {
                    let n = u32_at(d, p)? as usize;
                    let p = p + 4;
                    if ty.tag == self.s.id_tag {
                        if n > 10_000_000 / self.w {
                            return None;
                        }
                        let v: Option<Vec<i64>> = (0..n).map(|i| self.id(p + i * self.w)).collect();
                        return Some((Some(G::Ids(v?)), p + n * self.w));
                    }
                    if n > 2_000_000 {
                        return None;
                    }
                    let mut p = p;
                    let mut l = Vec::with_capacity(n.min(4096));
                    for _ in 0..n {
                        let mut e = Obj::default();
                        p = self.fields(ty.tag, p, &mut e, depth + 1)?;
                        l.push(e);
                    }
                    Some((Some(G::L(l)), p))
                }
                0 => {
                    if ty.tag == self.s.id_tag {
                        return Some((Some(G::I(self.id(p)?)), p + self.w));
                    }
                    let mut o = Obj::default();
                    let p = self.fields(ty.tag, p, &mut o, depth + 1)?;
                    Some((Some(G::O(o)), p))
                }
                _ => None,
            };
        }
        if ty.b0 == 8 && ty.b1 == 0x60 {
            let n = u32_at(d, p)? as usize;
            if n > 1 << 20 {
                return None;
            }
            let b = d.get(p + 4..p + 4 + 2 * n)?;
            let u: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            return Some((Some(G::S(String::from_utf16_lossy(&u))), p + 4 + 2 * n));
        }
        // compounds (0d): `en` elements of `e0` each
        let (eb0, per) = if ty.b0 == 0x0d { (ty.e0, ty.en.max(1) as usize) } else { (ty.b0, 1) };
        if ty.b0 == 0x0d && eb0 == 0x0e && ty.tag == self.s.id_tag {
            // compounds of ids (id triples…): the ids in a row
            let (n, p) = match ty.b1 {
                0 => (per, p),
                0x10 => (per * ty.n as usize, p),
                0x50 => (per * u32_at(d, p)? as usize, p + 4),
                _ => return None,
            };
            if n > 10_000_000 / self.w {
                return None;
            }
            let v: Option<Vec<i64>> = (0..n).map(|i| self.id(p + i * self.w)).collect();
            return Some((Some(G::Ids(v?)), p + n * self.w));
        }
        prim_size(eb0)?;
        match ty.b1 {
            0 => {
                if per == 1 {
                    let g = self.prim(eb0, p)?;
                    Some((Some(g), p + prim_size(eb0)?))
                } else {
                    let (g, q) = self.run(eb0, p, per)?;
                    Some((Some(g), q))
                }
            }
            0x10 => {
                let (g, q) = self.run(eb0, p, per * ty.n as usize)?;
                Some((Some(g), q))
            }
            0x50 => {
                let n = u32_at(d, p)? as usize;
                if n > 50_000_000 {
                    return None;
                }
                let (g, q) = self.run(eb0, p + 4, per * n)?;
                Some((Some(g), q))
            }
            _ => None,
        }
    }

    /// Decodes an object of class `tag` at `p` with all its deferred contents (FIFO).
    pub fn tree(&mut self, tag: u16, p: usize) -> Option<Tree> {
        self.pending.clear();
        self.seen.clear();
        let mut t = Tree { start: p, ..Default::default() };
        let mut p = self.fields(tag, p, &mut t.root, 0)?;
        let mut i = 0;
        while i < self.pending.len() {
            if i >= self.max_objs {
                t.failed = Some(0);
                break;
            }
            let (tg, _) = self.pending[i];
            let mut o = Obj::default();
            match self.fields(tg, p, &mut o, 0) {
                Some(q) => {
                    p = q;
                    t.objs.push(o);
                }
                None => {
                    if self.debug {
                        eprintln!(" deferred #{i} {} failed at {p:#x}; pending: {:?}", self.s.name(tg), self.pending[i..].iter().take(40).map(|x| self.s.name(x.0)).collect::<Vec<_>>());
                    }
                    t.failed = Some(tg);
                    break;
                }
            }
            i += 1;
        }
        t.end = p;
        t.local = std::mem::take(&mut self.seen);
        t.id = t.root.int("m_id").unwrap_or(i64::MIN);
        Some(t)
    }
}

/// Element headers `[id][u32][u32 size][u16 tag]` of classes in `want`, with the body decoded
/// (`m_id` = header id). Returns (header offset, tag, size, tree).
pub fn elements(s: &Schema, d: &[u8], w: usize, want: &dyn Fn(u16) -> bool, mut f: impl FnMut(usize, u16, u32, Tree)) {
    elements_dbg(s, d, w, want, &mut f, &mut |_, _| {})
}

pub fn elements_dbg(s: &Schema, d: &[u8], w: usize, want: &dyn Fn(u16) -> bool, f: &mut dyn FnMut(usize, u16, u32, Tree), fail: &mut dyn FnMut(usize, u16)) {
    let mut dec = Dec::new(s, d, w);
    if d.len() < 16 + w {
        return;
    }
    let mut at = 8 + w;
    while at < d.len() - 6 {
        let t = u16::from_le_bytes([d[at], d[at + 1]]);
        if !want(t) {
            at += 1;
            continue;
        }
        let v = u32::from_le_bytes([d[at + 2], d[at + 3], d[at + 4], d[at + 5]]);
        if v != 0 && v != 0xffff_ffff {
            at += 1;
            continue;
        }
        let Some(eid) = dec.id(at - 8 - w) else {
            at += 1;
            continue;
        };
        let size = u32::from_le_bytes([d[at - 4], d[at - 3], d[at - 2], d[at - 1]]);
        if !(17..0x1000_0000).contains(&size) || eid <= 0 {
            at += 1;
            continue;
        }
        match dec.tree(t, at + 2) {
            Some(tr) if tr.id == eid => {
                let end = tr.end;
                f(at - 8 - w, t, size, tr);
                at = end.max(at + 1);
                continue;
            }
            None => fail(at - 8 - w, t),
            _ => {}
        }
        at += 1;
    }
}
