//! Probe: classes of the elements of a family document in the project. Usage: rvtfamel <file> <family id>…
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let rd = |cf: &mut cfb::CompoundFile<std::fs::File>, n: &str| craft_formats::rvt_model::read_stream(cf, n, 2 << 30, 1 << 30, |_| true).unwrap().data;
    let s = rd(&mut cf, "/Formats/Latest");
    let sc = Schema::parse(&s);
    let fams: Vec<i64> = a[1..].iter().filter_map(|x| x.parse().ok()).collect();
    let names: Vec<String> = cf.walk().filter(|e| e.is_stream()).map(|e| e.path().to_string_lossy().to_string()).filter(|n| n.starts_with("/Partitions")).collect();
    let mut c = std::collections::BTreeMap::<String, usize>::new();
    for n in names {
        let inf = rd(&mut cf, &n);
        let w = craft_formats::rvt_model::walk_chain(&inf).id_width;
        let w = if w == 0 { 8 } else { w };
        let want = |t: u16| !sc.isa(t, "DBView") && !sc.isa(t, "FamilyInstance") && !sc.name(t).contains("Symbol") && sc.isa(t, "Element");
        elements_dbg(&sc, &inf, w, &want, &mut |_, t, _, tr| {
            let f = tr.root.int("m_famId").unwrap_or(-1);
            if f > 0 { *c.entry(format!("{} {}", sc.name(t), if fams.contains(&f) { "IN" } else { "other" })).or_default() += 1; }
        }, &mut |_, _| {});
    }
    for (k, v) in c { println!("{v:6} {k}"); }
}
