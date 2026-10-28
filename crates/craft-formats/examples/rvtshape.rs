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
    let mut bad = 0;
    for (id, m) in &meshes {
        if let (Some((lo, hi)), Some((blo, bhi))) = (m.bbox(), bm.get(id)) {
            let e = (0..3).map(|k| ((lo[k] as f64 - blo[k]).abs()).max((hi[k] as f64 - bhi[k]).abs())).fold(0.0, f64::max);
            if e > 0.05 { bad += 1; if bad < 15 { println!("  id {id}: shape {lo:?}-{hi:?} box {blo:?}-{bhi:?}"); } }
        }
    }
    println!("shapes off their box by >5 cm: {bad}");
    if let Some(o) = a.get(1) {
        let mut f = std::io::BufWriter::new(std::fs::File::create(o).unwrap());
        let mut base = 1;
        for (id, m) in &meshes {
            writeln!(f, "o s{id}").unwrap();
            for p in &m.pos { writeln!(f, "v {} {} {}", p[0], p[1], p[2]).unwrap(); }
            for t in &m.tri { writeln!(f, "f {} {} {}", t[0] + base, t[1] + base, t[2] + base).unwrap(); }
            base += m.pos.len() as u32;
        }
        for b in boxes.iter().filter(|b| !meshes.contains_key(&b.id)) {
            writeln!(f, "o b{}", b.id).unwrap();
            for k in 0..8 { writeln!(f, "v {} {} {}", if k & 1 == 0 { b.min[0] } else { b.max[0] }, if k & 2 == 0 { b.min[1] } else { b.max[1] }, if k & 4 == 0 { b.min[2] } else { b.max[2] }).unwrap(); }
            for q in [[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]] {
                writeln!(f, "f {} {} {} {}", q[0] + base, q[1] + base, q[2] + base, q[3] + base).unwrap();
            }
            base += 8;
        }
    }
}
