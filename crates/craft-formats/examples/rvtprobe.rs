//! Probe (public samples only): occurrences of a category value (i32 / i64) in the record chain,
//! with the readable UTF-16 strings that follow. Usage: rvtprobe <file> <category> [window]
fn strings(d: &[u8], from: usize, to: usize) -> Vec<(usize, String)> {
    let mut out = vec![];
    let mut o = from;
    while o + 4 < to.min(d.len()) {
        let n = u32::from_le_bytes(d[o..o + 4].try_into().unwrap()) as usize;
        if (1..80).contains(&n) && o + 4 + 2 * n <= d.len() {
            let w = &d[o + 4..o + 4 + 2 * n];
            let s: Option<String> =
                char::decode_utf16(w.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])))
                    .map(|r| r.ok().filter(|c| !c.is_control()))
                    .collect();
            if let Some(s) = s.filter(|s| s.chars().any(|c| c.is_alphanumeric())) {
                out.push((o - from, s));
                o += 4 + 2 * n;
                continue;
            }
        }
        o += 1;
    }
    out
}
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let cat: i64 = a[1].parse().unwrap();
    let win: usize = a.get(2).and_then(|x| x.parse().ok()).unwrap_or(0x300);
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let names: Vec<String> = cf
        .walk()
        .filter(|e| e.is_stream())
        .map(|e| e.path().to_string_lossy().into_owned())
        .filter(|n| n.contains("Partitions"))
        .collect();
    for n in names {
        let Some(inf) = craft_formats::rvt_model::read_stream(
            &mut cf,
            &n,
            2 << 30,
            1 << 30,
            craft_formats::rvt_model::until_chain_end(),
        ) else {
            continue;
        };
        let ch = craft_formats::rvt_model::walk_chain(&inf.data);
        let d = &inf.data[..ch.end.min(inf.data.len())];
        let pat: Vec<u8> = if ch.id_width == 8 {
            cat.to_le_bytes().to_vec()
        } else {
            (cat as i32).to_le_bytes().to_vec()
        };
        let mut i = 0;
        let mut k = 0;
        while let Some(p) = d[i..].windows(pat.len()).position(|w| w == pat.as_slice()) {
            let at = i + p;
            // enclosing top-level record
            let r = ch
                .records
                .partition_point(|&(o, _)| o <= at)
                .saturating_sub(1);
            let (ro, rid) = ch.records[r];
            println!(
                "{n} cat@{at:#x} rec {rid} +{:#x} {:?}",
                at - ro,
                strings(d, at, at + win)
                    .into_iter()
                    .take(6)
                    .collect::<Vec<_>>()
            );
            k += 1;
            i = at + 1;
        }
        println!(
            "{n}: {k} occurrences, {} records, id width {}",
            ch.records.len(),
            ch.id_width
        );
    }
}
