//! Probe: the elements holding an object of a given class (searched in every element of the
//! project document), with that object's fields. Usage: rvtfindobj <file> <class> [n]
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let s = craft_formats::rvt_model::read_stream(&mut cf, "/Formats/Latest", 64 << 20, 64 << 20, |_| true).unwrap();
    let sc = Schema::parse(&s.data);
    let names: Vec<String> = cf.walk().filter(|e| e.is_stream()).map(|e| e.path().to_string_lossy().to_string()).filter(|n| n.starts_with("/Partitions")).collect();
    let maxd: usize = a.get(2).and_then(|x| x.parse().ok()).unwrap_or(20);
    let mut found = 0;
    for n in names {
        let inf = craft_formats::rvt_model::read_stream(&mut cf, &n, 2 << 30, 1 << 30, |_| true).unwrap();
        let w = craft_formats::rvt_model::walk_chain(&inf.data).id_width;
        let w = if w == 0 { 8 } else { w };
        elements_dbg(&sc, &inf.data, w, &|_| true, &mut |_, t, _, tr| {
            if found >= maxd {
                return;
            }
            for o in tr.objs.iter().filter(|o| sc.name(o.tag).contains(a[1].as_str())) {
                found += 1;
                println!("== {} id {} owner view {:?}: {} {:?}", sc.name(t), tr.id, tr.root.int("m_ownerDBViewId"), sc.name(o.tag), o.fields.iter().map(|(n, g)| format!("{n}={g:?}")).collect::<Vec<_>>());
            }
        }, &mut |_, _| {});
    }
    println!("{found} found");
}
