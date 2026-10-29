//! Probe (local only): instance → symbol → family → family document (content doc) for a category.
//! Usage: rvtinst <file> <category> [n]
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
use std::collections::HashMap;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let cat: i64 = a[1].parse().unwrap();
    let nmax: usize = a.get(2).and_then(|x| x.parse().ok()).unwrap_or(3);
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
    let mut members: HashMap<i64, HashMap<String, usize>> = HashMap::new();
    for n in names {
        let inf =
            craft_formats::rvt_model::read_stream(&mut cf, &n, 2 << 30, 1 << 30, |_| true).unwrap();
        let w = craft_formats::rvt_model::walk_chain(&inf.data).id_width;
        let w = if w == 0 { 8 } else { w };
        let want = |t: u16| sc.isa(t, "Element");
        elements(&sc, &inf.data, w, &want, |_, t, _, tr| {
            let nm = sc.name(t);
            if let Some(f) = tr.root.int("m_famId") {
                if f > 0 {
                    *members
                        .entry(f)
                        .or_default()
                        .entry(nm.to_string())
                        .or_default() += 1;
                }
            }
            if nm == "FamilyInstance" || nm == "FamilySymbol" || nm == "Family" {
                els.insert(tr.id, (t, tr));
            }
        });
    }
    let key = |tr: &Tree, ks: &[&str]| {
        ks.iter()
            .filter_map(|k| tr.root.get(k).map(|g| format!("{k}={g:?}")))
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(600)
            .collect::<String>()
    };
    let mut n = 0;
    let mut ids: Vec<_> = els.keys().copied().collect();
    ids.sort();
    for id in ids {
        let (t, tr) = &els[&id];
        if sc.name(*t) != "FamilyInstance" || tr.root.int("m_famId").unwrap_or(-1) > 0 {
            continue;
        }
        let Some(ii) = tr.root.ptr("m_pInstanceInfo").and_then(|i| tr.objs.get(i)) else {
            continue;
        };
        let sym = ii.int("m_symbolId").unwrap_or(-1);
        let Some((_, st)) = els.get(&sym) else {
            continue;
        };
        let fam = st.root.int("m_familyId").unwrap_or(-1);
        let Some((_, ft)) = els.get(&fam) else {
            continue;
        };
        if ft.root.int("m_categoryId") != Some(cat) {
            continue;
        }
        n += 1;
        if n > nmax {
            break;
        }
        println!(
            "== inst {id} famId={:?} trf={:?}",
            tr.root.int("m_famId"),
            ii.obj("m_Trf").map(|o| format!("{:?}", o.fields))
        );
        println!(
            "  sym {sym}: {}",
            key(
                st,
                &["m_famId", "m_familyId", "m_partitionSurrogateId", "m_name"]
            )
        );
        println!("  sym members {:?}", members.get(&sym));
        let doc = ft.root.ptr("m_oFamDoc").and_then(|i| ft.objs.get(i));
        println!(
            "  fam {fam}: {} doc {:?}",
            key(ft, &["m_name", "m_famId", "m_ownerSymbolId", "m_inPlace"]),
            doc.map(|d| format!("{:?}", d.get("m_contentDocGUID")))
        );
        println!("  fam members {:?}", members.get(&fam));
        // other symbol fields
        println!(
            "  symfields {:?}",
            st.root
                .fields
                .iter()
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>()
        );
    }
}
