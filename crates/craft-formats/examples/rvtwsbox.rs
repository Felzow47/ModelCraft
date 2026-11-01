//! Probe: the drawn elements (boxes) by workset. Usage: rvtwsbox <file>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let names: Vec<(String, u64)> = cf.walk().filter(|e| e.is_stream()).map(|e| (e.path().to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/").trim_start_matches('/').to_string(), e.len())).collect();
    let rd = |cf: &mut cfb::CompoundFile<std::fs::File>, n: &str| craft_formats::rvt_model::read_stream(cf, n, 1 << 30, 1 << 30, |_| true).unwrap().data;
    let p = rd(&mut cf, "/Global/PartitionTable");
    let ws = craft_formats::rvt_model::worksets(&p);
    let el = craft_formats::rvt_model::elem_worksets(&rd(&mut cf, "/Global/ElemTable"), &ws);
    let boxes = craft_formats::rvt::element_boxes(&mut cf, &names, false).unwrap();
    let mut by = std::collections::BTreeMap::<Option<u32>, usize>::new();
    for b in &boxes { *by.entry(el.get(&b.id).copied()).or_default() += 1; }
    println!("{} boxes", boxes.len());
    let e = rd(&mut cf, "/Global/ElemTable");
    for b in boxes.iter().filter(|b| !el.contains_key(&b.id)).take(6) {
        let pos: Vec<usize> = e.windows(8).enumerate().filter(|(_, w)| *w == b.id.to_le_bytes()).map(|(i, _)| i).take(4).collect();
        println!("missing {} at {:x?} (mod 40: {:?})", b.id, pos, pos.iter().map(|p| (p + 34) % 40).collect::<Vec<_>>());
    }
    for (w, n) in by { println!("{w:?} {n}"); }
    if let Some(want) = a.get(1).and_then(|x| x.parse::<u32>().ok()) {
        for b in boxes.iter().filter(|b| el.get(&b.id) == Some(&want)) {
            println!("ws {want}: id {} cat {} {:?} {:?} ({:?})", b.id, b.category, b.min, b.max, craft_formats::rvt_model::category(b.category).map(|c| c.0));
        }
    }
}
