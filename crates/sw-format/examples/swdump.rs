//! Read-only: list streams and dump selected (decompressed) streams to an output folder.
//! `swdump <file> <outdir> [name-substring ...]` (outdir must be outside the source tree).
use std::path::Path;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 2 {
        eprintln!("usage: swdump <file> <outdir> [substr...]");
        std::process::exit(2);
    }
    let c = sw_format::Container::open(Path::new(&a[0])).expect("open");
    println!("container: {:?} size={}", c.kind(), c.file_size());
    std::fs::create_dir_all(&a[1]).unwrap();
    for s in c.streams() {
        let want = a.len() > 2 && a[2..].iter().any(|k| k == "*" || s.name.to_lowercase().contains(&k.to_lowercase()));
        let mut line = format!("  {:60} stored={:>11}", s.name, s.stored_size);
        if want {
            match c.read_info(s) {
                Ok(d) => {
                    let fname: String = s.name.chars().map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch } else { '_' }).collect();
                    std::fs::write(Path::new(&a[1]).join(format!("{fname}.bin")), &d).unwrap();
                    line += &format!(" raw={} DUMPED", d.len());
                }
                Err(e) => line += &format!(" ERR {e}"),
            }
        }
        println!("{line}");
    }
}
