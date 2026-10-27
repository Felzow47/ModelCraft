//! Wavefront OBJ with `o`/`g` objects, `usemtl` materials from the `.mtl` library and
//! textures.
//!
//! - Polygons (n-gons, often concave in Blender exports: outlines, lettering) are triangulated
//!   by ear clipping in the polygon's plane, not as a fan, so their shape is kept.
//! - `vt` texture coordinates and `vn` normals are used per corner (Blender's own smoothing and
//!   sharp edges); without `vn`, normals come from the geometry.
//! - Materials: `Kd` colour, `d` / `Tr` opacity, `map_Kd` diffuse texture (drawn mapped on the
//!   model). Blender writes `Kd` in linear colour: it is converted to sRGB for display.
//! - Units: Blender exports are in metres, other OBJ files are assumed to be in millimetres.

use crate::{component, empty_scene, stem, Loaded};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use sw_format::{MeshSource, Rgba, SceneMesh, TriMesh};

const NONE: u32 = u32::MAX;

/// One polygon corner: position, texture coordinate and normal indices (NONE if absent).
type Corner = [u32; 3];

struct Group {
    name: String,
    material: String,
    corners: Vec<Corner>,
    /// Start of each polygon in `corners`.
    polys: Vec<u32>,
}

impl Group {
    fn new(name: &str, material: &str) -> Group {
        Group { name: name.to_string(), material: material.to_string(), corners: Vec::new(), polys: Vec::new() }
    }
    fn poly(&self, k: usize) -> &[Corner] {
        let a = self.polys[k] as usize;
        let b = self.polys.get(k + 1).map(|&x| x as usize).unwrap_or(self.corners.len());
        &self.corners[a..b]
    }
}

#[derive(Clone, Default)]
struct Material {
    kd: Option<[f32; 3]>,
    alpha: f32,
    texture: Option<PathBuf>,
}

fn resolve(idx: i64, len: usize) -> u32 {
    let i = if idx < 0 { len as i64 + idx } else { idx - 1 };
    if i >= 0 && (i as usize) < len {
        i as u32
    } else {
        NONE
    }
}

pub fn load(path: &Path, bytes: &[u8]) -> Result<Loaded, String> {
    let t0 = Instant::now();
    let text = String::from_utf8_lossy(bytes);
    let mut v: Vec<[f32; 3]> = Vec::new();
    let mut vt: Vec<[f32; 2]> = Vec::new();
    let mut vn: Vec<[f32; 3]> = Vec::new();
    let mut groups: Vec<Group> = vec![Group::new(&stem(path), "")];
    let mut mtllibs: Vec<String> = Vec::new();
    let mut cur_obj = stem(path);
    let mut cur_mat = String::new();
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
            Some("vt") => {
                let u = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let w = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                vt.push([u, w]);
            }
            Some("vn") => {
                let mut n = [0f32; 3];
                for c in &mut n {
                    *c = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                }
                vn.push(n);
            }
            Some("f") => {
                let g = groups.last_mut().unwrap();
                let start = g.corners.len();
                for tok in it {
                    let mut f = tok.split('/');
                    let mut idx = |len: usize| f.next().filter(|s| !s.is_empty()).and_then(|s| s.parse::<i64>().ok()).map(|i| resolve(i, len)).unwrap_or(NONE);
                    let c = [idx(v.len()), idx(vt.len()), idx(vn.len())];
                    if c[0] != NONE {
                        g.corners.push(c);
                    }
                }
                if g.corners.len() - start >= 3 {
                    g.polys.push(start as u32);
                } else {
                    g.corners.truncate(start);
                }
            }
            Some("o") | Some("g") => {
                let name: String = it.collect::<Vec<_>>().join(" ");
                if !name.is_empty() {
                    cur_obj = name;
                    groups.push(Group::new(&cur_obj, &cur_mat));
                }
            }
            Some("usemtl") => {
                cur_mat = it.collect::<Vec<_>>().join(" ");
                groups.push(Group::new(&cur_obj, &cur_mat));
            }
            Some("mtllib") => mtllibs.push(it.collect::<Vec<_>>().join(" ")),
            _ => {}
        }
    }
    if v.is_empty() {
        return Err("aucun sommet dans le fichier OBJ".into());
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut mats: HashMap<String, Material> = HashMap::new();
    if mtllibs.is_empty() {
        mtllibs.push(format!("{}.mtl", stem(path)));
    }
    let mut warnings = Vec::new();
    for lib in &mtllibs {
        match find_file(dir, lib) {
            Some(f) => {
                if let Ok(t) = std::fs::read_to_string(&f) {
                    parse_mtl(&t, f.parent().unwrap_or(dir), &mut mats);
                }
            }
            None if !lib.is_empty() && text.contains("mtllib") => warnings.push(format!("bibliothèque de matériaux introuvable : {lib}")),
            None => {}
        }
    }
    // textures, decoded once each
    let mut textures: HashMap<PathBuf, Option<Arc<Rgba>>> = HashMap::new();
    for m in mats.values() {
        if let Some(t) = &m.texture {
            textures.entry(t.clone()).or_insert_with(|| load_texture(t).map(Arc::new));
        }
    }
    // merge groups by (object, material)
    let mut merged: Vec<Group> = Vec::new();
    for g in groups.into_iter().filter(|g| !g.polys.is_empty()) {
        match merged.iter_mut().find(|m| m.name == g.name && m.material == g.material) {
            Some(m) => {
                let off = m.corners.len() as u32;
                m.polys.extend(g.polys.iter().map(|p| p + off));
                m.corners.extend(g.corners);
            }
            None => merged.push(g),
        }
    }
    let mut s = empty_scene(path, "OBJ");
    s.warnings.extend(warnings);
    s.up_z = false; // OBJ is conventionally Y-up (Blender's default export too)
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
            let mat = mats.get(&g.material);
            let tex = mat.and_then(|m| m.texture.as_ref()).and_then(|t| textures.get(t).cloned().flatten());
            // texture coordinates that are not a real unwrap (Blender text converted to mesh:
            // every cap triangle squashed onto one line of the image) would paint random texels
            // and holes from the alpha: such a group is drawn in its plain material colour
            let has_vt = g.corners.iter().any(|c| c[1] != NONE);
            let tex = tex.filter(|t| !has_vt || uv_usable(g, &v, &vt, t));
            let mut mesh = build_group(g, &v, &vt, &vn, scale, tex.clone());
            // a texture without texture coordinates can only tint the part with its average colour
            let avg = if mesh.texture.is_none() && !has_vt { tex.as_deref().map(average) } else { None };
            let tex = if mesh.texture.is_none() { None } else { tex };
            // no drawn edges on textured materials: the image carries the detail, and CAD edges
            // on lettering or decals turn into a lace over it (the global switch still applies
            // to everything else)
            if mat.is_some_and(|m| m.texture.is_some()) {
                mesh.edges.clear();
            }
            let color = mat.map(|m| {
                let kd = match (m.kd, &tex) {
                    // a texture carries the colour (Blender writes the unused base colour 0.8)
                    (Some(k), Some(_)) if k == [0.8; 3] => [1.0; 3],
                    (Some(k), _) => k,
                    (None, _) => [if m.texture.is_some() { 1.0 } else { 0.8 }; 3],
                };
                let a = avg.unwrap_or([1.0; 3]);
                let kd = if avg.is_some() && m.kd.map_or(true, |k| k == [0.8; 3]) { a } else { [kd[0] * a[0], kd[1] * a[1], kd[2] * a[2]] };
                [kd[0], kd[1], kd[2], m.alpha]
            });
            let label = if g.material.is_empty() { g.name.clone() } else { format!("{} [{}]", g.name, g.material) };
            s.meshes.push(SceneMesh { name: label.clone(), mesh, source: MeshSource::Embedded, color });
            let depth = if parent.is_some() { 1 } else { 0 };
            s.components.push(component(&label, depth, parent, false, Some(s.meshes.len() - 1)));
        }
    }
    s.timings.mesh = tm.elapsed();
    s.timings.decode = t0.elapsed() - s.timings.mesh;
    Ok(s)
}

// ---------------------------------------------------------------- geometry

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    if l > 0.0 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Polygon normal (Newell's method: robust for concave and slightly non-planar polygons).
fn poly_normal(p: &[[f32; 3]]) -> [f32; 3] {
    let mut n = [0f32; 3];
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    n
}

/// Triangulate a simple polygon (indices into `p`, in order) by ear clipping in its plane.
/// Convex and concave polygons keep their outline; a degenerate one falls back to a fan.
pub(crate) fn triangulate(p: &[[f32; 3]], out: &mut Vec<[u32; 3]>) {
    let n = p.len();
    if n < 3 {
        return;
    }
    if n == 3 {
        out.push([0, 1, 2]);
        return;
    }
    let nrm = poly_normal(p);
    // 2D projection dropping the dominant axis, keeping the winding counter-clockwise
    let ax = (0..3).max_by(|&a, &b| nrm[a].abs().total_cmp(&nrm[b].abs())).unwrap_or(2);
    let (i, j) = match ax {
        0 => (1, 2),
        1 => (2, 0),
        _ => (0, 1),
    };
    let flip = nrm[ax] < 0.0;
    let q: Vec<[f32; 2]> = p.iter().map(|v| if flip { [v[j], v[i]] } else { [v[i], v[j]] }).collect();
    let area2 = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    let fan = |out: &mut Vec<[u32; 3]>, idx: &[u32]| {
        for k in 1..idx.len() - 1 {
            out.push([idx[0], idx[k], idx[k + 1]]);
        }
    };
    if n > 4000 || dot(nrm, nrm) <= 0.0 {
        fan(out, &(0..n as u32).collect::<Vec<_>>());
        return;
    }
    let mut idx: Vec<u32> = (0..n as u32).collect();
    let inside = |p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]| area2(a, b, p) >= 0.0 && area2(b, c, p) >= 0.0 && area2(c, a, p) >= 0.0;
    let mut guard = 0usize;
    let mut k = 0usize;
    while idx.len() > 3 {
        let m = idx.len();
        let (ia, ib, ic) = (idx[(k + m - 1) % m], idx[k % m], idx[(k + 1) % m]);
        let (a, b, c) = (q[ia as usize], q[ib as usize], q[ic as usize]);
        let mut ear = area2(a, b, c) > 0.0;
        if ear {
            for &o in &idx {
                if o == ia || o == ib || o == ic {
                    continue;
                }
                let po = q[o as usize];
                // vertices shared with the ear (duplicate positions from bridged holes) do not block it
                if po == a || po == b || po == c {
                    continue;
                }
                if inside(po, a, b, c) {
                    ear = false;
                    break;
                }
            }
        }
        if ear {
            out.push([ia, ib, ic]);
            idx.remove(k % m);
            guard = 0;
            if k >= idx.len() {
                k = 0;
            }
        } else {
            k = (k + 1) % m;
            guard += 1;
            if guard > m {
                // no ear left (self-intersecting or degenerate rest): finish as a fan
                fan(out, &idx);
                return;
            }
        }
    }
    out.push([idx[0], idx[1], idx[2]]);
}

/// Mesh of one (object, material) group.
fn build_group(g: &Group, v: &[[f32; 3]], vt: &[[f32; 2]], vn: &[[f32; 3]], scale: f32, texture: Option<Arc<Rgba>>) -> TriMesh {
    let sc = |p: [f32; 3]| [p[0] * scale, p[1] * scale, p[2] * scale];
    let np = g.polys.len();
    let has_attr = g.corners.iter().any(|c| c[1] != NONE || c[2] != NONE);
    let mut tri_local: Vec<[u32; 3]> = Vec::new();
    if !has_attr {
        // plain geometry (typical CAD export): weld + smooth by crease angle, as for STL
        let mut map: HashMap<u32, u32> = HashMap::new();
        let mut pos: Vec<[f32; 3]> = Vec::new();
        let mut tris: Vec<[u32; 3]> = Vec::new();
        let mut pts: Vec<[f32; 3]> = Vec::new();
        for k in 0..np {
            let poly = g.poly(k);
            pts.clear();
            pts.extend(poly.iter().map(|c| v[c[0] as usize]));
            tri_local.clear();
            triangulate(&pts, &mut tri_local);
            for t in &tri_local {
                tris.push(t.map(|i| {
                    let vi = poly[i as usize][0];
                    *map.entry(vi).or_insert_with(|| {
                        pos.push(sc(v[vi as usize]));
                        (pos.len() - 1) as u32
                    })
                }));
            }
        }
        return sw_format::mesh::build_from_triangles(&pos, &tris, 30.0, true);
    }
    // per-corner vertices (position, uv, normal), one face per polygon
    let mut m = TriMesh { texture: texture.clone(), ..TriMesh::default() };
    let textured = texture.is_some();
    let mut vmap: HashMap<(u32, u32, u32, u32), u32> = HashMap::new();
    let mut first_of_pos: HashMap<u32, u32> = HashMap::new();
    let mut pnorm: Vec<[f32; 3]> = Vec::with_capacity(np);
    // polygon edges (welded positions) → polygons, for the drawn edges
    let mut emap: HashMap<(u32, u32), (u32, u32, u32)> = HashMap::new();
    let mut pts: Vec<[f32; 3]> = Vec::new();
    for k in 0..np {
        let poly = g.poly(k);
        pts.clear();
        pts.extend(poly.iter().map(|c| v[c[0] as usize]));
        let fnrm = normalize(poly_normal(&pts));
        pnorm.push(fnrm);
        tri_local.clear();
        triangulate(&pts, &mut tri_local);
        if tri_local.is_empty() {
            continue;
        }
        m.face_start.push((m.indices.len() / 3) as u32);
        for t in &tri_local {
            for &i in t {
                let c = poly[i as usize];
                // without a file normal, the corner is flat-shaded with its polygon
                let key = (c[0], if textured { c[1] } else { NONE }, c[2], if c[2] == NONE { k as u32 } else { NONE });
                let idx = *vmap.entry(key).or_insert_with(|| {
                    m.positions.push(sc(v[c[0] as usize]));
                    let n = if c[2] != NONE { normalize(vn[c[2] as usize]) } else { fnrm };
                    m.normals.push(n);
                    if textured {
                        let uv = if c[1] != NONE { vt[c[1] as usize] } else { [0.0, 0.0] };
                        // OBJ v grows upward, image rows downward
                        m.uvs.push([uv[0], 1.0 - uv[1]]);
                    }
                    (m.positions.len() - 1) as u32
                });
                first_of_pos.entry(c[0]).or_insert(idx);
                m.indices.push(idx);
            }
        }
        for e in 0..poly.len() {
            let (a, b) = (poly[e][0], poly[(e + 1) % poly.len()][0]);
            if a == b {
                continue;
            }
            let key = if a < b { (a, b) } else { (b, a) };
            let ent = emap.entry(key).or_insert((0, k as u32, NONE));
            ent.0 += 1;
            if ent.0 == 2 {
                ent.2 = k as u32;
            }
        }
    }
    // drawn edges: outline (one polygon), non-manifold, or a fold sharper than 30°
    let cos_crease = 30f32.to_radians().cos();
    let mut keys: Vec<_> = emap.into_iter().collect();
    keys.sort_unstable_by_key(|e| e.0);
    for ((a, b), (count, p0, p1)) in keys {
        let sharp = count != 2 || dot(pnorm[p0 as usize], pnorm[p1 as usize]) < cos_crease;
        if sharp {
            if let (Some(&ia), Some(&ib)) = (first_of_pos.get(&a), first_of_pos.get(&b)) {
                m.edges.push(ia);
                m.edges.push(ib);
            }
        }
    }
    m.face_count = m.face_start.len();
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in &m.positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    m.bbox_min = lo;
    m.bbox_max = hi;
    m
}

// ---------------------------------------------------------------- materials

/// Resolve a file named in an OBJ/MTL (relative, Windows separators, wrong case, or an absolute
/// path from the author's machine) next to the file that names it.
fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
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
    for d in [dir.to_path_buf(), dir.join("textures"), dir.join("Textures"), dir.join("textures").join(".."), dir.join("..").join("textures")] {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().to_lowercase() == lower {
                return Some(e.path());
            }
        }
    }
    None
}

/// Largest texture side kept in memory and on the GPU.
const MAX_TEXTURE: u32 = 4096;

fn load_texture(path: &Path) -> Option<Rgba> {
    if std::fs::metadata(path).ok()?.len() > 256 << 20 {
        return None;
    }
    let mut img = image::open(path).ok()?;
    if img.width() > MAX_TEXTURE || img.height() > MAX_TEXTURE {
        img = img.resize(MAX_TEXTURE, MAX_TEXTURE, image::imageops::FilterType::Triangle);
    }
    let rgba = img.to_rgba8();
    Some(Rgba { width: rgba.width(), height: rgba.height(), pixels: rgba.into_raw() })
}

/// Whether a group's texture coordinates map its surface onto the image: false when most of
/// the area (≥ 30 %) has triangles flattened to less than a quarter of a texel.
fn uv_usable(g: &Group, v: &[[f32; 3]], vt: &[[f32; 2]], t: &Rgba) -> bool {
    let min_uv = 0.25 / (t.width.max(1) as f64 * t.height.max(1) as f64);
    let (mut total, mut flat) = (0f64, 0f64);
    for k in 0..g.polys.len() {
        let poly = g.poly(k);
        for i in 1..poly.len() - 1 {
            let c = [poly[0], poly[i], poly[i + 1]];
            let p = c.map(|c| v[c[0] as usize]);
            let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
            let a3 = (n[0] as f64).hypot(n[1] as f64).hypot(n[2] as f64) * 0.5;
            total += a3;
            let uv = c.map(|c| vt.get(c[1] as usize).copied());
            let auv = match uv {
                [Some(a), Some(b), Some(c)] => (((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) as f64).abs() * 0.5,
                _ => 0.0,
            };
            if auv < min_uv {
                flat += a3;
            }
        }
    }
    total <= 0.0 || flat < 0.3 * total
}

/// Average colour of a texture (sRGB, 0..1), weighted by alpha (transparent texels do not count).
fn average(t: &Rgba) -> [f32; 3] {
    let mut sum = [0f64; 3];
    let mut n = 0f64;
    let step = ((t.pixels.len() / 4) / 65536).max(1);
    for px in t.pixels.chunks_exact(4).step_by(step) {
        let a = px[3] as f64 / 255.0;
        for k in 0..3 {
            sum[k] += px[k] as f64 * a;
        }
        n += a;
    }
    if n == 0.0 {
        return [1.0; 3];
    }
    sum.map(|x| (x / n / 255.0) as f32)
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c >= 1.0 {
        1.0
    } else if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn parse_mtl(t: &str, dir: &Path, out: &mut HashMap<String, Material>) {
    // Blender writes its (scene-linear) base colour as Kd; the viewer works in sRGB
    let linear = t.lines().take(3).any(|l| l.starts_with('#') && l.contains("Blender"));
    let mut cur = String::new();
    let new = || Material { alpha: 1.0, ..Default::default() };
    for line in t.lines() {
        let line = line.trim();
        let mut it = line.split_ascii_whitespace();
        let Some(key) = it.next() else { continue };
        match key {
            "newmtl" => {
                cur = it.collect::<Vec<_>>().join(" ");
                out.entry(cur.clone()).or_insert_with(new);
            }
            "Kd" => {
                let c: Vec<f32> = it.filter_map(|s| s.parse().ok()).collect();
                if c.len() >= 3 {
                    let k = [c[0], c[1], c[2]].map(|x| x.clamp(0.0, 1.0));
                    let k = if linear && k != [0.8; 3] { k.map(linear_to_srgb) } else { k };
                    out.entry(cur.clone()).or_insert_with(new).kd = Some(k);
                }
            }
            "d" => {
                if let Some(d) = it.next().and_then(|s| s.parse::<f32>().ok()) {
                    out.entry(cur.clone()).or_insert_with(new).alpha = d.clamp(0.05, 1.0);
                }
            }
            "Tr" => {
                if let Some(tr) = it.next().and_then(|s| s.parse::<f32>().ok()) {
                    out.entry(cur.clone()).or_insert_with(new).alpha = (1.0 - tr).clamp(0.05, 1.0);
                }
            }
            "map_Kd" => {
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
                if let Some(f) = find_file(dir, &name) {
                    out.entry(cur.clone()).or_insert_with(new).texture = Some(f);
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

    fn area(m: &TriMesh) -> f32 {
        m.indices
            .chunks_exact(3)
            .map(|t| {
                let (a, b, c) = (m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
                let n = cross(sub(b, a), sub(c, a));
                dot(n, n).sqrt() / 2.0
            })
            .sum()
    }

    #[test]
    fn concave_ngon_keeps_its_shape() {
        // an "L" (area 3) and a "U" outline (area 5): a fan from the first corner covers the notch
        let dir = std::env::temp_dir().join(format!("mc_obj_ngon_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let obj = "# Blender 4.2.0\no L\nv 0 0 0\nv 2 0 0\nv 2 1 0\nv 1 1 0\nv 1 2 0\nv 0 2 0\nvn 0 0 1\nf 3//1 4//1 5//1 6//1 1//1 2//1\n\
                   o U\nv 0 0 1\nv 3 0 1\nv 3 3 1\nv 2 3 1\nv 2 1 1\nv 1 1 1\nv 1 3 1\nv 0 3 1\nf -6 -5 -4 -3 -2 -1 -8 -7\n";
        let p = dir.join("ngon.obj");
        std::fs::write(&p, obj).unwrap();
        let s = load(&p, obj.as_bytes()).unwrap();
        let l = &s.meshes.iter().find(|m| m.name == "L").unwrap().mesh;
        assert!((area(l) - 3.0).abs() < 1e-4, "L area {}", area(l));
        assert_eq!(l.face_count, 1);
        assert!(l.normals.iter().all(|n| (n[2] - 1.0).abs() < 1e-6));
        let u = &s.meshes.iter().find(|m| m.name == "U").unwrap().mesh;
        assert!((area(u) - 7.0).abs() < 1e-4, "U area {}", area(u));
        // the outline is drawn, not the triangulation diagonals
        assert_eq!(u.edges.len() / 2, 8);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn texture_mapped_with_uvs_and_linear_kd() {
        let dir = std::env::temp_dir().join(format!("mc_obj_uv_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut img = image::RgbaImage::new(4, 2);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = if x < 2 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 255, 0, 255]) };
        }
        img.save(dir.join("board.png")).unwrap();
        std::fs::write(dir.join("b.mtl"), "# Blender 4.2 MTL File: 'None'\nnewmtl Board\nKd 0.8 0.8 0.8\nmap_Kd board.png\nnewmtl Grey\nKd 0.214041 0.214041 0.214041\n").unwrap();
        let obj = "# Blender 4.2\nmtllib b.mtl\no Board\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nvn 0 0 1\nusemtl Board\nf 1/1/1 2/2/1 3/3/1 4/4/1\nusemtl Grey\nf 1 3 2\n";
        let p = dir.join("b.obj");
        std::fs::write(&p, obj).unwrap();
        let s = load(&p, obj.as_bytes()).unwrap();
        let b = s.meshes.iter().find(|m| m.name.contains("Board")).unwrap();
        let t = b.mesh.texture.as_ref().expect("texture kept");
        assert_eq!((t.width, t.height), (4, 2));
        assert_eq!(b.mesh.uvs.len(), b.mesh.positions.len());
        // v is flipped for image rows
        let i = b.mesh.positions.iter().position(|p| *p == [0.0, 1.0, 0.0]).unwrap();
        assert_eq!(b.mesh.uvs[i], [0.0, 0.0]);
        assert_eq!(b.color, Some([1.0, 1.0, 1.0, 1.0]));
        // linear 0.214 is sRGB 0.5
        let g = s.meshes.iter().find(|m| m.name.contains("Grey")).unwrap().color.unwrap();
        assert!((g[0] - 0.5).abs() < 0.01, "{g:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Private Blender export (SW_SAMPLES5=dir with Uv2.obj/.mtl/texture.png); skipped if absent.
    #[test]
    fn private_blender_totem() {
        let Some(dir) = std::env::var_os("SW_SAMPLES5") else { return };
        let p = std::path::Path::new(&dir).join("Uv2.obj");
        let Ok(bytes) = std::fs::read(&p) else { return };
        let s = load(&p, &bytes).unwrap();
        let (lo, hi) = s.meshes.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), m| {
            (std::array::from_fn(|k| lo[k].min(m.mesh.bbox_min[k])), std::array::from_fn(|k| hi[k].max(m.mesh.bbox_max[k])))
        });
        // upright: the totem is tallest along +Y
        assert!(hi[1] - lo[1] > 3.0 * (hi[0] - lo[0]).max(hi[2] - lo[2]), "{lo:?} {hi:?}");
        let tex: Vec<_> = s.meshes.iter().filter(|m| m.name.contains("Material.002")).collect();
        assert!(!tex.is_empty());
        if std::path::Path::new(&dir).join("texture.png").exists() {
            // boards and logo are mapped; the lettering (flattened UVs) is plain white
            for m in &tex {
                let text = m.name.contains("Text");
                assert_eq!(m.mesh.texture.is_none(), text, "{}", m.name);
                assert!(m.mesh.edges.is_empty(), "{}", m.name);
                if text {
                    assert_eq!(m.color, Some([1.0, 1.0, 1.0, 1.0]));
                } else if !m.name.starts_with("Poteau") {
                    assert_eq!(m.mesh.uvs.len(), m.mesh.positions.len());
                }
            }
        }
        let red = s.meshes.iter().find(|m| m.name.contains("Material.001")).and_then(|m| m.color).unwrap();
        assert!(red[0] > 0.7 && red[1] < 0.05, "linear 0.52 red shown as sRGB {red:?}");
    }

    #[test]
    fn flattened_uvs_fall_back_to_plain_colour_without_edges() {
        let dir = std::env::temp_dir().join(format!("mc_obj_flat_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut img = image::RgbaImage::new(4, 4);
        img.put_pixel(3, 0, image::Rgba([255, 255, 255, 255]));
        img.save(dir.join("atlas.png")).unwrap();
        std::fs::write(dir.join("t.mtl"), "# Blender MTL File\nnewmtl Atlas\nmap_Kd atlas.png\nmap_d atlas.png\n").unwrap();
        // "lettering": the cap's UVs all lie on the line v = 1 (zero area)
        let obj = "# Blender 4.2\nmtllib t.mtl\no Text\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 1\nvt 0.5 1\nvt 1 1\nvn 0 0 1\nusemtl Atlas\nf 1/1/1 2/2/1 3/3/1 4/1/1\n\
                   o Board\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nf 5/4/1 6/5/1 7/6/1 8/7/1\n";
        let p = dir.join("t.obj");
        std::fs::write(&p, obj).unwrap();
        let s = load(&p, obj.as_bytes()).unwrap();
        let t = s.meshes.iter().find(|m| m.name.starts_with("Text")).unwrap();
        assert!(t.mesh.texture.is_none(), "flattened UVs are not mapped");
        assert_eq!(t.color, Some([1.0, 1.0, 1.0, 1.0]), "plain (white) material colour");
        assert!(t.mesh.edges.is_empty());
        let b = s.meshes.iter().find(|m| m.name.starts_with("Board")).unwrap();
        assert!(b.mesh.texture.is_some());
        assert!(b.mesh.edges.is_empty(), "no CAD edges over a textured material");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
