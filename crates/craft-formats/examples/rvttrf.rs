//! Probe (local only): the stored placement of every family instance of the project: id,
//! `m_3x3`, `m_or`, `m_flippedX`, `m_flippedY`. Usage: rvttrf <file>
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let rd = |cf: &mut cfb::CompoundFile<std::fs::File>, n: &str| craft_formats::rvt_model::read_stream(cf, n, 2 << 30, 1 << 30, |_| true).unwrap().data;
    let s = rd(&mut cf, "/Formats/Latest");
    let sc = Schema::parse(&s);
    let names: Vec<String> = cf.walk().filter(|e| e.is_stream()).map(|e| e.path().to_string_lossy().to_string()).filter(|n| n.starts_with("/Partitions")).collect();
    for n in names {
        let inf = rd(&mut cf, &n);
        let w = craft_formats::rvt_model::walk_chain(&inf).id_width;
        let w = if w == 0 { 8 } else { w };
        elements_dbg(&sc, &inf, w, &|t: u16| sc.name(t) == "FamilyInstance", &mut |_, _, _, tr| {
            if tr.root.int("m_famId").unwrap_or(-1) != -1 {
                return;
            }
            let Some(ii) = tr.root.ptr("m_pInstanceInfo").and_then(|i| tr.objs.get(i)) else { return };
            let Some(t) = ii.obj("m_Trf") else { return };
            let (Some(m), Some(o)) = (t.fs("m_3x3"), t.fs("m_or")) else { return };
            println!(
                "{} m {} o {} fx {} fy {}",
                tr.id,
                m.iter().map(|v| format!("{v:.6}")).collect::<Vec<_>>().join(" "),
                o.iter().map(|v| format!("{v:.4}")).collect::<Vec<_>>().join(" "),
                tr.root.int("m_flippedX").unwrap_or(0),
                tr.root.int("m_flippedY").unwrap_or(0)
            );
        }, &mut |_, _| {});
    }
}
