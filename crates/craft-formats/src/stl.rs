//! STL (ASCII and binary).

use crate::{single_mesh_scene, Loaded};
use std::path::Path;

pub fn load(path: &Path, bytes: &[u8]) -> Result<Loaded, String> {
    let (pos, color) = if is_binary(bytes) {
        binary(bytes)?
    } else {
        ascii(bytes)?
    };
    if pos.is_empty() {
        return Err("aucun triangle dans le fichier STL".into());
    }
    let tris: Vec<[u32; 3]> = (0..pos.len() as u32 / 3)
        .map(|i| [3 * i, 3 * i + 1, 3 * i + 2])
        .collect();
    Ok(single_mesh_scene(path, "STL", &pos, &tris, color, 0.001))
}

fn is_binary(b: &[u8]) -> bool {
    if b.len() < 84 {
        return false;
    }
    let n = u32::from_le_bytes([b[80], b[81], b[82], b[83]]) as usize;
    if 84 + n * 50 == b.len() {
        return true;
    }
    // ASCII files start with "solid" and contain "facet"; some binary files also start with "solid".
    let head = &b[..b.len().min(512)];
    !(head.starts_with(b"solid") && head.windows(5).any(|w| w.eq_ignore_ascii_case(b"facet")))
}

type Soup = (Vec<[f32; 3]>, Option<[f32; 4]>);

fn binary(b: &[u8]) -> Result<Soup, String> {
    let n = u32::from_le_bytes([b[80], b[81], b[82], b[83]]) as usize;
    let n = n.min((b.len() - 84) / 50);
    // Magics/SolidView put "COLOR=" + RGBA in the header for a default colour.
    let color = b[..80].windows(10).find(|w| &w[..6] == b"COLOR=").map(|w| {
        [
            w[6] as f32 / 255.0,
            w[7] as f32 / 255.0,
            w[8] as f32 / 255.0,
            1.0,
        ]
    });
    let mut pos = Vec::with_capacity(n * 3);
    let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    for i in 0..n {
        let o = 84 + i * 50 + 12;
        for v in 0..3 {
            let q = o + v * 12;
            pos.push([f(q), f(q + 4), f(q + 8)]);
        }
    }
    Ok((pos, color))
}

fn ascii(b: &[u8]) -> Result<Soup, String> {
    let text = String::from_utf8_lossy(b);
    let mut pos = Vec::new();
    let mut it = text.split_ascii_whitespace();
    while let Some(tok) = it.next() {
        if tok.eq_ignore_ascii_case("vertex") {
            let mut p = [0f32; 3];
            for c in &mut p {
                *c = it
                    .next()
                    .and_then(|s| s.parse().ok())
                    .ok_or("STL ASCII : coordonnée invalide")?;
            }
            pos.push(p);
        }
    }
    pos.truncate(pos.len() / 3 * 3);
    Ok((pos, None))
}
