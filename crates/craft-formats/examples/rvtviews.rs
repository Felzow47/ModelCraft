//! Probe: browser rows whose label contains a text (any case). Usage: rvtviews <file> <text>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let s = craft_formats::load_any(std::path::Path::new(&a[0]), &Default::default()).unwrap();
    let f = a[1].to_lowercase();
    for r in s.browser.iter().filter(|r| r.label.to_lowercase().contains(&f)) {
        println!("{:?} {} {}", r.kind, r.id, r.label);
    }
}
