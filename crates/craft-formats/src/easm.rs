//! eDrawings files (.easm/.eprt, "2008+ archive"): clean-room, best effort.
//!
//! The container is a small header followed by zip local-file records (`eModel`, `preview.jpg`,
//! `materials.xml`, `scene.xml`). The geometry (`eModel`) is a HOOPS Stream File, which has no
//! open reader we can use, so only the embedded preview image and the material list are shown.

use crate::{empty_scene, Loaded};
use std::io::Read;
use std::path::Path;
use sw_format::Rgba;

/// (name, decompressed bytes) of each zip local record found in the file.
pub fn entries(b: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = b[i..].windows(4).position(|w| w == b"PK\x03\x04") {
        let o = i + p;
        i = o + 4;
        let h = |k: usize| b.get(o + k..o + k + 2).map(|x| u16::from_le_bytes([x[0], x[1]]) as usize);
        let w = |k: usize| b.get(o + k..o + k + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]) as usize);
        let (Some(method), Some(cs), Some(nl), Some(el)) = (h(8), w(18), h(26), h(28)) else { break };
        let name = String::from_utf8_lossy(b.get(o + 30..o + 30 + nl).unwrap_or(&[])).into_owned();
        let start = o + 30 + nl + el;
        let Some(data) = b.get(start..start + cs) else { continue };
        let bytes = match method {
            0 => data.to_vec(),
            8 => {
                let mut v = Vec::new();
                if flate2::read::DeflateDecoder::new(data).read_to_end(&mut v).is_err() {
                    continue;
                }
                v
            }
            _ => continue,
        };
        out.push((name, bytes));
        i = start + cs;
    }
    out
}

pub fn load(path: &Path, b: &[u8]) -> Result<Loaded, String> {
    if !b.windows(12).take(64).any(|w| w == b"2008+ archiv") {
        return Err("fichier eDrawings d'un format inconnu".into());
    }
    let ents = entries(b);
    let mut s = empty_scene(path, "eDrawings");
    s.stream_count = ents.len();
    if let Some((_, jpg)) = ents.iter().find(|(n, _)| n.eq_ignore_ascii_case("preview.jpg") || n.eq_ignore_ascii_case("preview.png")) {
        if let Ok(img) = image::load_from_memory(jpg) {
            let img = img.to_rgba8();
            s.preview = Some(Rgba { width: img.width(), height: img.height(), pixels: img.into_raw() });
        }
    }
    let hsf = ents.iter().find(|(n, _)| n == "eModel").map(|(_, d)| String::from_utf8_lossy(&d[..d.len().min(13)]).trim().to_string());
    s.warnings.push(format!(
        "Géométrie eDrawings ({}) non décodée : seule la vignette intégrée est affichée. Ouvrez l'assemblage .SLDASM d'origine pour la 3D.",
        hsf.unwrap_or_else(|| "format inconnu".into()).trim_start_matches(";; ")
    ));
    Ok(s)
}
