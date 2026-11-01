//! Probe: the worksets each view hides (`WorksetVisibilityViewSettings`), by name, and the
//! globally hidden ones. Usage: rvtws <file> [view name part]
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let rd = |cf: &mut cfb::CompoundFile<std::fs::File>, n: &str| craft_formats::rvt_model::read_stream(cf, n, 1 << 30, 1 << 30, |_| true).unwrap().data;
    let p = rd(&mut cf, "/Global/PartitionTable");
    let ws = craft_formats::rvt_model::worksets(&p);
    let el = craft_formats::rvt_model::elem_worksets(&rd(&mut cf, "/Global/ElemTable"), &ws);
    println!("{} worksets, {} elements", ws.len(), el.len());
    let s = rd(&mut cf, "/Formats/Latest");
    let sc = Schema::parse(&s);
    let guid = |o: &Obj| -> Option<[u8; 16]> {
        let mut o = o;
        loop {
            match o.fields.first().map(|f| &f.1) {
                Some(G::O(x)) => o = x,
                Some(G::Raw(r)) if r.len() == 16 => return r.as_slice().try_into().ok(),
                _ => return None,
            }
        }
    };
    let names: Vec<String> = cf.walk().filter(|e| e.is_stream()).map(|e| e.path().to_string_lossy().to_string()).filter(|n| n.starts_with("/Partitions")).collect();
    let filt = a.get(1).cloned().unwrap_or_default();
    for n in names {
        let inf = rd(&mut cf, &n);
        let w = craft_formats::rvt_model::walk_chain(&inf).id_width;
        let w = if w == 0 { 8 } else { w };
        elements_dbg(&sc, &inf, w, &|t: u16| sc.isa(t, "DBView") || sc.name(t) == "WorksetVisibilitySettingsElem" || sc.isa(t, "Viewer"), &mut |_, t, _, tr| {
            if sc.name(t) == "WorksetVisibilitySettingsElem" {
                if let Some(G::L(l)) = tr.root.get("m_invisibleWorksets") {
                    println!("GLOBAL invisible: {:?}", l.iter().filter_map(|o| guid(o)).map(|g| ws.get(&g)).collect::<Vec<_>>());
                }
                return;
            }
            if sc.isa(t, "Viewer") {
                if std::env::var("VIEWER").is_ok_and(|v| v.split(',').any(|v| v == tr.id.to_string())) {
                    println!("VIEWER {} {:?}", tr.id, tr.root.fields.iter().filter(|(n, _)| !n.starts_with("m_pParam")).map(|(n, g)| format!("{n}={}", &format!("{g:?}").chars().take(300).collect::<String>())).collect::<Vec<_>>());
                    for o in &tr.objs { println!("  {} {:?}", sc.name(o.tag), o.fields.iter().map(|(n, g)| format!("{n}={}", &format!("{g:?}").chars().take(300).collect::<String>())).collect::<Vec<_>>()); }
                }
                return;
            }
            if !filt.is_empty() && tr.root.get("m_viewName").is_some_and(|n| matches!(n, G::S(s) if s.contains(&filt))) {
                println!("VIEW {:?} viewer {:?} dir {:?}", tr.root.get("m_viewName"), tr.root.int("m_viewerId"), tr.root.fs("m_viewDir"));
            }
            let name = match tr.root.get("m_viewName") { Some(G::S(s)) => s.clone(), _ => String::new() };
            if !name.contains(&filt) { return; }
            if std::env::var("DUMP").is_ok_and(|d| d == name) {
                println!("ROOT {:?}", tr.root.fields.iter().map(|(n, g)| format!("{n}={}", &format!("{g:?}").chars().take(200).collect::<String>())).collect::<Vec<_>>());
                for o in &tr.objs { println!("  {} {:?}", sc.name(o.tag), o.fields.iter().map(|(n, g)| format!("{n}={}", &format!("{g:?}").chars().take(160).collect::<String>())).collect::<Vec<_>>()); }
            }
            for o in tr.objs.iter().filter(|o| sc.name(o.tag) == "WorksetVisibilityViewSettings") {
                if let Some(G::L(l)) = o.get("m_worksetVisibilityPGUIDMap") {
                    let v: Vec<_> = l.iter().map(|e| (match e.get("first") { Some(G::O(x)) => guid(x).and_then(|g| ws.get(&g).copied()), _ => None }, e.get("second").cloned())).collect();
                    println!("{name} ({}): {v:?}", sc.name(t));
                }
            }
        }, &mut |_, _| {});
    }
}
