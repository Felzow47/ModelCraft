//! Probe: how many components each view hides. Usage: rvtviewhide <file> [view name part]
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let s = craft_formats::load_any(std::path::Path::new(&a[0]), &Default::default()).unwrap();
    let f = a.get(1).cloned().unwrap_or_default();
    for v in &s.view_frames {
        let Some(r) = s.browser.iter().find(|r| r.id == v.id) else { continue };
        if r.label.contains(&f) {
            println!("{:40} hides {:6} of {}", r.label, v.hide.len(), s.components.len());
        }
    }
}
