//! `MeshData/Config-N-Mesh-*` streams (recent SolidWorks, mesh bodies, e.g. a scanned or
//! imported STL/OBJ kept as "graphics" / mesh B-rep bodies). These files often carry no
//! tessellation in `Contents/DisplayLists`: the mesh is only here.
//!
//! Layout, from observation (little endian). Every value is a tagged record that starts
//! with the 12-byte marker `02 00 00 00 FE FF FF FF FE FF FF FF`, followed by a u32 type:
//!
//! * type 1, u32 count = 3, 3 × f64: a vertex. A body starts with its vertex list
//!   (vertex count in the preceding type-0 record), indices below are local to the body.
//! * type 2, u32 count = 3, 3 × f64, u32: a facet normal, then the facet's first vertex
//!   index.
//! * type 0, u32 0, u32 value: the facet's second and third vertex indices come in the next
//!   two type-0 records (a third type-0 record closes the facet).
//!
//! Bodies are separated by small trailer records (bounding boxes, colours) that are skipped.
//! The decoder is streaming: it is fed the decompressed bytes chunk by chunk (the stream can
//! be several GB), and keeps only the vertices and triangles.

use crate::Result;

const MARK: [u8; 12] = [
    0x02, 0, 0, 0, 0xFE, 0xFF, 0xFF, 0xFF, 0xFE, 0xFF, 0xFF, 0xFF,
];
/// Longest record we read from a marker (type 2: 12 + 4 + 4 + 24 + 4).
const REC: usize = 48;

/// Triangle soup with per-body indexing resolved to global indices.
#[derive(Debug, Default)]
pub struct MeshBodies {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    /// First triangle of each body.
    pub body_start: Vec<u32>,
    /// Facets whose indices pointed outside their body (skipped).
    pub bad: usize,
}

#[derive(Default)]
pub struct Decoder {
    out: MeshBodies,
    /// Start (global vertex index) of the current body's vertices.
    base: usize,
    /// The last record was a vertex (consecutive vertices belong to the same body).
    in_vertices: bool,
    /// Facet under construction: indices gathered so far.
    facet: [u32; 3],
    facet_n: u8,
    carry: Vec<u8>,
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn f64_at(b: &[u8], o: usize) -> f64 {
    f64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    fn record(&mut self, r: &[u8]) -> Result<()> {
        let t = u32_at(r, 12);
        match t {
            1 if u32_at(r, 16) == 3 => {
                let p = [
                    f64_at(r, 20) as f32,
                    f64_at(r, 28) as f32,
                    f64_at(r, 36) as f32,
                ];
                if !p.iter().all(|v| v.is_finite()) {
                    return Ok(());
                }
                if !self.in_vertices {
                    // a new body
                    self.base = self.out.positions.len();
                    self.out.body_start.push(self.out.triangles.len() as u32);
                    self.in_vertices = true;
                }
                if self.out.positions.len() == self.out.positions.capacity() {
                    let extra = (self.out.positions.len() / 2).max(1 << 16);
                    crate::ctl::reserve(&mut self.out.positions, extra, "sommets")?;
                }
                self.out.positions.push(p);
                self.facet_n = 0;
            }
            2 if u32_at(r, 16) == 3 => {
                self.in_vertices = false;
                self.facet = [u32_at(r, 44), 0, 0];
                self.facet_n = 1;
            }
            0 if self.facet_n > 0 => {
                self.facet[self.facet_n as usize] = u32_at(r, 20);
                self.facet_n += 1;
                if self.facet_n == 3 {
                    self.facet_n = 0;
                    let n = (self.out.positions.len() - self.base) as u32;
                    let f = self.facet;
                    if f.iter().all(|&i| i < n) {
                        if self.out.triangles.len() == self.out.triangles.capacity() {
                            let extra = (self.out.triangles.len() / 2).max(1 << 16);
                            crate::ctl::reserve(&mut self.out.triangles, extra, "triangles")?;
                        }
                        let b = self.base as u32;
                        self.out.triangles.push([b + f[0], b + f[1], b + f[2]]);
                    } else {
                        self.out.bad += 1;
                    }
                }
            }
            _ => {
                self.facet_n = 0;
            }
        }
        Ok(())
    }

    /// Feed the next decompressed bytes.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<()> {
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(chunk);
        let finder = memchr::memmem::Finder::new(&MARK);
        let mut pos = 0usize;
        loop {
            match finder.find(&buf[pos..]) {
                None => {
                    // a marker may straddle the chunk boundary
                    self.carry = buf[pos.max(buf.len().saturating_sub(MARK.len() - 1))..].to_vec();
                    break;
                }
                Some(rel) => {
                    let p = pos + rel;
                    if p + REC > buf.len() {
                        self.carry = buf[p..].to_vec();
                        break;
                    }
                    self.record(&buf[p..p + REC])?;
                    pos = p + 1;
                }
            }
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<MeshBodies> {
        let tail = std::mem::take(&mut self.carry);
        let mut padded = tail;
        padded.extend_from_slice(&[0u8; REC]);
        let finder = memchr::memmem::Finder::new(&MARK);
        let mut pos = 0usize;
        let real = padded.len() - REC;
        while let Some(rel) = finder.find(&padded[pos..real]) {
            let p = pos + rel;
            self.record(&padded[p..p + REC])?;
            pos = p + 1;
        }
        self.out.positions.shrink_to_fit();
        self.out.triangles.shrink_to_fit();
        Ok(self.out)
    }
}

/// Decode a whole stream held in memory (tests, small files).
pub fn decode(bytes: &[u8]) -> Result<MeshBodies> {
    let mut d = Decoder::new();
    d.feed(bytes)?;
    d.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn vtx(o: &mut Vec<u8>, p: [f64; 3]) {
        o.extend_from_slice(&MARK);
        o.extend_from_slice(&1u32.to_le_bytes());
        o.extend_from_slice(&3u32.to_le_bytes());
        for v in p {
            o.extend_from_slice(&v.to_le_bytes());
        }
    }
    fn facet(o: &mut Vec<u8>, i: [u32; 3]) {
        o.extend_from_slice(&MARK);
        o.extend_from_slice(&2u32.to_le_bytes());
        o.extend_from_slice(&3u32.to_le_bytes());
        for v in [0.0f64, 0.0, 1.0] {
            o.extend_from_slice(&v.to_le_bytes());
        }
        o.extend_from_slice(&i[0].to_le_bytes());
        for v in [i[1], i[2], 0] {
            o.extend_from_slice(&MARK);
            o.extend_from_slice(&0u32.to_le_bytes());
            o.extend_from_slice(&0u32.to_le_bytes());
            o.extend_from_slice(&v.to_le_bytes());
        }
    }
    fn sample() -> Vec<u8> {
        let mut o = vec![1, 2, 3];
        for b in 0..2 {
            let z = b as f64;
            vtx(&mut o, [0.0, 0.0, z]);
            vtx(&mut o, [1.0, 0.0, z]);
            vtx(&mut o, [1.0, 1.0, z]);
            vtx(&mut o, [0.0, 1.0, z]);
            facet(&mut o, [0, 1, 2]);
            facet(&mut o, [2, 3, 0]);
            facet(&mut o, [0, 9, 1]); // out of range
            o.extend_from_slice(&[0xAA; 37]); // trailer
        }
        o
    }
    #[test]
    fn two_bodies() {
        let m = decode(&sample()).unwrap();
        assert_eq!(m.positions.len(), 8);
        assert_eq!(
            m.triangles,
            vec![[0, 1, 2], [2, 3, 0], [4, 5, 6], [6, 7, 4]]
        );
        assert_eq!(m.body_start, vec![0, 2]);
        assert_eq!(m.bad, 2);
    }
    #[test]
    fn chunked_same_result() {
        let s = sample();
        for step in [1usize, 5, 13, 47, 48, 49, 100] {
            let mut d = Decoder::new();
            for c in s.chunks(step) {
                d.feed(c).unwrap();
            }
            let m = d.finish().unwrap();
            assert_eq!(m.triangles.len(), 4, "step {step}");
            assert_eq!(m.positions.len(), 8, "step {step}");
        }
    }
}
