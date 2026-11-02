//! Probe: what each view shows and hides, by category. Usage: rvtviewhide <file> [view name part]
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let s = craft_formats::load_any(std::path::Path::new(&a[0]), &Default::default()).unwrap();
    let f = a.get(1).cloned().unwrap_or_default();
    for v in &s.view_frames {
        let Some(r) = s.browser.iter().find(|r| r.id == v.id) else { continue };
        if !r.label.contains(&f) {
            continue;
        }
        println!("{:40} hides {:6} of {}", r.label, v.hide.len(), s.components.len());
        if std::env::var("CATS").is_ok() {
            let hide: std::collections::HashSet<usize> = v.hide.iter().copied().collect();
            let mut by = std::collections::BTreeMap::<String, (usize, usize)>::new();
            for (i, c) in s.components.iter().enumerate().filter(|(_, c)| c.mesh.is_some()) {
                let cat = c.parent.map(|p| s.components[p].name.clone()).unwrap_or_default();
                let e = by.entry(cat.split(" (").next().unwrap_or("").to_string()).or_default();
                if hide.contains(&i) { e.1 += 1 } else { e.0 += 1 }
            }
            for (k, (sh, hi)) in by { println!("   {k:40} shown {sh:6} hidden {hi:6}"); }
        }
    }
}
