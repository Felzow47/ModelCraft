//! Probe (local only): plan view ranges and 3D section boxes, per view.
//! Usage: rvtrange <file>
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
        let w = if w == 0 { 8 } else { w };
        let want = |t: u16| sc.isa(t, "DBView") || sc.name(t) == "Level";
        elements_dbg(
            &sc,
            &inf.data,
            w,
            &want,
            &mut |_, t, _, tr| {
                let r = &tr.root;
                let f = |k: &str| match r.get(k) {
                    Some(G::F(v)) => format!("{v:.3}"),
                    Some(G::I(v)) => v.to_string(),
                    _ => "-".into(),
                };
                if sc.name(t) == "Level" {
                    let z = r
                        .ptr("m_pSurface")
                        .and_then(|i| tr.objs.get(i))
                        .and_then(|p| p.fs("m_origin").map(|v| v[2]));
                    println!(
                        "LEVEL {} z={:?} {:?}",
                        tr.id,
                        z,
                        r.get("m_name")
                            .or(r.get("m_levelName"))
                            .map(|g| format!("{g:?}"))
                    );
                    return;
                }
                let pvr = tr.objs.iter().find(|o| sc.name(o.tag) == "PlanViewRange");
                let pvr2 = tr.objs.iter().find(|o| sc.name(o.tag) == "PlanViewRange2");
                println!(
                    "{} {} {:?} cut={} top={} bot={} below={} lvl={} gen={} pvr={:?} pvr2={:?}",
                    sc.name(t),
                    tr.id,
                    r.get("m_viewName")
                        .map(|g| format!("{g:?}"))
                        .unwrap_or_default(),
                    f("m_cutPlaneElev"),
                    f("m_topClipElev"),
                    f("m_bottomClipElev"),
                    f("m_levelBelowElev"),
                    f("m_levelId"),
                    f("m_genElemId"),
                    pvr.map(|o| format!("{:?} {:?}", o.fs("m_offsets"), o.get("m_levelPos"))),
                    pvr2.map(|o| o
                        .fields
                        .iter()
                        .map(|(k, g)| format!("{k}={g:?}"))
                        .collect::<Vec<_>>()
                        .join(" "))
                );
            },
            &mut |_, _| {},
        );
    }
}
