//! Probe: the compound file's streams and their sizes.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    for e in cf.walk() {
        if e.is_stream() {
            println!("{} {}", e.path().display(), e.len());
        }
    }
}
