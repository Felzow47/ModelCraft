//! Probe (local only): what is read for the browser rows whose label holds one of the given
//! texts: a 2D view's drawing (lines, strokes, texts, fills, images, placed views), a model
//! view's frame (direction, region, clip, hidden elements).
//! Usage: rvtcheck <file.rvt> <text> [text...]
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let t = std::time::Instant::now();
    let s = craft_formats::rvt::load(std::path::Path::new(&a[0]), &Default::default()).unwrap();
    eprintln!("loaded in {:.1}s, {} sheets, {} frames", t.elapsed().as_secs_f64(), s.sheets.len(), s.view_frames.len());
    let pats: Vec<String> = a[1..].iter().map(|p| p.to_lowercase()).collect();
    for r in s.browser.iter().filter(|r| pats.iter().any(|p| r.label.to_lowercase().contains(p.as_str()))) {
        let sheet = s.sheets.iter().find(|sh| sh.id == r.id);
        let frame = s.view_frames.iter().find(|f| f.id == r.id);
        let what = match (sheet, frame) {
            (Some(sh), _) => format!("2D lines {} strokes {} texts {} fills {} images {} views {} extent {:?}", sh.lines.len(), sh.strokes.len(), sh.texts.len(), sh.fills.len(), sh.images.len(), sh.views.len(), sh.extent),
            (None, Some(f)) => format!("frame dir {:?} region {} clip {} hide {}", f.dir, f.region.is_some(), f.clip.is_some(), f.hide.len()),
            (None, None) => "NOTHING".to_string(),
        };
        println!("{:?} {} {:?}: {what}", r.kind, r.id, r.label);
    }
}
