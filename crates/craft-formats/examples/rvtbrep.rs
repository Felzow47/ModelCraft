//! Probe (local only): element classes that carry a stored boundary representation
//! (`Geometry` with faces and edges), and the surface classes of their faces.
//! Usage: rvtbrep <file>
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
use std::collections::HashMap;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let s = craft_formats::rvt_model::read_stream(&mut cf, "/Formats/Latest", 64 << 20, 64 << 20, |_| true).unwrap();
    let sc = Schema::parse(&s.data);
    let names: Vec<String> = cf.walk().filter(|e| e.is_stream()).map(|e| e.path().to_string_lossy().to_string()).filter(|n| n.starts_with("/Partitions")).collect();
    let mut by: HashMap<String, (usize, usize, usize)> = HashMap::new();
    let mut surf: HashMap<String, usize> = HashMap::new();
    let mut holders: HashMap<String, usize> = HashMap::new();
    for n in names {
        let inf = craft_formats::rvt_model::read_stream(&mut cf, &n, 2 << 30, 1 << 30, |_| true).unwrap();
        let w = craft_formats::rvt_model::walk_chain(&inf.data).id_width;
        let w = if w == 0 { 8 } else { w };
        elements(&sc, &inf.data, w, &|t| sc.isa(t, "Element"), |_, t, _, tr| {
            let geos: Vec<&Obj> = tr.objs.iter().filter(|o| o.get("m_pFaces").is_some() && o.get("m_pEdges").is_some()).collect();
            if geos.is_empty() { return; }
            let e = by.entry(sc.name(t).to_string()).or_default();
            e.0 += 1;
            e.1 += geos.len();
            for g in &geos {
                *holders.entry(sc.name(g.tag).to_string()).or_default() += 1;
                for f in g.ptrs("m_pFaces") {
                    e.2 += 1;
                    if let Some(sf) = tr.objs.get(f).and_then(|f| f.ptr("m_pSurf")).and_then(|i| tr.objs.get(i)) {
                        *surf.entry(sc.name(sf.tag).to_string()).or_default() += 1;
                    }
                }
            }
            if let Ok(p) = std::env::var("BREP_SHOW") {
                if sc.name(t).contains(p.as_str()) {
                    for o in tr.objs.iter().filter(|o| { let n = sc.name(o.tag); !matches!(n, "Plane" | "Face" | "Edge" | "EdgeLoop" | "GLine") }).take(40) {
                        println!("  {} {} {:?}", tr.id, sc.name(o.tag), o.fields.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>());
                    }
                }
            }
        });
    }
    let mut v: Vec<_> = by.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1 .0));
    for (n, (c, g, f)) in v { println!("{n}: {c} elems, {g} geometries, {f} faces"); }
    println!("--- holders {holders:?}");
    let mut v: Vec<_> = surf.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1));
    println!("--- surfaces {v:?}");
}
