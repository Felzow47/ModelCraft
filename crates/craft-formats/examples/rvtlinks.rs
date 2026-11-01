//! Probe: the external references listed in /TransmissionData (linked models' paths).
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let mut raw = Vec::new();
    std::io::Read::read_to_end(&mut cf.open_stream("/TransmissionData").unwrap(), &mut raw).unwrap();
    let u: Vec<u16> = raw.chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    for off in 0..2 {
        let s: String = char::decode_utf16(u[off.min(u.len())..].iter().copied()).map(|c| c.unwrap_or('\u{1}')).collect();
        for part in s.split(|c: char| (c as u32) < 32) {
            if part.to_lowercase().contains(".rvt") {
                println!("{part}");
            }
        }
        let shifted: Vec<u16> = raw[1..].chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let _ = shifted;
    }
}
