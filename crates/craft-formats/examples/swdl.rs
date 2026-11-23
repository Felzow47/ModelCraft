//! Dump a SolidWorks file's decompressed DisplayLists stream and its faces' offsets.
//! Usage: swdl <file> <out.bin>  (prints: block path, face offset, vertices, strips)
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let c = sw_format::container::Container::open(std::path::Path::new(&a[0])).unwrap();
    let raw = c.read(sw_format::DISPLAY_LISTS).unwrap().unwrap();
    let d = sw_format::container::decode_zlb(&raw).unwrap_or(raw);
    std::fs::write(&a[1], &d).unwrap();
    let dl = sw_format::dl::decode(&d);
    for b in &dl.blocks {
        for f in &b.faces {
            println!("{:?}|{}|{}|{}", b.path, f.offset, f.positions.len(), f.strips.len());
        }
    }
}
