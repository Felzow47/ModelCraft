//! 3D XML (Dassault Systèmes): a zip with a product structure (`Reference3D`/`Instance3D`) and
//! tessellated `.3DRep` files (`PolygonalRepType` faces as triangles/strips/fans, per-face colours).

use crate::{component, empty_scene, mat_mul, Loaded, IDENTITY};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::time::Instant;
use sw_format::{MeshSource, SceneMesh};

#[derive(Default)]
struct Structure {
    root: String,
    refs: HashMap<String, String>,                  // id -> name
    instances: Vec<(String, String, String, [f32; 16])>, // name, parent ref, child ref, matrix
    rep_files: HashMap<String, String>,             // ReferenceRep id -> file
    ref_reps: HashMap<String, Vec<String>>,         // Reference3D id -> ReferenceRep ids
}

fn read_entry(zip: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>, name: &str) -> Option<String> {
    let idx = (0..zip.len()).find(|&i| zip.by_index(i).ok().is_some_and(|f| f.name().eq_ignore_ascii_case(name)))?;
    let mut f = zip.by_index(idx).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s)
}

fn child_text<'a>(n: roxmltree::Node<'a, 'a>, tag: &str) -> Option<&'a str> {
    n.children().find(|c| c.tag_name().name() == tag).and_then(|c| c.text()).map(str::trim)
}

fn matrix(s: &str) -> [f32; 16] {
    let v: Vec<f32> = s.split_ascii_whitespace().filter_map(|x| x.parse().ok()).collect();
    if v.len() < 12 {
        return IDENTITY;
    }
    let k = 0.001; // millimetres → metres
    [v[0], v[1], v[2], 0., v[3], v[4], v[5], 0., v[6], v[7], v[8], 0., v[9] * k, v[10] * k, v[11] * k, 1.]
}

fn parse_structure(xml: &str) -> Result<Structure, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("3DXML : XML invalide ({e})"))?;
    let mut st = Structure::default();
    let ps = doc.descendants().find(|n| n.tag_name().name() == "ProductStructure").ok_or("3DXML : ProductStructure absente")?;
    st.root = ps.attribute("root").unwrap_or("1").to_string();
    for n in ps.children().filter(|n| n.is_element()) {
        let id = n.attribute("id").unwrap_or("").to_string();
        match n.tag_name().name() {
            "Reference3D" => {
                st.refs.insert(id, n.attribute("name").unwrap_or("").to_string());
            }
            "Instance3D" => st.instances.push((
                n.attribute("name").unwrap_or("").to_string(),
                child_text(n, "IsAggregatedBy").unwrap_or("").to_string(),
                child_text(n, "IsInstanceOf").unwrap_or("").to_string(),
                child_text(n, "RelativeMatrix").map(matrix).unwrap_or(IDENTITY),
            )),
            "ReferenceRep" => {
                let f = n.attribute("associatedFile").unwrap_or("");
                st.rep_files.insert(id, f.trim_start_matches("urn:3DXML:").to_string());
            }
            "InstanceRep" => {
                let by = child_text(n, "IsAggregatedBy").unwrap_or("").to_string();
                let of = child_text(n, "IsInstanceOf").unwrap_or("").to_string();
                st.ref_reps.entry(by).or_default().push(of);
            }
            _ => {}
        }
    }
    Ok(st)
}

fn floats(s: &str) -> Vec<f32> {
    s.split(|c: char| c == ',' || c.is_ascii_whitespace()).filter(|x| !x.is_empty()).filter_map(|x| x.parse().ok()).collect()
}
fn ints(s: &str) -> Vec<u32> {
    s.split_ascii_whitespace().filter_map(|x| x.parse().ok()).collect()
}

/// Parse one .3DRep into (colour, positions in metres, triangles) groups.
fn parse_rep(xml: &str) -> Vec<(Option<[f32; 4]>, Vec<[f32; 3]>, Vec<[u32; 3]>)> {
    let Ok(doc) = roxmltree::Document::parse(xml) else { return Vec::new() };
    let mut out: Vec<(Option<[f32; 4]>, Vec<[f32; 3]>, Vec<[u32; 3]>)> = Vec::new();
    for rep in doc.descendants().filter(|n| n.tag_name().name() == "Rep" && n.attribute(("http://www.w3.org/2001/XMLSchema-instance", "type")) == Some("PolygonalRepType")) {
        let Some(vb) = rep.children().find(|c| c.tag_name().name() == "VertexBuffer") else { continue };
        let p = floats(child_text(vb, "Positions").unwrap_or(""));
        let pos: Vec<[f32; 3]> = p.chunks_exact(3).map(|c| [c[0] * 0.001, c[1] * 0.001, c[2] * 0.001]).collect();
        let Some(faces) = rep.children().find(|c| c.tag_name().name() == "Faces") else { continue };
        let mut local: Vec<(Option<[f32; 4]>, Vec<[u32; 3]>)> = Vec::new();
        for f in faces.children().filter(|c| c.tag_name().name() == "Face") {
            let color = f.descendants().find(|c| c.tag_name().name() == "Color").map(|c| {
                let g = |a: &str| c.attribute(a).and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
                [g("red"), g("green"), g("blue"), g("alpha")]
            });
            let mut tris: Vec<[u32; 3]> = Vec::new();
            if let Some(t) = f.attribute("triangles") {
                tris.extend(ints(t).chunks_exact(3).map(|c| [c[0], c[1], c[2]]));
            }
            if let Some(s) = f.attribute("strips") {
                for strip in s.split(',') {
                    let v = ints(strip);
                    for j in 2..v.len() {
                        tris.push(if j % 2 == 0 { [v[j - 2], v[j - 1], v[j]] } else { [v[j - 1], v[j - 2], v[j]] });
                    }
                }
            }
            if let Some(s) = f.attribute("fans") {
                for fan in s.split(',') {
                    let v = ints(fan);
                    for j in 2..v.len() {
                        tris.push([v[0], v[j - 1], v[j]]);
                    }
                }
            }
            tris.retain(|t| t.iter().all(|&i| (i as usize) < pos.len()));
            if tris.is_empty() {
                continue;
            }
            match local.iter_mut().find(|o| o.0 == color) {
                Some(o) => o.1.extend(tris),
                None => local.push((color, tris)),
            }
        }
        for (c, t) in local {
            out.push((c, pos.clone(), t));
        }
    }
    // merge groups of identical colour across reps
    let mut merged: Vec<(Option<[f32; 4]>, Vec<[f32; 3]>, Vec<[u32; 3]>)> = Vec::new();
    for (c, p, t) in out {
        match merged.iter_mut().find(|m| m.0 == c) {
            Some(m) => {
                let base = m.1.len() as u32;
                m.1.extend(p);
                m.2.extend(t.into_iter().map(|x| x.map(|i| i + base)));
            }
            None => merged.push((c, p, t)),
        }
    }
    merged
}

pub fn load(path: &Path, bytes: &[u8]) -> Result<Loaded, String> {
    let t0 = Instant::now();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("3DXML : archive illisible ({e})"))?;
    let manifest = read_entry(&mut zip, "Manifest.xml").ok_or("3DXML : Manifest.xml absent")?;
    let root_file = roxmltree::Document::parse(&manifest)
        .ok()
        .and_then(|d| d.descendants().find(|n| n.tag_name().name() == "Root").and_then(|n| n.text()).map(|s| s.trim().to_string()))
        .ok_or("3DXML : racine introuvable dans le manifeste")?;
    let root_xml = read_entry(&mut zip, &root_file).ok_or("3DXML : fichier racine absent")?;
    let st = parse_structure(&root_xml)?;
    let mut s = empty_scene(path, "3DXML");
    // meshes per rep file: list of mesh indices
    let mut rep_meshes: HashMap<String, Vec<usize>> = HashMap::new();
    let tm = Instant::now();
    let mut stack: Vec<(String, Option<usize>, usize, [f32; 16], String)> = vec![(st.root.clone(), None, 0, IDENTITY, String::new())];
    let mut guard = 0;
    while let Some((rid, parent, depth, world, inst_name)) = stack.pop() {
        guard += 1;
        if guard > 200_000 {
            s.warnings.push("3DXML : structure trop profonde, lecture tronquée".into());
            break;
        }
        let name = if inst_name.is_empty() { st.refs.get(&rid).cloned().unwrap_or_default() } else { inst_name };
        let children: Vec<&(String, String, String, [f32; 16])> = st.instances.iter().filter(|i| i.1 == rid).collect();
        let reps = st.ref_reps.get(&rid).cloned().unwrap_or_default();
        let mut mesh_ids: Vec<usize> = Vec::new();
        for rep in &reps {
            let Some(file) = st.rep_files.get(rep) else { continue };
            if !rep_meshes.contains_key(file) {
                let mut ids = Vec::new();
                if let Some(xml) = read_entry(&mut zip, file) {
                    for (c, p, t) in parse_rep(&xml) {
                        let mesh = sw_format::mesh::build_from_triangles(&p, &t, 30.0, true);
                        s.meshes.push(SceneMesh { name: file.clone(), mesh, source: MeshSource::Embedded, color: c });
                        ids.push(s.meshes.len() - 1);
                    }
                } else {
                    s.warnings.push(format!("3DXML : représentation « {file} » absente"));
                }
                rep_meshes.insert(file.clone(), ids);
            }
            mesh_ids.extend(rep_meshes[file].iter().copied());
        }
        let is_asm = !children.is_empty() || mesh_ids.len() > 1;
        let mut c = component(&name, depth, parent, is_asm, if is_asm { None } else { mesh_ids.first().copied() });
        c.world = world;
        s.components.push(c);
        let me = s.components.len() - 1;
        if is_asm {
            for (k, m) in mesh_ids.iter().enumerate() {
                let mut c = component(&format!("{name} · corps {}", k + 1), depth + 1, Some(me), false, Some(*m));
                c.world = world;
                s.components.push(c);
            }
        }
        for ch in children.into_iter().rev() {
            stack.push((ch.2.clone(), Some(me), depth + 1, mat_mul(&world, &ch.3), ch.0.clone()));
        }
    }
    s.timings.mesh = tm.elapsed();
    s.timings.decode = t0.elapsed() - s.timings.mesh;
    crate::finish_tree(&mut s);
    Ok(s)
}
