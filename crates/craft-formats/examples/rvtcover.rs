//! Probe (local only): which family instances have family form elements available.
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
use std::collections::HashMap;
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
    let mut els: HashMap<i64, (u16, Tree)> = HashMap::new();
    for n in names {
        let inf =
            craft_formats::rvt_model::read_stream(&mut cf, &n, 2 << 30, 1 << 30, |_| true).unwrap();
        let w = craft_formats::rvt_model::walk_chain(&inf.data).id_width;
        let w = if w == 0 { 8 } else { w };
        let want = |t: u16| sc.isa(t, "Element");
        elements(&sc, &inf.data, w, &want, |_, t, _, tr| {
            els.insert(tr.id, (t, tr));
        });
    }
    let mut fam_forms: HashMap<i64, Vec<String>> = HashMap::new();
    let mut kinds: HashMap<String, usize> = HashMap::new();
    for (_, (t, tr)) in &els {
        *kinds.entry(sc.name(*t).to_string()).or_default() += 1;
        if let Some(f) = tr.root.int("m_famId") {
            if f > 0 {
                fam_forms
                    .entry(f)
                    .or_default()
                    .push(sc.name(*t).to_string());
            }
        }
    }
    let mut by_cls: HashMap<String, (usize, usize)> = HashMap::new();
    for (_, (t, tr)) in &els {
        if sc.name(*t) != "FamilyInstance" {
            continue;
        }
        let Some(ii) = tr.root.ptr("m_pInstanceInfo").and_then(|i| tr.objs.get(i)) else {
            continue;
        };
        let sym = ii.int("m_symbolId").unwrap_or(-1);
        let fam = els
            .get(&sym)
            .and_then(|(_, s)| s.root.int("m_familyId").or(s.root.int("m_famId")))
            .unwrap_or(-1);
        let famname = els
            .get(&fam)
            .map(|(t, _)| sc.name(*t).to_string())
            .unwrap_or("?".into());
        let forms = fam_forms
            .get(&fam)
            .map(|v| {
                v.iter()
                    .filter(|n| n.contains("Elem") || n.contains("Form"))
                    .count()
            })
            .unwrap_or(0);
        let e = by_cls.entry(format!("fam {fam} ({famname})")).or_default();
        e.0 += 1;
        e.1 = forms;
    }
    if std::env::var_os("FORMS").is_some() {
        let mut h: HashMap<String, usize> = HashMap::new();
        for (_, (t, tr)) in &els {
            let n = sc.name(*t);
            if !(n.ends_with("Elem")
                && (n.contains("Extrusion")
                    || n.contains("Sweep")
                    || n.contains("Blend")
                    || n.contains("Revolution")))
            {
                continue;
            }
            let f = tr.root.int("m_famId").unwrap_or(-1);
            let fc = els
                .get(&f)
                .map(|(t, _)| sc.name(*t).to_string())
                .unwrap_or("?".into());
            *h.entry(format!("{n} fam {f} ({fc})")).or_default() += 1;
        }
        let mut v: Vec<_> = h.into_iter().collect();
        v.sort();
        for x in v {
            println!("{x:?}");
        }
        return;
    }
    let mut v: Vec<_> = by_cls.into_iter().collect();
    v.sort();
    for (k, (n, f)) in v {
        println!(
            "{k}: {n} instances, {f} family elems {:?}",
            fam_forms
                .get(&k.split(' ').nth(1).unwrap().parse::<i64>().unwrap())
                .map(|v| {
                    let mut m: HashMap<&str, usize> = HashMap::new();
                    for x in v {
                        *m.entry(x).or_default() += 1;
                    }
                    m
                })
        );
    }
}
// (appended) histogram of form elements' m_famId owners
#[allow(dead_code)]
fn _unused() {}
