//! swcraft-scan — strictly read-only batch check of a folder of 3D / CAD files.
//!
//! Walks a folder (never writes into it), tries to load every supported file
//! (SolidWorks, STEP/IGES, STL, OBJ, 3DS, 3DXML, eDrawings…) exactly like the
//! viewer does, and reports per file: ok / partial / no_mesh / unsupported / error.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use sw_format::DocKind;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    Ok,
    Partial,
    NoMesh,
    Unsupported,
    Error,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Partial => "partial",
            Status::NoMesh => "no_mesh",
            Status::Unsupported => "unsupported",
            Status::Error => "error",
        }
    }
}

struct Row {
    path: PathBuf,
    size: u64,
    kind: &'static str,
    format: String,
    container: String,
    status: Status,
    faces: usize,
    triangles: usize,
    components: usize,
    missing: usize,
    preview: bool,
    ms: f64,
    message: String,
}

struct Args {
    root: PathBuf,
    csv: Option<PathBuf>,
    json: Option<PathBuf>,
    threads: usize,
    external: bool,
    quiet: bool,
}

fn usage() -> ! {
    eprintln!(
        "swcraft-scan {} — vérification en LECTURE SEULE d'un dossier de modèles 3D

Usage: swcraft-scan <dossier> [--csv rapport.csv] [--json rapport.json] [--threads N] [--no-external] [--quiet]

  Parcourt <dossier> récursivement, ouvre chaque fichier 3D reconnu ({exts})
  sans jamais rien écrire dedans, et indique si ModelCraft peut l'afficher :
    ok           maillage complet
    partial      assemblage dont certains composants n'ont pas de maillage
    no_mesh      aucun maillage enregistré (seule la vignette est affichable)
    unsupported  format reconnu mais non lisible (ex. Parasolid .x_t)
    error        fichier illisible / corrompu
  Les rapports CSV/JSON ne peuvent pas être écrits à l'intérieur du dossier scanné.",
        env!("CARGO_PKG_VERSION"),
        exts = craft_formats::open_extensions().iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(" ")
    );
    std::process::exit(2)
}

fn parse_args() -> Args {
    let mut root = None;
    let mut csv = None;
    let mut json = None;
    let mut threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
    let mut external = true;
    let mut quiet = false;
    let mut it = std::env::args_os().skip(1);
    while let Some(a) = it.next() {
        match a.to_str() {
            Some("-h") | Some("--help") => usage(),
            Some("--csv") => csv = it.next().map(PathBuf::from),
            Some("--json") => json = it.next().map(PathBuf::from),
            Some("--threads") | Some("-j") => {
                threads = it.next().and_then(|s| s.to_str().and_then(|s| s.parse().ok())).unwrap_or(threads).max(1)
            }
            Some("--no-external") => external = false,
            Some("--quiet") | Some("-q") => quiet = true,
            Some(s) if s.starts_with("--") => {
                eprintln!("option inconnue : {s}");
                usage()
            }
            _ => root = Some(PathBuf::from(a)),
        }
    }
    let Some(root) = root else { usage() };
    Args { root, csv, json, threads, external, quiet }
}

/// Recursive walk; never follows symlinks/junctions, only reads directory listings.
fn walk(root: &Path, out: &mut Vec<PathBuf>, errors: &mut Vec<(PathBuf, String)>) {
    let exts = craft_formats::open_extensions();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) => {
                errors.push((dir, e.to_string()));
                continue;
            }
        };
        for ent in rd.flatten() {
            let p = ent.path();
            let Ok(ft) = ent.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(p);
            } else if ft.is_file() {
                let name = ent.file_name().to_string_lossy().to_lowercase();
                if name.starts_with("~$") {
                    continue; // SolidWorks lock/temp files
                }
                let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
                if !ext.is_empty() && exts.contains(&ext) {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
}

fn check(path: &Path, external: bool) -> Row {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let lower = path.to_string_lossy().to_lowercase();
    let mut row = Row {
        path: path.to_path_buf(),
        size,
        kind: if lower.ends_with(".sldasm") || lower.ends_with(".easm") {
            "assembly"
        } else if lower.ends_with(".sldprt") || lower.ends_with(".eprt") {
            "part"
        } else {
            "model"
        },
        format: craft_formats::ext_of(path),
        container: String::new(),
        status: Status::Error,
        faces: 0,
        triangles: 0,
        components: 0,
        missing: 0,
        preview: false,
        ms: 0.0,
        message: String::new(),
    };
    let opts = craft_formats::Options { edges: false, resolve_external: external, preview: true, quality: 0.5, progress: None };
    let t = Instant::now();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| craft_formats::load_any(path, &opts)));
    row.ms = t.elapsed().as_secs_f64() * 1000.0;
    match r {
        Ok(Ok(s)) => {
            row.kind = match s.kind {
                DocKind::Part => "part",
                DocKind::Assembly => "assembly",
                DocKind::Model => "model",
            };
            row.container = s.container.label().to_string();
            row.faces = s.meshes.iter().map(|m| m.mesh.face_count).sum();
            row.triangles = s.drawn_triangles();
            row.components = s.components.iter().filter(|c| !c.is_assembly).count();
            row.missing = if s.kind == DocKind::Assembly { s.missing_components() } else { 0 };
            row.preview = s.preview.is_some();
            row.status = if !s.has_geometry() {
                Status::NoMesh
            } else if row.missing > 0 {
                Status::Partial
            } else {
                Status::Ok
            };
            row.message = s.warnings.join(" | ");
        }
        Ok(Err(e)) => {
            if !craft_formats::is_supported(path) || e.contains("compilée sans OpenCascade") {
                row.status = Status::Unsupported;
            }
            row.message = e
        }
        Err(p) => {
            let m = p
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".into());
            row.message = format!("erreur interne : {m}");
        }
    }
    row
}

fn csv_field(s: &str) -> String {
    if s.contains([',', ';', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn inside(root: &Path, out: &Path) -> bool {
    let Ok(root) = std::fs::canonicalize(root) else { return false };
    let abs = if out.is_absolute() { out.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(out) };
    let parent = abs.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    match std::fs::canonicalize(&parent) {
        Ok(p) => p.starts_with(&root),
        Err(_) => false,
    }
}

fn main() {
    let args = parse_args();
    if !args.root.is_dir() {
        eprintln!("Erreur : {} n'est pas un dossier", args.root.display());
        std::process::exit(2);
    }
    for out in [&args.csv, &args.json].into_iter().flatten() {
        if inside(&args.root, out) {
            eprintln!(
                "Refusé : le rapport {} serait écrit dans le dossier scanné (lecture seule). Choisissez un autre emplacement.",
                out.display()
            );
            std::process::exit(2);
        }
    }

    let t0 = Instant::now();
    let mut files = Vec::new();
    let mut walk_errors = Vec::new();
    walk(&args.root, &mut files, &mut walk_errors);
    let n = files.len();
    eprintln!("{} fichiers 3D trouvés en {:.1} s — analyse sur {} threads…", n, t0.elapsed().as_secs_f64(), args.threads);
    for (d, e) in &walk_errors {
        eprintln!("  dossier illisible : {} ({e})", d.display());
    }

    let files = Arc::new(files);
    let next = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicUsize::new(0));
    let rows: Arc<Mutex<Vec<Option<Row>>>> = Arc::new(Mutex::new((0..n).map(|_| None).collect()));
    let t_scan = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..args.threads.min(n.max(1)) {
        let (files, next, done, rows) = (files.clone(), next.clone(), done.clone(), rows.clone());
        let (external, quiet) = (args.external, args.quiet);
        handles.push(std::thread::spawn(move || loop {
            let i = next.fetch_add(1, Ordering::SeqCst);
            if i >= files.len() {
                break;
            }
            let row = check(&files[i], external);
            if !quiet {
                eprintln!(
                    "[{:>5}/{}] {:<8} {:>8.1} ms  {}{}",
                    done.fetch_add(1, Ordering::SeqCst) + 1,
                    files.len(),
                    row.status.as_str(),
                    row.ms,
                    row.path.display(),
                    if row.status == Status::Error { format!("  — {}", row.message) } else { String::new() }
                );
            } else {
                done.fetch_add(1, Ordering::SeqCst);
            }
            rows.lock().unwrap()[i] = Some(row);
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    let rows: Vec<Row> = Arc::try_unwrap(rows).ok().unwrap().into_inner().unwrap().into_iter().flatten().collect();
    let scan_s = t_scan.elapsed().as_secs_f64();

    // ---- summary
    let count = |s: Status| rows.iter().filter(|r| r.status == s).count();
    let (ok, partial, nomesh, err) = (count(Status::Ok), count(Status::Partial), count(Status::NoMesh), count(Status::Error));
    let unsup = count(Status::Unsupported);
    let nomesh_prev = rows.iter().filter(|r| r.status == Status::NoMesh && r.preview).count();
    let parts = rows.iter().filter(|r| r.kind == "part").count();
    let mut times: Vec<f64> = rows.iter().map(|r| r.ms).collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| if times.is_empty() { 0.0 } else { times[((times.len() - 1) as f64 * p).round() as usize] };
    let pc = |k: usize| if n == 0 { 0.0 } else { k as f64 * 100.0 / n as f64 };
    println!();
    println!("=== swcraft-scan : {} ===", args.root.display());
    let asms = rows.iter().filter(|r| r.kind == "assembly").count();
    println!("Fichiers          : {n} ({parts} pièces, {asms} assemblages, {} modèles)", n - parts - asms);
    let mut by_fmt: std::collections::BTreeMap<&str, (usize, usize)> = Default::default();
    for r in &rows {
        let e = by_fmt.entry(r.format.as_str()).or_default();
        e.0 += 1;
        if matches!(r.status, Status::Ok | Status::Partial) {
            e.1 += 1;
        }
    }
    println!("Par format        : {}", by_fmt.iter().map(|(f, (t, o))| format!(".{f} {o}/{t}")).collect::<Vec<_>>().join(" · "));
    println!("OK                : {ok:>6}  ({:.1} %)", pc(ok));
    println!("Partiel           : {partial:>6}  ({:.1} %)  assemblages avec composants sans maillage", pc(partial));
    println!("Sans maillage     : {nomesh:>6}  ({:.1} %)  dont {nomesh_prev} avec vignette", pc(nomesh));
    println!("Non supporté      : {unsup:>6}  ({:.1} %)", pc(unsup));
    println!("Erreur            : {err:>6}  ({:.1} %)", pc(err));
    println!(
        "Temps par fichier : médiane {:.1} ms · p95 {:.1} ms · max {:.1} ms   (total {:.1} s)",
        pct(0.5),
        pct(0.95),
        pct(1.0),
        scan_s
    );
    if err > 0 {
        let mut msgs: std::collections::BTreeMap<String, usize> = Default::default();
        for r in rows.iter().filter(|r| r.status == Status::Error) {
            *msgs.entry(r.message.chars().take(80).collect()).or_default() += 1;
        }
        println!("Erreurs les plus fréquentes :");
        let mut v: Vec<_> = msgs.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        for (m, c) in v.into_iter().take(8) {
            println!("  {c:>5} × {m}");
        }
    }

    // ---- reports
    if let Some(p) = &args.csv {
        let mut s = String::from("\u{FEFF}path;size_bytes;format;kind;container;status;faces;triangles;components;missing;preview;ms;message\n");
        for r in &rows {
            s.push_str(&format!(
                "{};{};{};{};{};{};{};{};{};{};{};{:.2};{}\n",
                csv_field(&r.path.to_string_lossy()),
                r.size,
                r.format,
                r.kind,
                csv_field(&r.container),
                r.status.as_str(),
                r.faces,
                r.triangles,
                r.components,
                r.missing,
                if r.preview { "yes" } else { "no" },
                r.ms,
                csv_field(&r.message)
            ));
        }
        write_report(p, s.as_bytes());
    }
    if let Some(p) = &args.json {
        let mut s = String::from("{\n");
        s.push_str(&format!("  \"root\": {},\n", json_str(&args.root.to_string_lossy())));
        s.push_str(&format!(
            "  \"summary\": {{\"files\": {n}, \"ok\": {ok}, \"partial\": {partial}, \"no_mesh\": {nomesh}, \"unsupported\": {unsup}, \"error\": {err}, \"seconds\": {scan_s:.2}}},\n"
        ));
        s.push_str("  \"files\": [\n");
        for (i, r) in rows.iter().enumerate() {
            s.push_str(&format!(
                "    {{\"path\": {}, \"size\": {}, \"format\": \"{}\", \"kind\": \"{}\", \"container\": {}, \"status\": \"{}\", \"faces\": {}, \"triangles\": {}, \"components\": {}, \"missing\": {}, \"preview\": {}, \"ms\": {:.2}, \"message\": {}}}{}\n",
                json_str(&r.path.to_string_lossy()),
                r.size,
                r.format,
                r.kind,
                json_str(&r.container),
                r.status.as_str(),
                r.faces,
                r.triangles,
                r.components,
                r.missing,
                r.preview,
                r.ms,
                json_str(&r.message),
                if i + 1 < rows.len() { "," } else { "" }
            ));
        }
        s.push_str("  ]\n}\n");
        write_report(p, s.as_bytes());
    }
}

fn write_report(p: &Path, data: &[u8]) {
    match std::fs::File::create(p).and_then(|mut f| f.write_all(data)) {
        Ok(_) => println!("Rapport écrit : {}", p.display()),
        Err(e) => eprintln!("Impossible d'écrire {} : {e}", p.display()),
    }
}
