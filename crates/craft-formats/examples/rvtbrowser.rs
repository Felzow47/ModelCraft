//! Prints the project browser of a .rvt (`--counts`: anonymous counts only).
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let counts = a.iter().any(|x| x == "--counts");
    for p in a.iter().skip(1).filter(|x| !x.starts_with("--")) {
        let t = std::time::Instant::now();
        let f = std::fs::File::open(p).unwrap();
        let mut cf = cfb::CompoundFile::open(std::io::BufReader::new(f)).unwrap();
        let names: Vec<(String, u64)> = cf.walk().filter(|e| e.is_stream()).map(|e| (e.path().to_string_lossy().replace('\\', "/").trim_start_matches('/').to_string(), e.len())).collect();
        let Some(m) = craft_formats::rvt_browser::read_model(&mut cf, &names, 2 << 30) else {
            println!("{}: no model", if counts { "?" } else { p });
            continue;
        };
        let rows = craft_formats::rvt_browser::build(&m);
        if counts {
            let roots: Vec<String> = rows.iter().filter(|r| r.depth == 0).map(|r| format!("{:?}", r.kind)).collect();
            let per = |k| rows.iter().filter(|r| r.kind == k).count();
            use sw_format::BrowserKind as K;
            println!("views={} types={} orgs={} phases={} current={:?} initial_view_found={:?} rows={} roots={:?} folders={} sheets={} schedules={} legends={} plans={} maxdepth={} unknown_folders={} numeric_folders={} vieworg_params={:?} sheetorg_params={:?} {:.1}s",
                m.views.len(), m.types.len(), m.orgs.len(), m.phases.len(), m.current.map(|c| c.is_some()), m.initial_view.map(|_| rows.iter().find(|r| r.start).map(|r| format!("{:?}", r.kind))), rows.len(), roots, per(K::Folder), per(K::Sheet), per(K::Schedule), per(K::Legend), per(K::Plan), rows.iter().map(|r| r.depth).max().unwrap_or(0),
                rows.iter().filter(|r| r.kind == K::Folder && r.label.contains("???")).count(),
                rows.iter().filter(|r| r.kind == K::Folder && r.label.chars().all(|c| c.is_ascii_digit() || c == '-')).count(),
                m.current[0].and_then(|i| m.orgs.get(&i)).map(|o| (o.folders.clone(), o.filters.iter().map(|f| (f.0, f.1)).collect::<Vec<_>>())),
                m.current[1].and_then(|i| m.orgs.get(&i)).map(|o| o.folders.clone()),
                t.elapsed().as_secs_f32());
            // families / groups / links: counts only (built-in category ids are not model data)
            let mut other: std::collections::BTreeMap<i64, usize> = Default::default();
            for f in m.families.iter().filter(|f| craft_formats::rvt_browser::category_label(f.category) == "Autres familles") {
                *other.entry(f.category).or_default() += 1;
            }
            let mut states: std::collections::BTreeMap<String, usize> = Default::default();
            for l in &m.links {
                *states.entry(if l.state.is_empty() { "-".into() } else { l.state.clone() }).or_default() += 1;
            }
            let inst: usize = m.links.iter().map(|l| l.instances.len()).sum();
            let inst_named: usize = m.links.iter().map(|l| l.instances.iter().filter(|i| !i.trim().is_empty()).count()).sum();
            let inst_digits: usize = m.links.iter().map(|l| l.instances.iter().filter(|i| !i.is_empty() && i.chars().all(|c| c.is_ascii_digit())).count()).sum();
            println!("    fam_surrogates={} in_place={} fam_rows={} famtype_rows={} fam_cats={} other_cat_ids={:?} groups={} group_rows={} groups_detail={} groups_attached={} links={} link_named={} link_path={} link_states={:?} link_inst={} inst_named={} inst_digits={} unloaded_rows={}",
                m.families.len(), m.in_place.len(), per(K::Family), per(K::FamilyType),
                rows.iter().skip_while(|r| r.kind != K::Families).skip(1).take_while(|r| r.depth > 0).filter(|r| r.depth == 1).count(),
                other, m.groups.len(), per(K::Group), m.groups.iter().filter(|g| g.detail).count(), m.groups.iter().filter(|g| g.attached_to > 0).count(),
                m.links.len(), m.links.iter().filter(|l| !l.name.trim().is_empty()).count(), m.links.iter().filter(|l| !l.path_name.is_empty()).count(), states, inst, inst_named, inst_digits, per(K::LinkUnloaded));
        } else {
            println!("== {p} ({:.1}s) current={:?} initial_view={:?}", t.elapsed().as_secs_f32(), m.current, m.initial_view);
            for r in rows {
                println!("{}{} [{:?}]{}", "  ".repeat(r.depth as usize), r.label, r.kind, if r.start { "  <== VUE DE DEMARRAGE" } else { "" });
            }
        }
    }
}
