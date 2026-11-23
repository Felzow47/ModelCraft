//! Dump every stream of a SolidWorks file (decompressed when zlib) into a folder, and the
//! preview image the loader finds. Usage: swstreams <file> <outdir>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let c = sw_format::container::Container::open(std::path::Path::new(&a[0])).unwrap();
    std::fs::create_dir_all(&a[1]).unwrap();
    for s in c.streams() {
        let Ok(raw) = c.read_info(s) else { continue };
        let d = sw_format::container::decode_zlb(&raw).unwrap_or(raw);
        let name = s.name.replace(|c: char| matches!(c, '/' | '\\' | ':'), "_");
        std::fs::write(format!("{}/{}", a[1], name), &d).unwrap();
        println!("{} {}", s.name, d.len());
    }
}
