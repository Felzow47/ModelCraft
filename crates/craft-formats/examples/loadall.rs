//! Load every file given on the command line (or found in the given folders) and print a summary.
//! cargo run --release -p craft-formats --features assimp --example loadall -- <files or dirs>
fn main() {
    let mut files = Vec::new();
    for a in std::env::args().skip(1) {
        let p = std::path::PathBuf::from(a);
        if p.is_dir() {
            let mut v: Vec<_> = std::fs::read_dir(&p).unwrap().flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
            v.sort();
            files.extend(v);
        } else {
            files.push(p);
        }
    }
    for f in files {
        if !craft_formats::is_supported(&f) {
            continue;
        }
        let t = std::time::Instant::now();
        let r = std::panic::catch_unwind(|| craft_formats::load_any(&f, &craft_formats::Options::default()));
        let name = f.display();
        match r {
            Ok(Ok(s)) => {
                let tris: usize = s.meshes.iter().map(|m| m.mesh.triangle_count()).sum();
                let mut lo = [f32::MAX; 3];
                let mut hi = [f32::MIN; 3];
                if std::env::var_os("LOADALL_COLORS").is_some() {
                    for c in s.components.iter().filter(|c| c.mesh.is_some()).take(25) {
                        let m = &s.meshes[c.mesh.unwrap()].mesh;
                        let mut vol = 0f64;
                        for t in m.indices.chunks_exact(3) {
                            let p = |i: u32| { let q = m.positions[i as usize]; [q[0] as f64, q[1] as f64, q[2] as f64] };
                            let (a, b, c2) = (p(t[0]), p(t[1]), p(t[2]));
                            vol += (a[0] * (b[1] * c2[2] - b[2] * c2[1]) - a[1] * (b[0] * c2[2] - b[2] * c2[0]) + a[2] * (b[0] * c2[1] - b[1] * c2[0])) / 6.0;
                        }
                        let bad = m.normals.iter().filter(|n| { let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt(); !(l > 0.5) }).count();
                        println!("  {:40} {:?} vol {vol:.4} badn {bad}/{} tris {}", c.name, c.color, m.normals.len(), m.indices.len() / 3);
                    }
                }
                for c in &s.components {
                    if let Some(m) = c.mesh {
                        let b = &s.meshes[m].mesh;
                        for p in b.positions.iter().step_by(1 + b.positions.len() / 2000) {
                            let w = &c.world;
                            let q = [0, 1, 2].map(|r| w[r] * p[0] + w[4 + r] * p[1] + w[8 + r] * p[2] + w[12 + r]);
                            for k in 0..3 {
                                lo[k] = lo[k].min(q[k]);
                                hi[k] = hi[k].max(q[k]);
                            }
                        }
                    }
                }
                let ext = [0, 1, 2].map(|k| hi[k] - lo[k]);
                println!("OK   {name}: {} meshes, {} comps, {tris} tris, size {:.3}x{:.3}x{:.3} m, up_z={} {:?} ({} ms){}", s.meshes.len(), s.components.len(), ext[0], ext[1], ext[2], s.up_z, s.container, t.elapsed().as_millis(), if s.warnings.is_empty() { String::new() } else { format!(" warn: {}", s.warnings.join(" | ")) });
            }
            Ok(Err(e)) => println!("ERR  {name}: {e}"),
            Err(_) => println!("PANIC {name}"),
        }
    }
}
