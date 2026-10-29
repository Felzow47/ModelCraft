//! Probe (local only): the 2D content read for the sheets of a Revit file, written as SVG.
//! Usage: rvtsheet <file.rvt> <out dir> [sheet id]
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let t = std::time::Instant::now();
    let s = craft_formats::rvt::load(std::path::Path::new(&a[0]), &Default::default()).unwrap();
    eprintln!("loaded in {:.1}s, {} sheets", t.elapsed().as_secs_f64(), s.sheets.len());
    if std::env::var_os("FRAMES").is_some() {
        for f in &s.view_frames {
            let name = s.browser.iter().find(|r| r.id == f.id).map(|r| format!("{:?} {:?}", r.kind, r.label)).unwrap_or_default();
            eprintln!("frame {} {name}: dir {:?} up {:?} region {:?} clip {:?}", f.id, f.dir, f.up, f.region, f.clip.map(|c| (c.lo, c.hi)));
        }
    }
    let only: Option<i64> = a.get(2).and_then(|x| x.parse().ok());
    for sh in &s.sheets {
        if only.is_some_and(|o| o != sh.id) {
            continue;
        }
        let name = s.browser.iter().find(|r| r.id == sh.id).map(|r| r.label.clone()).unwrap_or_default();
        eprintln!("{} {:?}: {} lines, {} fine, {} texts, {} images, {} views, extent {:?}", sh.id, name, sh.lines.len(), sh.fine.len(), sh.texts.len(), sh.images.len(), sh.views.len(), sh.extent);
        if std::env::var_os("SHEET_TEXTS").is_some() {
            for t in &sh.texts {
                eprintln!("   {:?} h{:.4} a{}{} {:?}", t.pos, t.height, t.halign, t.valign, t.text);
            }
        }
        let e = sh.extent;
        let k = 1000.0; // mm
        let (w, h) = ((e[2] - e[0]) * k, (e[3] - e[1]) * k);
        let tx = |p: [f32; 2]| ((p[0] - e[0]) * k, (e[3] - p[1]) * k);
        let mut svg = format!("<svg xmlns='http://www.w3.org/2000/svg' width='{}' height='{}' viewBox='-10 -10 {} {}'><rect x='-10' y='-10' width='{}' height='{}' fill='white'/>", w * 2.0, h * 2.0, w + 20.0, h + 20.0, w + 20.0, h + 20.0);
        for l in &sh.lines {
            let pts: Vec<String> = l.iter().map(|p| { let (x, y) = tx(*p); format!("{x:.2},{y:.2}") }).collect();
            svg += &format!("<polyline points='{}' fill='none' stroke='black' stroke-width='0.35'/>", pts.join(" "));
        }
        for s in &sh.fine {
            let (a, b) = (tx(s[0]), tx(s[1]));
            svg += &format!("<line x1='{:.2}' y1='{:.2}' x2='{:.2}' y2='{:.2}' stroke='#444' stroke-width='0.12'/>", a.0, a.1, b.0, b.1);
        }
        for v in &sh.views {
            let (a, b) = (tx([v.rect[0], v.rect[3]]), tx([v.rect[2], v.rect[1]]));
            svg += &format!("<rect x='{:.2}' y='{:.2}' width='{:.2}' height='{:.2}' fill='none' stroke='#08f' stroke-width='0.2' stroke-dasharray='2 2'/>", a.0, a.1, b.0 - a.0, b.1 - a.1);
        }
        for i in &sh.images {
            let (a, b) = (tx([i.rect[0], i.rect[3]]), tx([i.rect[2], i.rect[1]]));
            svg += &format!("<rect x='{:.2}' y='{:.2}' width='{:.2}' height='{:.2}' fill='#cde' stroke='none'/><text x='{:.2}' y='{:.2}' font-size='5'>image {}x{}</text>", a.0, a.1, b.0 - a.0, b.1 - a.1, a.0 + 2.0, a.1 + 6.0, i.image.width, i.image.height);
        }
        for t in &sh.texts {
            let (x, y) = tx(t.pos);
            let anchor = ["start", "middle", "end"][t.halign as usize];
            let base = ["hanging", "middle", "auto"][t.valign as usize];
            let ang = -(t.dir[1].atan2(t.dir[0])).to_degrees();
            for (n, line) in t.text.lines().enumerate() {
                let esc = line.replace('&', "&amp;").replace('<', "&lt;");
                svg += &format!("<text x='{x:.2}' y='{:.2}' font-family='Arial' font-size='{:.2}' text-anchor='{anchor}' dominant-baseline='{base}' transform='rotate({ang:.1} {x:.2} {y:.2})'>{esc}</text>", y + n as f32 * t.height * k * 1.6, t.height * k * 1.4);
            }
        }
        svg += "</svg>";
        std::fs::write(format!("{}/sheet_{}.svg", a[1], sh.id), svg).unwrap();
    }
}
