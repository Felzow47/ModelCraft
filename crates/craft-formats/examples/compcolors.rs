//! Load a file and print its components with their mesh and override colours.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let s = craft_formats::load_any(std::path::Path::new(&a[0]), &craft_formats::Options::default()).unwrap();
    for c in &s.components {
        let mc = c.mesh.and_then(|m| s.meshes.get(m)).map(|m| (m.name.clone(), m.color));
        println!("{:<3} {:<50} comp {:?} mesh {:?}", c.depth, c.name, c.color, mc);
    }
}
