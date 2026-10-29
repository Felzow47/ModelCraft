//! Probe: classes of /Formats/Latest whose name contains a substring, with own fields.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let s = craft_formats::rvt_model::read_stream(&mut cf, "/Formats/Latest", 64 << 20, 64 << 20, |_| true).unwrap();
    let sc = craft_formats::rvt_browser::Schema::parse(&s.data);
    println!("{} classes", sc.tags_list().len());
    for p in &a[1..] {
        for (t, n, par, f) in sc.find(p) {
            println!("{t:#x} {n} : {par} {f:?}");
        }
    }
}
