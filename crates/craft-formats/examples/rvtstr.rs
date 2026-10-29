//! Probe (public samples only): UTF-16 strings in the inflated partitions that look like
//! view / sheet names, with the record offset. Usage: rvtstr <file> <needle>…
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let f = std::fs::File::open(&a[0]).unwrap();
    let mut cf = cfb::CompoundFile::open(f).unwrap();
    let names: Vec<String> = cf
        .walk()
        .filter(|e| e.is_stream())
        .map(|e| e.path().to_string_lossy().into_owned())
        .collect();
    for n in names
        .iter()
        .filter(|n| n.contains("Partitions") || n.contains("Global"))
    {
        let Some(inf) =
            craft_formats::rvt_model::read_stream(&mut cf, n, 2 << 30, 1 << 30, |_| true)
        else {
            continue;
        };
        let d = &inf.data;
        let mut hits = 0;
        for needle in &a[1..] {
            let pat: Vec<u8> = needle
                .encode_utf16()
                .flat_map(|u| u.to_le_bytes())
                .collect();
            let mut i = 0;
            while let Some(p) = d[i..].windows(pat.len()).position(|w| w == pat.as_slice()) {
                let at = i + p;
                // read the full string around: walk back to the length prefix
                let mut s = at;
                while s >= 2 && d[s - 1] == 0 && d[s - 2].is_ascii_graphic()
                    || s >= 2 && d[s - 1] == 0 && d[s - 2] == b' '
                {
                    s -= 2;
                }
                let mut e = at;
                while e + 1 < d.len() && d[e + 1] == 0 && (d[e].is_ascii_graphic() || d[e] == b' ')
                {
                    e += 2;
                }
                let txt: String = d[s..e].chunks(2).map(|c| c[0] as char).collect();
                let pre = &d[s.saturating_sub(16)..s];
                if hits < 40 {
                    println!("{n} @{s:#x} {txt:?} pre={pre:02x?}");
                }
                hits += 1;
                i = at + pat.len();
            }
        }
        if hits > 0 {
            println!("{n}: {hits} hits");
        }
    }
}
