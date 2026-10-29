//! Anonymous read-only check of a folder of Revit files: for each `.rvt`/`.rfa` (recursively),
//! prints only its index, size, Revit version, worksharing mode and the boxes found per category.
//! File names and paths are never printed (models under NDA). Nothing is written anywhere.
//!
//! Usage: rvtcount <folder>
use std::path::{Path, PathBuf};

fn walk(d: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(d) else { return };
    let mut v: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    v.sort();
    for p in v {
        if p.is_dir() {
            walk(&p, out);
        } else if matches!(craft_formats::ext_of(&p).as_str(), "rvt" | "rfa") {
            out.push(p);
        }
    }
}

fn main() {
    let Some(root) = std::env::args_os().nth(1) else {
        eprintln!("usage: rvtcount <folder>");
        std::process::exit(2);
    };
    let mut files = Vec::new();
    walk(Path::new(&root), &mut files);
    println!("{} file(s)", files.len());
    for (i, p) in files.iter().enumerate() {
        let mb = std::fs::metadata(p).map(|m| m.len() as f64 / 1e6).unwrap_or(0.0);
        let (mut ws, mut parts) = (String::from("?"), 0usize);
        if let Ok(f) = std::fs::File::open(p) {
            if let Ok(mut cf) = cfb::CompoundFile::open(f) {
                parts = cf.walk().filter(|e| e.is_stream() && e.path().to_string_lossy().contains("Partitions")).count();
                if let Ok(mut st) = cf.open_stream("/BasicFileInfo") {
                    let mut b = Vec::new();
                    let _ = std::io::Read::read_to_end(&mut st, &mut b);
                    if let Some((_, v)) = craft_formats::rvt::basic_file_info(&b).into_iter().find(|(k, _)| k == "Worksharing") {
                        ws = v;
                    }
                }
            }
        }
        if std::env::args().any(|a| a == "--diag") {
            diag(p);
        }
        let t = std::time::Instant::now();
        match craft_formats::rvt::load(p, &craft_formats::Options::default()) {
            Ok(s) => {
                let ver = s.properties.iter().find(|(k, _)| k == "Revit").map(|(_, v)| v.clone()).unwrap_or_else(|| "?".into());
                let n: usize = s.meshes.iter().map(|m| m.mesh.triangle_count() / 12).sum();
                let cats: Vec<String> = s.components.iter().filter(|c| c.name.ends_with(')')).map(|c| c.name.clone()).collect();
                let warn = if n == 0 { s.warnings.join(" | ") } else { String::new() };
                println!("#{:02} {mb:7.1} MB  Revit {ver}  ws={ws}  partitions={parts}  boxes={n}  {:.2}s  {}{}", i + 1, t.elapsed().as_secs_f64(), cats.join(", "), if warn.is_empty() { String::new() } else { format!("  [{warn}]") });
            }
            Err(e) => println!("#{:02} {mb:7.1} MB  ws={ws}  partitions={parts}  ERROR {e}", i + 1),
        }
    }
}

/// Per-partition numbers only (sizes, record and frame counts): `--diag`.
fn diag(p: &Path) {
    use craft_formats::rvt_model as m;
    let Ok(f) = std::fs::File::open(p) else { return };
    let Ok(mut cf) = cfb::CompoundFile::open(f) else { return };
    let outline = m::read_stream(&mut cf, "/Formats/Latest", 64 << 20, 64 << 20, |_| true).and_then(|s| m::schema_inline_tag(&s.data, "Outline"));
    println!("    outline tag {outline:x?}");
    let names: Vec<(String, u64)> = cf.walk().filter(|e| e.is_stream()).map(|e| (e.path().to_string_lossy().replace('\\', "/"), e.len())).collect();
    for (n, len) in names.iter().filter(|(n, _)| n.contains("Partitions/")) {
        let full = m::read_stream(&mut cf, n, 2 << 30, m::MAX_INFLATED, |_| true);
        let Some(inf) = full else {
            println!("    partition stored {len} B: unreadable");
            continue;
        };
        let c = m::walk_chain(&inf.data);
        let sc = outline.map(|o| m::scan_boxes(&inf.data, &c, o, usize::MAX));
        if let Some(sc) = &sc {
            let mut by: std::collections::BTreeMap<i64, usize> = Default::default();
            for b in sc.boxes.iter().filter(|b| m::category(b.category).is_none()) {
                *by.entry(b.category).or_default() += 1;
            }
            let mut v: Vec<_> = by.into_iter().collect();
            v.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
            v.truncate(20);
            println!("    not shown (category: count) {v:?}");
        }
        println!(
            "    partition stored {len} B, inflated {} B in {} members ({} verified), chain {} records w{} end {} closed {}, frames {}, boxes {}",
            inf.data.len(), inf.members, inf.verified, c.records.len(), c.id_width, c.end, c.closed,
            sc.as_ref().map_or(0, |s| s.frames), sc.as_ref().map_or(0, |s| s.boxes.len())
        );
    }
}
