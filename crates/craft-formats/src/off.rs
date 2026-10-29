//! Object File Format (.off, also COFF / NOFF headers): vertices and polygons (fan-triangulated),
//! optional per-face colours ignored. Unitless: read as millimetres like STL.
use crate::{single_mesh_scene, Loaded};
use std::path::Path;

pub fn load(path: &Path, bytes: &[u8]) -> Result<Loaded, String> {
    let text = String::from_utf8_lossy(bytes);
    // strip comments
    let mut toks = text
        .lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .flat_map(|l| l.split_whitespace());
    let head = toks.next().ok_or("fichier OFF vide")?;
    let mut first: Option<&str> = None;
    let up = head.to_ascii_uppercase();
    if !up.ends_with("OFF") {
        if up.starts_with(|c: char| c.is_ascii_digit()) {
            first = Some(head); // header line missing, counts directly
        } else {
            return Err("en-tête OFF absent".into());
        }
    }
    if up.contains("4") || up.starts_with("N") && up.contains("ND") {
        return Err("variante OFF non gérée (4OFF / nOFF)".into());
    }
    let has_tex = up.contains("ST");
    let has_nrm = up.contains('N');
    let has_col = up.contains('C');
    let mut num = |what: &str| -> Result<f64, String> {
        let t = first
            .take()
            .or_else(|| toks.next())
            .ok_or_else(|| format!("fin de fichier inattendue ({what})"))?;
        t.parse::<f64>()
            .map_err(|_| format!("nombre attendu ({what}) : « {t} »"))
    };
    let nv = num("sommets")? as usize;
    let nf = num("faces")? as usize;
    let _ne = num("arêtes")?;
    if nv > 50_000_000 || nf > 100_000_000 {
        return Err("fichier OFF trop grand".into());
    }
    let extra =
        if has_nrm { 3 } else { 0 } + if has_col { 4 } else { 0 } + if has_tex { 2 } else { 0 };
    let mut pos = Vec::with_capacity(nv);
    for _ in 0..nv {
        let p = [num("x")? as f32, num("y")? as f32, num("z")? as f32];
        for _ in 0..extra {
            num("attribut")?;
        }
        pos.push(p);
    }
    let mut tris = Vec::with_capacity(nf * 2);
    // per-face colour values follow the indices; read the whole remaining line-free stream
    let rest: Vec<&str> = toks.collect();
    let mut i = 0;
    for _ in 0..nf {
        let Some(n) = rest.get(i).and_then(|t| t.parse::<usize>().ok()) else {
            break;
        };
        i += 1;
        let idx: Vec<u32> = rest
            .get(i..i + n)
            .unwrap_or(&[])
            .iter()
            .filter_map(|t| t.parse().ok())
            .collect();
        i += n;
        // skip optional face colour: tokens until the next face would start are ambiguous, so
        // only skip when they are clearly not integers or when 3/4 floats follow
        while let Some(t) = rest.get(i) {
            if t.contains('.') {
                i += 1;
            } else {
                break;
            }
        }
        for k in 1..idx.len().saturating_sub(1) {
            let t = [idx[0], idx[k], idx[k + 1]];
            if t.iter().all(|&v| (v as usize) < nv) {
                tris.push(t);
            }
        }
    }
    if tris.is_empty() {
        return Err("aucune face dans le fichier OFF".into());
    }
    Ok(single_mesh_scene(path, "OFF", &pos, &tris, None, 0.001))
}

#[cfg(test)]
mod tests {
    #[test]
    fn cube() {
        let s = "OFF\n# cube\n8 6 0\n0 0 0\n1 0 0\n1 1 0\n0 1 0\n0 0 1\n1 0 1\n1 1 1\n0 1 1\n4 0 3 2 1\n4 4 5 6 7\n4 0 1 5 4\n4 1 2 6 5\n4 2 3 7 6\n4 3 0 4 7\n";
        let sc = super::load(std::path::Path::new("c.off"), s.as_bytes()).unwrap();
        assert_eq!(sc.meshes[0].mesh.triangle_count(), 12);
    }
}
