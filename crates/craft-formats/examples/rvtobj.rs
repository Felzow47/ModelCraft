//! Probe (public samples only): dump the elements of one class (project document only).
//! Usage: rvtobj <file> <class> [n]
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
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
    let names: Vec<String> = cf
        .walk()
        .filter(|e| e.is_stream())
        .map(|e| e.path().to_string_lossy().to_string())
        .filter(|n| n.starts_with("/Partitions"))
        .collect();
    let maxd: usize = a.get(2).and_then(|x| x.parse().ok()).unwrap_or(5);
    let mut dumps = 0;
    for n in names {
        let inf =
            craft_formats::rvt_model::read_stream(&mut cf, &n, 2 << 30, 1 << 30, |_| true).unwrap();
        let w = craft_formats::rvt_model::walk_chain(&inf.data).id_width;
        let w = if w == 0 { 8 } else { w };
        let want = |t: u16| sc.name(t) == a[1];
        elements_dbg(
            &sc,
            &inf.data,
            w,
            &want,
            &mut |_, t, _, tr| {
                if tr.root.int("m_famId").unwrap_or(-1) != -1 || dumps >= maxd {
                    return;
                }
                dumps += 1;
                println!(
                    "== {} id {} failed {:?}",
                    sc.name(t),
                    tr.id,
                    tr.failed.map(|x| sc.name(x).to_string())
                );
                println!(
                    "  ROOT {:?}",
                    tr.root
                        .fields
                        .iter()
                        .map(|(n, g)| format!("{n}={g:?}"))
                        .collect::<Vec<_>>()
                );
                for (i, o) in tr.objs.iter().enumerate().take(40) {
                    println!(
                        "  #{i} {} {:?}",
                        sc.name(o.tag),
                        o.fields
                            .iter()
                            .map(|(n, g)| format!("{n}={g:?}"))
                            .collect::<Vec<_>>()
                    );
                }
            },
            &mut |_, _| {},
        );
    }
}
