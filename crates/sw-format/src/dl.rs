//! Decoder for the `Contents/DisplayLists` stream (cached display tessellation).
//!
//! The stream is an MFC-style serialized archive. We do not (yet) interpret the
//! full object graph; instead we locate the per-face tessellation records, which
//! have a very regular layout made of "generic arrays":
//!
//! ```text
//! array := u32 elem_size, u32 type_tag, u32 2, u32 count, data[elem_size * count]
//! face  := ..., u32[S] strip vertex counts, float3[V] positions (metres),
//!          float3[V or 1] normals, ...edge info, bbox, visual props
//! ```
//!
//! Triangle strips have no index array: strip i covers vertices [o, o+Vi).
//!
//! Faces are attributed to a component by the most recent document path string
//! (`...\\name.SLDPRT`) serialized before them; this is how cached component
//! meshes are grouped inside assembly display lists.

const POS_HDR: [u8; 12] = [0x0c, 0, 0, 0, 0x64, 0, 0, 0, 2, 0, 0, 0];

/// One B-rep face worth of tessellation.
#[derive(Debug, Clone, Default)]
pub struct FacePatch {
    pub positions: Vec<[f32; 3]>,
    /// Either one normal per vertex, or a single normal for a planar face.
    pub normals: Vec<[f32; 3]>,
    /// Vertex count of each triangle strip.
    pub strips: Vec<u32>,
    /// Byte offset of the position array in the stream (diagnostics).
    pub offset: usize,
}

/// A run of faces belonging to one document (part) referenced in the stream.
#[derive(Debug, Clone, Default)]
pub struct DlBlock {
    /// Full document path as stored (e.g. `K:\\...\\part.SLDPRT`), `None` for the
    /// file's own geometry (single part files).
    pub path: Option<String>,
    pub faces: Vec<FacePatch>,
}

impl DlBlock {
    /// Lower-case file name (`part.sldprt`) used to match assembly components.
    pub fn key(&self) -> Option<String> {
        self.path.as_deref().map(file_key)
    }
}

/// Lower-case file name of a Windows or Unix path.
pub fn file_key(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).trim().to_lowercase()
}

#[derive(Debug, Clone, Default)]
pub struct DisplayList {
    pub blocks: Vec<DlBlock>,
}

impl DisplayList {
    pub fn face_count(&self) -> usize {
        self.blocks.iter().map(|b| b.faces.len()).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.face_count() == 0
    }
}

#[inline]
fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Generic array header at `o`: returns (elem_size, count, data_start, data_end).
fn array_at(d: &[u8], o: usize) -> Option<(u32, usize, usize, usize)> {
    let es = u32_at(d, o)?;
    let two = u32_at(d, o + 8)?;
    let n = u32_at(d, o + 12)? as usize;
    if two != 2 || !matches!(es, 1 | 2 | 4 | 8 | 12) || n > 50_000_000 {
        return None;
    }
    let start = o + 16;
    let end = start.checked_add(es as usize * n)?;
    if end > d.len() {
        return None;
    }
    Some((es, n, start, end))
}

fn read_f3(d: &[u8], start: usize, n: usize) -> Vec<[f32; 3]> {
    let mut v = Vec::with_capacity(n);
    for k in 0..n {
        let o = start + k * 12;
        let f = |i: usize| f32::from_le_bytes(d[o + i..o + i + 4].try_into().unwrap());
        v.push([f(0), f(4), f(8)]);
    }
    v
}

/// Read an MFC CString with the Unicode marker `FF FE FF <len>` at `o`.
fn cstring_at(d: &[u8], o: usize) -> Option<(String, usize)> {
    if d.get(o..o + 3)? != [0xff, 0xfe, 0xff] {
        return None;
    }
    let mut p = o + 3;
    let mut len = *d.get(p)? as usize;
    p += 1;
    if len == 0xff {
        len = u16::from_le_bytes(d.get(p..p + 2)?.try_into().ok()?) as usize;
        p += 2;
        if len == 0xffff {
            len = u32_at(d, p)? as usize;
            p += 4;
        }
    }
    if len > 4096 {
        return None;
    }
    let bytes = d.get(p..p + 2 * len)?;
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Some((String::from_utf16_lossy(&units), p + 2 * len))
}

fn is_doc_path(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    (l.ends_with(".sldprt") || l.ends_with(".sldasm")) && (l.contains('\\') || l.contains('/'))
}

/// Try to decode a face whose position array header is at `i`.
/// Returns the face and the offset just after its normal array.
fn face_at(d: &[u8], i: usize) -> Option<(FacePatch, usize)> {
    let (_, nv, ps, pe) = array_at(d, i)?;
    if nv < 3 {
        return None;
    }
    // Normals: next array, float3, count == nv (per vertex) or 1 (planar face).
    let (nes, nn, ns, ne) = array_at(d, pe)?;
    if nes != 12 || !(nn == nv || nn == 1) {
        return None;
    }
    // Strip vertex counts: u32 array that ends exactly at `i` and sums to nv.
    let max_k = nv.min(1 << 20);
    let mut strips = None;
    for k in 1..=max_k {
        let Some(s) = i.checked_sub(16 + 4 * k) else { break };
        if u32_at(d, s) == Some(4) && u32_at(d, s + 8) == Some(2) && u32_at(d, s + 12) == Some(k as u32) {
            let mut sum = 0usize;
            let mut v = Vec::with_capacity(k);
            for j in 0..k {
                let c = u32_at(d, s + 16 + 4 * j)? as usize;
                sum += c;
                v.push(c as u32);
            }
            if sum == nv {
                strips = Some(v);
            }
            break;
        }
    }
    let strips = strips?;
    let positions = read_f3(d, ps, nv);
    let normals = read_f3(d, ns, nn);
    if positions.iter().any(|p| !p.iter().all(|x| x.is_finite() && x.abs() < 1.0e5)) {
        return None;
    }
    Some((FacePatch { positions, normals, strips, offset: i }, ne))
}

/// Decode all face tessellations in a DisplayLists stream.
pub fn decode(d: &[u8]) -> DisplayList {
    decode_ctl(d, &crate::Ctl::default(), (0.0, 0.0)).unwrap_or_default()
}

/// [`decode`] with progress (mapped onto `range`) and cancellation.
pub fn decode_ctl(d: &[u8], ctl: &crate::Ctl, range: (f32, f32)) -> crate::Result<DisplayList> {
    let stage = "Analyse du maillage…";
    let mut blocks: Vec<DlBlock> = Vec::new();
    let mut index: std::collections::HashMap<Option<String>, usize> = std::collections::HashMap::new();
    let mut dec = Decoder::default();
    let len = d.len().max(1);
    let mut visit = |path: Option<&str>, face: FacePatch| -> crate::Result<()> {
        // Blocks referring to the same document are merged (they can be split by other strings).
        let key = path.map(file_key);
        let k = match index.get(&key) {
            Some(&k) => k,
            None => {
                index.insert(key, blocks.len());
                blocks.push(DlBlock { path: path.map(str::to_string), faces: Vec::new() });
                blocks.len() - 1
            }
        };
        blocks[k].faces.push(face);
        Ok(())
    };
    let mut prog = |pos: usize| -> crate::Result<()> {
        ctl.report(range.0 + (range.1 - range.0) * (pos as f32 / len as f32), stage);
        ctl.check()
    };
    dec.run(d, 0, true, &mut visit, &mut prog)?;
    Ok(DisplayList { blocks })
}

/// Incremental display-list decoder: the stream can be fed in pieces (see [`StreamDecoder`])
/// so a multi-gigabyte decompressed stream never has to be held in memory at once.
///
/// Events (document path strings and face records) are processed in stream order; a face is
/// attributed to the most recent document path seen before it.
#[derive(Default)]
pub struct Decoder {
    cur_path: Option<String>,
    /// Global offsets of the next place to search for a path marker / a face header.
    mpos: usize,
    fpos: usize,
    next_m: Option<usize>,
    next_f: Option<usize>,
    next_report: usize,
}

enum Extent {
    Complete,
    NeedMore,
    Invalid,
}

/// Is the face record at `i` entirely inside `d`?
fn face_extent(d: &[u8], i: usize) -> Extent {
    let Some(h) = d.get(i..i + 16) else { return Extent::NeedMore };
    let n = u32::from_le_bytes(h[12..16].try_into().unwrap()) as usize;
    if n > 50_000_000 {
        return Extent::Invalid;
    }
    let pe = i + 16 + 12 * n;
    let Some(h) = d.get(pe..pe + 16) else { return Extent::NeedMore };
    let es = u32::from_le_bytes(h[0..4].try_into().unwrap());
    let two = u32::from_le_bytes(h[8..12].try_into().unwrap());
    let nn = u32::from_le_bytes(h[12..16].try_into().unwrap()) as usize;
    if es != 12 || two != 2 || !(nn == n || nn == 1) {
        return Extent::Invalid;
    }
    if pe + 16 + 12 * nn > d.len() {
        return Extent::NeedMore;
    }
    Extent::Complete
}

/// Bytes kept before the current position when compacting a streaming buffer (strip-count
/// arrays precede each face's position array).
pub const LOOKBACK: usize = 4 << 20;

impl Decoder {
    /// Process the events of `d`, whose first byte is at global offset `base`. With `last`
    /// false, processing stops before an event that may continue past the end of `d`.
    /// Returns the global offset from which data must be kept for the next call.
    pub fn run(
        &mut self,
        d: &[u8],
        base: usize,
        last: bool,
        visit: &mut dyn FnMut(Option<&str>, FacePatch) -> crate::Result<()>,
        prog: &mut dyn FnMut(usize) -> crate::Result<()>,
    ) -> crate::Result<usize> {
        const STEP: usize = 16 << 20;
        let end = base + d.len();
        let marker = memchr::memmem::Finder::new(&[0xff, 0xfe, 0xff]);
        let finder = memchr::memmem::Finder::new(&POS_HDR);
        loop {
            // next candidates (global offsets); None = not in this buffer
            if self.next_m.is_none_or(|m| m < self.mpos) {
                let from = self.mpos.max(base);
                self.next_m = if from < end { marker.find(&d[from - base..]).map(|r| from + r) } else { None };
                if self.next_m.is_none() {
                    self.mpos = self.mpos.max(end.saturating_sub(2));
                }
            }
            if self.next_f.is_none_or(|f| f < self.fpos) {
                let from = self.fpos.max(base);
                self.next_f = if from < end { finder.find(&d[from - base..]).map(|r| from + r) } else { None };
                if self.next_f.is_none() {
                    self.fpos = self.fpos.max(end.saturating_sub(11));
                }
            }
            let (m, f) = (self.next_m, self.next_f);
            let take_marker = match (m, f) {
                (None, None) => break,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (Some(m), Some(f)) => m < f,
            };
            let at = if take_marker { m.unwrap() } else { f.unwrap() };
            if at >= self.next_report {
                self.next_report = at + STEP;
                prog(at)?;
            }
            let li = at - base;
            if take_marker {
                match cstring_at(d, li) {
                    Some((s, e)) => {
                        if is_doc_path(&s) {
                            self.cur_path = Some(s);
                        }
                        self.mpos = (base + e).max(at + 1);
                    }
                    None => {
                        if !last && d.len() - li < 16 + 2 * 4096 {
                            break; // the string may continue in the next piece
                        }
                        self.mpos = at + 1;
                    }
                }
            } else {
                match face_extent(d, li) {
                    Extent::NeedMore if !last => break,
                    Extent::Complete => match face_at(d, li) {
                        Some((mut face, next)) => {
                            face.offset = at;
                            visit(self.cur_path.as_deref(), face)?;
                            self.fpos = base + next;
                        }
                        None => self.fpos = at + 1,
                    },
                    _ => self.fpos = at + 1,
                }
            }
        }
        Ok(self.mpos.min(self.fpos).saturating_sub(LOOKBACK))
    }
}

/// Feed a display-list stream piece by piece (e.g. straight from the decompressor).
/// Memory use is bounded by the largest face record plus [`LOOKBACK`] and the piece size.
pub struct StreamDecoder<'a> {
    dec: Decoder,
    buf: Vec<u8>,
    base: usize,
    keep: usize,
    visit: &'a mut dyn FnMut(Option<&str>, FacePatch) -> crate::Result<()>,
    ctl: &'a crate::Ctl,
}

impl<'a> StreamDecoder<'a> {
    pub fn new(ctl: &'a crate::Ctl, visit: &'a mut dyn FnMut(Option<&str>, FacePatch) -> crate::Result<()>) -> Self {
        StreamDecoder { dec: Decoder::default(), buf: Vec::new(), base: 0, keep: 0, visit, ctl }
    }

    pub fn feed(&mut self, chunk: &[u8]) -> crate::Result<()> {
        // compact once enough consumed data has accumulated
        let drop = self.keep.saturating_sub(self.base).min(self.buf.len());
        if drop >= 32 << 20 || (drop > 0 && drop * 2 >= self.buf.len()) {
            self.buf.drain(..drop);
            self.base += drop;
        }
        crate::ctl::reserve(&mut self.buf, chunk.len(), "analyse du maillage")?;
        self.buf.extend_from_slice(chunk);
        let ctl = self.ctl;
        self.keep = self.dec.run(&self.buf, self.base, false, self.visit, &mut |_| ctl.check())?;
        Ok(())
    }

    pub fn finish(mut self) -> crate::Result<()> {
        let ctl = self.ctl;
        self.dec.run(&self.buf, self.base, true, self.visit, &mut |_| ctl.check())?;
        Ok(())
    }

    /// Current size of the internal buffer (diagnostics).
    pub fn buffer_len(&self) -> usize {
        self.buf.len()
    }
}
