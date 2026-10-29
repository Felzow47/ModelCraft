//! Autodesk 3D Studio (.3ds) chunk reader: named tri-mesh objects with their material colours.

use crate::{component, empty_scene, Loaded};
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;
use sw_format::{MeshSource, SceneMesh};

struct Obj {
    name: String,
    verts: Vec<[f32; 3]>,
    faces: Vec<[u32; 3]>,
    /// (material name, face indices)
    groups: Vec<(String, Vec<u32>)>,
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn f32_at(b: &[u8], o: usize) -> Option<f32> {
    Some(f32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn cstr(b: &[u8], o: usize) -> (String, usize) {
    let end = b[o.min(b.len())..].iter().position(|&c| c == 0).map(|p| o + p).unwrap_or(b.len());
    (String::from_utf8_lossy(&b[o.min(end)..end]).into_owned(), (end + 1).min(b.len()))
}

/// Iterate sub-chunks in `b[start..end]`: (id, data_start, chunk_end).
fn chunks(b: &[u8], start: usize, end: usize) -> Vec<(u16, usize, usize)> {
    let mut out = Vec::new();
    let mut o = start;
    while o + 6 <= end {
        let (Some(id), Some(len)) = (u16_at(b, o), u32_at(b, o + 2)) else { break };
        let len = len as usize;
        if len < 6 || o + len > end {
            break;
        }
        out.push((id, o + 6, o + len));
        o += len;
    }
    out
}

fn color(b: &[u8], start: usize, end: usize) -> Option<[f32; 4]> {
    for (id, d, _e) in chunks(b, start, end) {
        match id {
            0x0011 | 0x0012 => return Some([*b.get(d)? as f32 / 255.0, *b.get(d + 1)? as f32 / 255.0, *b.get(d + 2)? as f32 / 255.0, 1.0]),
            0x0010 | 0x0013 => return Some([f32_at(b, d)?, f32_at(b, d + 4)?, f32_at(b, d + 8)?, 1.0]),
            _ => {}
        }
    }
    None
}

pub fn load(path: &Path, b: &[u8]) -> Result<Loaded, String> {
    let t0 = Instant::now();
    if u16_at(b, 0) != Some(0x4D4D) {
        return Err("ce n'est pas un fichier 3DS (en-tête 0x4D4D absent)".into());
    }
    let total = (u32_at(b, 2).unwrap_or(0) as usize).min(b.len());
    let mut objs: Vec<Obj> = Vec::new();
    let mut mats: HashMap<String, [f32; 4]> = HashMap::new();
    let mut master_scale = 1.0f32;
    for (id, d, e) in chunks(b, 6, total) {
        if id != 0x3D3D {
            continue;
        }
        for (id2, d2, e2) in chunks(b, d, e) {
            match id2 {
                0x0100 => master_scale = f32_at(b, d2).unwrap_or(1.0),
                0xAFFF => {
                    let mut name = String::new();
                    let mut diffuse = None;
                    for (id3, d3, e3) in chunks(b, d2, e2) {
                        match id3 {
                            0xA000 => name = cstr(b, d3).0,
                            0xA020 => diffuse = color(b, d3, e3),
                            0xA050 => {
                                // transparency percentage
                                if let Some(c) = chunks(b, d3, e3).first() {
                                    let pct = u16_at(b, c.1).unwrap_or(0) as f32 / 100.0;
                                    if let Some(dc) = diffuse.as_mut() {
                                        let dc: &mut [f32; 4] = dc;
                                        dc[3] = 1.0 - pct;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    if let Some(c) = diffuse {
                        mats.insert(name, c);
                    }
                }
                0x4000 => {
                    let (name, after) = cstr(b, d2);
                    for (id3, d3, e3) in chunks(b, after, e2) {
                        if id3 != 0x4100 {
                            continue;
                        }
                        let mut o = Obj { name: name.clone(), verts: Vec::new(), faces: Vec::new(), groups: Vec::new() };
                        for (id4, d4, e4) in chunks(b, d3, e3) {
                            match id4 {
                                0x4110 => {
                                    let n = u16_at(b, d4).unwrap_or(0) as usize;
                                    for i in 0..n {
                                        let q = d4 + 2 + i * 12;
                                        if let (Some(x), Some(y), Some(z)) = (f32_at(b, q), f32_at(b, q + 4), f32_at(b, q + 8)) {
                                            o.verts.push([x, y, z]);
                                        }
                                    }
                                }
                                0x4120 => {
                                    let n = u16_at(b, d4).unwrap_or(0) as usize;
                                    for i in 0..n {
                                        let q = d4 + 2 + i * 8;
                                        if let (Some(a), Some(bb), Some(c)) = (u16_at(b, q), u16_at(b, q + 2), u16_at(b, q + 4)) {
                                            o.faces.push([a as u32, bb as u32, c as u32]);
                                        }
                                    }
                                    let sub = d4 + 2 + n * 8;
                                    for (id5, d5, _e5) in chunks(b, sub, e4) {
                                        if id5 == 0x4130 {
                                            let (mname, after) = cstr(b, d5);
                                            let k = u16_at(b, after).unwrap_or(0) as usize;
                                            let list: Vec<u32> = (0..k).filter_map(|i| u16_at(b, after + 2 + i * 2).map(|x| x as u32)).collect();
                                            o.groups.push((mname, list));
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        if !o.faces.is_empty() {
                            objs.push(o);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if objs.is_empty() {
        return Err("aucun maillage dans le fichier 3DS".into());
    }
    // 3DS units are free; master scale is "1 unit = x" with CAD exports mostly in millimetres.
    let scale = 0.001 * if master_scale > 0.0 && master_scale.is_finite() { master_scale } else { 1.0 };
    let mut s = empty_scene(path, "3DS");
    let tm = Instant::now();
    let top = objs.len() > 1;
    if top {
        s.components.push(component(&crate::stem(path), 0, None, true, None));
    }
    for o in &objs {
        let verts: Vec<[f32; 3]> = o.verts.iter().map(|p| [p[0] * scale, p[1] * scale, p[2] * scale]).collect();
        let valid = |f: &[u32; 3]| f.iter().all(|&i| (i as usize) < verts.len());
        // split by material when it carries a colour
        let mut assigned = vec![false; o.faces.len()];
        let mut parts: Vec<(Option<[f32; 4]>, Vec<[u32; 3]>)> = Vec::new();
        for (m, list) in &o.groups {
            let c = mats.get(m).copied();
            let tris: Vec<[u32; 3]> = list
                .iter()
                .filter_map(|&i| {
                    let f = o.faces.get(i as usize)?;
                    assigned[i as usize] = true;
                    valid(f).then_some(*f)
                })
                .collect();
            match parts.iter_mut().find(|p| p.0 == c) {
                Some(p) => p.1.extend(tris),
                None => parts.push((c, tris)),
            }
        }
        let rest: Vec<[u32; 3]> = o.faces.iter().zip(&assigned).filter(|(f, a)| !**a && valid(f)).map(|(f, _)| *f).collect();
        if !rest.is_empty() {
            match parts.iter_mut().find(|p| p.0.is_none()) {
                Some(p) => p.1.extend(rest),
                None => parts.push((None, rest)),
            }
        }
        parts.retain(|p| !p.1.is_empty());
        let multi = parts.len() > 1;
        let parent = if top { Some(0) } else { None };
        let obj_parent = if multi {
            s.components.push(component(&o.name, if top { 1 } else { 0 }, parent, true, None));
            Some(s.components.len() - 1)
        } else {
            parent
        };
        for (k, (c, tris)) in parts.into_iter().enumerate() {
            let mesh = sw_format::mesh::build_from_triangles(&verts, &tris, 30.0, true);
            let name = if multi { format!("{} #{}", o.name, k + 1) } else { o.name.clone() };
            s.meshes.push(SceneMesh { name: name.clone(), mesh, source: MeshSource::Embedded, color: c });
            let depth = obj_parent.map(|p| s.components[p].depth + 1).unwrap_or(0);
            s.components.push(component(&name, depth, obj_parent, false, Some(s.meshes.len() - 1)));
        }
    }
    s.timings.mesh = tm.elapsed();
    s.timings.decode = t0.elapsed() - s.timings.mesh;
    Ok(s)
}
