//! Probe: the full field chain (with types) of classes, as the decoder reads them; with
//! `type=<b0>` instead of class names, every own field of that primitive type.
//! Usage: rvtchain <file> <class>… | rvtchain <file> type=06
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let s = craft_formats::rvt_model::read_stream(&mut cf, "/Formats/Latest", 64 << 20, 64 << 20, |_| true).unwrap();
    let sc = craft_formats::rvt_browser::Schema::parse(&s.data);
    for c in &a[1..] {
        if let Some(b0) = c.strip_prefix("type=") {
            let mut seen = std::collections::BTreeSet::new();
            for t in sc.tags_list() {
                for (n, ty) in sc.describe(t) {
                    if ty.starts_with(&format!("{b0} ")) && seen.insert(n.clone()) {
                        println!("  {} {n}: {ty}", sc.name(t));
                    }
                }
            }
            continue;
        }
        let Some(t) = sc.tag_of(c) else { continue };
        println!("{c} {t:#x}");
        for (n, ty) in sc.describe(t) {
            println!("  {n}: {ty}");
        }
    }
}
