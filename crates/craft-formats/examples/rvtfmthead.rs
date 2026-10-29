//! Probe: hex of Formats/Latest from an offset. Usage: rvtfmthead <file> <offset> <len>
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
    let o: usize = a[1].parse().unwrap();
    let n: usize = a[2].parse().unwrap();
    eprintln!("len {:#x}", s.data.len());
    for (r, row) in s.data[o..(o + n).min(s.data.len())].chunks(16).enumerate() {
        let hex: Vec<String> = row.iter().map(|b| format!("{b:02x}")).collect();
        let asc: String = row
            .iter()
            .map(|&b| if b.is_ascii_graphic() { b as char } else { '.' })
            .collect();
        println!("{:06x}  {:<47}  {}", o + 16 * r, hex.join(" "), asc);
    }
}
