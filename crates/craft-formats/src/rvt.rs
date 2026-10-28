//! Autodesk Revit (.rvt project, .rfa family, .rte/.rft templates): clean-room reader of the
//! public parts of the file.
//!
//! A Revit file is an OLE compound document. We only open the small, self-describing streams:
//! `BasicFileInfo` (UTF-16 "Key: value" lines: Revit build, worksharing, central model path…),
//! `RevitPreview4.0` (a PNG thumbnail after a short header), and `ProjectInformation` /
//! `PartAtom` (an Atom XML document, possibly inside a one-record ZIP, with the project or
//! family parameters). The model itself (`Global/*`, `Partitions/*`) is a proprietary,
//! gzip-compressed serialization of Revit's object database without any cached tessellation, so
//! no 3D geometry is decoded. Streams are read individually from the file: the (often hundreds of
//! megabytes) model partitions are never loaded.

use crate::{empty_scene, rvt_model};
use std::io::Read;
use std::path::Path;
use sw_format::{Rgba, Scene};

const MAX_SMALL: u64 = 32 << 20;

fn read_capped<F: Read + std::io::Seek>(cf: &mut cfb::CompoundFile<F>, name: &str) -> Option<Vec<u8>> {
    let len = cf.entry(name).ok()?.len();
    if len > MAX_SMALL {
        return None;
    }
    let mut st = cf.open_stream(name).ok()?;
    let mut v = Vec::with_capacity(len as usize);
    st.read_to_end(&mut v).ok()?;
    Some(v)
}

/// UTF-16LE text of `BasicFileInfo` (tries both byte alignments).
fn utf16_text(b: &[u8]) -> String {
    let dec = |off: usize| -> String {
        let u: Vec<u16> = b.get(off..).unwrap_or(&[]).chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&u)
    };
    // From 2019 the `Key: value` block sits at an odd offset after a binary header that holds
    // the path at an even one: prefer the alignment that has the keys.
    let (a, b2) = (dec(0), dec(1));
    for key in ["Worksharing:", "Revit Build:", "Format:", "Revit"] {
        if a.contains(key) {
            return a;
        }
        if b2.contains(key) {
            return b2;
        }
    }
    a
}

/// `Key: value` pairs of BasicFileInfo.
pub fn basic_file_info(b: &[u8]) -> Vec<(String, String)> {
    let text = utf16_text(b);
    let mut out = Vec::new();
    // U+0A0D: a CR LF pair read one byte off, between the binary header and the first key
    for line in text.split(['\r', '\n', '\u{0a0d}']) {
        let line: String = line.chars().filter(|c| !c.is_control()).collect();
        if let Some((k, v)) = line.split_once(": ") {
            // the first key can be glued to binary header bytes: keep its trailing words
            let k = k.rsplit(|c: char| !(c.is_ascii_alphanumeric() || " '_-".contains(c))).next().unwrap_or("").trim();
            // a value running over the binary header (bytes read one off) is not a value
            let garbled = v.chars().any(|c| ('\u{2c00}'..='\u{9fff}').contains(&c));
            if !garbled && !k.is_empty() && k.len() < 80 && k.chars().all(|c| c.is_alphanumeric() || " '_-".contains(c)) {
                out.push((k.to_string(), v.trim().to_string()));
            }
        }
    }
    // Before 2019 the build is also stored in a binary header; keep a fallback.
    if !out.iter().any(|(k, _)| k == "Revit Build") {
        if let Some(i) = text.find("Autodesk Revit") {
            let s: String = text[i..].chars().take_while(|c| !c.is_control() && *c != '\u{fffd}').take(80).collect();
            out.push(("Revit Build".into(), s));
        }
    }
    out
}

/// The Atom XML of `ProjectInformation` / `PartAtom` (plain or inside a one-record ZIP).
fn part_atom_xml(b: &[u8]) -> Option<String> {
    if b.starts_with(b"PK\x03\x04") && b.len() > 30 {
        let method = u16::from_le_bytes([b[8], b[9]]);
        let cs = u32::from_le_bytes(b[18..22].try_into().ok()?) as usize;
        let nl = u16::from_le_bytes([b[26], b[27]]) as usize;
        let xl = u16::from_le_bytes([b[28], b[29]]) as usize;
        let start = 30 + nl + xl;
        let data = b.get(start..(start + cs).min(b.len()))?;
        let raw = if method == 8 {
            let mut v = Vec::new();
            flate2::read::DeflateDecoder::new(data).take(MAX_SMALL).read_to_end(&mut v).ok()?;
            v
        } else {
            data.to_vec()
        };
        return Some(String::from_utf8_lossy(&raw).into_owned());
    }
    let s = String::from_utf8_lossy(b);
    let i = s.find('<')?;
    Some(s[i..].to_string())
}

/// Title, dates and parameters from a PartAtom document.
pub fn part_atom(xml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(doc) = roxmltree::Document::parse(xml) else { return out };
    let root = doc.root_element();
    for c in root.children().filter(|n| n.is_element()) {
        match c.tag_name().name() {
            "title" => out.push(("Titre".into(), c.text().unwrap_or("").trim().to_string())),
            "updated" => out.push(("Mis à jour".into(), c.text().unwrap_or("").trim().replace('T', " ").replace('Z', " UTC"))),
            _ => {}
        }
    }
    if let Some(v) = doc.descendants().find(|n| n.tag_name().name() == "product-version").and_then(|n| n.text()) {
        out.push(("Version du produit".into(), v.trim().to_string()));
    }
    if let Some(cat) = doc.descendants().find(|n| n.tag_name().name() == "category").and_then(|n| n.descendants().find(|d| d.tag_name().name() == "term")).and_then(|n| n.text()) {
        out.push(("Catégorie".into(), cat.trim().to_string()));
    }
    // parameters: leaf elements of the feature groups
    let mut n = 0;
    for g in doc.descendants().filter(|n| n.tag_name().name() == "group") {
        for p in g.children().filter(|n| n.is_element() && n.tag_name().name() != "title" && n.tag_name().name() != "group") {
            let v = p.text().unwrap_or("").trim();
            if v.is_empty() {
                continue;
            }
            let k = p.attribute("displayName").map(str::to_string).unwrap_or_else(|| p.tag_name().name().replace('_', " "));
            out.push((k, v.to_string()));
            n += 1;
            if n >= 60 {
                return out;
            }
        }
    }
    out
}

/// PNG thumbnail inside `RevitPreview4.0`.
pub fn preview(b: &[u8]) -> Option<Rgba> {
    let i = memchr_png(b)?;
    let img = image::load_from_memory_with_format(&b[i..], image::ImageFormat::Png).ok()?.to_rgba8();
    Some(Rgba { width: img.width(), height: img.height(), pixels: img.into_raw() })
}

fn memchr_png(b: &[u8]) -> Option<usize> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    b.windows(8).position(|w| w == SIG)
}

fn label(k: &str) -> Option<&'static str> {
    Some(match k {
        "Revit Build" => "Version de Revit",
        "Format" => "Format",
        "Worksharing" => "Travail partagé",
        "Username" => "Utilisateur",
        "Central Model Path" => "Modèle central",
        "Last Save Path" => "Dernier enregistrement",
        "Locale when saved" => "Langue d'enregistrement",
        "Unique Document GUID" => "GUID du document",
        "Unique Document Increments" => "Révisions",
        _ => return None,
    })
}

pub fn load(path: &Path, opts: &crate::Options) -> Result<Scene, String> {
    let ctl = opts.ctl();
    ctl.report(0.05, "Ouverture du conteneur…");
    let f = std::fs::File::open(path).map_err(|e| format!("lecture impossible : {e}"))?;
    let mut cf = cfb::CompoundFile::open(f).map_err(|_| "fichier Revit illisible (conteneur OLE invalide)".to_string())?;
    let mut s = empty_scene(path, "Revit");
    let mut names: Vec<(String, u64)> = Vec::new();
    for e in cf.walk() {
        if e.is_stream() {
            names.push((e.path().to_string_lossy().replace('\\', "/").trim_start_matches('/').to_string(), e.len()));
        }
    }
    s.stream_count = names.len();
    if !names.iter().any(|(n, _)| n == "BasicFileInfo") {
        return Err("ce fichier OLE n'est pas un fichier Revit (pas de BasicFileInfo)".into());
    }
    ctl.report(0.3, "Lecture des informations…");
    let mut props: Vec<(String, String)> = Vec::new();
    if let Some(b) = read_capped(&mut cf, "/BasicFileInfo") {
        let bfi = basic_file_info(&b);
        if let Some((_, build)) = bfi.iter().find(|(k, _)| k == "Revit Build") {
            if let Some(i) = build.find("Revit ") {
                let year: String = build[i + 6..].chars().take_while(|c| c.is_ascii_digit()).collect();
                if !year.is_empty() {
                    props.push(("Revit".into(), year));
                }
            }
        }
        if let Some((_, f)) = bfi.iter().find(|(k, _)| k == "Format") {
            if f.len() == 4 && f.chars().all(|c| c.is_ascii_digit()) && !props.iter().any(|(k, _)| k == "Revit") {
                props.push(("Revit".into(), f.clone()));
            }
        }
        for (k, v) in bfi {
            if let Some(l) = label(&k) {
                if !v.is_empty() && !props.iter().any(|(pk, _)| pk == l) {
                    props.push((l.to_string(), v));
                }
            }
        }
    }
    ctl.check().map_err(|e| e.to_string())?;
    for name in ["/ProjectInformation", "/PartAtom"] {
        if let Some(xml) = read_capped(&mut cf, name).and_then(|b| part_atom_xml(&b)) {
            for (k, v) in part_atom(&xml) {
                if !props.iter().any(|(pk, pv)| *pk == k && *pv == v) {
                    props.push((k, v));
                }
            }
        }
    }
    ctl.report(0.6, "Vignette…");
    if opts.preview {
        s.preview = read_capped(&mut cf, "/RevitPreview4.0").and_then(|b| preview(&b));
    }
    let (np, sz) = names.iter().filter(|(n, _)| n.starts_with("Partitions/")).fold((0, 0u64), |a, (_, l)| (a.0 + 1, a.1 + l));
    props.push(("Données du modèle".into(), format!("{np} partition(s), {:.1} Mo (format Autodesk propriétaire)", sz as f64 / 1048576.0)));
    ctl.check().map_err(|e| e.to_string())?;
    ctl.report(0.7, "Éléments du modèle…");
    let t0 = std::time::Instant::now();
    let family = matches!(crate::ext_of(path).as_str(), "rfa" | "rft");
    let boxes = element_boxes(&mut cf, &names, family);
    s.timings.decode = t0.elapsed();
    ctl.check().map_err(|e| e.to_string())?;
    let n = match &boxes {
        Ok(b) => build_boxes(&mut s, b, family),
        Err(_) => 0,
    };
    if n > 0 {
        props.push(("Éléments 3D".into(), format!("{n} (boîtes englobantes)")));
        s.warnings.push(
            "Revit : chaque élément est affiché par sa boîte englobante, colorée par catégorie (pas encore sa forme exacte). Pour la géométrie complète : dans Revit, Fichier › Exporter › IFC, puis ouvrez le .ifc dans ModelCraft."
                .into(),
        );
    } else {
        s.warnings.push(
            match &boxes {
                Ok(_) => NO_ELEMENTS,
                Err(_) => NO_ELEMENTS_FORMAT,
            }
            .into(),
        );
    }
    s.properties = props;
    Ok(s)
}

/// Warning when the file holds no placed element with a box (translated by the viewer).
pub const NO_ELEMENTS: &str = "Aucun élément 3D placé dans ce fichier";
/// Same when the element records are not recognised (Revit 2008 and similar old formats).
pub const NO_ELEMENTS_FORMAT: &str = "Aucun élément 3D placé lisible dans ce fichier (format Revit trop ancien ou non reconnu)";

/// Stored size above which a partition is not read (memory bound).
const MAX_PARTITION: u64 = 2 << 30;
/// Most boxes drawn.
const MAX_BOXES: usize = 400_000;

/// The placed elements' boxes of every partition (see `rvt_model`).
pub fn element_boxes<F: Read + std::io::Seek>(cf: &mut cfb::CompoundFile<F>, names: &[(String, u64)], family: bool) -> Result<Vec<rvt_model::ElemBox>, String> {
    let schema = rvt_model::read_stream(cf, "/Formats/Latest", 64 << 20, 64 << 20, |_| true).ok_or("schéma illisible")?;
    if schema.verified == 0 {
        return Err("schéma illisible".into());
    }
    let outline = rvt_model::schema_inline_tag(&schema.data, "Outline").ok_or("format de modèle non reconnu (classe Outline absente)")?;
    drop(schema);
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut any_chain = false;
    for (n, _) in names.iter().filter(|(n, _)| n.starts_with("Partitions/")) {
        let Some(inf) = rvt_model::read_stream(cf, &format!("/{n}"), MAX_PARTITION, rvt_model::MAX_INFLATED, rvt_model::until_chain_end()) else { continue };
        let chain = rvt_model::walk_chain(&inf.data);
        if chain.records.is_empty() {
            continue;
        }
        any_chain = true;
        for b in rvt_model::scan_boxes(&inf.data, &chain, outline, MAX_BOXES).boxes {
            if rvt_model::category_in(b.category, family).is_some() && (!family || b.category != -1 || (0..3).all(|a| b.max[a] - b.min[a] > 1e-3)) && seen.insert((b.category, b.min.map(f64::to_bits), b.max.map(f64::to_bits))) {
                out.push(b);
            }
        }
    }
    if !any_chain {
        return Err("enregistrements d'éléments non reconnus".into());
    }
    Ok(out)
}

/// One mesh and one component per category; returns the number of boxes.
fn build_boxes(s: &mut Scene, boxes: &[rvt_model::ElemBox], family: bool) -> usize {
    let t = std::time::Instant::now();
    let mut by: std::collections::BTreeMap<&'static str, (Vec<[f32; 3]>, Vec<[u32; 3]>, [f32; 4], usize)> = Default::default();
    for b in boxes {
        let Some((name, color)) = rvt_model::category_in(b.category, family) else { continue };
        let e = by.entry(name).or_insert_with(|| (Vec::new(), Vec::new(), color, 0));
        rvt_model::push_box(b, &mut e.0, &mut e.1);
        e.3 += 1;
    }
    let mut n = 0;
    let root = s.components.len();
    s.components.push(crate::component(&crate::stem(&s.path), 0, None, true, None));
    for (name, (pos, tri, color, count)) in by {
        let mesh = sw_format::mesh::build_from_triangles(&pos, &tri, 30.0, true);
        s.meshes.push(sw_format::SceneMesh { name: name.to_string(), mesh, source: sw_format::MeshSource::Embedded, color: Some(color) });
        s.components.push(crate::component(&format!("{name} ({count})"), 1, Some(root), false, Some(s.meshes.len() - 1)));
        n += count;
    }
    if n == 0 {
        s.components.truncate(root);
    }
    s.timings.mesh = t.elapsed();
    s.up_z = true;
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_file_info_lines() {
        let text = "\u{1}\u{0}xx\u{a4}Worksharing: Central\r\nUsername: \r\nRevit Build: Autodesk Revit 2018 (Build: 20000101_0000(x64))\r\nLocale when saved: FRA\r\n";
        let b: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let kv = basic_file_info(&b);
        assert!(kv.contains(&("Worksharing".into(), "Central".into())));
        assert!(kv.contains(&("Locale when saved".into(), "FRA".into())));
        assert!(kv.iter().any(|(k, v)| k == "Revit Build" && v.starts_with("Autodesk Revit 2018")));
    }

    #[test]
    fn part_atom_params() {
        let xml = r#"<?xml version="1.0"?><entry xmlns="http://www.w3.org/2005/Atom" xmlns:A="urn:schemas-autodesk-com:partatom"><title>Projet</title><updated>2000-01-01T00:00:00Z</updated><link><A:design-file><A:product-version>2018</A:product-version></A:design-file></link><A:features><A:feature><A:group><A:title>Id</A:title><Nom displayName="Nom du bâtiment" type="system">B-42</Nom><Vide/></A:group></A:feature></A:features></entry>"#;
        let kv = part_atom(xml);
        assert!(kv.contains(&("Titre".into(), "Projet".into())));
        assert!(kv.contains(&("Version du produit".into(), "2018".into())));
        assert!(kv.contains(&("Nom du bâtiment".into(), "B-42".into())));
    }

    /// Autodesk's sample files (not in the repository): RVT_SAMPLES=/folder, default
    /// /workspace/rvt-samples; skipped when absent.
    #[test]
    fn sample_boxes() {
        let dir = std::env::var_os("RVT_SAMPLES").map(std::path::PathBuf::from).unwrap_or_else(|| "/workspace/rvt-samples".into());
        let get = |n: &str| {
            let p = dir.join(n);
            p.exists().then(|| load(&p, &crate::Options::default()).expect(n))
        };
        let count = |s: &Scene| s.meshes.iter().map(|m| m.mesh.triangle_count() / 12).sum::<usize>();
        if let Some(s) = get("DynamoSample_2025.rvt") {
            let n = count(&s);
            eprintln!("DynamoSample_2025: {n} boxes, {:?}", s.components.iter().map(|c| c.name.clone()).collect::<Vec<_>>());
            assert!(n > 100, "{n}");
            assert!(s.components.iter().any(|c| c.name.starts_with("Murs")));
            // a building a few tens of metres wide
            let (lo, hi) = s.meshes.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(l, h), m| {
                (std::array::from_fn(|i| l[i].min(m.mesh.bbox_min[i])), std::array::from_fn(|i| h[i].max(m.mesh.bbox_max[i])))
            });
            assert!((0..3).all(|i| hi[i] - lo[i] < 200.0), "{lo:?} {hi:?}");
        }
        if let Some(s) = get("M_Chair-Desk.rfa") {
            eprintln!("M_Chair-Desk: {} boxes", count(&s));
            assert!(count(&s) >= 3);
            assert!(s.properties.iter().any(|(k, _)| k == "Revit"));
        }
        // Autodesk's published sample projects (revit.downloads.autodesk.com), 32-bit ids
        for (n, min) in [("ad/rst_2018.rvt", 500), ("ad/rac_2018.rvt", 300), ("ad/rst_2023.rvt", 500), ("ad/rac_2021.rvt", 300), ("ad/rst_2025.rvt", 500)] {
            if let Some(s) = get(n) {
                let k = count(&s);
                eprintln!("{n}: {k} boxes, {:?}", s.components.iter().map(|c| c.name.clone()).collect::<Vec<_>>());
                assert!(k >= min, "{n}: {k}");
                assert!(s.properties.iter().any(|(k, _)| k == "Revit"), "{n}");
            }
        }
        if let Some(s) = get("BatchPrintInitial.rvt") {
            eprintln!("BatchPrintInitial: {} boxes", count(&s));
            assert_eq!(count(&s), 0);
            assert!(!s.warnings.is_empty());
        }
    }

    /// Real files: SW_SAMPLES3=/folder with .rvt files (skipped if absent).
    #[test]
    fn real_rvt_files() {
        let Some(dir) = std::env::var_os("SW_SAMPLES3") else { return };
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if crate::ext_of(&p) != "rvt" {
                continue;
            }
            let s = load(&p, &crate::Options::default()).expect("rvt");
            assert!(s.properties.iter().any(|(k, _)| k == "Revit"), "{}", p.display());
            assert!(s.properties.iter().any(|(k, _)| k == "Titre"), "{}", p.display());
        }
    }
}
