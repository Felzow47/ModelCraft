//! Probe (local only, never commit outputs): inflate every stream to <outdir>/<name>.bin.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let names: Vec<String> = cf
        .walk()
        .filter(|e| e.is_stream())
        .map(|e| e.path().to_string_lossy().to_string())
        .collect();
    std::fs::create_dir_all(&a[1]).unwrap();
    for n in names {
        if !(n.starts_with("/Partitions") || n.starts_with("/Global") || n.starts_with("/Formats"))
        {
            continue;
        }
        if let Some(inf) =
            craft_formats::rvt_model::read_stream(&mut cf, &n, 2 << 30, 1 << 30, |_| true)
        {
            let f = format!(
                "{}/{}.bin",
                a[1],
                n.trim_start_matches('/').replace('/', "_")
            );
            eprintln!("{n} -> {} ({} members)", inf.data.len(), inf.verified);
            std::fs::write(f, &inf.data).unwrap();
        }
    }
}
