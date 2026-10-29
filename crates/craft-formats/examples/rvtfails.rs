//! Probe (public samples only): element trees that do not decode fully, by class and failing tag.
//! Usage: rvtfails <file> [class]
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
use std::collections::BTreeMap;
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
    let mut h: BTreeMap<(String, String), (usize, Vec<i64>)> = BTreeMap::new();
    let mut tot: BTreeMap<String, usize> = BTreeMap::new();
    let want_cls = a.get(1).cloned();
    let mut nt: Vec<String> = Vec::new();
    for n in names {
        let inf =
            craft_formats::rvt_model::read_stream(&mut cf, &n, 2 << 30, 1 << 30, |_| true).unwrap();
        let w = craft_formats::rvt_model::walk_chain(&inf.data).id_width;
        let w = if w == 0 { 8 } else { w };
        let want = |t: u16| {
            want_cls
                .as_ref()
                .map_or(sc.isa(t, "Element"), |c| sc.name(t) == c)
        };
        elements_dbg(
            &sc,
            &inf.data,
            w,
            &want,
            &mut |_, t, _, tr| {
                *tot.entry(sc.name(t).to_string()).or_default() += 1;
                if let Some(f) = tr.failed {
                    let e = h
                        .entry((sc.name(t).to_string(), sc.name(f).to_string()))
                        .or_default();
                    e.0 += 1;
                    if e.1.len() < 4 {
                        e.1.push(tr.id);
                    }
                }
            },
            &mut |_, t| {
                nt.push(sc.name(t).to_string());
            },
        );
    }
    for c in nt {
        h.entry((c, "<no tree>".into())).or_default().0 += 1;
    }
    for ((c, f), (n, ids)) in h {
        println!(
            "{n:6} / {:6} {c} fails at {f} e.g. {ids:?}",
            tot.get(&c).copied().unwrap_or(0)
        );
    }
}
