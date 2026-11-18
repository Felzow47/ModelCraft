//! Probe (local only): the reference faces a family type keeps (`m_refFaces`, regenerated for
//! its sizes) next to the reference planes of its family document, to see which face is
//! which plane. Usage: rvtrefs <file> <type id> [<type id>…]
use craft_formats::rvt_browser::Schema;
use craft_formats::rvt_geom::*;
use std::collections::BTreeMap;

fn plane(o: &Obj) -> Option<([f64; 3], [f64; 3])> {
    let (p, x, y) = (o.fs("m_origin")?, o.fs("m_xVec")?, o.fs("m_yVec")?);
    let n = [x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]];
    Some(([p[0], p[1], p[2]], n))
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut cf = cfb::CompoundFile::open(std::fs::File::open(&a[0]).unwrap()).unwrap();
    let rd = |cf: &mut cfb::CompoundFile<std::fs::File>, n: &str| craft_formats::rvt_model::read_stream(cf, n, 2 << 30, 1 << 30, |_| true).unwrap().data;
    let s = rd(&mut cf, "/Formats/Latest");
    let sc = Schema::parse(&s);
    let syms: Vec<i64> = a[1..].iter().filter_map(|x| x.parse().ok()).collect();
    let names: Vec<String> = cf.walk().filter(|e| e.is_stream()).map(|e| e.path().to_string_lossy().to_string()).filter(|n| n.starts_with("/Partitions")).collect();
    // family document -> its reference planes by id: (name, kind, origin, normal)
    let mut planes: BTreeMap<i64, BTreeMap<i64, (String, i64, [f64; 3], [f64; 3])>> = BTreeMap::new();
    let mut faces: BTreeMap<i64, Vec<(i64, [f64; 3], [f64; 3])>> = BTreeMap::new();
    for n in names {
        let inf = rd(&mut cf, &n);
        let w = craft_formats::rvt_model::walk_chain(&inf).id_width;
        let w = if w == 0 { 8 } else { w };
        let want = |t: u16| matches!(sc.name(t), "RefPlane" | "FamilySymbol");
        elements_dbg(&sc, &inf, w, &want, &mut |_, t, _, tr| {
            if sc.name(t) == "RefPlane" {
                let fam = tr.root.int("m_famId").unwrap_or(-1);
                let name = match tr.root.get("m_text") {
                    Some(G::S(t)) => t.clone(),
                    _ => String::new(),
                };
                if let Some((o, n)) = tr.root.ptr("m_pSurface").and_then(|i| tr.objs.get(i)).and_then(plane) {
                    planes.entry(fam).or_default().insert(tr.id, (name, tr.root.int("m_refName").unwrap_or(-1), o, n));
                }
                return;
            }
            if !syms.contains(&tr.id) {
                return;
            }
            let mut f = Vec::new();
            for k in tr.root.ptrs("m_refFaces") {
                let Some(face) = tr.objs.get(k) else { continue };
                let tag = face.obj("m_GInfo").and_then(|g| g.int("m_tag")).unwrap_or(-1);
                if let Some((o, n)) = face.ptr("m_pSurf").and_then(|i| tr.objs.get(i)).and_then(plane) {
                    f.push((tag, o, n));
                }
            }
            faces.insert(tr.id, f);
        }, &mut |_, _| {});
    }
    let off = |o: [f64; 3], n: [f64; 3]| o[0] * n[0] + o[1] * n[1] + o[2] * n[2];
    for (sym, f) in &faces {
        println!("== type {sym}: {} faces", f.len());
        for (tag, o, n) in f {
            println!("  face {tag:3} n ({:6.3} {:6.3} {:6.3}) d {:9.4}", n[0], n[1], n[2], off(*o, *n));
        }
        // the family documents whose planes, in id order, are faces 9…
        if std::env::var("REFS_FAM").is_ok() {
            let fam: i64 = std::env::var("REFS_FAM").unwrap().parse().unwrap();
            let mut kinds = std::collections::BTreeMap::<i64, usize>::new();
            for (id, (name, kind, o, n)) in planes.get(&fam).into_iter().flatten() {
                *kinds.entry(*kind).or_default() += 1;
                println!("    doc plane {id} {name:?} kind {kind} n ({:6.3} {:6.3} {:6.3}) d {:9.4}", n[0], n[1], n[2], off(*o, *n));
            }
            println!("  kinds {kinds:?}");
        }
        for (fam, ps) in &planes {
            if ps.len() + 9 != f.len() {
                continue;
            }
            let same = ps.values().zip(f.iter().skip(9)).filter(|((_, _, _, n), (_, _, m))| (n[0] * m[0] + n[1] * m[1] + n[2] * m[2]).abs() > 0.999).count();
            println!("  family document {fam}: {} planes, {same} with the same normal as faces 9…", ps.len());
            if same + 2 >= ps.len() {
                for ((id, (name, kind, o, n)), (tag, fo, fnn)) in ps.iter().zip(f.iter().skip(9)) {
                    println!("    plane {id} {name:?} kind {kind} d {:9.4} -> face {tag} d {:9.4}", off(*o, *n), off(*fo, *fnn) * (n[0] * fnn[0] + n[1] * fnn[1] + n[2] * fnn[2]).signum());
                }
            }
        }
    }
}
