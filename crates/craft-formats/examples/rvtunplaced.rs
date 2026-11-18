//! Probe (local only): the elements whose `m_unplacedOwnerId` is set (members of a group that
//! is not placed), by class, in the project and in its family documents. Usage: rvtunplaced <file>
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
    let mut owners = std::collections::BTreeMap::<i64, usize>::new();
    for n in names {
        let inf = rd(&mut cf, &n);
        let w = craft_formats::rvt_model::walk_chain(&inf).id_width;
        let w = if w == 0 { 8 } else { w };
        elements_dbg(&sc, &inf, w, &|t: u16| sc.isa(t, "Element"), &mut |_, t, _, tr| {
            let u = tr.root.int("m_unplacedOwnerId").unwrap_or(-1);
            if u == -1 {
                return;
            }
            let f = tr.root.int("m_famId").unwrap_or(-1);
            *c.entry(format!("{} {}", sc.name(t), if f > 0 { "family" } else { "project" })).or_default() += 1;
            *owners.entry(u).or_default() += 1;
            if f <= 0 && matches!(sc.name(t), "DBDrawing" | "Viewer" | "Viewport" | "DBViewAnonDrafting" | "ExtentElem") {
                let name = tr.objs.iter().find_map(|o| match o.get("m_name").or_else(|| o.get("m_viewName")) {
                    Some(G::S(x)) => Some(x.clone()),
                    _ => None,
                });
                println!("  {} {} owner {u} view {:?} name {name:?}", sc.name(t), tr.id, tr.root.int("m_ownerDBViewId"));
            }
        }, &mut |_, _| {});
    }
    for (k, v) in c {
        println!("{v:6} {k}");
    }
    println!("{} owners, e.g. {:?}", owners.len(), owners.iter().take(8).collect::<Vec<_>>());
}
