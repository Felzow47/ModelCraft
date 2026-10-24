//! Wavefront OBJ with `o`/`g` objects and `usemtl` colours from the `.mtl` library (Kd).

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
    for lib in &mtllibs {
        if let Ok(t) = std::fs::read_to_string(dir.join(lib)) {
            parse_mtl(&t, &mut colors);
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
    let scale = 0.001; // OBJ has no unit: CAD exports are almost always millimetres
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

fn parse_mtl(t: &str, out: &mut HashMap<String, [f32; 4]>) {
    let mut cur = String::new();
    for line in t.lines() {
        let mut it = line.split_ascii_whitespace();
        match it.next() {
            Some("newmtl") => cur = it.collect::<Vec<_>>().join(" "),
            Some("Kd") => {
                let c: Vec<f32> = it.filter_map(|s| s.parse().ok()).collect();
                if c.len() >= 3 {
                    let a = out.get(&cur).map(|x| x[3]).unwrap_or(1.0);
                    out.insert(cur.clone(), [c[0], c[1], c[2], a]);
                }
            }
            Some("d") => {
                if let Some(d) = it.next().and_then(|s| s.parse::<f32>().ok()) {
                    out.entry(cur.clone()).or_insert([0.8, 0.8, 0.8, 1.0])[3] = d;
                }
            }
            _ => {}
        }
    }
}
