//! Probe (local only): instance → symbol → family chain, and the family documents' forms.
//! Usage: rvtlink <file> [category]
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
use std::collections::HashMap;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let cat: i64 = a.get(1).and_then(|x| x.parse().ok()).unwrap_or(-2000023);
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
    let mut els: HashMap<i64, (u16, Tree)> = HashMap::new();
    for n in names {
        let inf =
            craft_formats::rvt_model::read_stream(&mut cf, &n, 2 << 30, 1 << 30, |_| true).unwrap();
        let w = craft_formats::rvt_model::walk_chain(&inf.data).id_width;
        let w = if w == 0 { 8 } else { w };
        let want = |t: u16| {
            let n = sc.name(t);
            n == "FamilyInstance"
                || n == "FamilySymbol"
                || n == "Family"
                || n.ends_with("Elem")
                || n == "FamilySurrogate"
                || n == "FamSymSurrogate"
                || n.contains("Partition")
        };
        elements(&sc, &inf.data, w, &want, |_, t, _, tr| {
            els.insert(tr.id, (t, tr));
        });
    }
    let show = |id: i64| {
        if let Some((t, tr)) = els.get(&id) {
            let f: Vec<String> = tr
                .root
                .fields
                .iter()
                .filter(|(_, g)| !matches!(g, G::P(None)))
                .map(|(n, g)| format!("{n}={}", short(g)))
                .collect();
            println!(
                "  [{}] {id}: {}",
                sc.name(*t),
                f.join(" ").chars().take(2500).collect::<String>()
            );
        } else {
            println!("  {id}: missing");
        }
    };
    let mut n = 0;
    for (id, (t, tr)) in &els {
        if sc.name(*t) != "Family" || tr.root.int("m_categoryId") != Some(cat) {
            continue;
        }
        n += 1;
        if n > 4 {
            break;
        }
        println!("== family {id}");
        show(*id);
        let mut kinds: HashMap<&str, usize> = HashMap::new();
        for (_, (t2, tr2)) in &els {
            if tr2.root.int("m_famId") == Some(*id) {
                *kinds.entry(sc.name(*t2)).or_default() += 1;
            }
        }
        println!("  members {kinds:?}");
        if let Some(o) = tr.root.ptr("m_oFamDoc").and_then(|i| tr.objs.get(i)) {
            println!(
                "  FAMDOC [{}] {}",
                sc.name(o.tag),
                o.fields
                    .iter()
                    .map(|(n, g)| format!("{n}={}", short(g)))
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(3000)
                    .collect::<String>()
            );
            for (k, oo) in tr.objs.iter().enumerate().take(40) {
                println!(
                    "   #{k} [{}] {}",
                    sc.name(oo.tag),
                    oo.fields
                        .iter()
                        .map(|(n, g)| format!("{n}={}", short(g)))
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(400)
                        .collect::<String>()
                );
            }
        }
    }
    for (id, (t, tr)) in &els {
        let nm = sc.name(*t);
        if nm.contains("Surrogate") || nm.contains("Partition") {
            n += 1;
            if n < 14 {
                show(*id);
            }
            let _ = tr;
        }
    }
}
fn short(g: &G) -> String {
    match g {
        G::I(v) => v.to_string(),
        G::F(v) => format!("{v:.4}"),
        G::S(s) => format!("{s:?}"),
        G::Fs(v) => format!("F{:?}", &v[..v.len().min(9)]),
        G::Is(v) => format!("I{:?}", &v[..v.len().min(12)]),
        G::Ids(v) => format!("Ids{:?}", &v[..v.len().min(8)]),
        G::O(o) => format!(
            "{{{}}}",
            o.fields
                .iter()
                .map(|(n, g)| format!("{n}={}", short(g)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        G::L(l) => format!("L[{}]", l.len()),
        G::P(p) => format!("P{p:?}"),
        G::Ps(v) => format!("Ps({})", v.len()),
        G::R(r) => format!("R{r}"),
        G::Raw(b) => {
            if b.len() == 16 {
                b.iter().map(|x| format!("{x:02x}")).collect()
            } else {
                format!("raw{}", b.len())
            }
        }
    }
}
