//! Wavefront OBJ with `o`/`g` objects and `usemtl` colours from the `.mtl` library: `Kd`
//! diffuse colour, `d` / `Tr` opacity, and for textured materials (Blender, SketchUp exports)
//! the average colour of the `map_Kd` image, so each material keeps a recognisable colour
//! (textures themselves are not mapped).

use crate::{component, empty_scene, stem, Loaded};
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;
use sw_format::{MeshSource, SceneMesh};

struct Group {
    name: String,
    material: String,
    tris: Vec<[u32; 3]>,
}

pub fn load(path: &Path, bytes: &[u8]) -> Result<Loaded, String> {
    let t0 = Instant::now();
    let text = String::from_utf8_lossy(bytes);
    let mut v: Vec<[f32; 3]> = Vec::new();
    let mut groups: Vec<Group> = vec![Group { name: stem(path), material: String::new(), tris: Vec::new() }];
    let mut mtllibs: Vec<String> = Vec::new();
    let mut cur_obj = stem(path);
    let mut cur_mat = String::new();
    let mut poly: Vec<u32> = Vec::new();
    // Blender writes metres ("# Blender 4.2.0" / "# Blender v2.93 OBJ File"); CAD exports mm
    let blender = text.lines().take(5).any(|l| l.starts_with('#') && l.contains("Blender"));
    for line in text.lines() {
        let line = line.trim();
        let mut it = line.split_ascii_whitespace();
        match it.next() {
            Some("v") => {
                let mut p = [0f32; 3];
                for c in &mut p {
                    *c = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                }
                v.push(p);
            }
            Some("f") => {
                poly.clear();
                for tok in it {
                    let idx = tok.split('/').next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                    let i = if idx < 0 { v.len() as i64 + idx } else { idx - 1 };
                    if i >= 0 && (i as usize) < v.len() {
                        poly.push(i as u32);
                    }
                }
                let g = groups.last_mut().unwrap();
                for k in 1..poly.len().saturating_sub(1) {
                    g.tris.push([poly[0], poly[k], poly[k + 1]]);
                }
            }
            Some("o") | Some("g") => {
                let name: String = it.collect::<Vec<_>>().join(" ");
                if !name.is_empty() {
                    cur_obj = name;
                    groups.push(Group { name: cur_obj.clone(), material: cur_mat.clone(), tris: Vec::new() });
                }
            }
            Some("usemtl") => {
                cur_mat = it.collect::<Vec<_>>().join(" ");
                groups.push(Group { name: cur_obj.clone(), material: cur_mat.clone(), tris: Vec::new() });
            }
            Some("mtllib") => mtllibs.push(it.collect::<Vec<_>>().join(" ")),
            _ => {}
        }
    }
    if v.is_empty() {
        return Err("aucun sommet dans le fichier OBJ".into());
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut colors: HashMap<String, [f32; 4]> = HashMap::new();
    if mtllibs.is_empty() {
        mtllibs.push(format!("{}.mtl", stem(path)));
    }
    for lib in &mtllibs {
        if let Some(f) = find_file(dir, lib) {
            if let Ok(t) = std::fs::read_to_string(&f) {
                parse_mtl(&t, f.parent().unwrap_or(dir), &mut colors);
            }
        }
    }
    // merge groups by (object, material)
    let mut merged: Vec<Group> = Vec::new();
    for g in groups.into_iter().filter(|g| !g.tris.is_empty()) {
        match merged.iter_mut().find(|m| m.name == g.name && m.material == g.material) {
            Some(m) => m.tris.extend(g.tris),
            None => merged.push(g),
        }
    }
    let mut s = empty_scene(path, "OBJ");
    s.up_z = false; // OBJ is conventionally Y-up
    // OBJ has no unit: CAD exports are almost always millimetres, Blender works in metres
    let scale = if blender { 1.0 } else { 0.001 };
    let objects: Vec<String> = {
        let mut o: Vec<String> = Vec::new();
        for g in &merged {
            if !o.contains(&g.name) {
                o.push(g.name.clone());
            }
        }
        o
    };
    let tm = Instant::now();
    for oname in &objects {
        let parts: Vec<&Group> = merged.iter().filter(|g| &g.name == oname).collect();
        let parent = if parts.len() == 1 {
            None
        } else {
            s.components.push(component(oname, 0, None, true, None));
            Some(s.components.len() - 1)
        };
        for g in parts {
            // compact the vertices used by this group
            let mut map: HashMap<u32, u32> = HashMap::new();
            let mut pos: Vec<[f32; 3]> = Vec::new();
            let tris: Vec<[u32; 3]> = g
                .tris
                .iter()
                .map(|t| {
                    t.map(|i| {
                        *map.entry(i).or_insert_with(|| {
                            let p = v[i as usize];
                            pos.push([p[0] * scale, p[1] * scale, p[2] * scale]);
                            (pos.len() - 1) as u32
                        })
                    })
                })
                .collect();
            let mesh = sw_format::mesh::build_from_triangles(&pos, &tris, 30.0, true);
            let label = if g.material.is_empty() { g.name.clone() } else { format!("{} [{}]", g.name, g.material) };
            s.meshes.push(SceneMesh { name: label.clone(), mesh, source: MeshSource::Embedded, color: colors.get(&g.material).copied() });
            let depth = if parent.is_some() { 1 } else { 0 };
            s.components.push(component(&label, depth, parent, false, Some(s.meshes.len() - 1)));
        }
    }
    s.timings.mesh = tm.elapsed();
    s.timings.decode = t0.elapsed() - s.timings.mesh;
    Ok(s)
}

/// Resolve a file named in an OBJ/MTL (relative, Windows separators, wrong case, or an absolute
/// path from the author's machine) next to the file that names it.
fn find_file(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    let name = name.trim().trim_matches('"').replace('\\', "/");
    if name.is_empty() {
        return None;
    }
    let direct = dir.join(&name);
    if direct.is_file() {
        return Some(direct);
    }
    let base = name.rsplit('/').next().unwrap_or(&name).to_string();
    let lower = base.to_lowercase();
    for d in [dir.to_path_buf(), dir.join("textures"), dir.join("Textures")] {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().to_lowercase() == lower {
                return Some(e.path());
            }
        }
    }
    None
}

/// Average colour of a texture (linear-ish 0..1), from a small downscaled copy.
fn texture_average(path: &Path) -> Option<[f32; 3]> {
    if std::fs::metadata(path).ok()?.len() > 64 << 20 {
        return None;
    }
    let img = image::open(path).ok()?.thumbnail(32, 32).to_rgb8();
    let n = (img.width() * img.height()).max(1) as f32;
    let mut acc = [0f32; 3];
    for p in img.pixels() {
        for k in 0..3 {
            acc[k] += p[k] as f32 / 255.0;
        }
    }
    Some(acc.map(|c| c / n))
}

fn parse_mtl(t: &str, dir: &Path, out: &mut HashMap<String, [f32; 4]>) {
    let mut cur = String::new();
    // materials that only have a texture: Kd defaults to white (Blender writes 0.8)
    let mut kd_seen: HashMap<String, bool> = HashMap::new();
    for line in t.lines() {
        let line = line.trim();
        let mut it = line.split_ascii_whitespace();
        match it.next() {
            Some("newmtl") => {
                cur = it.collect::<Vec<_>>().join(" ");
                out.entry(cur.clone()).or_insert([0.8, 0.8, 0.8, 1.0]);
            }
            Some("Kd") => {
                let c: Vec<f32> = it.filter_map(|s| s.parse().ok()).collect();
                if c.len() >= 3 {
                    let e = out.entry(cur.clone()).or_insert([0.8, 0.8, 0.8, 1.0]);
                    e[..3].copy_from_slice(&[c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0)]);
                    kd_seen.insert(cur.clone(), true);
                }
            }
            Some("d") => {
                if let Some(d) = it.next().and_then(|s| s.parse::<f32>().ok()) {
                    out.entry(cur.clone()).or_insert([0.8, 0.8, 0.8, 1.0])[3] = d.clamp(0.05, 1.0);
                }
            }
            Some("Tr") => {
                if let Some(tr) = it.next().and_then(|s| s.parse::<f32>().ok()) {
                    out.entry(cur.clone()).or_insert([0.8, 0.8, 0.8, 1.0])[3] = (1.0 - tr).clamp(0.05, 1.0);
                }
            }
            Some("map_Kd") => {
                // options (-s 1 1 1, -bm …) come first; the file name is the rest after them
                let toks: Vec<&str> = it.collect();
                let mut k = 0;
                while k < toks.len() && toks[k].starts_with('-') {
                    k += match toks[k] {
                        "-s" | "-o" | "-t" => 4,
                        "-mm" => 3,
                        _ => 2,
                    };
                }
                let name = toks.get(k..).map(|t| t.join(" ")).unwrap_or_default();
                if let Some(avg) = find_file(dir, &name).and_then(|f| texture_average(&f)) {
                    let e = out.entry(cur.clone()).or_insert([0.8, 0.8, 0.8, 1.0]);
                    // Blender writes Kd 0.8 with a texture: the texture is the colour
                    let kd = if kd_seen.get(&cur).copied().unwrap_or(false) && !(e[0] == 0.8 && e[1] == 0.8 && e[2] == 0.8) { [e[0], e[1], e[2]] } else { [1.0; 3] };
                    e[0] = kd[0] * avg[0];
                    e[1] = kd[1] * avg[1];
                    e[2] = kd[2] * avg[2];
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blender_obj_with_mtl_and_texture() {
        let dir = std::env::temp_dir().join(format!("mc_obj_test_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        // 2×2 pure red texture
        let img = image::RgbImage::from_pixel(2, 2, image::Rgb([255, 0, 0]));
        img.save(dir.join("textures").join("Red.png")).unwrap();
        std::fs::write(
            dir.join("cube.mtl"),
            "# Blender MTL File\nnewmtl Blue\nKd 0.0 0.0 1.0\nd 0.5\nnewmtl Tex\nKd 0.8 0.8 0.8\nmap_Kd -s 1 1 1 textures\\red.png\n",
        )
        .unwrap();
        let obj = "# Blender 4.2.0\nmtllib cube.mtl\no Cube\nv 0 0 0\nv 2 0 0\nv 2 2 0\nv 0 2 0\nusemtl Blue\nf 1 2 3\nusemtl Tex\nf 1 3 4\n";
        let p = dir.join("cube.obj");
        std::fs::write(&p, obj).unwrap();
        let s = load(&p, obj.as_bytes()).unwrap();
        let col = |m: &str| s.meshes.iter().find(|x| x.name.contains(m)).and_then(|x| x.color).unwrap();
        assert_eq!(col("Blue"), [0.0, 0.0, 1.0, 0.5]);
        let t = col("Tex");
        assert!(t[0] > 0.95 && t[1] < 0.05 && t[2] < 0.05, "texture average {t:?}");
        // Blender works in metres: a 2 m quad stays 2 m
        let hi = s.meshes.iter().map(|m| m.mesh.bbox_max[0]).fold(0f32, f32::max);
        assert!((hi - 2.0).abs() < 1e-4, "{hi}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
