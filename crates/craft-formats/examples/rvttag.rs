//! Probe: describe classes by tag.
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
        let t = u16::from_str_radix(p.trim_start_matches("0x"), 16).unwrap();
        println!("{t:#x} {}", sc.name(t));
        for (f, ty) in sc.describe(t) {
            println!("    {f}: {ty}");
        }
    }
}
