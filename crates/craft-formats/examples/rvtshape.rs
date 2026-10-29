//! Probe (local only): rebuilt element shapes vs element boxes; writes an OBJ of shapes + boxes.
//! Usage: rvtshape <file> [out.obj]
use craft_formats::rvt_browser::Schema;
use std::collections::HashMap;
use std::io::Write;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let family = a[0].ends_with(".rfa");
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let names: Vec<(String, u64)> = cf.walk().filter(|e| e.is_stream()).map(|e| (e.path().to_string_lossy().trim_start_matches('/').to_string(), e.len())).collect();
    let boxes = craft_formats::rvt::element_boxes(&mut cf, &names, family).unwrap();
    let s = craft_formats::rvt_model::read_stream(&mut cf, "/Formats/Latest", 64 << 20, 64 << 20, |_| true).unwrap();
    let sc = Schema::parse(&s.data);
    let mut sh = craft_formats::rvt_shape::Shapes::default();
    if let Some(cd) = craft_formats::rvt_model::read_stream(&mut cf, "/Global/ContentDocuments", 2 << 30, 1 << 30, |_| true) { sh.content_docs(&sc, &cd.data); }
    let t = std::time::Instant::now();
    for (n, _) in names.iter().filter(|(n, _)| n.starts_with("Partitions/")) {
        let inf = craft_formats::rvt_model::read_stream(&mut cf, &format!("/{n}"), 2 << 30, 1 << 30, |_| true).unwrap();
        let w = craft_formats::rvt_model::walk_chain(&inf.data).id_width;
        let w = if w == 0 { 4 } else { w };
        sh.partition(&sc, &inf.data, w);
    }
    let bm: HashMap<i64, ([f64; 3], [f64; 3])> = boxes.iter().filter(|b| b.id != 0).map(|b| (b.id, (b.min, b.max))).collect();
    let meshes = sh.finish(&bm, family);
    eprintln!("decode+build {:?}", t.elapsed());
    let matched = meshes.keys().filter(|k| bm.contains_key(k)).count();
    println!("boxes {} (with id {}), shapes {}, shapes with a box {}", boxes.len(), bm.len(), meshes.len(), matched);
    let mut bad = 0; let mut big = 0;
    for (id, m) in &meshes {
        if let (Some((lo, hi)), Some((blo, bhi))) = (m.bbox(), bm.get(id)) {
            let e = (0..3).map(|k| ((lo[k] as f64 - blo[k]).abs()).max((hi[k] as f64 - bhi[k]).abs())).fold(0.0, f64::max);
            if e > 0.05 { bad += 1; if e > 0.3 { big += 1; } if bad < 15 { println!("  id {id}: shape {lo:?}-{hi:?} box {blo:?}-{bhi:?}"); } }
        }
    }
    println!("shapes off their box by >5 cm: {bad} (>30 cm: {big})");
    if let Ok(ids) = std::env::var("BOX_IDS") {
        for id in ids.split(',').filter_map(|x| x.parse::<i64>().ok()) {
            if let Some(b) = boxes.iter().find(|b| b.id == id) { println!("  box {id} cat {} {:?} {:?} shape {:?}", b.category, b.min, b.max, meshes.get(&id).and_then(|m| m.bbox())); }
        }
    }
    if let Ok(p) = std::env::var("CATS_OUT") {
        let mut f = std::fs::File::create(p).unwrap();
        for b in &boxes { writeln!(f, "{} {} {}", b.id, b.category, craft_formats::rvt_model::category_in(b.category as i64, family).is_some() as u8).unwrap(); }
    }
    let mut left: HashMap<i64, (usize, usize)> = HashMap::new();
    for b in &boxes {
        if craft_formats::rvt_model::category_in(b.category as i64, family).is_none() { continue; }
        let e = left.entry(b.category as i64).or_default();
        e.0 += 1;
        if meshes.contains_key(&b.id) { e.1 += 1; }
    }
    let mut left: Vec<_> = left.into_iter().collect();
    left.sort_by_key(|x| std::cmp::Reverse(x.1 .0 - x.1 .1));
    for (c, (n, k)) in left.iter().take(14) {
        println!("  cat {} {:?}: {k}/{n} shaped", c, craft_formats::rvt_model::category(*c).map(|x| x.0));
    }
    if let Some(o) = a.get(1) {
        let mut f = std::io::BufWriter::new(std::fs::File::create(o).unwrap());
        let mut base = 1;
        let cat: Option<i64> = std::env::var("CAT").ok().and_then(|c| c.parse().ok());
        let cats: HashMap<i64, i64> = boxes.iter().map(|b| (b.id, b.category as i64)).collect();
        for (id, m) in &meshes {
            if cat.is_some() && cats.get(id).copied() != cat { continue; }
            writeln!(f, "o s{id}").unwrap();
            for p in &m.pos { writeln!(f, "v {} {} {}", p[0], p[2], -p[1]).unwrap(); }
            for t in &m.tri { writeln!(f, "f {} {} {}", t[0] + base, t[1] + base, t[2] + base).unwrap(); }
            base += m.pos.len() as u32;
        }
        for b in boxes.iter().filter(|b| !meshes.contains_key(&b.id) && std::env::var_os("SHAPES_ONLY").is_none() && (0..3).all(|k| b.max[k] - b.min[k] < 20.0)) {
            writeln!(f, "o b{}", b.id).unwrap();
            for k in 0..8 { writeln!(f, "v {} {} {}", if k & 1 == 0 { b.min[0] } else { b.max[0] }, if k & 4 == 0 { b.min[2] } else { b.max[2] }, -(if k & 2 == 0 { b.min[1] } else { b.max[1] })).unwrap(); }
            for q in [[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]] {
                writeln!(f, "f {} {} {} {}", q[0] + base, q[1] + base, q[2] + base, q[3] + base).unwrap();
            }
            base += 8;
        }
    }
}
