//! Probe (local only): the elements a view owns (its detail lines, regions, texts…), by class,
//! with the line style of the lines and the type of the regions. Usage: rvtviewelems <file> <view id>
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let rd = |cf: &mut cfb::CompoundFile<std::fs::File>, n: &str| craft_formats::rvt_model::read_stream(cf, n, 2 << 30, 1 << 30, |_| true).unwrap().data;
    let s = rd(&mut cf, "/Formats/Latest");
    let sc = Schema::parse(&s);
    let view: i64 = a[1].parse().unwrap();
    let names: Vec<String> = cf.walk().filter(|e| e.is_stream()).map(|e| e.path().to_string_lossy().to_string()).filter(|n| n.starts_with("/Partitions")).collect();
    let mut c = std::collections::BTreeMap::<String, usize>::new();
    for n in names {
        let inf = rd(&mut cf, &n);
        let w = craft_formats::rvt_model::walk_chain(&inf).id_width;
        let w = if w == 0 { 8 } else { w };
        elements_dbg(&sc, &inf, w, &|t: u16| sc.isa(t, "Element"), &mut |_, t, _, tr| {
            if tr.root.int("m_ownerDBViewId").unwrap_or(-1) != view {
                return;
            }
            let sketch = tr.objs.iter().find(|o| sc.name(o.tag) == "SketchMembership").and_then(|o| o.int("m_groupId"));
            let k = match sc.name(t) {
                "CurveElem" => format!("CurveElem style {:?} sketch {:?}", tr.root.int("m_categoryId"), sketch),
                "FilledRegion" => format!("FilledRegion {} type {:?} sketch {:?}", tr.id, tr.root.int("m_typeId").or_else(|| tr.root.int("m_symbolId")), tr.root.int("m_sketchId")),
                x => x.to_string(),
            };
            *c.entry(k).or_default() += 1;
        }, &mut |_, _| {});
    }
    for (k, v) in c {
        println!("{v:6} {k}");
    }
}
