//! Parse an inflated HSF stream (or an .easm/.eprt) and print what was read and where it stopped.
use craft_formats::{easm, hsf};

fn main() {
    let p = std::env::args().nth(1).expect("path");
    let b = std::fs::read(&p).expect("read");
    let m = if b.windows(12).take(64).any(|w| w == b"2008+ archiv") {
        let e = easm::entries(&b).into_iter().find(|(n, _)| n.eq_ignore_ascii_case("eModel")).expect("eModel").1;
        hsf::parse_file(&e).expect("hsf")
    } else {
        hsf::parse(&b)
    };
    let tris: usize = m.shells.iter().flatten().map(|s| s.tris.len()).sum();
    println!("segments {} shells {} (filled {}) tris {} stats {:?}", m.segments.len(), m.shells.len(), m.shells.iter().flatten().count(), tris, m.stats);
    println!("error {:?}", m.error);
    if let Ok(f) = std::env::var("SEG") {
        for s in m.segments.iter().filter(|s| s.path.contains(&f)) {
            println!("{} shells {:?} inc {:?} vis {:?} mat {:?}", s.path, s.shells, s.includes, s.face_visible, s.matrix);
        }
    }
    for (o, op) in &m.recent {
        println!("  @{o} 0x{op:02x} {:?}", *op as char);
    }
}
