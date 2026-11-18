//! Probe (local only): the elements of one family document of the project, with the parts
//! of each that decide whether it is drawn (sketch, skeleton, category, visibility flags).
//! Usage: rvtfamcurves <file> <family document root>
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let rd = |cf: &mut cfb::CompoundFile<std::fs::File>, n: &str| craft_formats::rvt_model::read_stream(cf, n, 2 << 30, 1 << 30, |_| true).unwrap().data;
    let s = rd(&mut cf, "/Formats/Latest");
    let sc = Schema::parse(&s);
    let fam: i64 = a[1].parse().unwrap();
    let names: Vec<String> = cf.walk().filter(|e| e.is_stream()).map(|e| e.path().to_string_lossy().to_string()).filter(|n| n.starts_with("/Partitions")).collect();
    for n in names {
        let inf = rd(&mut cf, &n);
        let w = craft_formats::rvt_model::walk_chain(&inf).id_width;
        let w = if w == 0 { 8 } else { w };
        let want = |t: u16| sc.isa(t, "Element");
        elements_dbg(&sc, &inf, w, &want, &mut |_, t, _, tr| {
            if tr.root.int("m_famId").unwrap_or(-1) != fam {
                return;
            }
            let parts: Vec<&str> = tr.objs.iter().map(|o| sc.name(o.tag)).collect();
            let fev = tr.root.obj("m_famElemVisibility").and_then(|o| o.int("m_flags"));
            let sk = tr.objs.iter().find(|o| sc.name(o.tag) == "SketchMembership").map(|o| format!("{:?}", o));
            if sc.name(t) == "CurveElem" {
                println!("  sketch {}", sk.unwrap_or_default().chars().take(400).collect::<String>());
            }
            println!(
                "{} {} cat {:?} view {:?} sym {:?} fev {:?} parts {:?}",
                tr.id,
                sc.name(t),
                tr.root.int("m_categoryId"),
                tr.root.int("m_ownerDBViewId"),
                tr.root.int("m_symbolId"),
                fev.map(|f| format!("{f:#x}")),
                parts
            );
        }, &mut |_, _| {});
    }
}
