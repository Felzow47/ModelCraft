//! Probe: full field chains (with types) of classes whose name matches.
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
    for p in &a[1..] {
        let exact = p.starts_with('=');
        for (t, n, par, _) in sc.find(p.trim_start_matches('=')) {
            if exact && n != p[1..] {
                continue;
            }
            println!("{t:#x} {n} : {par}");
            for (f, ty) in sc.describe(t) {
                println!("    {f}: {ty}");
            }
        }
    }
}
