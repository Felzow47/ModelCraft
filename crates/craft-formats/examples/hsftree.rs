use craft_formats::hsf;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let b = std::fs::read(&a[1]).unwrap();
    let ents = craft_formats::easm::entries(&b);
    let (_, em) = ents.iter().find(|(n, _)| n == "eModel").unwrap();
    let m = hsf::parse_file(em).unwrap();
    println!("err {:?} segs {}", m.error, m.segments.len());
    fn rec(m: &hsf::Model, i: usize, d: usize) {
        let s = &m.segments[i];
        let name = s.path.rsplit('/').next().unwrap();
        let tris: usize = s.shells.iter().filter_map(|&k| m.shells[k].as_ref()).map(|x| x.tris.len()).sum();
        println!("{}{} vis={:?} col={:?} mat={} sh={} tris={} inc={:?} txt={:?}", "  ".repeat(d), name, s.face_visible, s.face_color.map(|c| c.map(|x| (x*100.).round()/100.)), s.matrix.map(|m| format!("{:?}", m.map(|x| (x*1000.).round()/1000.))).unwrap_or("-".into()), s.shells.len(), tris, s.includes, s.texts);
        if d < 40 { for &c in &s.children { rec(m, c, d + 1); } }
    }
    rec(&m, 0, 0);
}
