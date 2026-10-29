//! Probe: hex around a name in Formats/Latest. Usage: rvtfmt <file> <name> [before] [after]
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
    let b: usize = a.get(2).and_then(|x| x.parse().ok()).unwrap_or(32);
    let af: usize = a.get(3).and_then(|x| x.parse().ok()).unwrap_or(96);
    let pat = a[1].as_bytes();
    let mut i = 0;
    let mut k = 0;
    while let Some(p) = d[i..].windows(pat.len()).position(|w| w == pat) {
        let at = i + p;
        i = at + 1;
        if d.get(at + pat.len())
            .is_some_and(|c| c.is_ascii_alphanumeric())
        {
            continue;
        }
        k += 1;
        if k > 3 {
            break;
        }
        let o = at.saturating_sub(b);
        println!("-- @{at:#x}");
        for (r, row) in d[o..(at + af).min(d.len())].chunks(16).enumerate() {
            let hex: Vec<String> = row.iter().map(|b| format!("{b:02x}")).collect();
            let asc: String = row
                .iter()
                .map(|&b| if b.is_ascii_graphic() { b as char } else { '.' })
                .collect();
            println!("{:06x}  {:<47}  {}", o + 16 * r, hex.join(" "), asc);
        }
    }
}
