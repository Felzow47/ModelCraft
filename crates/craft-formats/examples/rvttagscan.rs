//! Probe (local only): where the tag of a class appears in the (inflated) streams of a file,
//! with the bytes that follow. Usage: rvttagscan <file> <class name> [bytes]
use craft_formats::rvt_browser::Schema;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let rd = |cf: &mut cfb::CompoundFile<std::fs::File>, n: &str| craft_formats::rvt_model::read_stream(cf, n, 2 << 30, 1 << 30, |_| true).map(|s| s.data);
    let s = rd(&mut cf, "/Formats/Latest").unwrap();
    let sc = Schema::parse(&s);
    let Some(tag) = sc.tag_of(&a[1]) else {
        println!("no class {}", a[1]);
        return;
    };
    let n: usize = a.get(2).and_then(|x| x.parse().ok()).unwrap_or(64);
    println!("{} = tag {tag:#x}", a[1]);
    let names: Vec<String> = cf.walk().filter(|e| e.is_stream()).map(|e| e.path().to_string_lossy().to_string()).collect();
    let pat = tag.to_le_bytes();
    for name in names {
        let Some(d) = rd(&mut cf, &name) else { continue };
        let mut hits = 0;
        for i in 0..d.len().saturating_sub(2) {
            if d[i] == pat[0] && d[i + 1] == pat[1] {
                hits += 1;
                if hits <= 6 {
                    let end = (i + n).min(d.len());
                    println!("{name} @{i}: {}", d[i..end].iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "));
                }
            }
        }
        if hits > 6 {
            println!("{name}: {hits} hits");
        }
    }
}
