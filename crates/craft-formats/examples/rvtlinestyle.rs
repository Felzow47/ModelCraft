//! Probe (local only): line styles of the project's view-owned curves (`CurveElem` →
//! `GStyleElem` → category name), split by filled-region boundary or not. Usage: rvtlinestyle <file>
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
    let mut gs: HashMap<i64, (i64, String)> = HashMap::new();
    let mut cats: HashMap<i64, (String, i64)> = HashMap::new();
    let mut curves: HashMap<(i64, bool), usize> = HashMap::new();
    for n in names {
        let inf =
            craft_formats::rvt_model::read_stream(&mut cf, &n, 2 << 30, 1 << 30, |_| true).unwrap();
        let w = craft_formats::rvt_model::walk_chain(&inf.data).id_width;
        let w = if w == 0 { 8 } else { w };
        let want = |t: u16| matches!(sc.name(t), "GStyleElem" | "CategoryElem" | "CurveElem");
        elements(&sc, &inf.data, w, &want, |_, t, _, tr| {
            if tr.root.int("m_famId").unwrap_or(-1) != -1 {
                return;
            }
            match sc.name(t) {
                "GStyleElem" => {
                    let g = tr.root.ptr("m_pGStyle").and_then(|i| tr.objs.get(i));
                    let d = g
                        .map(|g| {
                            format!(
                                "pen {:?} color {:#x} pat {:?}",
                                g.int("m_penNumber"),
                                g.int("m_color").unwrap_or(0),
                                g.int("m_linePatternId")
                            )
                        })
                        .unwrap_or_default();
                    gs.insert(tr.id, (tr.root.int("m_categoryId").unwrap_or(0), d));
                }
                "CategoryElem" => {
                    if let Some(c) = tr.root.ptr("m_pCategory").and_then(|i| tr.objs.get(i)) {
                        let nm = match c.get("m_name") {
                            Some(G::S(s)) => s.clone(),
                            _ => String::new(),
                        };
                        cats.insert(tr.id, (nm, c.int("m_parentCategoryId").unwrap_or(0)));
                    }
                }
                _ => {
                    if tr.root.int("m_ownerDBViewId").unwrap_or(-1) <= 0 {
                        return;
                    }
                    let fr = tr
                        .root
                        .obj("m_data")
                        .and_then(|d| d.ptr("m_oUserData"))
                        .and_then(|i| tr.objs.get(i))
                        .is_some_and(|o| sc.name(o.tag) == "FilledRegionCurveData");
                    *curves
                        .entry((tr.root.int("m_categoryId").unwrap_or(0), fr))
                        .or_default() += 1;
                }
            }
        });
    }
    let mut v: Vec<_> = curves.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1));
    for ((st, fr), c) in v {
        let (cat, d) = gs.get(&st).cloned().unwrap_or_default();
        println!(
            "style {st} region {fr}: {c} curves, cat {cat} {:?}, {d}",
            cats.get(&cat)
        );
    }
}
