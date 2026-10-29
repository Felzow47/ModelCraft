//! Probe (public samples only): dump elements by id, or list the elements of one family document.
//! Usage: rvtid <file> <id,id,...> [fam=<root id>] [depth]
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
    let ids: Vec<i64> = a[1].split(',').filter_map(|x| x.parse().ok()).collect();
    let fams: Vec<i64> = a
        .iter()
        .filter_map(|x| x.strip_prefix("fam="))
        .flat_map(|x| x.split(','))
        .filter_map(|x| x.parse().ok())
        .collect();
    let nobj: usize = std::env::var("NOBJ")
        .ok()
        .and_then(|x| x.parse().ok())
        .unwrap_or(60);
    let names: Vec<String> = cf
        .walk()
        .filter(|e| e.is_stream())
        .map(|e| e.path().to_string_lossy().to_string())
        .filter(|n| n.starts_with("/Partitions"))
        .collect();
    for n in names {
        let inf =
            craft_formats::rvt_model::read_stream(&mut cf, &n, 2 << 30, 1 << 30, |_| true).unwrap();
        let w = craft_formats::rvt_model::walk_chain(&inf.data).id_width;
        let w = if w == 0 { 4 } else { w };
        let want = |t: u16| sc.isa(t, "Element");
        elements_dbg(
            &sc,
            &inf.data,
            w,
            &want,
            &mut |_, t, _, tr| {
                let fam = tr.root.int("m_famId").unwrap_or(-1);
                if fams.contains(&fam) && !ids.contains(&tr.id) {
                    println!(
                        "-- fam {fam} {} id {} ({} objs) failed {:?}",
                        sc.name(t),
                        tr.id,
                        tr.objs.len(),
                        tr.failed.map(|x| sc.name(x).to_string())
                    );
                }
                if let Ok(g) = std::env::var("CLASSGREP") {
                    if sc.name(t).contains(&g) {
                        println!(
                            "-- {} id {} failed {:?}",
                            sc.name(t),
                            tr.id,
                            tr.failed.map(|x| sc.name(x).to_string())
                        );
                    }
                }
                if !ids.contains(&tr.id) {
                    return;
                }
                println!(
                    "== {} id {} in {n} failed {:?}",
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
                for (i, o) in tr.objs.iter().enumerate().take(nobj) {
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
            &mut |p, t| {
                if std::env::var_os("FAILS").is_some() {
                    println!("FAIL at {p} {}", sc.name(t));
                }
            },
        );
    }
}
