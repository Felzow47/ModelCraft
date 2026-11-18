//! Probe (local only): the ducts of one size (width and height in mm) and what their
//! connectors join. Usage: rvtducts <file> <width mm> <height mm>
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let rd = |cf: &mut cfb::CompoundFile<std::fs::File>, n: &str| craft_formats::rvt_model::read_stream(cf, n, 2 << 30, 1 << 30, |_| true).unwrap().data;
    let s = rd(&mut cf, "/Formats/Latest");
    let sc = Schema::parse(&s);
    let (w, h): (f64, f64) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    let names: Vec<String> = cf.walk().filter(|e| e.is_stream()).map(|e| e.path().to_string_lossy().to_string()).filter(|n| n.starts_with("/Partitions")).collect();
    for n in names {
        let inf = rd(&mut cf, &n);
        let wd = craft_formats::rvt_model::walk_chain(&inf).id_width;
        let wd = if wd == 0 { 8 } else { wd };
        elements_dbg(&sc, &inf, wd, &|t: u16| sc.isa(t, "RbsCurve"), &mut |_, t, _, tr| {
            let mm = |k: &str| tr.root.float(k).map(|v| (v * 304.8).round());
            if mm("m_dWidthOrDiameter") != Some(w) || mm("m_dHeight").is_some_and(|v| v != h) {
                return;
            }
            let links: Vec<i64> = tr.objs.iter().filter(|o| sc.name(o.tag) == "Connector").flat_map(|o| o.list("m_arrRefs").iter()).filter_map(|r| r.int("m_id")).filter(|v| *v > 0 && *v != tr.id).collect();
            let line = tr.root.ptr("m_pCurveDriver").and_then(|i| tr.objs.get(i)).and_then(|d| d.ptr("m_pCrv")).and_then(|i| tr.objs.get(i)).map(|c| (c.fs("m_origin").map(|v| v.to_vec()), c.fs("m_dirVec").map(|v| v.to_vec()), c.fs("m_endParams").map(|v| v.to_vec())));
            println!("{} {} links {:?} line {:?}", tr.id, sc.name(t), links, line);
        }, &mut |_, _| {});
    }
}
