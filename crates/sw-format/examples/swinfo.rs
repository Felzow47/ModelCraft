//! Print what sw-format sees in a file: `cargo run --example swinfo -- file.SLDPRT [--streams] [--obj out.obj]`
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(file) = args.first() else {
        eprintln!("usage: swinfo <file> [--streams] [--obj out.obj]");
        std::process::exit(2);
    };
    let p = Path::new(file);
    if args.iter().any(|a| a == "--streams") {
        let c = sw_format::Container::open(p).expect("open");
        println!("container: {:?}", c.kind());
        for s in c.streams() {
            println!("  {:50} {:>10}", s.name, s.stored_size);
        }
    }
    let s = sw_format::load_scene(p, &Default::default()).expect("load");
    let t = &s.timings;
    println!(
        "{}: {:?} {:?} meshes={} comps={} tris={} missing={} preview={:?}",
        p.display(),
        s.kind,
        s.container,
        s.meshes.len(),
        s.components.len(),
        s.drawn_triangles(),
        s.missing_components(),
        s.preview.as_ref().map(|i| (i.width, i.height))
    );
    println!(
        "  timings: open {:.2?} read {:.2?} decode {:.2?} mesh {:.2?} ext {:.2?} total {:.2?}",
        t.open, t.read, t.decode, t.mesh, t.external, t.total
    );
    for m in &s.meshes {
        println!(
            "  mesh {:30} faces={:4} tris={:6} edges={:6} bbox={:?}..{:?} {:?}",
            m.name, m.mesh.face_count, m.mesh.triangle_count(), m.mesh.edges.len() / 2, m.mesh.bbox_min, m.mesh.bbox_max, m.source
        );
    }
    for c in &s.components {
        let w = &c.world;
        println!(
            "  {}{} mesh={:?} sup={} hid={} t=({:.3},{:.3},{:.3})",
            "  ".repeat(c.depth), c.name, c.mesh, c.suppressed, c.hidden, w[12], w[13], w[14]
        );
    }
    for w in &s.warnings {
        println!("  WARN {w}");
    }
    if let Some(i) = args.iter().position(|a| a == "--obj") {
        use std::io::Write;
        let mut f = std::io::BufWriter::new(std::fs::File::create(&args[i + 1]).unwrap());
        let mut base = 1;
        for c in &s.components {
            let Some(mi) = c.mesh else { continue };
            let m = &s.meshes[mi].mesh;
            let w = glam::Mat4::from_cols_array(&c.world);
            for p in &m.positions {
                let q = w.transform_point3(glam::Vec3::from(*p));
                writeln!(f, "v {} {} {}", q.x, q.y, q.z).unwrap();
            }
            for t in m.indices.chunks_exact(3) {
                writeln!(f, "f {} {} {}", t[0] as usize + base, t[1] as usize + base, t[2] as usize + base).unwrap();
            }
            base += m.positions.len();
        }
    }
}
