//! Print what craft-formats reads from files: `cargo run --example cfinfo -- file...`
fn main() {
    for a in std::env::args().skip(1) {
        let p = std::path::Path::new(&a);
        match craft_formats::load_any(p, &Default::default()) {
            Ok(s) => {
                let (lo, hi) = s.bounds().unwrap_or(([0.0; 3], [0.0; 3]));
                let colored = s.meshes.iter().filter(|m| m.color.is_some()).count();
                println!(
                    "{a}: {:?} {} comps={} meshes={} (colored {colored}) tris={} faces={} bbox(mm)={:.1}x{:.1}x{:.1} t={:.1}ms warn={:?}",
                    s.kind,
                    s.container.label(),
                    s.components.len(),
                    s.meshes.len(),
                    s.drawn_triangles(),
                    s.meshes.iter().map(|m| m.mesh.face_count).sum::<usize>(),
                    (hi[0] - lo[0]) * 1000.0,
                    (hi[1] - lo[1]) * 1000.0,
                    (hi[2] - lo[2]) * 1000.0,
                    s.timings.total.as_secs_f64() * 1000.0,
                    s.warnings
                );
            }
            Err(e) => println!("{a}: ERREUR {e}"),
        }
    }
}
