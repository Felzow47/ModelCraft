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
    // 1. document path markers
    let mut paths: Vec<(usize, String)> = Vec::new();
    let marker = memchr::memmem::Finder::new(&[0xff, 0xfe, 0xff]);
    let mut pos = 0;
    while let Some(r) = marker.find(&d[pos..]) {
        let o = pos + r;
        if let Some((s, end)) = cstring_at(d, o) {
            if is_doc_path(&s) {
                paths.push((o, s));
            }
            pos = end.max(o + 1);
        } else {
            pos = o + 1;
        }
    }

    // 2. faces
    let mut list = DisplayList::default();
    let mut cur = DlBlock::default();
    let mut pi = 0usize;
    let finder = memchr::memmem::Finder::new(&POS_HDR);
    let mut pos = 0usize;
    while let Some(r) = finder.find(&d[pos..]) {
        let i = pos + r;
        match face_at(d, i) {
            Some((face, next)) => {
                // switch block if a document path appeared before this face
                let mut new_path = None;
                while pi < paths.len() && paths[pi].0 < i {
                    new_path = Some(paths[pi].1.clone());
                    pi += 1;
                }
                if let Some(p) = new_path {
                    if cur.path.as_deref() != Some(p.as_str()) {
                        if !cur.faces.is_empty() {
                            list.blocks.push(std::mem::take(&mut cur));
                        }
                        cur = DlBlock { path: Some(p), faces: Vec::new() };
                    }
                }
                cur.faces.push(face);
                pos = next;
            }
            None => pos = i + 1,
        }
    }
    if !cur.faces.is_empty() {
        list.blocks.push(cur);
    }
    // Merge blocks referring to the same document (can be split by other strings).
    let mut merged: Vec<DlBlock> = Vec::new();
    for b in list.blocks {
        if let Some(m) = merged.iter_mut().find(|m| m.key() == b.key()) {
            m.faces.extend(b.faces);
        } else {
            merged.push(b);
        }
    }
    DisplayList { blocks: merged }
}
