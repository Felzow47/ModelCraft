//! Probe (local only): the categories of the lines of all family documents of the project
//! (built-in ones by id, family subcategories as "sub"), with their visibility flags.
//! Usage: rvtfamcats <file>
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let rd = |cf: &mut cfb::CompoundFile<std::fs::File>, n: &str| craft_formats::rvt_model::read_stream(cf, n, 2 << 30, 1 << 30, |_| true).unwrap().data;
    let s = rd(&mut cf, "/Formats/Latest");
    let sc = Schema::parse(&s);
    let names: Vec<String> = cf.walk().filter(|e| e.is_stream()).map(|e| e.path().to_string_lossy().to_string()).filter(|n| n.starts_with("/Partitions")).collect();
    let mut c = std::collections::BTreeMap::<String, usize>::new();
    for n in names {
        let inf = rd(&mut cf, &n);
        let w = craft_formats::rvt_model::walk_chain(&inf).id_width;
        let w = if w == 0 { 8 } else { w };
        let want = |t: u16| sc.name(t) == "CurveElem";
        elements_dbg(&sc, &inf, w, &want, &mut |_, _, _, tr| {
            if tr.root.int("m_famId").unwrap_or(-1) <= 0 {
                return;
            }
            let cat = tr.root.int("m_categoryId").unwrap_or(-1);
            let fev = tr.root.obj("m_famElemVisibility").and_then(|o| o.int("m_flags")).unwrap_or(-1);
            let skel = tr.objs.iter().any(|o| sc.name(o.tag) == "SkeletonCurveData");
            let view = tr.root.int("m_ownerDBViewId").unwrap_or(-1) != -1;
            let k = format!("{} fev {:#x} skel {} view {}", if cat > 0 { "sub".to_string() } else { cat.to_string() }, fev, skel, view);
            *c.entry(k).or_default() += 1;
        }, &mut |_, _| {});
    }
    for (k, v) in c { println!("{v:7} {k}"); }
}
