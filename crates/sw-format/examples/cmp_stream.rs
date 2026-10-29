//! Compare in-memory and streamed loading (and a forced simplification) on every file given.
use sw_format::{load_scene, LoadOptions};
fn main() {
    for a in std::env::args().skip(1) {
        let p = std::path::Path::new(&a);
        let n = match load_scene(p, &LoadOptions::default()) {
            Ok(s) => s,
            Err(e) => {
                println!("{a}: ERR {e}");
                continue;
            }
        };
        std::env::set_var("MODELCRAFT_STREAM", "1");
        let s = load_scene(p, &LoadOptions::default()).unwrap();
        std::env::set_var("MODELCRAFT_MAX_VERTICES", "10000");
        let l = load_scene(p, &LoadOptions::default()).unwrap();
        std::env::remove_var("MODELCRAFT_STREAM");
        std::env::remove_var("MODELCRAFT_MAX_VERTICES");
        let v = |s: &sw_format::Scene| {
            s.meshes
                .iter()
                .map(|m| m.mesh.positions.len())
                .sum::<usize>()
        };
        println!(
            "{}: tris {} / streamed {} | verts {} -> simplified {} ({} tris) {}",
            p.file_name().unwrap().to_string_lossy(),
            n.drawn_triangles(),
            s.drawn_triangles(),
            v(&n),
            v(&l),
            l.drawn_triangles(),
            if n.drawn_triangles() == s.drawn_triangles() {
                "OK"
            } else {
                "MISMATCH"
            }
        );
    }
}
