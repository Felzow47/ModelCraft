//! eDrawings files (.easm/.eprt, "2008+ archive"), read natively with no eDrawings install.
//!
//! The container is a small header followed by zip local-file records (`eModel`, `preview.jpg`,
//! `materials.xml`, `scene.xml`). `eModel` is a HOOPS Stream File decoded by [`crate::hsf`]
//! (clean-room, from the public format documentation). The scene is rebuilt from its segment
//! tree: `…/3dmodel/includes/subassemN/assempartN/part` segments carry the placement matrix and
//! include shared bodies from `/include library/shared/bodyN`, whose shells hold the triangles.

use crate::hsf::{self, Model};
use crate::{component, empty_scene, mat_mul, Loaded, IDENTITY};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::time::Instant;
use sw_format::{DocKind, MeshSource, Rgba, SceneMesh};

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
    let t0 = Instant::now();
    let ents = entries(b);
    let mut s = empty_scene(path, "eDrawings");
    s.stream_count = ents.len();
    s.up_z = false; // SolidWorks models are Y-up
    if let Some((_, jpg)) = ents.iter().find(|(n, _)| n.eq_ignore_ascii_case("preview.jpg") || n.eq_ignore_ascii_case("preview.png")) {
        if let Ok(img) = image::load_from_memory(jpg) {
            let img = img.to_rgba8();
            s.preview = Some(Rgba { width: img.width(), height: img.height(), pixels: img.into_raw() });
        }
    }
    let Some((_, emodel)) = ents.iter().find(|(n, _)| n.eq_ignore_ascii_case("eModel")) else {
        s.warnings.push("Fichier eDrawings sans modèle 3D (eModel absent) : seule la vignette est affichée.".into());
        return Ok(s);
    };
    if let Some(v) = hsf::version(emodel) {
        s.properties.push(("Flux HOOPS (HSF)".into(), v));
    }
    let model = match hsf::parse_file(emodel) {
        Ok(m) => m,
        Err(e) => {
            s.warnings.push(format!("Modèle eDrawings illisible ({e}) : seule la vignette est affichée."));
            return Ok(s);
        }
    };
    s.timings.decode = t0.elapsed();
    let t1 = Instant::now();
    build_scene(&model, &mut s);
    s.timings.mesh = t1.elapsed();
    if let Some(e) = &model.error {
        s.warnings.push(format!("Lecture du flux HSF interrompue ({e}) : la géométrie lue jusque-là est affichée."));
    }
    if s.meshes.is_empty() {
        s.warnings.push("Aucune géométrie 3D trouvée dans le modèle eDrawings : seule la vignette est affichée.".into());
    }
    Ok(s)
}

/// Column-major 4x4 from a row-vector 4x3 HOOPS matrix (`p' = p * M`).
fn to_cm(m: &[f32; 12]) -> [f32; 16] {
    [m[0], m[1], m[2], 0., m[3], m[4], m[5], 0., m[6], m[7], m[8], 0., m[9], m[10], m[11], 1.]
}

fn xform(w: &[f32; 16], p: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|r| w[r] * p[0] + w[4 + r] * p[1] + w[8 + r] * p[2] + w[12 + r])
}

fn is_identity(w: &[f32; 16]) -> bool {
    w.iter().zip(IDENTITY.iter()).all(|(a, b)| (a - b).abs() < 1e-7)
}

type ColorKey = Option<[u32; 3]>;
/// Colour, positions and triangles of one colour group.
type Group = (Option<[f32; 3]>, Vec<[f32; 3]>, Vec<[u32; 3]>);

/// Triangles of a subtree, grouped by colour, in the frame of the subtree root.
#[derive(Default)]
struct Groups {
    order: Vec<ColorKey>,
    map: HashMap<ColorKey, Group>,
}

impl Groups {
    fn add(&mut self, color: Option<[f32; 3]>, w: &[f32; 16], shell: &hsf::Shell) {
        let key = color.map(|c| c.map(f32::to_bits));
        let e = self.map.entry(key).or_insert_with(|| {
            self.order.push(key);
            (color, Vec::new(), Vec::new())
        });
        let base = e.1.len() as u32;
        if is_identity(w) {
            e.1.extend_from_slice(&shell.points);
        } else {
            e.1.extend(shell.points.iter().map(|&p| xform(w, p)));
        }
        e.2.extend(shell.tris.iter().map(|t| t.map(|i| i + base)));
    }
}

struct Builder<'a> {
    m: &'a Model,
    /// (library segment, inherited visibility) -> meshes (index, has its own colour)
    lib: HashMap<(usize, bool), Vec<(usize, bool)>>,
    triangles: usize,
    skipped_configs: usize,
}

/// A mesh placed by the walk, waiting for its component.
struct Placed {
    mesh: usize,
    world: [f32; 16],
    color: Option<[f32; 3]>,
}

fn seg_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// eDrawings names assembly nodes `subassemN` and part instances `assempartN`.
fn node_kind(name: &str) -> Option<bool> {
    let num = |p: &str| name.strip_prefix(p).is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()));
    if num("subassem") {
        Some(true)
    } else if num("assempart") {
        Some(false)
    } else {
        None
    }
}

impl<'a> Builder<'a> {
    /// Collect a library segment subtree (its own matrix included) into colour groups.
    fn gather(&self, seg: usize, w: [f32; 16], vis: bool, color: Option<[f32; 3]>, g: &mut Groups, depth: usize) {
        let s = &self.m.segments[seg];
        let vis = s.face_visible.unwrap_or(vis);
        let color = s.face_color.or(color);
        let w = match &s.matrix {
            Some(m) => mat_mul(&w, &to_cm(m)),
            None => w,
        };
        if vis {
            for &sh in &s.shells {
                if let Some(Some(shell)) = self.m.shells.get(sh) {
                    g.add(color, &w, shell);
                }
            }
        }
        if depth > 32 {
            return;
        }
        for (inc, _) in &s.includes {
            if let Some(&i) = self.m.by_path.get(inc) {
                self.gather(i, w, vis, color, g, depth + 1);
            }
        }
        for &c in &s.children {
            self.gather(c, w, vis, color, g, depth + 1);
        }
    }

    fn library_meshes(&mut self, seg: usize, vis: bool, scene: &mut Loaded) -> Vec<(usize, bool)> {
        if let Some(v) = self.lib.get(&(seg, vis)) {
            return v.clone();
        }
        let mut g = Groups::default();
        self.gather(seg, IDENTITY, vis, None, &mut g, 0);
        let name = seg_name(&self.m.segments[seg].path).to_string();
        let mut out = Vec::new();
        for key in &g.order {
            let Some((color, pos, tris)) = g.map.remove(key) else {
                continue;
            };
            if tris.is_empty() {
                continue;
            }
            self.triangles += tris.len();
            let mesh = sw_format::mesh::build_from_triangles(&pos, &tris, 30.0, true);
            out.push((scene.meshes.len(), color.is_some()));
            scene.meshes.push(SceneMesh { name: name.clone(), mesh, source: MeshSource::Embedded, color: color.map(|c| [c[0], c[1], c[2], 1.]) });
        }
        self.lib.insert((seg, vis), out.clone());
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn walk(&mut self, seg: usize, world: [f32; 16], vis: bool, color: Option<[f32; 3]>, parent: Option<usize>, depth: usize, sink: &mut Vec<Placed>, scene: &mut Loaded) {
        let m = self.m;
        let s = &m.segments[seg];
        if s.path == "/include library" || depth > 64 {
            return;
        }
        let vis = s.face_visible.unwrap_or(vis);
        let color = s.face_color.or(color);
        let world = match &s.matrix {
            Some(m) => mat_mul(&world, &to_cm(m)),
            None => world,
        };
        let name = seg_name(&s.path);
        let kind = node_kind(name);
        let mut local = Vec::new();
        let (me, sink_here): (Option<usize>, &mut Vec<Placed>) = match kind {
            Some(is_asm) => {
                let label = s
                    .children
                    .iter()
                    .map(|&c| &m.segments[c])
                    .find(|c| seg_name(&c.path) == "name")
                    .and_then(|c| c.texts.iter().find(|t| !t.trim().is_empty()))
                    .map(|t| t.trim().to_string())
                    .unwrap_or_else(|| name.to_string());
                let depth_c = parent.map(|p| scene.components[p].depth + 1).unwrap_or(0);
                let mut c = component(&label, depth_c, parent, is_asm, None);
                c.world = world;
                scene.components.push(c);
                (Some(scene.components.len() - 1), &mut local)
            }
            None => (parent, sink),
        };
        // geometry stored directly in this segment
        if vis && !s.shells.is_empty() {
            let mut g = Groups::default();
            for &sh in &s.shells {
                if let Some(Some(shell)) = m.shells.get(sh) {
                    g.add(color, &IDENTITY, shell);
                }
            }
            for key in &g.order {
                let Some((c, pos, tris)) = g.map.remove(key) else {
                    continue;
                };
                if tris.is_empty() {
                    continue;
                }
                self.triangles += tris.len();
                let mesh = sw_format::mesh::build_from_triangles(&pos, &tris, 30.0, true);
                sink_here.push(Placed { mesh: scene.meshes.len(), world, color: None });
                scene.meshes.push(SceneMesh { name: name.to_string(), mesh, source: MeshSource::Embedded, color: c.map(|c| [c[0], c[1], c[2], 1.]) });
            }
        }
        for (inc, _) in &s.includes {
            let Some(&i) = m.by_path.get(inc) else {
                continue;
            };
            for (mesh, own) in self.library_meshes(i, vis, scene) {
                sink_here.push(Placed { mesh, world, color: if own { None } else { color } });
            }
        }
        // several SolidWorks configurations (`config0`, `config1`…) hold alternative versions of
        // the same model: show the first one only
        let mut seen_config = false;
        for &c in &s.children {
            let n = seg_name(&m.segments[c].path);
            if n.strip_prefix("config").is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit())) {
                if seen_config {
                    self.skipped_configs += 1;
                    continue;
                }
                seen_config = true;
            }
            self.walk(c, world, vis, color, me, depth + 1, sink_here, scene);
        }
        if let Some(me) = me.filter(|_| kind.is_some()) {
            let n = local.len();
            if n == 1 && !scene.components[me].is_assembly {
                let p = local.pop().unwrap();
                let c = &mut scene.components[me];
                c.mesh = Some(p.mesh);
                c.world = p.world;
                c.color = p.color.map(|c| [c[0], c[1], c[2], 1.]);
            } else {
                let label = scene.components[me].name.clone();
                let d = scene.components[me].depth + 1;
                for (k, p) in local.into_iter().enumerate() {
                    let mut c = component(&format!("{label} · corps {}", k + 1), d, Some(me), false, Some(p.mesh));
                    c.world = p.world;
                    c.color = p.color.map(|c| [c[0], c[1], c[2], 1.]);
                    scene.components.push(c);
                }
            }
        }
    }
}

/// Turn the decoded HSF segment tree into scene meshes and components.
pub fn build_scene(m: &Model, scene: &mut Loaded) {
    let Some(&root) = m.by_path.get("/") else {
        return;
    };
    let mut b = Builder { m, lib: HashMap::new(), triangles: 0, skipped_configs: 0 };
    let mut top = Vec::new();
    b.walk(root, IDENTITY, true, None, None, 0, &mut top, scene);
    // geometry outside any part node (single-part .eprt files)
    let stem = crate::stem(&scene.path);
    let n = top.len();
    for (k, p) in top.into_iter().enumerate() {
        let name = if n == 1 { stem.clone() } else { format!("{stem} · corps {}", k + 1) };
        let mut c = component(&name, 0, None, false, Some(p.mesh));
        c.world = p.world;
        c.color = p.color.map(|c| [c[0], c[1], c[2], 1.]);
        scene.components.push(c);
    }
    if b.skipped_configs > 0 {
        scene.warnings.push(format!("{} autre(s) configuration(s) enregistrée(s) dans le fichier : seule la première est affichée.", b.skipped_configs));
    }
    scene.kind = if scene.components.iter().any(|c| c.is_assembly) { DocKind::Assembly } else { DocKind::Part };
    let st = &m.stats;
    scene.properties.push(("Segments HSF".into(), m.segments.len().to_string()));
    scene.properties.push(("Maillages HSF".into(), format!("{} ({} affinés, {} répétés)", m.shells.len(), st.refinements, st.repeats)));
    scene.properties.push(("Triangles".into(), b.triangles.to_string()));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real eDrawings assembly (not in the repository): EASM_SAMPLE=/path/to/file.easm, default
    /// /workspace/sw/samples2/edw1.EASM; skipped when absent.
    #[test]
    fn sample_assembly() {
        let p = std::env::var_os("EASM_SAMPLE").map(std::path::PathBuf::from).unwrap_or_else(|| "/workspace/sw/samples2/edw1.EASM".into());
        let Ok(bytes) = std::fs::read(&p) else { return };
        let s = load(&p, &bytes).expect("load");
        let tris: usize = s.components.iter().filter_map(|c| c.mesh).map(|m| s.meshes[m].mesh.indices.len() / 3).sum();
        eprintln!("{} meshes, {} components, {} instanced triangles, warnings {:?}", s.meshes.len(), s.components.len(), tris, s.warnings);
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
        assert!(s.meshes.len() > 100);
        assert!(tris > 1_000_000);
        assert_eq!(s.kind, DocKind::Assembly);
        assert!(s.components.iter().any(|c| c.name == "Montant Avant-1"));
    }
}
