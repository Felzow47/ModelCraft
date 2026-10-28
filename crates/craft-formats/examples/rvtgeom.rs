//! Probe (local only): decode every element generically, report classes and deferred objects.
//! Usage: rvtgeom <file> [class-substring-to-dump] [max dumps]
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
use std::collections::HashMap;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let s = craft_formats::rvt_model::read_stream(&mut cf, "/Formats/Latest", 64 << 20, 64 << 20, |_| true).unwrap();
    let sc = Schema::parse(&s.data);
    let names: Vec<String> = cf.walk().filter(|e| e.is_stream()).map(|e| e.path().to_string_lossy().to_string()).filter(|n| n.starts_with(&std::env::var("RVT_STREAMS").unwrap_or("/Partitions".into()))).collect();
    let mut by: HashMap<String, (usize, usize, usize, usize)> = HashMap::new();
    let mut defs: HashMap<String, usize> = HashMap::new();
    let mut dumps = 0;
    let maxd: usize = a.get(2).and_then(|x| x.parse().ok()).unwrap_or(3);
    for n in names {
        let inf = craft_formats::rvt_model::read_stream(&mut cf, &n, 2 << 30, 1 << 30, |_| true).unwrap();
        let w = craft_formats::rvt_model::walk_chain(&inf.data).id_width;
        let w = if w == 0 { 8 } else { w };
        let want = |t: u16| sc.isa(t, "Element");
        let mut fails: HashMap<String, usize> = HashMap::new();
        elements_dbg(&sc, &inf.data, w, &want, &mut |at, t, size, tr| {
            let e = by.entry(sc.name(t).to_string()).or_default();
            e.0 += 1;
            if tr.failed.is_some() { e.1 += 1; }
            e.2 += tr.objs.len();
            // header size vs consumed
            if (tr.end - at) as u32 == size + 16 { e.3 += 1; }
            for o in &tr.objs { *defs.entry(sc.name(o.tag).to_string()).or_default() += 1; }
            if let Some(p) = a.get(1) {
                if sc.name(t).contains(p.as_str()) && dumps < maxd {
                    dumps += 1;
                    println!("== {} id {} at {at:#x} size {size} consumed {} failed {:?}", sc.name(t), tr.id, tr.end - at, tr.failed.map(|x| sc.name(x).to_string()));
                    println!("  ROOT {:?}", tr.root.fields.iter().map(|(n, g)| format!("{n}={}", short(g))).collect::<Vec<_>>());
                    for (i, o) in tr.objs.iter().enumerate().take(60) { println!("  #{i} {} {:?}", sc.name(o.tag), o.fields.iter().map(|(n, g)| format!("{n}={}", short(g))).collect::<Vec<_>>()); }
                }
            }
        }, &mut |_, t| *fails.entry(sc.name(t).to_string()).or_default() += 1);
        let mut v: Vec<_> = fails.into_iter().filter(|x| x.1 > 2).collect();
        v.sort_by_key(|x| std::cmp::Reverse(x.1));
        println!("--- root failures {n}: {:?}", &v[..v.len().min(60)]);
    }
    let mut v: Vec<_> = by.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1 .0));
    for (n, (c, f, o, sz)) in v.iter().take(100000) { println!("{n}: {c} elems, {f} partial, {o} objs, size-match {sz}"); }
    let mut v: Vec<_> = defs.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1));
    println!("--- deferred classes");
    for (n, c) in v.iter().take(100000) { println!("{n}: {c}"); }
}
fn short(g: &G) -> String {
    match g {
        G::I(v) => v.to_string(),
        G::F(v) => format!("{v:.4}"),
        G::S(s) => format!("{s:?}"),
        G::Fs(v) => format!("F{:?}", &v[..v.len().min(9)].iter().map(|x| (x * 1000.0).round() / 1000.0).collect::<Vec<_>>()) + if v.len() > 9 { &"…" } else { &"" } + &format!("({})", v.len()),
        G::Is(v) => format!("I{:?}({})", &v[..v.len().min(12)], v.len()),
        G::Ids(v) => format!("Ids{:?}", &v[..v.len().min(6)]),
        G::O(o) => format!("{{{}}}", o.fields.iter().map(|(n, g)| format!("{n}={}", short(g))).collect::<Vec<_>>().join(",")),
        G::L(l) => if l.len() <= 6 { format!("L[{}]", l.iter().map(|o| format!("{{{}}}", o.fields.iter().map(|(n, g)| format!("{n}={}", short(g))).collect::<Vec<_>>().join(","))).collect::<Vec<_>>().join(";")) } else { format!("L[{}]", l.len()) },
        G::P(p) => format!("P{p:?}"),
        G::Ps(v) => format!("Ps{:?}", &v[..v.len().min(8)]) + &format!("({})", v.len()),
        G::R(r) => format!("R{r}"),
        G::Raw(b) => format!("raw{}", b.len()),
    }
}
