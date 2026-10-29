//! Probe (local only): elements still shown as boxes (no rebuilt shape), by class and category.
//! Usage: rvtgap <file>
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
use std::collections::HashMap;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let t_start = std::time::Instant::now();
    let family = a[0].ends_with(".rfa");
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let names: Vec<(String, u64)> = cf
        .walk()
        .filter(|e| e.is_stream())
        .map(|e| {
            (
                e.path()
                    .to_string_lossy()
                    .trim_start_matches('/')
                    .to_string(),
                e.len(),
            )
        })
        .collect();
    let boxes = craft_formats::rvt::element_boxes(&mut cf, &names, family).unwrap();
    let s = craft_formats::rvt_model::read_stream(
        &mut cf,
        "/Formats/Latest",
        64 << 20,
        64 << 20,
        |_| true,
    )
    .unwrap();
    let sc = Schema::parse(&s.data);
    let mut sh = craft_formats::rvt_shape::Shapes::default();
    if let Some(cd) = craft_formats::rvt_model::read_stream(
        &mut cf,
        "/Global/ContentDocuments",
        2 << 30,
        1 << 30,
        |_| true,
    ) {
        sh.content_docs(&sc, &cd.data);
    }
    let want = craft_formats::rvt_shape::Shapes::wants(&sc);
    let mut cls: HashMap<i64, String> = HashMap::new();
    for (n, _) in names.iter().filter(|(n, _)| n.starts_with("Partitions/")) {
        let inf = craft_formats::rvt_model::read_stream(
            &mut cf,
            &format!("/{n}"),
            2 << 30,
            1 << 30,
            |_| true,
        )
        .unwrap();
        let w = craft_formats::rvt_model::walk_chain(&inf.data).id_width;
        let w = if w == 0 { 4 } else { w };
        elements(
            &sc,
            &inf.data,
            w,
            &|t| sc.isa(t, "Element"),
            |_, t, _, tr| {
                cls.insert(tr.id, sc.name(t).to_string());
                if want[t as usize] {
                    sh.take(&sc, t, &tr);
                }
            },
        );
    }
    let bm: HashMap<i64, ([f64; 3], [f64; 3])> = boxes
        .iter()
        .filter(|b| b.id != 0)
        .map(|b| (b.id, (b.min, b.max)))
        .collect();
    eprintln!("decoded in {:?}", t_start.elapsed());
    let meshes = sh.finish(&bm, family);
    let mut by: HashMap<(String, String), (usize, usize)> = HashMap::new();
    for b in &boxes {
        let c = craft_formats::rvt_model::category(b.category)
            .map(|c| c.0.to_string())
            .unwrap_or(format!("{}", b.category));
        let e = by
            .entry((cls.get(&b.id).cloned().unwrap_or("?".into()), c))
            .or_default();
        e.0 += 1;
        if meshes.contains_key(&b.id) {
            e.1 += 1;
        }
    }
    let mut v: Vec<_> = by.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1 .0 - x.1 .1));
    let (t, d): (usize, usize) = v.iter().fold((0, 0), |a, x| (a.0 + x.1 .0, a.1 + x.1 .1));
    println!("total {t} shaped {d}");
    for ((c, k), (n, m)) in v.iter().take(40) {
        println!("{:6} box-only / {:6}  {c} [{k}]", n - m, n);
    }
}
