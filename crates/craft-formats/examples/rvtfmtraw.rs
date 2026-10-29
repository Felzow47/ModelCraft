//! Probe (public samples only): write an inflated stream to a file. Usage: rvtfmtraw <file> <stream> <out>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let s =
        craft_formats::rvt_model::read_stream(&mut cf, &a[1], 2 << 30, 1 << 30, |_| true).unwrap();
    std::fs::write(&a[2], &s.data).unwrap();
}
