//! Probe (public samples only): hex dump of an inflated partition range, and its chain end.
//! Usage: rvtdump <file> <partition> <offset hex> <len>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let inf =
        craft_formats::rvt_model::read_stream(&mut cf, &a[1], 2 << 30, 1 << 30, |_| true).unwrap();
    let ch = craft_formats::rvt_model::walk_chain(&inf.data);
    eprintln!(
        "len {:#x} chain end {:#x} records {}",
        inf.data.len(),
        ch.end,
        ch.records.len()
    );
    let o = usize::from_str_radix(a[2].trim_start_matches("0x"), 16).unwrap();
    let n: usize = a[3].parse().unwrap();
    for (k, row) in inf.data[o..(o + n).min(inf.data.len())]
        .chunks(16)
        .enumerate()
    {
        let hex: Vec<String> = row.iter().map(|b| format!("{b:02x}")).collect();
        let asc: String = row
            .iter()
            .map(|&b| if b.is_ascii_graphic() { b as char } else { '.' })
            .collect();
        println!("{:08x}  {}  {}", o + 16 * k, hex.join(" "), asc);
    }
}
