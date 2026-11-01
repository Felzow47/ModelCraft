//! Probe: the viewports showing given views, every field. Usage: rvtvp <file> <view id>…
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let rd = |cf: &mut cfb::CompoundFile<std::fs::File>, n: &str| craft_formats::rvt_model::read_stream(cf, n, 2 << 30, 1 << 30, |_| true).unwrap().data;
    let s = rd(&mut cf, "/Formats/Latest");
    let sc = Schema::parse(&s);
    let ids: Vec<i64> = a[1..].iter().filter_map(|x| x.parse().ok()).collect();
    let names: Vec<String> = cf.walk().filter(|e| e.is_stream()).map(|e| e.path().to_string_lossy().to_string()).filter(|n| n.starts_with("/Partitions")).collect();
    for n in names {
        let inf = rd(&mut cf, &n);
        let w = craft_formats::rvt_model::walk_chain(&inf).id_width;
        let w = if w == 0 { 8 } else { w };
        elements_dbg(&sc, &inf, w, &|t: u16| sc.name(t) == std::env::var("CLASS").unwrap_or("Viewport".into()), &mut |_, t, _, tr| {
            let v = tr.root.int("m_dbViewId").unwrap_or(0);
            if let Ok(f) = std::env::var("FAM") { if tr.root.int("m_famId").unwrap_or(0).to_string() != f { return; } }
            if !ids.contains(&v) && !ids.contains(&tr.id) && std::env::var("EVERY").is_err() { return; }
            println!("== {} {} failed {:?}", sc.name(t), tr.id, tr.failed.map(|x| sc.name(x).to_string()));
            println!("  {:?}", tr.root.fields.iter().filter(|(n, _)| !n.starts_with("m_pParam")).map(|(n, g)| format!("{n}={}", &format!("{g:?}").chars().take(200).collect::<String>())).collect::<Vec<_>>());
            for o in &tr.objs { println!("  {} {:?}", sc.name(o.tag), o.fields.iter().map(|(n, g)| format!("{n}={}", &format!("{g:?}").chars().take(200).collect::<String>())).collect::<Vec<_>>()); }
        }, &mut |_, _| {});
    }
}
