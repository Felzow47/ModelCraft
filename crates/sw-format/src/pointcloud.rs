//! ScanTo3D point clouds (`ThirdPty/SWNGREStream`, "####SWNGRE MODEL#### V1.3").
//!
//! Layout (clean-room, from sample files): UTF-16LE tagged text sections
//! (`$Model Begin`, `$Point Cloud Begin`, ...) then `@Point Begin:<N>` (UTF-16 digits)
//! followed by N fixed 27-byte records: 3 tag bytes (`40 80 80`), position xyz (f32, metres),
//! normal xyz (f32, usually zero). The cloud ends with `$Point Cloud End`.
//!
//! ModelCraft has no point primitive, so the cloud is shown as tiny splats (three orthogonal
//! triangles per point), uniformly sub-sampled to stay inside the vertex budget.

use crate::mesh::TriMesh;

pub const STREAM: &str = "ThirdPty/SWNGREStream";
const MAGIC: &str = "####SWNGRE MODEL####";
const REC: usize = 27;

/// A decoded cloud: all points (metres).
pub struct Cloud {
    pub points: Vec<[f32; 3]>,
    /// Points announced by the headers (may exceed `points` if a block was truncated).
    pub declared: usize,
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

pub fn is_cloud(d: &[u8]) -> bool {
    let m = utf16(MAGIC);
    memchr::memmem::find(&d[..d.len().min(256)], &m).is_some()
}

/// Parse every `@Point Begin:<N>` block of the stream.
pub fn decode(d: &[u8]) -> Option<Cloud> {
    if !is_cloud(d) {
        return None;
    }
    let tag = utf16("@Point Begin:");
    let mut points = Vec::new();
    let mut declared = 0usize;
    let mut pos = 0usize;
    while let Some(i) = memchr::memmem::find(&d[pos..], &tag).map(|i| pos + i + tag.len()) {
        // UTF-16 decimal count
        let mut j = i;
        let mut n: usize = 0;
        while j + 1 < d.len() && d[j + 1] == 0 && d[j].is_ascii_digit() {
            n = n.saturating_mul(10).saturating_add((d[j] - b'0') as usize);
            j += 2;
        }
        if j == i {
            pos = i;
            continue;
        }
        declared += n;
        let avail = (d.len() - j) / REC;
        let take = n.min(avail);
        points.reserve(take);
        for r in 0..take {
            let o = j + r * REC + 3;
            let f = |k: usize| f32::from_le_bytes(d[o + 4 * k..o + 4 * k + 4].try_into().unwrap());
            let p = [f(0), f(1), f(2)];
            if p.iter().all(|v| v.is_finite() && v.abs() < 1e7) {
                points.push(p);
            }
        }
        pos = j + take * REC;
    }
    (!points.is_empty()).then_some(Cloud { points, declared })
}

/// Splat mesh of at most `max_points` points (uniform sub-sampling).
pub fn splats(c: &Cloud, max_points: usize) -> TriMesh {
    let n = c.points.len();
    let keep = n.min(max_points.max(1));
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in &c.points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let ext = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    let diag = (ext[0] * ext[0] + ext[1] * ext[1] + ext[2] * ext[2]).sqrt().max(1e-6);
    // surface-ish spacing between kept points; a splat covers about one spacing
    let h = (diag / (keep as f32).sqrt() * 0.6).clamp(diag * 1e-5, diag * 0.01);
    let mut m = TriMesh::new_empty();
    m.positions.reserve(keep * 9);
    m.normals.reserve(keep * 9);
    m.indices.reserve(keep * 9);
    let axes: [([f32; 3], [f32; 3], [f32; 3]); 3] = [
        ([h, 0.0, 0.0], [0.0, h, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, h, 0.0], [0.0, 0.0, h], [1.0, 0.0, 0.0]),
        ([0.0, 0.0, h], [h, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ];
    for s in 0..keep {
        let p = c.points[(s as u64 * n as u64 / keep as u64) as usize];
        for (u, v, nrm) in &axes {
            let base = m.positions.len() as u32;
            let a = [p[0] - 0.5 * u[0] - 0.29 * v[0], p[1] - 0.5 * u[1] - 0.29 * v[1], p[2] - 0.5 * u[2] - 0.29 * v[2]];
            let b = [p[0] + 0.5 * u[0] - 0.29 * v[0], p[1] + 0.5 * u[1] - 0.29 * v[1], p[2] + 0.5 * u[2] - 0.29 * v[2]];
            let t = [p[0] + 0.58 * v[0], p[1] + 0.58 * v[1], p[2] + 0.58 * v[2]];
            m.positions.extend_from_slice(&[a, b, t]);
            m.normals.extend_from_slice(&[*nrm, *nrm, *nrm]);
            m.indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
    }
    m.face_start.push(0);
    m.face_count = 1;
    m.bbox_min = [lo[0] - h, lo[1] - h, lo[2] - h];
    m.bbox_max = [hi[0] + h, hi[1] + h, hi[2] + h];
    m.finish();
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth(n: usize) -> Vec<u8> {
        let mut d = utf16("####SWNGRE MODEL#### V1.3\n");
        d.extend(utf16("$Point Cloud Begin "));
        d.extend(utf16(&format!("@Point Begin:{n}")));
        for i in 0..n {
            d.extend_from_slice(&[0x40, 0x80, 0x80]);
            for v in [i as f32, 2.0 * i as f32, -1.0, 0.0, 0.0, 0.0] {
                d.extend_from_slice(&v.to_le_bytes());
            }
        }
        d.extend_from_slice(&32u32.to_le_bytes());
        d.extend(utf16("$Point Cloud End"));
        d
    }

    #[test]
    fn decodes_records() {
        let c = decode(&synth(1000)).unwrap();
        assert_eq!(c.points.len(), 1000);
        assert_eq!(c.declared, 1000);
        assert_eq!(c.points[7], [7.0, 14.0, -1.0]);
        let m = splats(&c, 100);
        assert_eq!(m.triangle_count(), 300);
        assert!(m.bbox_max[1] >= 1998.0);
    }

    #[test]
    fn rejects_other_streams() {
        assert!(decode(b"hello world, not a cloud at all......").is_none());
    }
}
