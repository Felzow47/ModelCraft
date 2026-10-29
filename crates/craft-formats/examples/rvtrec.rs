//! Probe (public samples only): the chain record of element ids: partition, offset, tag.
//! Usage: rvtrec <file> <id,id,...>
use craft_formats::rvt_browser::Schema;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let s = craft_formats::rvt_model::read_stream(
        &mut cf,
        "/Formats/Latest",
        64 << 20,
        64 << 20,
        |_| true,
    )
    .unwrap();
    let sc = Schema::parse(&s.data);
    let ids: Vec<u64> = a[1].split(',').filter_map(|x| x.parse().ok()).collect();
    let names: Vec<String> = cf
        .walk()
        .filter(|e| e.is_stream())
        .map(|e| e.path().to_string_lossy().to_string())
        .filter(|n| n.starts_with("/Partitions"))
        .collect();
    for n in names {
        let inf =
            craft_formats::rvt_model::read_stream(&mut cf, &n, 2 << 30, 1 << 30, |_| true).unwrap();
        let ch = craft_formats::rvt_model::walk_chain(&inf.data);
        let d = &inf.data;
        for (k, &(o, id)) in ch.records.iter().enumerate() {
            if !ids.contains(&id) {
                continue;
            }
            let next = ch.records.get(k + 1).map(|r| r.0).unwrap_or(d.len());
            println!(
                "{n} id {id} at {o:#x} len {:#x} width {}",
                next - o,
                ch.id_width
            );
            let w = if ch.id_width == 0 { 4 } else { ch.id_width };
            let at = o + 8 + w;
            if at + 2 <= d.len() {
                let t = u16::from_le_bytes([d[at], d[at + 1]]);
                println!("  tag {t:#x} {}", sc.name(t));
            }
            let hex: String = d[o..(o + 96).min(d.len())]
                .iter()
                .map(|b| format!("{b:02x} "))
                .collect();
            println!("  {hex}");
        }
    }
}
