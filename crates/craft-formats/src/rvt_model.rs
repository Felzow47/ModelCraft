//! Revit model data (Revit 2018 to 2026, 32- and 64-bit element ids), first level: one axis-aligned box per placed element, by category.
//!
//! Written for ModelCraft from byte observations of Autodesk's sample files (no Autodesk code,
//! SDK or disassembly; the idea of reading element boxes comes from BIMCraft's notes, see
//! docs/REFERENCES.md and docs/rvt-rea-notes.md). What it relies on:
//!
//! - The container stores `Formats/Latest`, `Global/*` and `Partitions/*` in pages of 65,249
//!   bytes counted from byte 0 of the stream, of which the last 353 belong to no data
//!   ([`depage`]); the data is then a run of gzip members (RFC 1952), each checked by its
//!   CRC-32 and length ([`inflate_members`]).
//! - `Formats/Latest` defines the class `Outline` inline as `[tag | 0x8000] 00 00 [len] Outline`
//!   ([`schema_inline_tag`]).
//! - A partition starts with a chain of element records: id, `u32` size, `u16` constant,
//!   `u16` count, the body, and a trailer whose last `u32` repeats the size. Ids are `u64` from
//!   Revit 2024 (16-byte trailer) and `u32` before (12-byte trailer); [`walk_chain`] tries both.
//!   The constant is the same for every record of the file.
//! - An element frame (a record, or one nested in a record's body) holds the marker
//!   `[Outline tag u16] ff ff ff ff [tag u16]` (the second tag is the same in the whole file),
//!   followed by a box of six `f64` (feet: min xyz, max xyz). Before it: the placement kind
//!   (`u32`) 14 bytes back, `0xffffef7f` for a placed instance, and the `BuiltInCategory`, an
//!   `i64` 0x3e bytes back from 2024, an `i32` 0x26 bytes back before (ids are half as wide).
//!
//! Every read is bounds-checked and every buffer capped.

use std::io::Read;
use std::collections::HashMap;

pub const PAGE: usize = 65_249;
pub const PAGE_DATA: usize = 64_896;
/// Cap on the inflated bytes kept for one stream.
pub const MAX_INFLATED: usize = 1 << 30;
const FEET: f64 = 0.3048;
const PLACED: u32 = 0xffff_ef7f;
/// Revit's working area: 20 miles from the origin, a bit more for margin.
const MAX_FEET: f64 = 3.0e5;

/// Removes the 353-byte run at the end of every 65,249-byte page, in place.
pub fn depage(b: &mut Vec<u8>) {
    if b.len() <= PAGE_DATA {
        return;
    }
    let mut w = 0;
    let mut r = 0;
    while r < b.len() {
        let end = (r + PAGE_DATA).min(b.len());
        b.copy_within(r..end, w);
        w += end - r;
        r += PAGE;
    }
    b.truncate(w);
}

/// Length of the gzip header at `b[0..]` (RFC 1952), if it is one.
fn gzip_header_len(b: &[u8]) -> Option<usize> {
    if b.len() < 10 || b[0] != 0x1f || b[1] != 0x8b || b[2] != 8 {
        return None;
    }
    let flg = b[3];
    if flg & 0xe0 != 0 {
        return None;
    }
    let mut k = 10;
    if flg & 4 != 0 {
        let xl = u16::from_le_bytes([*b.get(k)?, *b.get(k + 1)?]) as usize;
        k += 2 + xl;
    }
    for bit in [8u8, 16] {
        if flg & bit != 0 {
            k += b.get(k..)?.iter().position(|&c| c == 0)? + 1;
        }
    }
    if flg & 2 != 0 {
        k += 2;
    }
    (k <= b.len()).then_some(k)
}

/// Inflates one member starting at its header; returns (output, bytes consumed incl. trailer,
/// trailer matched).
fn inflate_one(b: &[u8], cap: usize) -> Option<(Vec<u8>, usize, bool)> {
    let h = gzip_header_len(b)?;
    let mut d = flate2::Decompress::new(false);
    let mut out: Vec<u8> = Vec::new();
    let src = &b[h..];
    loop {
        if out.len() >= cap {
            return None;
        }
        out.reserve((256 << 10).min(cap - out.len()).max(1));
        let before = d.total_in();
        let st = d.decompress_vec(&src[d.total_in() as usize..], &mut out, flate2::FlushDecompress::None).ok()?;
        match st {
            flate2::Status::StreamEnd => break,
            _ if d.total_in() == before && out.len() < out.capacity() => return None,
            _ => {}
        }
    }
    let end = h + d.total_in() as usize;
    let ok = b.get(end..end + 8).is_some_and(|t| {
        let crc = u32::from_le_bytes([t[0], t[1], t[2], t[3]]);
        let len = u32::from_le_bytes([t[4], t[5], t[6], t[7]]);
        let mut c = flate2::Crc::new();
        c.update(&out);
        c.sum() == crc && len == out.len() as u32
    });
    Some((out, end + if ok { 8 } else { 0 }, ok))
}

/// Result of inflating a stream: the joined data and how many members verified.
#[derive(Default)]
pub struct Inflated {
    pub data: Vec<u8>,
    pub members: usize,
    pub verified: usize,
}

/// Inflates the gzip members of `b` one after the other, keeping only those whose trailer
/// matches. `keep_going` sees the data so far after each member and can stop early.
pub fn inflate_members(b: &[u8], cap: usize, mut keep_going: impl FnMut(&[u8]) -> bool) -> Inflated {
    let mut r = Inflated::default();
    let mut i = 0;
    while let Some(j) = find(b, &[0x1f, 0x8b, 8], i) {
        match inflate_one(&b[j..], cap.saturating_sub(r.data.len())) {
            Some((out, used, true)) => {
                r.members += 1;
                r.verified += 1;
                r.data.extend_from_slice(&out);
                i = j + used;
                if r.data.len() >= cap || !keep_going(&r.data) {
                    break;
                }
            }
            _ => i = j + 1,
        }
    }
    r
}

fn find(h: &[u8], n: &[u8], from: usize) -> Option<usize> {
    h.get(from..)?.windows(n.len()).position(|w| w == n).map(|p| p + from)
}

/// Reads a stream, depages it when that makes its gzip members check, and inflates them.
pub fn read_stream<F: Read + std::io::Seek>(
    cf: &mut cfb::CompoundFile<F>,
    name: &str,
    max_stored: u64,
    cap: usize,
    keep_going: impl FnMut(&[u8]) -> bool,
) -> Option<Inflated> {
    let len = cf.entry(name).ok()?.len();
    if len > max_stored {
        return None;
    }
    let mut raw = Vec::new();
    raw.try_reserve_exact(len as usize).ok()?;
    cf.open_stream(name).ok()?.read_to_end(&mut raw).ok()?;
    if raw.len() <= PAGE_DATA {
        return Some(inflate_members(&raw, cap, keep_going));
    }
    // probe the first members both ways (a stream that is not paged loses data when depaged)
    let probe = |b: &[u8]| inflate_members(&b[..b.len().min(3 * PAGE)], cap, |_| true).verified;
    let mut paged = raw.clone();
    depage(&mut paged);
    if probe(&paged) >= probe(&raw) {
        drop(raw);
        Some(inflate_members(&paged, cap, keep_going))
    } else {
        Some(inflate_members(&raw, cap, keep_going))
    }
}

/// Tag of a class defined inline in `Formats/Latest`: `[tag | 0x8000] 00 00 [len u16] name`.
pub fn schema_inline_tag(schema: &[u8], name: &str) -> Option<u16> {
    let mut pat = (name.len() as u16).to_le_bytes().to_vec();
    pat.extend_from_slice(name.as_bytes());
    let mut i = 0;
    while let Some(j) = find(schema, &pat, i) {
        i = j + 1;
        // the name must end there (not a prefix of a longer name)
        if schema.get(j + pat.len()).is_some_and(|c| c.is_ascii_alphanumeric()) || j < 4 {
            continue;
        }
        let t = u16::from_le_bytes([schema[j - 4], schema[j - 3]]);
        if t & 0x8000 != 0 && schema[j - 2] == 0 && schema[j - 1] == 0 {
            return Some(t & 0x7fff);
        }
    }
    None
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}
fn f64_at(b: &[u8], o: usize) -> Option<f64> {
    Some(f64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// A `keep_going` for [`inflate_members`] that stops once the record chain is over (the rest
/// of a partition holds the loaded families' own documents, not needed for the boxes).
pub fn until_chain_end() -> impl FnMut(&[u8]) -> bool {
    let mut o = 0usize;
    let mut k: Option<u16> = None;
    let mut w: Option<usize> = None;
    move |p: &[u8]| loop {
        let width = match w {
            Some(x) => x,
            None => match id_width(p) {
                Ok(x) => x,
                Err(more) => return more,
            },
        };
        w = Some(width);
        let (Some(size), Some(c)) = (u32_at(p, o + width), u16_at(p, o + width + 4)) else { return true };
        let size = size as usize;
        if size < 16 || k.is_some_and(|x| x != c) {
            return false;
        }
        let Some(tr) = u32_at(p, o + size + width + 4) else { return true };
        if tr as usize != size {
            return false;
        }
        k = Some(c);
        o += size + width + 8;
    }
}

/// Width of the element ids (8 from Revit 2024, 4 before), from the first record's trailer.
/// `Err(true)`: not enough data yet; `Err(false)`: no chain.
fn id_width(p: &[u8]) -> Result<usize, bool> {
    let mut short = false;
    for w in [8usize, 4] {
        let Some(size) = u32_at(p, w) else {
            short = true;
            continue;
        };
        let size = size as usize;
        if size < 16 {
            continue;
        }
        match u32_at(p, size + w + 4) {
            Some(t) if t as usize == size => return Ok(w),
            None => short = true,
            _ => {}
        }
    }
    Err(short)
}

/// The record chain at the start of a partition.
#[derive(Debug, Default, Clone)]
pub struct Chain {
    /// (offset, element id) of each record.
    pub records: Vec<(usize, u64)>,
    /// End of the last whole record.
    pub end: usize,
    /// The constant of the records (all equal), if any.
    pub constant: Option<u16>,
    /// A header or trailer did not fit: the chain is over (rather than cut by the data so far).
    pub closed: bool,
    /// Width of the element ids: 8 (Revit 2024 and later) or 4.
    pub id_width: usize,
}

/// Walks the chain from byte 0; stops at the first header or trailer that does not fit.
pub fn walk_chain(p: &[u8]) -> Chain {
    let mut c = Chain::default();
    let Ok(w) = id_width(p) else { return c };
    c.id_width = w;
    let mut o = 0usize;
    loop {
        let id = if w == 8 { u64_at(p, o) } else { u32_at(p, o).map(u64::from) };
        let (Some(id), Some(size), Some(k)) = (id, u32_at(p, o + w), u16_at(p, o + w + 4)) else { break };
        let size = size as usize;
        if size < 16 || c.constant.is_some_and(|x| x != k) {
            c.closed = true;
            break;
        }
        let Some(tr) = u32_at(p, o + size + w + 4) else { break };
        if tr as usize != size {
            c.closed = true;
            break;
        }
        c.constant.get_or_insert(k);
        c.records.push((o, id));
        o += size + w + 8;
    }
    c.end = o;
    c
}

/// One placed element's box, in metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElemBox {
    /// Id of the element record holding the box (0: unknown).
    pub id: i64,
    pub category: i64,
    pub min: [f64; 3],
    pub max: [f64; 3],
}

/// Statistics of a scan, for the info panel and tests.
#[derive(Debug, Default, Clone)]
pub struct Scan {
    pub records: usize,
    pub frames: usize,
    pub boxes: Vec<ElemBox>,
}

/// Finds every element frame (top-level or nested) in `p[..chain.end]` by its marker and keeps
/// the placed ones with a box that is finite, ordered, within Revit's area and not empty in
/// two or more directions. Duplicates (same category and box) are dropped.
pub fn scan_boxes(p: &[u8], chain: &Chain, outline: u16, max_boxes: usize) -> Scan {
    let mut s = Scan { records: chain.records.len(), ..Default::default() };
    if chain.constant.is_none() {
        return s;
    }
    let mut mk = outline.to_le_bytes().to_vec();
    mk.extend_from_slice(&[0xff; 4]);
    let region = &p[..chain.end.min(p.len())];
    // the tag after the marker: the most frequent one (it is not tied to the constant before 2024)
    let mut tags: std::collections::HashMap<u16, usize> = Default::default();
    let mut i = 0;
    while let Some(j) = find(region, &mk, i) {
        i = j + 1;
        if let Some(t) = u16_at(region, j + 6) {
            *tags.entry(t).or_default() += 1;
        }
    }
    let Some(tag) = tags.into_iter().max_by_key(|&(t, n)| (n, t)).map(|(t, _)| t) else { return s };
    mk.extend_from_slice(&tag.to_le_bytes());
    let (cat_back, wide) = if chain.id_width == 4 { (0x26, false) } else { (0x3e, true) };
    let mut seen = std::collections::HashSet::new();
    let mut i = 0;
    while let Some(j) = find(region, &mk, i) {
        i = j + 1;
        let (Some(c), Some(k)) = (j.checked_sub(cat_back), j.checked_sub(0x0e)) else { continue };
        s.frames += 1;
        let cat = if wide { u64_at(region, c).map(|x| x as i64) } else { u32_at(region, c).map(|x| x as i32 as i64) };
        let (Some(cat), Some(kind)) = (cat, u32_at(region, k)) else { continue };
        if kind != PLACED || !(cat == -1 || (-2_100_000..-1_000_000).contains(&cat)) {
            continue;
        }
        let mut v = [0f64; 6];
        let mut ok = true;
        for (n, x) in v.iter_mut().enumerate() {
            match f64_at(region, j + 8 + 8 * n) {
                Some(y) if y.is_finite() && y.abs() < MAX_FEET => *x = y,
                _ => ok = false,
            }
        }
        if !ok || (0..3).any(|a| v[a] > v[a + 3]) || (0..3).filter(|&a| v[a + 3] - v[a] > 1e-4).count() < 2 {
            continue;
        }
        if !seen.insert((cat, v.map(f64::to_bits))) {
            continue;
        }
        // the record holding the frame: the last one starting before it
        let id = match chain.records.partition_point(|r| r.0 <= j) {
            0 => 0,
            k => chain.records[k - 1].1 as i64,
        };
        s.boxes.push(ElemBox { id, category: cat, min: [v[0] * FEET, v[1] * FEET, v[2] * FEET], max: [v[3] * FEET, v[4] * FEET, v[5] * FEET] });
        if s.boxes.len() >= max_boxes {
            break;
        }
    }
    s
}

/// Name and display colour of the `BuiltInCategory` values shown in 3D (Autodesk's published
/// API constants). `None`: a category with no physical body (lines, annotations, rooms…).
/// In a family document (`.rfa`, `.rft`), the family's own solids carry the category -1.
pub fn category_in(cat: i64, family: bool) -> Option<(&'static str, [f32; 4])> {
    if family && cat == -1 {
        return Some(("Formes de la famille", [0.75, 0.72, 0.68, 1.0]));
    }
    category(cat)
}

pub fn category(cat: i64) -> Option<(&'static str, [f32; 4])> {
    let c = |r: u8, g: u8, b: u8, a: f32| [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a];
    Some(match cat {
        -2000011 => ("Murs", c(0xd8, 0xd4, 0xcc, 1.0)),
        -2000032 => ("Sols", c(0xa8, 0xa0, 0x94, 1.0)),
        -2000035 => ("Toits", c(0xa0, 0x4a, 0x3a, 1.0)),
        -2000038 => ("Plafonds", c(0xee, 0xee, 0xe8, 1.0)),
        -2000023 => ("Portes", c(0x9a, 0x6b, 0x43, 1.0)),
        -2000014 => ("Fenêtres", c(0x8c, 0xc8, 0xe8, 0.6)),
        -2000100 => ("Poteaux", c(0xb8, 0xb8, 0xb8, 1.0)),
        -2000080 => ("Mobilier", c(0xc9, 0x8b, 0x4f, 1.0)),
        -2001100 => ("Systèmes de mobilier", c(0xc9, 0x8b, 0x4f, 1.0)),
        -2001000 => ("Meubles fixes", c(0xb0, 0x7a, 0x48, 1.0)),
        -2000151 => ("Modèles génériques", c(0x9e, 0xa7, 0xb5, 1.0)),
        -2000120 => ("Escaliers", c(0xb5, 0xa9, 0x95, 1.0)),
        -2000126 => ("Garde-corps", c(0x70, 0x70, 0x78, 1.0)),
        -2000180 => ("Rampes", c(0xb5, 0xa9, 0x95, 1.0)),
        -2000170 => ("Panneaux de mur-rideau", c(0x8c, 0xc8, 0xe8, 0.45)),
        -2000171 => ("Meneaux", c(0x60, 0x66, 0x70, 1.0)),
        -2003400 => ("Volumes", c(0xd9, 0xb3, 0xe6, 0.5)),
        -2001340 => ("Topographie", c(0x8f, 0xb0, 0x6a, 1.0)),
        -2001260 => ("Site", c(0x8f, 0xb0, 0x6a, 1.0)),
        -2001360 => ("Plantes", c(0x5f, 0x9e, 0x4a, 1.0)),
        -2001370 => ("Entourage", c(0x9e, 0xa7, 0xb5, 1.0)),
        -2001180 => ("Parkings", c(0x9e, 0xa7, 0xb5, 1.0)),
        -2001330 => ("Poteaux porteurs", c(0x8a, 0x8f, 0x99, 1.0)),
        -2001320 => ("Ossature", c(0x8a, 0x8f, 0x99, 1.0)),
        -2001300 => ("Fondations", c(0x99, 0x99, 0x99, 1.0)),
        -2001140 => ("Équipements mécaniques", c(0x6a, 0x9f, 0xd4, 1.0)),
        -2001160 => ("Appareils sanitaires", c(0xf2, 0xf2, 0xf2, 1.0)),
        -2001120 => ("Luminaires", c(0xf5, 0xd7, 0x6e, 1.0)),
        -2001040 => ("Équipements électriques", c(0xe0, 0xa0, 0x40, 1.0)),
        -2001060 => ("Installations électriques", c(0xe0, 0xa0, 0x40, 1.0)),
        -2001350 => ("Équipements spécialisés", c(0x9e, 0xa7, 0xb5, 1.0)),
        -2008000 => ("Gaines", c(0x6a, 0x9f, 0xd4, 1.0)),
        -2008010 => ("Raccords de gaine", c(0x6a, 0x9f, 0xd4, 1.0)),
        -2008044 => ("Canalisations", c(0x4f, 0xa8, 0x6b, 1.0)),
        -2008049 => ("Raccords de canalisation", c(0x4f, 0xa8, 0x6b, 1.0)),
        -2008130 => ("Chemins de câbles", c(0xe0, 0xa0, 0x40, 1.0)),
        -2008132 => ("Conduits", c(0xe0, 0xa0, 0x40, 1.0)),
        -2009000 => ("Armatures", c(0x80, 0x50, 0x40, 1.0)),
        -2009016 => ("Armatures", c(0x80, 0x50, 0x40, 1.0)),
        -2009060 => ("Armatures", c(0x80, 0x50, 0x40, 1.0)),
        -2000175 | -2000127 | -2000948 => ("Garde-corps", c(0x70, 0x70, 0x78, 1.0)),
        -2000946 | -2000947 => ("Mains courantes", c(0x70, 0x70, 0x78, 1.0)),
        -2000919 | -2000920 | -2000123 | -2000952 => ("Escaliers", c(0xb5, 0xa9, 0x95, 1.0)),
        -2000181 => ("Profilés de mur", c(0xc8, 0xc4, 0xbc, 1.0)),
        -2001390 => ("Bordures de toit", c(0xa0, 0x4a, 0x3a, 1.0)),
        -2001391 => ("Gouttières", c(0x80, 0x80, 0x88, 1.0)),
        -2001393 => ("Sous-faces de toit", c(0xa0, 0x4a, 0x3a, 1.0)),
        -2001392 => ("Bordures de dalle", c(0xa8, 0xa0, 0x94, 1.0)),
        -2000269 => ("Pièces", c(0xb0, 0xb0, 0xb0, 1.0)),
        -2001336 | -2009600 => ("Fermes", c(0x8a, 0x8f, 0x99, 1.0)),
        -2001354 => ("Raidisseurs", c(0x8a, 0x8f, 0x99, 1.0)),
        -2009030 => ("Assemblages structurels", c(0x6f, 0x74, 0x7e, 1.0)),
        -2001079 | -2001263 => ("Topographie", c(0x8f, 0xb0, 0x6a, 1.0)),
        -2001036 => ("Aménagements extérieurs", c(0xa8, 0xa0, 0x94, 1.0)),
        -2001220 => ("Routes", c(0x70, 0x70, 0x70, 1.0)),
        -2001046 => ("Équipements médicaux", c(0x9e, 0xa7, 0xb5, 1.0)),
        -2001043 => ("Équipements de restauration", c(0x9e, 0xa7, 0xb5, 1.0)),
        -2001058 => ("Signalétique", c(0xe0, 0x60, 0x40, 1.0)),
        -2001049 => ("Protection incendie", c(0xd0, 0x40, 0x40, 1.0)),
        -2008099 => ("Sprinkleurs", c(0xd0, 0x40, 0x40, 1.0)),
        -2008234 => ("Équipements de plomberie", c(0x4f, 0xa8, 0x6b, 1.0)),
        -2008232 => ("Commandes mécaniques", c(0x6a, 0x9f, 0xd4, 1.0)),
        -2008013 => ("Bouches d'aération", c(0x6a, 0x9f, 0xd4, 1.0)),
        -2008016 => ("Accessoires de gaine", c(0x6a, 0x9f, 0xd4, 1.0)),
        -2008020 => ("Gaines souples", c(0x6a, 0x9f, 0xd4, 1.0)),
        -2008050 => ("Canalisations souples", c(0x4f, 0xa8, 0x6b, 1.0)),
        -2008055 => ("Accessoires de canalisation", c(0x4f, 0xa8, 0x6b, 1.0)),
        -2008126 => ("Raccords de chemin de câbles", c(0xe0, 0xa0, 0x40, 1.0)),
        -2008128 => ("Raccords de conduit", c(0xe0, 0xa0, 0x40, 1.0)),
        -2008075 | -2008077 | -2008079 | -2008081 | -2008083 | -2008085 | -2008087 | -2001055 => ("Dispositifs électriques", c(0xe0, 0xa0, 0x40, 1.0)),
        _ => return None,
    })
}

/// Box of one element as 12 triangles appended to `pos` / `tri`. A direction with no extent
/// gets 1 cm so a plate stays visible.
/// Corners of the box as drawn: a side thinner than 1 cm is widened to 1 cm.
fn drawn(b: &ElemBox) -> ([f64; 3], [f64; 3]) {
    let mut lo = b.min;
    let mut hi = b.max;
    for a in 0..3 {
        if hi[a] - lo[a] < 0.01 {
            let m = (hi[a] + lo[a]) * 0.5;
            lo[a] = m - 0.005;
            hi[a] = m + 0.005;
        }
    }
    (lo, hi)
}

/// Origin (min corner) and size of the box as drawn, for a unit cube placed by a matrix.
pub fn box_extent(b: &ElemBox) -> ([f32; 3], [f32; 3]) {
    let (lo, hi) = drawn(b);
    (lo.map(|v| v as f32), std::array::from_fn(|a| (hi[a] - lo[a]) as f32))
}

pub fn push_box(b: &ElemBox, pos: &mut Vec<[f32; 3]>, tri: &mut Vec<[u32; 3]>) {
    let (lo, hi) = drawn(b);
    let base = pos.len() as u32;
    for k in 0..8u32 {
        pos.push([
            if k & 1 != 0 { hi[0] } else { lo[0] } as f32,
            if k & 2 != 0 { hi[1] } else { lo[1] } as f32,
            if k & 4 != 0 { hi[2] } else { lo[2] } as f32,
        ]);
    }
    // outward-facing (counter-clockwise seen from outside)
    const F: [[u32; 4]; 6] = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    for q in F {
        tri.push([base + q[0], base + q[1], base + q[2]]);
        tri.push([base + q[0], base + q[2], base + q[3]]);
    }
}

/// Worksets of a workshared project (`/Global/PartitionTable`): GUID → partition id. Each
/// entry is `[u32] guid id created modified dependent type deleted name`; user worksets are
/// type 0.
pub fn worksets(p: &[u8]) -> HashMap<[u8; 16], u32> {
    let u = |i: usize| u32::from_le_bytes(p[i..i + 4].try_into().unwrap());
    let mut out = HashMap::new();
    let mut i = 6;
    while i + 45 <= p.len() {
        let len = u(i + 41) as usize;
        if len > 4096 || i + 45 + 2 * len > p.len() {
            break;
        }
        out.insert(p[i + 4..i + 20].try_into().unwrap(), u(i + 20));
        i += 45 + 2 * len;
    }
    out
}

/// Workset of each element (`/Global/ElemTable`): element id → partition id. Records of 40
/// bytes after a 6-byte header: an id (i64), three u32, the element id (i64), a parent id,
/// partition (u32).
/// Empty when the table does not read as such (another layout).
pub fn elem_worksets(e: &[u8], known: &HashMap<[u8; 16], u32>) -> HashMap<i64, u32> {
    let ids: std::collections::HashSet<u32> = known.values().copied().collect();
    let mut out = HashMap::new();
    let (mut i, mut miss) = (6, 0);
    while i + 40 <= e.len() {
        let id = i64::from_le_bytes(e[i + 20..i + 28].try_into().unwrap());
        let ws = u32::from_le_bytes(e[i + 36..i + 40].try_into().unwrap());
        if !ids.contains(&ws) {
            miss += 1;
        }
        out.insert(id, ws);
        i += 40;
    }
    if out.is_empty() || miss * 20 > out.len() {
        return HashMap::new();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worksets_and_elements() {
        // partition table: header, then [u32] guid id created modified dependent type deleted name
        let mut p = vec![0x40, 0x0c, 1, 0, 0, 0];
        for (g, id, name) in [(7u8, 0u32, "A"), (9, 681, "MF_Fire")] {
            p.extend(1u32.to_le_bytes());
            p.extend([g; 16]);
            for v in [id, 1, 1, u32::MAX, 0] {
                p.extend(v.to_le_bytes());
            }
            p.push(0);
            p.extend((name.len() as u32).to_le_bytes());
            p.extend(name.encode_utf16().flat_map(|c| c.to_le_bytes()));
        }
        let ws = worksets(&p);
        assert_eq!((ws.get(&[7; 16]), ws.get(&[9; 16])), (Some(&0), Some(&681)));
        // element table: the element id is the second one of each record
        let mut e = vec![0xab, 0x05, 0xa4, 0x4e, 2, 0];
        for (first, id, w) in [(1i64, 1i64, 0u32), (5, 1576636, 681)] {
            e.extend(first.to_le_bytes());
            e.extend([0u8; 12]);
            e.extend(id.to_le_bytes());
            e.extend((-1i64).to_le_bytes());
            e.extend(w.to_le_bytes());
        }
        let el = elem_worksets(&e, &ws);
        assert_eq!((el.get(&1576636), el.get(&1), el.get(&5)), (Some(&681), Some(&0), None));
        // another layout: nothing rather than wrong worksets
        assert!(elem_worksets(&[0xff; 6 + 40 * 4], &ws).is_empty());
    }

    #[test]
    fn depage_strips_runs() {
        let mut b: Vec<u8> = (0..PAGE * 2 + 10).map(|i| if i % PAGE >= PAGE_DATA { 0xEE } else { 1 }).collect();
        depage(&mut b);
        assert_eq!(b.len(), PAGE_DATA * 2 + 10);
        assert!(b.iter().all(|&c| c == 1));
    }

    fn gz(data: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn members_checked_and_joined() {
        let mut b = vec![0u8; 8];
        b.extend(gz(b"hello "));
        b.extend([9u8; 40]);
        b.extend(gz(b"world"));
        let r = inflate_members(&b, 1 << 20, |_| true);
        assert_eq!(r.data, b"hello world");
        assert_eq!(r.verified, 2);
    }

    #[test]
    fn schema_tag_inline() {
        let mut s = vec![0u8, 0, 7, 0];
        s.extend(b"Outline2d\0");
        s.extend([0x59, 0x81, 0, 0, 7, 0]);
        s.extend(b"Outline\0\0");
        assert_eq!(schema_inline_tag(&s, "Outline"), Some(0x159));
    }

    /// A synthetic partition: one record with a placed box, one with a type (no box).
    #[test]
    fn chain_and_boxes() {
        let rec = |id: u64, placed: bool, cat: i64, bb: [f64; 6]| {
            let mut r = vec![0u8; 0x58 + 48 + 8];
            let size = r.len() as u32;
            r[0..8].copy_from_slice(&id.to_le_bytes());
            r[8..12].copy_from_slice(&size.to_le_bytes());
            r[12..14].copy_from_slice(&0x5c7u16.to_le_bytes());
            r[0x12..0x1a].copy_from_slice(&cat.to_le_bytes());
            r[0x42..0x46].copy_from_slice(&(if placed { PLACED } else { 0xffff8080 }).to_le_bytes());
            r[0x50..0x52].copy_from_slice(&0x159u16.to_le_bytes());
            r[0x52..0x56].copy_from_slice(&[0xff; 4]);
            r[0x56..0x58].copy_from_slice(&0x5d3u16.to_le_bytes());
            for (k, v) in bb.iter().enumerate() {
                r[0x58 + 8 * k..0x60 + 8 * k].copy_from_slice(&v.to_le_bytes());
            }
            let mut t = vec![0u8; 16];
            t[12..16].copy_from_slice(&size.to_le_bytes());
            r.extend(t);
            r
        };
        let mut p = rec(1, true, -2000011, [0.0, 0.0, 0.0, 10.0, 1.0, 9.0]);
        p.extend(rec(2, false, -2000011, [0.0, 0.0, 0.0, 1.0, 1.0, 1.0]));
        p.extend([0xAB; 64]);
        let c = walk_chain(&p);
        assert_eq!(c.records.len(), 2);
        assert!(c.closed);
        let s = scan_boxes(&p, &c, 0x159, 100);
        assert_eq!(s.boxes.len(), 1);
        assert!((s.boxes[0].max[0] - 3.048).abs() < 1e-9);
        let (mut pos, mut tri) = (Vec::new(), Vec::new());
        push_box(&s.boxes[0], &mut pos, &mut tri);
        assert_eq!((pos.len(), tri.len()), (8, 12));
    }

    /// Same with 32-bit ids (Revit 2023 and earlier): 12-byte header and trailer, `i32` category.
    #[test]
    fn chain_and_boxes_32() {
        let rec = |id: u32, placed: bool, cat: i32, bb: [f64; 6]| {
            let m = 0x40;
            let mut r = vec![0u8; m + 8 + 48 + 4];
            let size = r.len() as u32;
            r[0..4].copy_from_slice(&id.to_le_bytes());
            r[4..8].copy_from_slice(&size.to_le_bytes());
            r[8..10].copy_from_slice(&0x4ebu16.to_le_bytes());
            r[m - 0x26..m - 0x22].copy_from_slice(&cat.to_le_bytes());
            r[m - 0x0e..m - 0x0a].copy_from_slice(&(if placed { PLACED } else { 0xffff8080 }).to_le_bytes());
            r[m..m + 2].copy_from_slice(&0x109u16.to_le_bytes());
            r[m + 2..m + 6].copy_from_slice(&[0xff; 4]);
            r[m + 6..m + 8].copy_from_slice(&0x4f4u16.to_le_bytes());
            for (k, v) in bb.iter().enumerate() {
                r[m + 8 + 8 * k..m + 16 + 8 * k].copy_from_slice(&v.to_le_bytes());
            }
            let mut t = vec![0u8; 12];
            t[8..12].copy_from_slice(&size.to_le_bytes());
            r.extend(t);
            r
        };
        let mut p = rec(7, true, -2000032, [0.0, 0.0, 0.0, 10.0, 5.0, 1.0]);
        p.extend(rec(8, true, -2000011, [1.0, 1.0, 0.0, 2.0, 1.0, 9.0]));
        p.extend(rec(9, false, -2000011, [0.0, 0.0, 0.0, 1.0, 1.0, 1.0]));
        p.extend([0xAB; 64]);
        let c = walk_chain(&p);
        assert_eq!((c.records.len(), c.id_width, c.records[1].1), (3, 4, 8));
        let s = scan_boxes(&p, &c, 0x109, 100);
        assert_eq!(s.boxes.iter().map(|b| b.category).collect::<Vec<_>>(), [-2000032, -2000011]);
        let mut stop = until_chain_end();
        assert!(!stop(&p));
    }
}
