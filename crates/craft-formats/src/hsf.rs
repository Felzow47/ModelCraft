//! HOOPS Stream Format (HSF) reader, written from Tech Soft 3D's public format documentation
//! (opcode list, per-opcode operand layouts, shell and polyhedron attribute pages). No HOOPS code
//! or binaries are used.
//!
//! The stream is a sequence of one-byte opcodes, each followed by its operands (little endian).
//! Only what is needed to rebuild the shaded model is kept: the segment tree, modelling
//! matrices, includes, face visibility and face colour, and the triangle meshes (`TKE_Shell`).
//! Everything else is parsed only to step over it.
//!
//! eDrawings writes shells in two passes. The first pass stores empty ("null") shells, each
//! tagged by a following `TKE_Tag_Implicit` with the next sequential index. After a
//! `TKE_Pause` come refinements (`TKE_Shell` without `FIRSTPASS`) that carry the index of the
//! null shell they fill in, and `TKE_Repeat_Object` records that fill a null shell with a
//! transformed copy of another one.

use std::collections::HashMap;

/// One HOOPS segment. Paths are lowercase and absolute (`/include library/shared/body12`).
#[derive(Debug, Default, Clone)]
pub struct Segment {
    pub path: String,
    pub children: Vec<usize>,
    /// Shell indices (into [`Model::shells`]) inserted in this segment.
    pub shells: Vec<usize>,
    /// Included segment paths (`TKE_Include_Segment`), and whether the include is conditional.
    pub includes: Vec<(String, bool)>,
    /// Row-vector 4x3 modelling matrix (`p' = p * M`, translation in the last row).
    pub matrix: Option<[f32; 12]>,
    /// Local face visibility setting.
    pub face_visible: Option<bool>,
    /// Local diffuse face colour (0..1).
    pub face_color: Option<[f32; 3]>,
    /// Text strings inserted in this segment.
    pub texts: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub struct Shell {
    pub points: Vec<[f32; 3]>,
    pub tris: Vec<[u32; 3]>,
}

#[derive(Debug, Default)]
pub struct Stats {
    pub opcodes: usize,
    pub null_shells: usize,
    pub refinements: usize,
    pub first_pass_shells: usize,
    pub repeats: usize,
    pub skipped_lod: usize,
    pub unsupported_shells: usize,
}

#[derive(Debug, Default)]
pub struct Model {
    pub segments: Vec<Segment>,
    pub by_path: HashMap<String, usize>,
    pub shells: Vec<Option<Shell>>,
    pub stats: Stats,
    /// Set when the stream could not be read to its end (unknown opcode, truncated data…).
    pub error: Option<String>,
    /// Offsets and opcodes of the last few records read (diagnostics).
    pub recent: Vec<(usize, u8)>,
}

impl Model {
    pub fn segment(&self, path: &str) -> Option<&Segment> {
        self.by_path.get(path).map(|&i| &self.segments[i])
    }

    fn seg(&mut self, path: &str) -> usize {
        if let Some(&i) = self.by_path.get(path) {
            return i;
        }
        let i = self.segments.len();
        self.segments.push(Segment { path: path.to_string(), ..Default::default() });
        self.by_path.insert(path.to_string(), i);
        if path != "/" {
            let parent = match path.rfind('/') {
                Some(0) | None => "/".to_string(),
                Some(k) => path[..k].to_string(),
            };
            let p = self.seg(&parent);
            self.segments[p].children.push(i);
        }
        i
    }
}

/// Normalise a segment name to an absolute lowercase path, relative to `cur`.
fn resolve(cur: &str, name: &str) -> String {
    let mut n = name.trim().to_lowercase();
    if let Some(rest) = n.strip_prefix('?') {
        // aliases: ?Include Library, ?Style Library… live at the root
        n = format!("/{rest}");
    }
    let mut out: Vec<&str> = if n.starts_with('/') { Vec::new() } else { cur.split('/').filter(|s| !s.is_empty()).collect() };
    for part in n.split('/').filter(|s| !s.is_empty()) {
        match part {
            "." => {}
            ".." => {
                out.pop();
            }
            p => out.push(p),
        }
    }
    format!("/{}", out.join("/"))
}

struct R<'a> {
    d: &'a [u8],
    p: usize,
}

type Res<T> = Result<T, String>;

impl<'a> R<'a> {
    fn eof(&self, n: usize) -> String {
        format!("données tronquées à l'octet {} (besoin de {n})", self.p)
    }
    fn take(&mut self, n: usize) -> Res<&'a [u8]> {
        let e = self.p.checked_add(n).filter(|&e| e <= self.d.len()).ok_or_else(|| self.eof(n))?;
        let s = &self.d[self.p..e];
        self.p = e;
        Ok(s)
    }
    fn skip(&mut self, n: usize) -> Res<()> {
        self.take(n).map(|_| ())
    }
    fn u8(&mut self) -> Res<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Res<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn i32(&mut self) -> Res<i32> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn len(&mut self) -> Res<usize> {
        let v = self.i32()?;
        usize::try_from(v).map_err(|_| format!("longueur négative à l'octet {}", self.p))
    }
    fn f32(&mut self) -> Res<f32> {
        let b = self.take(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn floats(&mut self, n: usize) -> Res<Vec<f32>> {
        let b = self.take(n.checked_mul(4).ok_or_else(|| self.eof(usize::MAX))?)?;
        Ok(b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
    }
    /// Byte string preceded by a one-byte length.
    fn name8(&mut self) -> Res<&'a [u8]> {
        let n = self.u8()? as usize;
        self.take(n)
    }
    /// Geometry mask of attribute opcodes: one byte, extended while the high bit is set.
    fn geom_mask(&mut self) -> Res<u32> {
        let mut m = self.u8()? as u32;
        let mut b = m;
        let mut shift = 8;
        while b & 0x80 != 0 && shift < 32 {
            b = self.u8()? as u32;
            m |= b << shift;
            shift += 8;
        }
        Ok(m)
    }
}

/// Unpack `count` values of `bits` bits, packed MSB-first in little-endian 32-bit words
/// (the CS_TRIVIAL sample layout).
fn unpack_bits(data: &[u8], bits: u32, count: usize) -> Vec<u32> {
    let mut out = Vec::with_capacity(count);
    let word = |i: usize| -> u32 {
        let mut w = [0u8; 4];
        for (k, b) in w.iter_mut().enumerate() {
            *b = data.get(i * 4 + k).copied().unwrap_or(0);
        }
        u32::from_le_bytes(w)
    };
    let mut bit = 0usize;
    for _ in 0..count {
        let mut v = 0u32;
        let mut need = bits;
        while need > 0 {
            let w = word(bit / 32);
            let off = (bit % 32) as u32;
            let avail = 32 - off;
            let n = need.min(avail);
            let chunk = ((w << off) >> (32 - n)) & if n == 32 { u32::MAX } else { (1 << n) - 1 };
            v = if n == 32 { chunk } else { (v << n) | chunk };
            need -= n;
            bit += n as usize;
        }
        out.push(v);
    }
    out
}

/// Turn a HOOPS face list (tristrips/fans when `strips`, else polygons) into triangles.
fn triangulate(vals: &[i64], strips: bool, npts: usize, out: &mut Vec<[u32; 3]>) {
    let mut i = 0;
    let ok = |v: i64| v >= 0 && (v as usize) < npts;
    while i < vals.len() {
        let n = vals[i];
        i += 1;
        let len = n.unsigned_abs() as usize;
        let Some(s) = vals.get(i..i + len) else { break };
        i += len;
        if strips {
            for k in 2..len {
                let (a, b, c) = if n < 0 {
                    (s[0], s[k - 1], s[k])
                } else if k % 2 == 1 {
                    (s[k - 1], s[k - 2], s[k])
                } else {
                    (s[k - 2], s[k - 1], s[k])
                };
                if ok(a) && ok(b) && ok(c) && a != b && b != c && a != c {
                    out.push([a as u32, b as u32, c as u32]);
                }
            }
        } else if n > 0 {
            // polygon: fan triangulation (holes, given as negative counts, are dropped)
            for k in 2..len {
                let (a, b, c) = (s[0], s[k - 1], s[k]);
                if ok(a) && ok(b) && ok(c) && a != b && b != c && a != c {
                    out.push([a as u32, b as u32, c as u32]);
                }
            }
        }
    }
}

/// What the last opcode created, for `TKE_Tag_Implicit`.
#[derive(Clone, Copy, Debug)]
enum Tag {
    Segment(usize),
    Shell(usize),
    Other,
}

struct Parser<'a> {
    r: R<'a>,
    m: Model,
    stack: Vec<usize>,
    tags: Vec<Tag>,
    last: Tag,
    repeats: Vec<(i32, i32, [f32; 12])>,
    /// Stream version x100 (`V19.10` -> 1910), 0 when unknown.
    version: u32,
    /// `TKE_Pause` records seen (the model is complete after the last refinement pass).
    pauses: usize,
    /// Set when the trailing tables after the model were reached.
    done: bool,
}

/// Parse an uncompressed HSF stream (the bytes that follow `TKE_Start_Compression`, inflated,
/// or a whole uncompressed file).
pub fn parse(data: &[u8]) -> Model {
    parse_version(data, 0)
}

/// [`parse`] for a stream of a known version (x100, as in [`version_number`]).
pub fn parse_version(data: &[u8], version: u32) -> Model {
    let mut p = Parser { r: R { d: data, p: 0 }, m: Model::default(), stack: Vec::new(), tags: Vec::new(), last: Tag::Other, repeats: Vec::new(), version, pauses: 0, done: false };
    let root = p.m.seg("/");
    p.stack.push(root);
    if let Err(e) = p.run() {
        p.m.error = Some(e);
    }
    p.resolve_repeats();
    p.m
}

/// Read an eDrawings/HOOPS `.hsf` stream as stored in the `eModel` record: a `;; HSF V…`
/// comment, then opcodes, usually switching to a zlib stream at `TKE_Start_Compression`.
pub fn parse_file(data: &[u8]) -> Result<Model, String> {
    if !data.starts_with(b";; HSF V") {
        return Err("flux HSF introuvable".into());
    }
    let ver = version_number(data);
    let mut r = R { d: data, p: 0 };
    // header opcodes before compression: comment, file info, bounding info…
    loop {
        let op = r.u8()?;
        match op {
            b';' => while r.u8()? != b'\n' {},
            b'I' => r.skip(4)?,
            b'B' | b'b' => {
                let n = if r.u8()? == 1 { 4 } else { 6 };
                r.skip(n * 4)?;
            }
            b'Z' => {
                let mut out = Vec::new();
                use std::io::Read;
                let mut z = flate2::read::ZlibDecoder::new(&data[r.p..]);
                if let Err(e) = z.read_to_end(&mut out) {
                    if out.is_empty() {
                        return Err(format!("décompression HSF impossible : {e}"));
                    }
                }
                return Ok(parse_version(&out, ver));
            }
            _ => {
                // uncompressed stream: hand everything from this opcode on to the parser
                r.p -= 1;
                return Ok(parse_version(&data[r.p..], ver));
            }
        }
    }
}

/// HSF version from the leading comment (`;; HSF V18.00`).
pub fn version(data: &[u8]) -> Option<String> {
    let line = data.split(|&b| b == b'\n').next()?;
    let s = String::from_utf8_lossy(line);
    s.strip_prefix(";; HSF V").map(|v| v.trim().to_string())
}

/// Version from the leading comment as a number: `V19.10` -> 1910 (0 when absent).
pub fn version_number(data: &[u8]) -> u32 {
    let Some(v) = version(data) else { return 0 };
    let (a, b) = v.split_once('.').unwrap_or((&v, "0"));
    let b: String = b.chars().take_while(|c| c.is_ascii_digit()).collect();
    a.parse::<u32>().unwrap_or(0) * 100 + format!("{b:0<2}")[..2].parse::<u32>().unwrap_or(0)
}

impl<'a> Parser<'a> {
    fn cur(&self) -> usize {
        *self.stack.last().unwrap_or(&0)
    }

    fn run(&mut self) -> Res<()> {
        while self.r.p < self.r.d.len() {
            let at = self.r.p;
            let op = self.r.u8()?;
            self.m.stats.opcodes += 1;
            if self.m.recent.len() >= 16 {
                self.m.recent.remove(0);
            }
            self.m.recent.push((at, op));
            self.opcode(op).map_err(|e| format!("opcode 0x{op:02x} à l'octet {at} : {e}"))?;
            if op == 0x00 || self.done {
                break;
            }
        }
        Ok(())
    }

    fn opcode(&mut self, op: u8) -> Res<()> {
        let r = &mut self.r;
        let mut last = Tag::Other;
        match op {
            0x00 => {} // termination
            0x01 => self.pauses += 1,
            b'z' => {}        // stop compression (the inflated data simply ends)
            b';' => {
                // comment: up to end of line
                while r.u8()? != b'\n' {}
            }
            b'(' => {
                let name = String::from_utf8_lossy(r.name8()?).into_owned();
                let path = resolve(&self.m.segments[self.cur()].path, &name);
                let s = self.m.seg(&path);
                self.stack.push(s);
            }
            b')' => {
                if self.stack.len() > 1 {
                    let s = self.stack.pop().unwrap_or(0);
                    last = Tag::Segment(s);
                }
            }
            b's' => {
                let i = r.i32()?;
                let s = match usize::try_from(i).ok().and_then(|i| self.tags.get(i)) {
                    Some(Tag::Segment(s)) => *s,
                    _ => return Err(format!("réouverture d'un segment inconnu ({i})")),
                };
                self.stack.push(s);
            }
            b'q' => {
                self.tags.push(self.last);
                return Ok(());
            }
            b'[' => {
                let n = r.len()?;
                r.skip(n)?;
                // a few writers pad the user data; the closing ']' follows within a few bytes
                let mut k = 0;
                while r.d.get(r.p) != Some(&b']') && k < 16 {
                    r.p += 1;
                    k += 1;
                }
                if r.d.get(r.p) != Some(&b']') && self.pauses > 0 {
                    // trailing index tables written after the paused passes (eDrawings 2015+):
                    // the model itself is complete
                    self.done = true;
                    return Ok(());
                }
                if r.u8()? != b']' {
                    return Err("fin de données utilisateur introuvable".into());
                }
            }
            b']' => {}
            b'?' => {
                // conditions: a one-byte length and the condition string
                r.name8()?;
            }
            b'I' => r.skip(4)?, // file info flags
            b'B' | b'b' => {
                // bounding info: type, then a cuboid (6 floats) or a sphere (4 floats)
                let n = if r.u8()? == 1 { 4 } else { 6 };
                r.skip(n * 4)?;
            }
            b'V' => {
                let mask = r.u8()?;
                let value = r.u8()?;
                if mask & 0x80 != 0 {
                    let m2 = r.u16()?;
                    r.u16()?;
                    if m2 & 0x8000 != 0 {
                        r.u8()?;
                        r.u8()?;
                    }
                }
                if mask & 0x01 != 0 {
                    let c = self.cur();
                    self.m.segments[c].face_visible = Some(value & 0x01 != 0);
                }
            }
            b'"' => {
                let geom = r.geom_mask()?;
                let mut ch = r.u8()? as u32;
                if ch & 0x80 != 0 {
                    ch |= (r.u8()? as u32) << 8;
                }
                let mut diffuse = None;
                for bit in [0x01, 0x02, 0x04, 0x08, 0x10] {
                    if ch & bit != 0 {
                        let c = var_color(r)?;
                        if bit == 0x01 {
                            diffuse = c;
                        }
                    }
                }
                if ch & 0x20 != 0 {
                    r.f32()?;
                }
                if ch & 0x40 != 0 {
                    r.f32()?;
                }
                for bit in [0x100, 0x200] {
                    if ch & bit != 0 {
                        var_color(r)?;
                    }
                }
                if geom & 0x01 != 0 {
                    if let Some(c) = diffuse {
                        let s = self.cur();
                        self.m.segments[s].face_color = Some(c);
                    }
                }
            }
            b'~' => {
                let geom = r.geom_mask()?;
                let c = r.take(3)?;
                if geom & 0x01 != 0 {
                    let s = self.cur();
                    self.m.segments[s].face_color = Some([c[0] as f32 / 255., c[1] as f32 / 255., c[2] as f32 / 255.]);
                }
            }
            0x08 => {
                r.geom_mask()?;
                r.u8()?;
            }
            0x0A => {
                r.geom_mask()?;
                r.f32()?;
            }
            b'<' | b'{' => {
                let n = r.u8()? as usize;
                let (name, cond) = if n == 0 {
                    let name = r.name8()?;
                    let cond = r.name8()?;
                    (name, !cond.is_empty())
                } else {
                    (r.take(n)?, false)
                };
                if op == b'<' {
                    let c = self.cur();
                    let path = resolve(&self.m.segments[c].path, &String::from_utf8_lossy(name));
                    self.m.segments[c].includes.push((path, cond));
                }
            }
            b'%' => {
                let v = r.floats(12)?;
                let mut m = [0f32; 12];
                m.copy_from_slice(&v);
                let c = self.cur();
                self.m.segments[c].matrix = Some(m);
            }
            b'$' => r.skip(48)?, // texture matrix
            b'S' => last = self.shell()?,
            b'M' => last = self.mesh()?,
            b'&' => {
                let from = r.i32()?;
                r.i32()?;
                let to = r.i32()?;
                r.i32()?;
                r.u8()?;
                let v = r.floats(12)?;
                let mut m = [0f32; 12];
                m.copy_from_slice(&v);
                self.repeats.push((from, to, m));
                self.m.stats.repeats += 1;
            }
            b'x' | b'T' => {
                r.skip(12)?;
                let enc = if op == b'x' { r.u8()? } else { 0 };
                let mut n = r.u8()? as usize;
                if n == 254 {
                    n = r.u16()? as usize;
                } else if n == 255 {
                    n = r.len()?;
                }
                let raw = r.take(n)?;
                let opts = r.u8()?;
                if opts & 0x01 != 0 {
                    r.u8()?;
                    let k = r.u8()? as usize;
                    r.skip(k * 12)?;
                }
                let text = if enc >= 2 && raw.len() % 2 == 0 {
                    String::from_utf16_lossy(&raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
                } else {
                    raw.iter().map(|&b| b as char).collect()
                };
                let c = self.cur();
                self.m.segments[c].texts.push(text.trim_end_matches('\0').to_string());
            }
            0x10 => poly_polyline(r)?,
            b'L' | b'G' => {
                let n = r.len()?;
                r.skip(n.checked_mul(12).ok_or("trop de points")?)?;
            }
            b'l' => r.skip(24)?,
            b'X' => r.skip(12)?, // marker
            b'i' => image(r, self.version)?,
            b'J' => {
                r.name8()?;
                let n = r.u16()? as usize;
                r.skip(n)?;
            }
            b'=' | 0x0E | b'+' => {
                if r.f32()? < 0. {
                    r.u8()?;
                }
            }
            b'@' | b'*' | b'h' | b'P' => r.skip(1)?,
            b'0' | b'v' | b'K' | b'k' => r.skip(4)?,
            b'|' => r.skip(12)?,
            b' ' => r.skip(4)?,
            0x16 => {
                // unicode user options: a count of 16-bit characters, then the characters
                let mut n = r.u16()? as usize;
                if n == 0xFFFF {
                    n = r.len()?;
                }
                r.skip(n.checked_mul(2).ok_or("options utilisateur trop longues")?)?;
            }
            b'U' => {
                let mut n = r.u16()? as usize;
                if n == 0xFFFF {
                    n = r.len()?;
                }
                r.skip(n)?;
            }
            b'-' | 0x0D => {
                // line / edge pattern: a pattern byte, or 0xFF and a pattern name
                if r.u8()? == 0xFF {
                    r.name8()?;
                }
            }
            b'R' => rendering_options(r)?,
            b't' => texture(r)?,
            b'H' => heuristics(r)?,
            b'}' | b'>' => {
                let proj = r.u8()?;
                let mut n = 11;
                for b in [0x04, 0x08, 0x10] {
                    if proj & b != 0 {
                        n += 1;
                    }
                }
                r.skip(n * 4)?;
                if op == b'}' {
                    r.name8()?;
                }
            }
            _ => return Err("opcode non pris en charge".into()),
        }
        self.last = last;
        Ok(())
    }

    fn shell(&mut self) -> Res<Tag> {
        let r = &mut self.r;
        let sub = r.u8()?;
        let sub2 = if sub & 0x80 != 0 { r.u16()? } else { 0 };
        let first = sub & 0x10 != 0;
        let index = if first { None } else { Some(r.i32()?) };
        let lod = r.u8()?;
        if sub2 & 0x0002 != 0 {
            // null shell, filled in later by a refinement or a repeat
            self.m.stats.null_shells += 1;
            let id = self.m.shells.len();
            self.m.shells.push(None);
            let c = self.cur();
            self.m.segments[c].shells.push(id);
            return Ok(Tag::Shell(id));
        }
        if sub & 0x20 != 0 {
            r.skip(24)?; // bounding only
            return Ok(Tag::Other);
        }
        if sub2 & 0x0001 != 0 {
            return Err("collection de niveaux de détail non prise en charge".into());
        }
        if sub & 0x40 != 0 {
            return Err("compression Edgebreaker non prise en charge".into());
        }
        // points
        let points: Vec<[f32; 3]> = if sub & 0x01 != 0 {
            compressed_points(r, sub2, None)?
        } else {
            let n = r.len()?;
            let v = r.floats(n.checked_mul(3).ok_or("trop de points")?)?;
            v.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect()
        };
        // faces: scheme, length (bytes, counting the bits byte), bits per value, values
        let _scheme = r.u8()?;
        let fl = r.len()?;
        let bits = r.u8()?;
        let data = r.take(fl.saturating_sub(1))?;
        let signed = sub2 & 0x0004 != 0;
        let vals: Vec<i64> = match bits {
            8 => data.iter().map(|&b| if signed { b as i8 as i64 } else { b as i64 }).collect(),
            16 => data.chunks_exact(2).map(|c| if signed { i16::from_le_bytes([c[0], c[1]]) as i64 } else { u16::from_le_bytes([c[0], c[1]]) as i64 }).collect(),
            32 => data.chunks_exact(4).map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as i64).collect(),
            b => return Err(format!("liste de faces sur {b} bits")),
        };
        let mut tris = Vec::new();
        triangulate(&vals, sub & 0x04 != 0, points.len(), &mut tris);
        if sub & 0x08 != 0 {
            optionals(r, points.len())?;
        }
        let shell = Shell { points, tris };
        match index {
            None => {
                self.m.stats.first_pass_shells += 1;
                let id = self.m.shells.len();
                self.m.shells.push(Some(shell));
                let c = self.cur();
                self.m.segments[c].shells.push(id);
                Ok(Tag::Shell(id))
            }
            Some(i) => {
                if lod != 0 {
                    self.m.stats.skipped_lod += 1;
                    return Ok(Tag::Other);
                }
                match usize::try_from(i).ok().and_then(|i| self.tags.get(i)) {
                    Some(Tag::Shell(id)) => {
                        self.m.stats.refinements += 1;
                        self.m.shells[*id] = Some(shell);
                    }
                    _ => self.m.stats.unsupported_shells += 1,
                }
                Ok(Tag::Other)
            }
        }
    }

    /// `TKE_Mesh`: a `rows` x `columns` grid of points, triangulated as quads.
    fn mesh(&mut self) -> Res<Tag> {
        let r = &mut self.r;
        let sub = r.u8()?;
        let sub2 = if sub & 0x80 != 0 { r.u16()? } else { 0 };
        let rows = r.len()?;
        let cols = r.len()?;
        let n = rows.checked_mul(cols).filter(|&n| n < 1 << 28).ok_or("maillage trop grand")?;
        let points: Vec<[f32; 3]> = if sub & 0x01 != 0 {
            compressed_points(r, sub2, None)?
        } else {
            let v = r.floats(n * 3)?;
            v.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect()
        };
        if points.len() != n {
            return Err("maillage : nombre de points inattendu".into());
        }
        if sub & 0x08 != 0 {
            optionals(r, n)?;
        }
        let mut tris = Vec::new();
        for i in 0..rows.saturating_sub(1) {
            for j in 0..cols.saturating_sub(1) {
                let a = (i * cols + j) as u32;
                let b = a + 1;
                let c = a + cols as u32;
                let d = c + 1;
                tris.push([a, b, d]);
                tris.push([a, d, c]);
            }
        }
        let id = self.m.shells.len();
        self.m.shells.push(Some(Shell { points, tris }));
        let c = self.cur();
        self.m.segments[c].shells.push(id);
        Ok(Tag::Shell(id))
    }

    fn resolve_repeats(&mut self) {
        let mut pending = std::mem::take(&mut self.repeats);
        loop {
            let before = pending.len();
            pending.retain(|&(from, to, m)| {
                let id = |i: i32| match usize::try_from(i).ok().and_then(|i| self.tags.get(i)) {
                    Some(Tag::Shell(s)) => Some(*s),
                    _ => None,
                };
                let (Some(f), Some(t)) = (id(from), id(to)) else {
                    return false;
                };
                let Some(src) = self.m.shells[f].as_ref() else {
                    return true;
                };
                let points = src.points.iter().map(|p| apply(&m, *p)).collect();
                let s = Shell { points, tris: src.tris.clone() };
                self.m.shells[t] = Some(s);
                false
            });
            if pending.is_empty() || pending.len() == before {
                break;
            }
        }
    }
}

/// Quantised point list: scheme, count, bits per coordinate, bounding box, byte length, data.
fn compressed_points(r: &mut R, sub2: u16, _count: Option<usize>) -> Res<Vec<[f32; 3]>> {
    let scheme = r.u8()?;
    let n = r.len()?;
    let bits = r.u8()? as u32;
    if scheme != 1 || !(2..=31).contains(&bits) || sub2 & 0x0004 != 0 {
        return Err(format!("points compressés (schéma {scheme}, {bits} bits) non pris en charge"));
    }
    let bb = r.floats(6)?;
    let len = r.len()?;
    if Some(len) != n.checked_mul(3 * bits as usize).map(|b| b.div_ceil(8)) {
        return Err("points compressés : longueur inattendue".into());
    }
    let data = r.take(len)?;
    let q = unpack_bits(data, bits, n * 3);
    let maxq = ((1u64 << bits) - 1) as f32;
    Ok(q.chunks_exact(3).map(|c| [0, 1, 2].map(|k| bb[k] + (bb[k + 3] - bb[k]) * c[k] as f32 / maxq)).collect())
}

/// `p * M` for a row-vector 4x3 matrix.
pub fn apply(m: &[f32; 12], p: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|j| p[0] * m[j] + p[1] * m[3 + j] + p[2] * m[6 + j] + m[9 + j])
}

/// Colour operand: a one-byte length, then either 3 RGB bytes (length 0) or a HOOPS colour
/// string such as `diffuse color=(r=0.25 g=0.5 b=1)` or a colour name.
fn var_color(r: &mut R) -> Res<Option<[f32; 3]>> {
    let n = r.u8()? as usize;
    if n == 0 {
        let c = r.take(3)?;
        return Ok(Some([c[0] as f32 / 255., c[1] as f32 / 255., c[2] as f32 / 255.]));
    }
    let s = String::from_utf8_lossy(r.take(n)?).to_lowercase();
    Ok(parse_color_string(&s))
}

fn parse_color_string(s: &str) -> Option<[f32; 3]> {
    let get = |k: &str| -> Option<f32> {
        let pat = format!("{k}=");
        let i = s.match_indices(&pat).find(|(i, _)| *i == 0 || matches!(s.as_bytes()[i - 1], b'(' | b' ' | b',' | b'`' | b'\''))?.0 + pat.len();
        let rest = &s[i..];
        let e = rest.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e')).unwrap_or(rest.len());
        rest[..e].parse().ok()
    };
    // the first r=/g=/b= triplet is the diffuse colour (it comes first in eDrawings strings)
    if let (Some(r), Some(g), Some(b)) = (get("r"), get("g"), get("b")) {
        return Some([r, g, b]);
    }
    let named = [
        ("black", [0., 0., 0.]),
        ("white", [1., 1., 1.]),
        ("red", [1., 0., 0.]),
        ("green", [0., 1., 0.]),
        ("blue", [0., 0., 1.]),
        ("yellow", [1., 1., 0.]),
        ("cyan", [0., 1., 1.]),
        ("magenta", [1., 0., 1.]),
        ("gray", [0.5, 0.5, 0.5]),
        ("grey", [0.5, 0.5, 0.5]),
    ];
    let word = s.split(['=', ',', ' ']).filter(|w| !w.is_empty()).next_back()?;
    named.iter().find(|(n, _)| *n == word).map(|(_, c)| *c)
}

/// Polyhedron optionals (attributes) list, up to `OPT_TERMINATE`; values are skipped.
fn optionals(r: &mut R, _npts: usize) -> Res<()> {
    loop {
        let op = r.u8()?;
        match op {
            0x00 => return Ok(()),
            // all normals / all parameters, compressed: scheme, [width, bbox], bits, length, data
            0x01 => {
                let scheme = r.u8()?;
                r.u8()?; // bits per sample
                if scheme == 1 || scheme == 7 || scheme == 3 {
                    let n = r.len()?;
                    r.skip(n)?;
                } else {
                    return Err(format!("normales : schéma {scheme} inconnu"));
                }
            }
            0x03 => {
                let scheme = r.u8()?;
                let width = r.u8()? as usize;
                if scheme == 1 {
                    r.skip(8 * width)?;
                }
                r.u8()?; // bits
                let n = r.len()?;
                r.skip(n)?;
            }
            // entity attributes: a list of attribute records (user options), up to a termination
            0x63 => loop {
                match r.u8()? {
                    0x00 => break,
                    b'U' => {
                        let mut n = r.u16()? as usize;
                        if n == 0xFFFF {
                            n = r.len()?;
                        }
                        r.skip(n)?;
                    }
                    0x16 => {
                        let mut n = r.u16()? as usize;
                        if n == 0xFFFF {
                            n = r.len()?;
                        }
                        r.skip(n.checked_mul(2).ok_or("options utilisateur trop longues")?)?;
                    }
                    o => return Err(format!("attribut d'entité 0x{o:02x} non pris en charge")),
                }
            },
            o => return Err(format!("attribut de maillage 0x{o:02x} non pris en charge")),
        }
    }
}

fn poly_polyline(r: &mut R) -> Res<()> {
    let so = r.u16()?;
    let n = r.len()?;
    let prims = if so & 0x02 != 0 {
        let pc = r.len()?;
        for _ in 0..pc {
            r.i32()?;
        }
        pc
    } else if so & 0x04 != 0 {
        1
    } else {
        let (mut total, mut k) = (0usize, 0usize);
        while total < n {
            total += r.len()?.max(1);
            k += 1;
        }
        k
    };
    let (mut dims, mut left) = (0usize, 0usize);
    for sh in [8, 10, 12] {
        match (so >> sh) & 3 {
            0 => dims += 1,
            2 => left += 1,
            3 => left += prims,
            _ => {}
        }
    }
    if so & 0x01 != 0 {
        r.u8()?;
        r.skip(8 * dims)?;
        r.u8()?;
        let l = r.len()?;
        r.skip(l)?;
    } else {
        r.skip(dims * n * 4)?;
    }
    if left > 0 {
        if left == 1 || so & 0x01 == 0 {
            r.skip(left * 4)?;
        } else {
            r.u8()?;
            r.skip(8)?;
            let l = r.len()?;
            r.skip(l)?;
        }
    }
    Ok(())
}

fn image(r: &mut R, version: u32) -> Res<()> {
    r.skip(12)?;
    let fmt = r.u8()?;
    if version >= 1900 {
        r.skip(3)?; // V19 widened the format/options field to 32 bits
    }
    if fmt & 0x80 != 0 {
        r.name8()?;
    }
    let w = r.len()?;
    let h = r.len()?;
    let comp = r.u8()?;
    let bpp = [1usize, 2, 3, 4, 4].get((fmt & 0x0F) as usize).copied().unwrap_or(4);
    match comp {
        0 => r.skip(w.saturating_mul(h).saturating_mul(bpp))?,
        1 | 2 => {
            let n = r.len()?;
            r.skip(n)?;
        }
        99 => {
            let n = r.u16()? as usize;
            r.skip(n)?;
        }
        c => return Err(format!("image : compression {c} inconnue")),
    }
    if matches!(fmt & 0x0F, 3 | 4) && comp == 2 {
        if r.u8()? == 0 {
            r.u8()?;
        } else {
            let n = r.len()?;
            r.skip(n)?;
        }
    }
    if fmt & 0x10 != 0 {
        r.skip(10)?;
    }
    Ok(())
}

/// Geometry types whose colour lock carries per-channel masks (`TKO_Geometry_Bits`): face, edge,
/// line, marker, text, window, face/window contrast, back, vertex, edge/line/marker/vertex
/// contrast, cut edge, simple reflection, cut face and text contrast. Image, ambient, light and
/// front carry none.
const COLOR_LOCK_TYPES: u32 = 0x0000_003F | 0x0000_0C00 | 0x0000_6000 | 0x000F_0000 | 0x0070_0000 | 0x0100_0000;

fn rendering_options(r: &mut R) -> Res<()> {
    // masks first (extended words included), then the values
    let m = r.i32()? as u32;
    let m2 = if m & 0x8000_0000 != 0 { r.i32()? as u32 } else { 0 };
    if m2 & 0x8000_0000 != 0 {
        return Err("options de rendu : troisième mot non pris en charge".into());
    }
    r.i32()?;
    if m & 0x8000_0000 != 0 {
        r.i32()?;
    }
    const KNOWN0: u32 = 0x0000_1000 | 0x0000_2000 | 0x0001_0000 | 0x0010_0000 | 0x0100_0000 | 0x8000_0000;
    const KNOWN1: u32 = 0x0000_0010;
    if m & !KNOWN0 != 0 || m2 & !KNOWN1 != 0 {
        return Err(format!("options de rendu 0x{m:08x}/0x{m2:08x} non prises en charge"));
    }
    if m & 0x0000_1000 != 0 {
        r.u8()?; // hidden surface removal algorithm
    }
    if m & 0x0000_2000 != 0 {
        r.u8()?; // transparent hidden surface removal algorithm
    }
    if m & 0x0010_0000 != 0 {
        // attribute lock mask and value; only the colour and visibility locks carry sub-masks
        let lm = r.i32()?;
        r.i32()?;
        if lm & 0x04 != 0 {
            // colour lock: geometry mask and value, then a channel mask and a channel value
            // for each lockable geometry type in the mask
            let cm = r.i32()? as u32;
            r.i32()?;
            for _ in 0..2 * (cm & COLOR_LOCK_TYPES).count_ones() {
                if r.u8()? & 0x80 != 0 {
                    r.u8()?;
                }
            }
        }
        if lm & 0x0800_0000 != 0 {
            r.skip(8)?; // visibility lock: geometry mask and value
        }
    }
    if m & 0x0100_0000 != 0 {
        // hidden line options: flag bits; only the HSR algorithm carries a payload here
        // (a byte, extended by 16 bits when 0x80 is set, and by a last byte when 0x8000 is set)
        let mut h = r.u8()? as u32;
        if h & 0x80 != 0 {
            h |= (r.u16()? as u32) << 8;
            if h & 0x8000 != 0 {
                h |= (r.u8()? as u32) << 24;
            }
        }
        const FLAGS: u32 = 0x01 | 0x02 | 0x20 | 0x40 | 0x80 | 0x0100 | 0x0200 | 0x8000 | 0x04_0000 | 0x08_0000 | 0x8000_0000;
        if h & !(FLAGS | 0x10_0000) != 0 {
            return Err(format!("options de lignes cachées 0x{h:06x} non prises en charge"));
        }
        if h & 0x10_0000 != 0 {
            r.u8()?; // hidden line HSR algorithm
        }
    }
    if m2 & 0x0000_0010 != 0 {
        r.skip(8)?; // depth range
    }
    Ok(())
}

fn texture(r: &mut R) -> Res<()> {
    r.name8()?;
    r.name8()?;
    let o1 = r.u16()? as u32;
    let o2 = if o1 & 0x8000 != 0 { r.u16()? as u32 } else { 0 };
    for b in 0..15 {
        if o1 & (1 << b) != 0 {
            match b {
                10 => {
                    r.name8()?;
                }
                11 => r.skip(2)?,
                12..=14 => {} // caching / down-sampling flags carry no operand
                _ => r.skip(1)?,
            }
        }
    }
    r.skip(o2.count_ones() as usize)
}

fn heuristics(r: &mut R) -> Res<()> {
    let mut m = r.u16()? as u32;
    let mut v = r.u16()? as u32;
    if m & 0x8000 != 0 {
        m |= (r.u16()? as u32) << 16;
        v |= (r.u16()? as u32) << 16;
    }
    let on = m & v;
    if on & !0x3FFF != 0 {
        return Err(format!("heuristiques 0x{m:x} non prises en charge"));
    }
    if on & 0x40 != 0 {
        r.i32()?;
    }
    if on & 0x80 != 0 {
        r.i32()?;
    }
    if on & 0x0C != 0 {
        r.u8()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(resolve("/a/b", "C"), "/a/b/c");
        assert_eq!(resolve("/a/b", "/Include Library/shared/Body1"), "/include library/shared/body1");
        assert_eq!(resolve("/a", "?Include Library/references"), "/include library/references");
        assert_eq!(resolve("/", "x/y"), "/x/y");
    }

    #[test]
    fn strips_and_fans() {
        let mut t = Vec::new();
        triangulate(&[4, 1, 0, 3, 2], true, 4, &mut t);
        assert_eq!(t, vec![[1, 0, 3], [3, 0, 2]]);
        t.clear();
        triangulate(&[-4, 0, 1, 2, 3], true, 4, &mut t);
        assert_eq!(t, vec![[0, 1, 2], [0, 2, 3]]);
    }

    #[test]
    fn bit_unpacking() {
        // 12-bit samples 0xBFF, 0x800 packed MSB-first in one little-endian word
        let w: u32 = (0xBFF << 20) | (0x800 << 8);
        assert_eq!(unpack_bits(&w.to_le_bytes(), 12, 2), vec![0xBFF, 0x800]);
    }

    #[test]
    fn colour_strings() {
        assert_eq!(parse_color_string("diffuse color=(r=0.25 g=0.5 b=1)"), Some([0.25, 0.5, 1.0]));
        assert_eq!(parse_color_string("diffuse=red"), Some([1., 0., 0.]));
    }

    /// A tiny hand-made stream: a segment with a matrix, colour and one tristrip shell.
    #[test]
    fn minimal_stream() {
        let mut s = Vec::new();
        s.push(b'(');
        s.push(4);
        s.extend_from_slice(b"Part");
        s.extend_from_slice(&[b'"', 0x01, 0x01, 0x00, 255, 0, 0]);
        s.push(b'S');
        s.push(0x14); // first pass, tristrips
        s.push(0); // lod
        s.extend_from_slice(&4i32.to_le_bytes());
        for p in [[0f32, 0., 0.], [1., 0., 0.], [0., 1., 0.], [1., 1., 0.]] {
            for c in p {
                s.extend_from_slice(&c.to_le_bytes());
            }
        }
        s.push(1);
        s.extend_from_slice(&6i32.to_le_bytes());
        s.push(8);
        s.extend_from_slice(&[4, 0, 1, 2, 3]);
        s.push(b')');
        s.push(0);
        let m = parse(&s);
        assert!(m.error.is_none(), "{:?}", m.error);
        let seg = m.segment("/part").unwrap();
        assert_eq!(seg.face_color, Some([1., 0., 0.]));
        assert_eq!(m.shells[seg.shells[0]].as_ref().unwrap().tris.len(), 2);
        // every truncation stops cleanly with an error instead of panicking
        for k in 0..s.len() - 1 {
            let _ = parse(&s[..k]);
        }
    }

    /// Records first met in HSF V19.10 files: a mesh, extended rendering options, line and edge
    /// patterns, a marker, a texture with payload-free flags and a named JPEG image.
    #[test]
    fn v19_records() {
        let mut s = Vec::new();
        // texture with the no-down-sampling flag (0x4000), then a segment close
        s.push(b'(');
        s.push(1);
        s.push(b'a');
        s.extend_from_slice(&[b't', 1, b'x', 1, b'x', 0x00, 0x40]);
        // mesh 2 x 2 with an optionals list holding parameters
        s.extend_from_slice(&[b'M', 0x08]);
        s.extend_from_slice(&2i32.to_le_bytes());
        s.extend_from_slice(&2i32.to_le_bytes());
        for p in [[0f32, 0., 0.], [1., 0., 0.], [0., 1., 0.], [1., 1., 0.]] {
            for c in p {
                s.extend_from_slice(&c.to_le_bytes());
            }
        }
        s.extend_from_slice(&[0x03, 0x01, 0x03]);
        s.extend_from_slice(&[0u8; 24]);
        s.push(8);
        s.extend_from_slice(&12i32.to_le_bytes());
        s.extend_from_slice(&[0u8; 12]);
        s.push(0);
        // rendering options: masks 0x81101000 / 0x10, values, HSR, colour lock, hidden lines, depth range
        s.push(b'R');
        for v in [0x8110_1000u32, 0x10, 0x0110_1000, 0x10, 0] {
            s.extend_from_slice(&v.to_le_bytes());
        }
        s.truncate(s.len() - 4);
        s.push(0); // HSR algorithm
        s.extend_from_slice(&4i32.to_le_bytes());
        s.extend_from_slice(&4i32.to_le_bytes());
        s.extend_from_slice(&[0x7f, 0x7f, 0x7f, 0x03, 0x7f, 0x7f, 0x7f, 0x03]);
        for _ in 0..36 {
            s.extend_from_slice(&[0xff, 0x01]);
        }
        s.push(0x20);
        s.extend_from_slice(&[0u8; 8]);
        // line pattern by name, edge pattern by name, marker
        s.extend_from_slice(&[b'-', 0xff, 2, b'p', b'1', 0x0D, 0xff, 2, b'p', b'1']);
        s.push(b'X');
        s.extend_from_slice(&[0u8; 12]);
        // named RGB image, JPEG-compressed, 32-bit format field
        s.push(b'i');
        s.extend_from_slice(&[0u8; 12]);
        s.extend_from_slice(&[0xa3, 0, 0, 0, 1, b'm']);
        s.extend_from_slice(&2i32.to_le_bytes());
        s.extend_from_slice(&2i32.to_le_bytes());
        s.push(2);
        s.extend_from_slice(&3i32.to_le_bytes());
        s.extend_from_slice(&[0xff, 0xd8, 0xd9, 0x00, 0xff]);
        s.extend_from_slice(&[b')', 0]);
        let m = parse_version(&s, 1910);
        assert!(m.error.is_none(), "{:?}", m.error);
        let seg = m.segment("/a").unwrap();
        assert_eq!(m.shells[seg.shells[0]].as_ref().unwrap().tris.len(), 2);
        assert_eq!(version_number(b";; HSF V19.10 \n"), 1910);
        assert_eq!(version_number(b";; HSF V18.00 \n"), 1800);
    }
    /// Records first met in Disney 2015/2018 eDrawings exports: unicode user options, conditions,
    /// colour and visibility locks, extended hidden line options with an HSR algorithm, a shell
    /// carrying entity attributes, and the index tables that follow the paused passes.
    #[test]
    fn v19_locks_and_attributes() {
        let mut s = Vec::new();
        s.extend_from_slice(&[b'(', 1, b'a']);
        // unicode user options "ab", conditions "c1"
        s.extend_from_slice(&[0x16, 2, 0, b'a', 0, b'b', 0, b'?', 2, b'c', b'1']);
        // rendering options: display lists, attribute lock (colour + visibility), hidden lines
        s.push(b'R');
        for v in [0x8111_0000u32, 0x10, 0x0111_0000, 0x10] {
            s.extend_from_slice(&v.to_le_bytes());
        }
        s.extend_from_slice(&0x0800_0004i32.to_le_bytes());
        s.extend_from_slice(&0x0800_0004i32.to_le_bytes());
        // colour lock on face, image (no channels) and back: two channel pairs
        s.extend_from_slice(&0x2041i32.to_le_bytes());
        s.extend_from_slice(&0x2041i32.to_le_bytes());
        s.extend_from_slice(&[0xff, 0x01, 0xff, 0x01, 0x01, 0x01]);
        s.extend_from_slice(&0x037f_ff7fi32.to_le_bytes());
        s.extend_from_slice(&0x037f_ff7fi32.to_le_bytes());
        s.extend_from_slice(&[0xa0, 0x80, 0x10, 0x00, 0x00]);
        s.extend_from_slice(&0f32.to_le_bytes());
        s.extend_from_slice(&0.1f32.to_le_bytes());
        // first-pass shell (one triangle) with entity attributes
        s.extend_from_slice(&[b'S', 0x18, 0]);
        s.extend_from_slice(&3i32.to_le_bytes());
        for c in [0f32, 0., 0., 1., 0., 0., 0., 1., 0.] {
            s.extend_from_slice(&c.to_le_bytes());
        }
        s.push(1);
        s.extend_from_slice(&5i32.to_le_bytes());
        s.extend_from_slice(&[8, 3, 0, 1, 2]);
        s.extend_from_slice(&[0x63, b'U', 3, 0, b'x', b'=', b'1', 0x00, 0x00]);
        s.extend_from_slice(&[b')', 0x01]);
        // trailing tables: user data whose payload is not closed by ']'
        s.push(b'[');
        s.extend_from_slice(&2i32.to_le_bytes());
        s.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9]);
        let m = parse_version(&s, 1910);
        assert!(m.error.is_none(), "{:?}", m.error);
        let seg = m.segment("/a").unwrap();
        assert_eq!(m.shells[seg.shells[0]].as_ref().unwrap().tris.len(), 1);
        for k in 0..s.len() - 1 {
            let _ = parse_version(&s[..k], 1910);
        }
    }
}
