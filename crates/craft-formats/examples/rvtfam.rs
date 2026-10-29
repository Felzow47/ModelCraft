//! Public-sample probe: class histogram of Symbol / Family / Category / group elements.
use craft_formats::rvt_browser::*;
use craft_formats::rvt_model as m;
use std::collections::BTreeMap;
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let show: Vec<String> = std::env::args().skip(2).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&p).unwrap()).unwrap();
    let fmt = m::read_stream(&mut cf, "/Formats/Latest", 64 << 20, 64 << 20, |_| true).unwrap();
    let s = Schema::parse(&fmt.data);
    let mut want = vec![false; 65536];
    for t in s.tags_list() {
        if ["Element"].iter().any(|b| s.isa(t, b)) {
            want[t as usize] = true;
        }
    }
    let names: Vec<String> = cf
        .walk()
        .filter(|e| e.is_stream())
        .map(|e| e.path().to_string_lossy().replace('\\', "/"))
        .collect();
    let mut h: BTreeMap<String, usize> = BTreeMap::new();
    for n in names.iter().filter(|n| n.starts_with("/Partitions/")) {
        let inf = m::read_stream(&mut cf, n, 2 << 30, m::MAX_INFLATED, |_| true).unwrap();
        let w = m::walk_chain(&inf.data).id_width;
        let w = if w == 0 { 4 } else { w };
        for e in scan(&s, &inf.data, w, &want) {
            let c = s.name(e.tag).to_string();
            if let Ok(r) = std::env::var("REF") {
                let r: Vec<i64> = r.split(',').filter_map(|x| x.parse().ok()).collect();
                let hit: Vec<_> =
                    e.f.iter()
                        .filter(|(k, v)| {
                            **k != "m_id"
                                && matches!(v, craft_formats::rvt_browser::V::I(x) if r.contains(x))
                        })
                        .map(|(k, _)| *k)
                        .collect();
                if !hit.is_empty() && !hit.contains(&"m_famId") {
                    println!("REF {} {c} {:?} sym={}", e.id, hit, s.isa(e.tag, "Symbol"));
                }
            }
            if let Ok(ids) = std::env::var("IDS") {
                if ids.split(',').any(|x| x.parse::<i64>().ok() == Some(e.id)) {
                    let mut f: Vec<_> =
                        e.f.iter()
                            .filter(|(k, _)| !k.starts_with("m_pParam"))
                            .map(|(k, v)| format!("{k}={v:?}"))
                            .collect();
                    f.sort();
                    println!(
                        "ID {} {c} {}",
                        e.id,
                        f.join(" ").chars().take(1500).collect::<String>()
                    );
                    for (dn, dm) in &e.def {
                        println!(
                            "   def {dn}: {:?}",
                            dm.iter()
                                .map(|(k, v)| format!("{k}={v:?}"))
                                .collect::<Vec<_>>()
                                .join(" ")
                                .chars()
                                .take(300)
                                .collect::<String>()
                        );
                    }
                }
            }
            *h.entry(c.clone()).or_default() += 1;
            if show.iter().any(|x| *x == c) && h[&c] <= 60 {
                let mut f: Vec<_> =
                    e.f.iter()
                        .filter(|(k, _)| !k.starts_with("m_pParam"))
                        .map(|(k, v)| format!("{k}={v:?}"))
                        .collect();
                f.sort();
                println!(
                    "{c} {} {}",
                    e.id,
                    f.join(" ").chars().take(900).collect::<String>()
                );
                for (dn, dm) in &e.def {
                    println!(
                        "   def {dn}: {:?}",
                        dm.iter()
                            .map(|(k, v)| format!("{k}={v:?}"))
                            .collect::<Vec<_>>()
                            .join(" ")
                            .chars()
                            .take(300)
                            .collect::<String>()
                    );
                }
            }
        }
    }
    for (k, v) in h {
        println!("{v:6} {k}");
    }
}
