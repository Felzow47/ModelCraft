//! Probe (local only): view-specific annotation elements of the project (not of family documents),
//! by class, with the number decoded without failure. Usage: rvtann <file> [class to dump] [n]
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
use std::collections::HashMap;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let s = craft_formats::rvt_model::read_stream(&mut cf, "/Formats/Latest", 64 << 20, 64 << 20, |_| true).unwrap();
    let sc = Schema::parse(&s.data);
    let names: Vec<String> = cf.walk().filter(|e| e.is_stream()).map(|e| e.path().to_string_lossy().to_string()).filter(|n| n.starts_with("/Partitions")).collect();
    let mut by: HashMap<String, (usize, usize)> = HashMap::new();
    let mut fails: HashMap<String, usize> = HashMap::new();
    let maxd: usize = a.get(2).and_then(|x| x.parse().ok()).unwrap_or(2);
    let mut dumps = 0;
    for n in names {
        let inf = craft_formats::rvt_model::read_stream(&mut cf, &n, 2 << 30, 1 << 30, |_| true).unwrap();
        let w = craft_formats::rvt_model::walk_chain(&inf.data).id_width;
        let w = if w == 0 { 8 } else { w };
        let want = |t: u16| sc.isa(t, "Element");
        elements_dbg(&sc, &inf.data, w, &want, &mut |_, t, _, tr| {
            if tr.root.int("m_famId").unwrap_or(-1) != -1 { return; }
            let Some(ov) = tr.root.int("m_ownerDBViewId").filter(|v| *v != -1) else { return };
            let e = by.entry(sc.name(t).to_string()).or_default();
            e.0 += 1;
            if tr.failed.is_some() { e.1 += 1; }
            if a.get(1).is_some_and(|p| p == sc.name(t)) && dumps < maxd {
                dumps += 1;
                println!("== {} id {} owner {ov} failed {:?}", sc.name(t), tr.id, tr.failed.map(|x| sc.name(x).to_string()));
                println!("  ROOT {:?}", tr.root.fields.iter().map(|(n, g)| format!("{n}={g:?}")).collect::<Vec<_>>());
                for (i, o) in tr.objs.iter().enumerate().take(40) { println!("  #{i} {} {:?}", sc.name(o.tag), o.fields.iter().map(|(n, g)| format!("{n}={g:?}")).collect::<Vec<_>>()); }
            }
        }, &mut |_, t| *fails.entry(sc.name(t).to_string()).or_default() += 1);
    }
    let mut v: Vec<_> = by.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1 .0));
    for (n, (c, f)) in v { println!("{n}: {c} ({f} partial)"); }
    let mut v: Vec<_> = fails.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1));
    println!("root failures (all docs): {:?}", &v[..v.len().min(15)]);
}
