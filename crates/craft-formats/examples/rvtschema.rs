//! Probe: inline class definitions of Formats/Latest (`[tag | 0x8000] 00 00 [len u16] name`).
//! Usage: rvtschema <file> [substring | 0xTAG]…
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
    let d = &s.data;
    let mut n = 0;
    for i in 0..d.len().saturating_sub(8) {
        // [u16 len] name [u16 tag | 0x8000]
        let l = u16::from_le_bytes([d[i], d[i + 1]]) as usize;
        if !(3..120).contains(&l) || i + 4 + l > d.len() {
            continue;
        }
        let t = u16::from_le_bytes([d[i + 2 + l], d[i + 3 + l]]);
        if t & 0x8000 == 0 {
            continue;
        }
        let w = &d[i + 2..i + 2 + l];
        if !(w
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || b"_:<>, ".contains(c))
            && w[0].is_ascii_alphabetic())
        {
            continue;
        }
        let name = std::str::from_utf8(w).unwrap();
        let tag = t & 0x7fff;
        n += 1;
        let ts = format!("{tag:#x}");
        if a.len() == 1 || a[1..].iter().any(|p| name.contains(p.as_str()) || *p == ts) {
            println!("{ts} {name}");
        }
    }
    eprintln!("{n} inline classes");
}
