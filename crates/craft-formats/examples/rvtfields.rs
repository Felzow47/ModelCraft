//! Probe: classes with a field whose name or type contains a substring.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let s = craft_formats::rvt_model::read_stream(
        &mut cf,
        "/Formats/Latest",
        64 << 20,
        64 << 20,
        |_| true,
    )
    .unwrap();
    let sc = craft_formats::rvt_browser::Schema::parse(&s.data);
    for (t, n, par, own) in sc.find("") {
        let d = sc.describe(t);
        let own: Vec<_> = d
            .iter()
            .filter(|(f, ty)| {
                own.contains(f)
                    && a[1..]
                        .iter()
                        .any(|p| f.contains(p.as_str()) || ty.contains(p.as_str()))
            })
            .collect();
        if !own.is_empty() {
            println!("{t:#x} {n} : {par} {own:?}");
        }
    }
}
