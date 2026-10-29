//! Graphics bodies stored as triangle strips inside `Contents/Config-N-ResolvedFeatures`
//! (older SolidWorks, e.g. an STL imported as "Graphiques STL1" / graphics body, saved without
//! tessellation in `Contents/DisplayLists` and without `MeshData` streams).
//!
//! Layout, from observation (little endian). Arrays are tagged with a 16-byte header
//! `u32 elem_size, u32 kind, u32 2, u32 count` followed by `count * elem_size` bytes:
//!
//! * `4, 8, 2, K`: K strip lengths (i32, vertices per strip),
//! * `12, 100, 2, S`: S = sum of the strip lengths, f32 xyz positions,
//! * `12, 100, 2, S`: the matching f32 vertex normals.
//!
//! A graphics body is split into blocks of up to 1024 strips; each block is self-contained.
//! Triangles follow the usual strip rule (winding flips every other triangle); degenerate
//! triangles (repeated vertices joining strips) are dropped. Blocks are found by scanning
//! for the strip-length header and checking that the two arrays that follow match its sum,
//! so unrelated bytes are never decoded.

use crate::meshdata::MeshBodies;

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}
fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

const STRIP_HDR: [u8; 12] = [4, 0, 0, 0, 8, 0, 0, 0, 2, 0, 0, 0];

fn arr_hdr(b: &[u8], o: usize, size: u32, kind: u32, n: u64) -> bool {
    u32_at(b, o) == Some(size) && u32_at(b, o + 4) == Some(kind) && u32_at(b, o + 8) == Some(2) && u32_at(b, o + 12).map(|x| x as u64) == Some(n)
}

/// Decode every strip block in `bytes`. Each block becomes one body of [`MeshBodies`].
pub fn decode(bytes: &[u8]) -> MeshBodies {
    let mut out = MeshBodies::default();
    let finder = memchr::memmem::Finder::new(&STRIP_HDR);
    let mut pos = 0usize;
    while let Some(rel) = finder.find(&bytes[pos..]) {
        let p = pos + rel;
        pos = p + 1;
        let Some(k) = u32_at(bytes, p + 12) else { break };
        let k = k as usize;
        if k == 0 || k > 50_000_000 {
            continue;
        }
        let lens_at = p + 16;
        let Some(lens) = bytes.get(lens_at..lens_at + 4 * k) else { continue };
        let mut sum = 0u64;
        let mut ok = true;
        for c in lens.chunks_exact(4) {
            let v = i32::from_le_bytes(c.try_into().unwrap());
            if v < 0 {
                ok = false;
                break;
            }
            sum += v as u64;
        }
        if !ok || sum < 3 {
            continue;
        }
        let pos_at = lens_at + 4 * k;
        if !arr_hdr(bytes, pos_at, 12, 100, sum) {
            continue;
        }
        let nrm_at = pos_at + 16 + 12 * sum as usize;
        if nrm_at + 16 + 12 * sum as usize > bytes.len() || !arr_hdr(bytes, nrm_at, 12, 100, sum) {
            continue;
        }
        // a valid block
        let base = out.positions.len() as u32;
        let s = sum as usize;
        out.body_start.push(out.triangles.len() as u32);
        out.positions.reserve(s);
        let mut finite = true;
        for i in 0..s {
            let o = pos_at + 16 + 12 * i;
            let v = [f32_at(bytes, o), f32_at(bytes, o + 4), f32_at(bytes, o + 8)];
            finite &= v.iter().all(|x| x.is_finite());
            out.positions.push(if finite { v } else { [0.0; 3] });
        }
        let mut start = 0u32;
        for c in lens.chunks_exact(4) {
            let n = u32::from_le_bytes(c.try_into().unwrap());
            for i in 0..n.saturating_sub(2) {
                let (mut a, mut b) = (start + i, start + i + 1);
                let c = start + i + 2;
                if i % 2 == 1 {
                    std::mem::swap(&mut a, &mut b);
                }
                let (pa, pb, pc) = (out.positions[(base + a) as usize], out.positions[(base + b) as usize], out.positions[(base + c) as usize]);
                if pa == pb || pb == pc || pa == pc {
                    continue;
                }
                out.triangles.push([base + a, base + b, base + c]);
            }
            start += n;
        }
        pos = nrm_at + 16 + 12 * s;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn block(o: &mut Vec<u8>, strips: &[&[[f32; 3]]]) {
        o.extend_from_slice(&STRIP_HDR);
        o.extend_from_slice(&(strips.len() as u32).to_le_bytes());
        let mut s = 0u32;
        for st in strips {
            o.extend_from_slice(&(st.len() as u32).to_le_bytes());
            s += st.len() as u32;
        }
        for _ in 0..2 {
            for v in [12u32, 100, 2, s] {
                o.extend_from_slice(&v.to_le_bytes());
            }
            for st in strips {
                for p in st.iter() {
                    for c in p {
                        o.extend_from_slice(&c.to_le_bytes());
                    }
                }
            }
        }
        o.extend_from_slice(&[0, 0, 0, 0, 4, 0, 0, 0, 8, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
    }
    #[test]
    fn strips_two_blocks() {
        let q: &[[f32; 3]] = &[[0., 0., 0.], [0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [1., 1., 0.]];
        let mut o = vec![9u8; 7];
        block(&mut o, &[q, q]);
        o.extend_from_slice(&[1, 2, 3]);
        block(&mut o, &[q]);
        let m = decode(&o);
        assert_eq!(m.body_start, vec![0, 4]);
        assert_eq!(m.triangles.len(), 6);
        // first triangle is degenerate (repeated vertex), then the winding alternates
        assert_eq!(m.triangles[0], [2, 1, 3]);
        assert_eq!(m.triangles[1], [2, 3, 4]);
    }
}
