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
    /// The reader crashed (native fault / abort) in the isolated child process.
    Crash,
    /// The reader did not finish within the time limit.
    Timeout,
    /// Skipped: larger than `--max-size`.
    TooLarge,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Partial => "partial",
            Status::NoMesh => "no_mesh",
            Status::Unsupported => "unsupported",
            Status::Error => "error",
            Status::Crash => "crash",
            Status::Timeout => "timeout",
            Status::TooLarge => "too_large",
        }
    }
    fn parse(s: &str) -> Status {
        match s {
            "ok" => Status::Ok,
            "partial" => Status::Partial,
            "no_mesh" => Status::NoMesh,
            "unsupported" => Status::Unsupported,
            "crash" => Status::Crash,
            "timeout" => Status::Timeout,
            "too_large" => Status::TooLarge,
            _ => Status::Error,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Isolate {
    /// Child process for STEP/IGES (OpenCascade) and big files.
    Auto,
    All,
    None,
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
    log: Option<PathBuf>,
    threads: usize,
    external: bool,
    quiet: bool,
    max_size: u64,
    isolate: Isolate,
    timeout: f64,
    mem_limit: Option<u64>,
    /// Started by double-click (no console of our own to return to): pause at the end.
    interactive: bool,
}

/// Files above this size are always read in a child process (Auto mode).
const BIG_FILE: u64 = 64 << 20;

fn usage() -> ! {
    eprintln!(
        "swcraft-scan {} — vérification en LECTURE SEULE d'un dossier de modèles 3D

Usage: {exe} <dossier> [--csv rapport.csv] [--json rapport.json] [--log progression.log] [--threads N]
          [--max-size 1G] [--timeout 120] [--isolate auto|all|none] [--mem-limit 8G] [--no-external] [--quiet]
       {exe}            (sans argument / double-clic : demande le dossier, rapport à côté de l'exe)

  Parcourt <dossier> récursivement, ouvre chaque fichier 3D reconnu ({exts})
  sans jamais rien écrire dedans, et indique si ModelCraft peut l'afficher :
    ok           maillage complet
    partial      assemblage dont certains composants n'ont pas de maillage
    no_mesh      aucun maillage enregistré (seule la vignette est affichable)
    unsupported  format reconnu mais non lisible (ex. Parasolid .x_t)
    error        fichier illisible / corrompu
    crash        le lecteur a planté sur ce fichier (isolé dans un processus séparé)
    timeout      lecture plus longue que --timeout secondes (défaut 120)
    too_large    plus gros que --max-size (défaut 1G), non ouvert

  Chaque fichier est noté dans le journal de progression AVANT d'être ouvert (en cas de plantage,
  la dernière ligne START indique le coupable). STEP/IGES et les fichiers > 64 Mo sont lus dans
  un processus enfant (--isolate all : tous les fichiers ; none : aucun), limité en mémoire sous Windows.
  Les rapports et le journal ne peuvent pas être écrits à l'intérieur du dossier scanné.",
        env!("CARGO_PKG_VERSION"),
        exe = exe_name(),
        exts = craft_formats::open_extensions().iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(" ")
    );
    std::process::exit(2)
}

fn exe_name() -> String {
    std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "swcraft-scan".into())
}

/// "1G", "500M", "200k", "123456" → bytes.
fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_lowercase();
    let s = s.trim_end_matches(['o', 'b']);
    let (num, mul) = match s.chars().last()? {
        'k' => (&s[..s.len() - 1], 1u64 << 10),
        'm' => (&s[..s.len() - 1], 1 << 20),
        'g' => (&s[..s.len() - 1], 1 << 30),
        't' => (&s[..s.len() - 1], 1 << 40),
        _ => (s, 1),
    };
    let v: f64 = num.trim().replace(',', ".").parse().ok()?;
    (v >= 0.0).then(|| (v * mul as f64) as u64)
}

fn parse_args() -> Args {
    let mut root = None;
    let mut csv = None;
    let mut json = None;
    let mut log = None;
    let mut threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
    let mut external = true;
    let mut quiet = false;
    let mut max_size = 1u64 << 30;
    let mut isolate = Isolate::Auto;
    let mut timeout: f64 = 120.0;
    let mut mem_limit = None;
    let mut it = std::env::args_os().skip(1);
    let bad = |what: &str| -> ! {
        eprintln!("valeur invalide pour {what}");
        usage()
    };
    while let Some(a) = it.next() {
        match a.to_str() {
            Some("-h") | Some("--help") => usage(),
            Some("--csv") => csv = it.next().map(PathBuf::from),
            Some("--json") => json = it.next().map(PathBuf::from),
            Some("--log") => log = it.next().map(PathBuf::from),
            Some("--max-size") => max_size = it.next().and_then(|s| s.to_str().and_then(parse_size)).unwrap_or_else(|| bad("--max-size")),
            Some("--mem-limit") => mem_limit = Some(it.next().and_then(|s| s.to_str().and_then(parse_size)).unwrap_or_else(|| bad("--mem-limit"))),
            Some("--timeout") => timeout = it.next().and_then(|s| s.to_str().and_then(|s| s.parse().ok())).unwrap_or_else(|| bad("--timeout")),
            Some("--isolate") => {
                isolate = match it.next().as_ref().and_then(|s| s.to_str()) {
                    Some("auto") | Some("occt") => Isolate::Auto,
                    Some("all") => Isolate::All,
                    Some("none") => Isolate::None,
                    _ => bad("--isolate"),
                }
            }
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
    let mut interactive = false;
    let root = match root {
        Some(r) => r,
        None => {
            // double-click / no argument: ask for the folder instead of flashing a usage text
            use std::io::IsTerminal;
            if !std::io::stdin().is_terminal() {
                usage()
            }
            interactive = launched_by_double_click();
            match ask_folder() {
                Some(r) => r,
                None => {
                    pause_if(interactive);
                    std::process::exit(2)
                }
            }
        }
    };
    if csv.is_none() && json.is_none() && interactive_default_report(&root) {
        csv = default_report_path(&root);
    }
    Args { root, csv, json, log, threads, external, quiet, max_size, isolate, timeout: timeout.max(1.0), mem_limit, interactive }
}

/// A report is written by default when the user gave no output option.
fn interactive_default_report(_root: &Path) -> bool {
    true
}

/// `<exe folder>/rapport-scan-<date>.csv`, or the user's Documents / home if that would be inside
/// the scanned folder (never write there).
fn default_report_path(root: &Path) -> Option<PathBuf> {
    let name = format!("rapport-scan-{}.csv", stamp());
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        dirs.push(d);
    }
    for v in ["USERPROFILE", "HOME"] {
        if let Some(h) = std::env::var_os(v) {
            dirs.push(PathBuf::from(&h).join("Documents"));
            dirs.push(PathBuf::from(h));
        }
    }
    dirs.push(std::env::temp_dir());
    dirs.into_iter().filter(|d| d.is_dir()).map(|d| d.join(&name)).find(|p| !inside(root, p))
}

/// Local-ish timestamp for file names (UTC, compact).
fn stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", rem / 3600, rem / 60 % 60, rem % 60)
}

fn ask_folder() -> Option<PathBuf> {
    println!("{} {} — vérification en lecture seule d'un dossier de modèles 3D", exe_name(), env!("CARGO_PKG_VERSION"));
    println!("(rien n'est jamais écrit dans le dossier analysé ; le rapport est enregistré à côté de ce programme)");
    println!();
    loop {
        print!("Dossier à analyser (glissez-le dans cette fenêtre puis Entrée, vide pour quitter) : ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).ok()? == 0 {
            return None;
        }
        let t = line.trim().trim_matches('"').trim_matches('\'').trim();
        if t.is_empty() {
            return None;
        }
        let p = PathBuf::from(t);
        if p.is_dir() {
            return Some(p);
        }
        println!("  « {t} » n'est pas un dossier accessible.");
    }
}

/// True when our console was created for us (Explorer double-click): nobody else is attached.
fn launched_by_double_click() -> bool {
    #[cfg(windows)]
    {
        let mut ids = [0u32; 4];
        // SAFETY: plain Win32 call with a valid buffer.
        let n = unsafe { windows_sys::Win32::System::Console::GetConsoleProcessList(ids.as_mut_ptr(), ids.len() as u32) };
        n == 1
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn pause_if(yes: bool) {
    if yes {
        print!("\nAppuyez sur Entrée pour fermer cette fenêtre…");
        let _ = std::io::stdout().flush();
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
    }
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

fn new_row(path: &Path) -> Row {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let lower = path.to_string_lossy().to_lowercase();
    let row = Row {
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
    row
}

/// Load a file in this process (like the viewer) and fill a report row.
fn check_inproc(path: &Path, external: bool) -> Row {
    let mut row = new_row(path);
    let opts = craft_formats::Options { edges: false, resolve_external: external, preview: true, quality: 0.5, progress: None, cancel: None };
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

const ROW_TAG: &str = "MCSCAN-ROW";

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n").replace('\r', "\\r")
}

fn unesc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => o.push('\t'),
                Some('n') => o.push('\n'),
                Some('r') => o.push('\r'),
                Some(c) => o.push(c),
                None => {}
            }
        } else {
            o.push(c);
        }
    }
    o
}

/// One line on stdout describing the row (child → parent).
fn row_line(r: &Row) -> String {
    format!(
        "{ROW_TAG}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        r.kind,
        esc(&r.container),
        r.status.as_str(),
        r.faces,
        r.triangles,
        r.components,
        r.missing,
        r.preview as u8,
        r.ms,
        esc(&r.message)
    )
}

fn parse_row_line(line: &str, row: &mut Row) -> bool {
    let f: Vec<&str> = line.trim_end_matches(['\r', '\n']).split('\t').collect();
    if f.len() < 11 || f[0] != ROW_TAG {
        return false;
    }
    row.kind = match f[1] {
        "part" => "part",
        "assembly" => "assembly",
        _ => "model",
    };
    row.container = unesc(f[2]);
    row.status = Status::parse(f[3]);
    row.faces = f[4].parse().unwrap_or(0);
    row.triangles = f[5].parse().unwrap_or(0);
    row.components = f[6].parse().unwrap_or(0);
    row.missing = f[7].parse().unwrap_or(0);
    row.preview = f[8] == "1";
    row.ms = f[9].parse().unwrap_or(0.0);
    row.message = unesc(f[10]);
    true
}

/// `--single <file>`: check one file and print its row (used for isolation).
fn single_mode(path: &Path, external: bool) -> ! {
    // test hooks for the isolation logic (never set in normal use)
    if let Ok(k) = std::env::var("SWCRAFT_SCAN_TEST_CRASH") {
        if !k.is_empty() && path.to_string_lossy().contains(&k) {
            std::process::abort();
        }
    }
    if let Ok(k) = std::env::var("SWCRAFT_SCAN_TEST_HANG") {
        if !k.is_empty() && path.to_string_lossy().contains(&k) {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
    let r = check_inproc(path, external);
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", row_line(&r));
    let _ = out.flush();
    std::process::exit(0)
}

/// Readable name of a process exit / NTSTATUS code.
fn exit_text(st: &std::process::ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = st.signal() {
            let n = match sig {
                11 => " (SIGSEGV)",
                6 => " (SIGABRT, souvent mémoire insuffisante)",
                9 => " (SIGKILL)",
                7 => " (SIGBUS)",
                _ => "",
            };
            return format!("signal {sig}{n}");
        }
    }
    match st.code() {
        Some(c) => {
            let u = c as u32;
            let n = match u {
                0xC0000005 => " (violation d'accès)",
                0xC00000FD => " (dépassement de pile)",
                0xC0000017 | 0xC000012D => " (mémoire insuffisante)",
                0xC0000409 => " (arrêt d'urgence : souvent mémoire insuffisante ou limite mémoire atteinte)",
                0xC000001D => " (instruction illégale)",
                0xC0000094 => " (division par zéro)",
                0xE06D7363 => " (exception C++ non gérée)",
                3 => " (abort)",
                _ => "",
            };
            if u >= 0x8000_0000 { format!("code 0x{u:08X}{n}") } else { format!("code {c}{n}") }
        }
        None => "fin anormale".into(),
    }
}

/// Load a file in a child process (`--single`) with a time limit; a crash there only costs
/// this one file.
fn check_child(path: &Path, args: &Args) -> Row {
    use std::process::{Command, Stdio};
    let mut row = new_row(path);
    let t = Instant::now();
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return check_inproc(path, args.external),
    };
    let mut cmd = Command::new(exe);
    cmd.arg("--single").arg(path).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    if !args.external {
        cmd.arg("--no-external");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            row.message = format!("processus enfant impossible : {e}");
            return row;
        }
    };
    // read stdout on a thread so a chatty child can never block on a full pipe
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = stdout.as_mut() {
            let _ = std::io::Read::read_to_string(o, &mut s);
        }
        s
    });
    let deadline = std::time::Duration::from_secs_f64(args.timeout);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if t.elapsed() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(if t.elapsed().as_millis() < 200 { 2 } else { 20 })),
            Err(_) => break None,
        }
    };
    let out = reader.join().unwrap_or_default();
    row.ms = t.elapsed().as_secs_f64() * 1000.0;
    match status {
        None => {
            row.status = Status::Timeout;
            row.message = format!("lecture interrompue après {:.0} s", args.timeout);
        }
        Some(st) => {
            let parsed = out.lines().any(|l| parse_row_line(l, &mut row));
            if !parsed {
                row.status = Status::Crash;
                row.message = format!("le lecteur a planté sur ce fichier : {}", exit_text(&st));
            } else {
                row.ms = t.elapsed().as_secs_f64() * 1000.0;
            }
        }
    }
    row
}

fn needs_child(path: &Path, size: u64, iso: Isolate) -> bool {
    match iso {
        Isolate::All => true,
        Isolate::None => false,
        Isolate::Auto => size > BIG_FILE || matches!(craft_formats::ext_of(path).as_str(), "step" | "stp" | "iges" | "igs"),
    }
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
    // child mode: `--single <file> [--no-external]`
    let raw: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if let Some(i) = raw.iter().position(|a| a == "--single") {
        let Some(p) = raw.get(i + 1) else { usage() };
        sys::quiet_crashes();
        single_mode(Path::new(p), !raw.iter().any(|a| a == "--no-external"));
    }
    let args = parse_args();
    let fail = |msg: String| -> ! {
        eprintln!("{msg}");
        pause_if(args.interactive);
        std::process::exit(2)
    };
    if !args.root.is_dir() {
        fail(format!("Erreur : {} n'est pas un dossier", args.root.display()));
    }
    for out in [&args.csv, &args.json, &args.log].into_iter().flatten() {
        if inside(&args.root, out) {
            fail(format!(
                "Refusé : {} serait écrit dans le dossier scanné (lecture seule). Choisissez un autre emplacement.",
                out.display()
            ));
        }
    }
    sys::quiet_crashes();
    let mem_limit = args.mem_limit.or_else(|| sys::total_memory().map(|t| t / 4 * 3));
    if args.isolate != Isolate::None {
        if let Some(l) = mem_limit {
            sys::limit_child_memory(l);
        }
    }

    // progress log: every file is written (and flushed) BEFORE it is opened
    let log_path = args.log.clone().or_else(|| {
        let base = args.csv.clone().or_else(|| args.json.clone()).or_else(|| default_report_path(&args.root))?;
        let stem = base.file_stem()?.to_string_lossy().into_owned();
        Some(base.with_file_name(format!("{stem}-progression.log")))
    });
    let log: Option<Arc<Mutex<std::fs::File>>> = log_path.as_ref().and_then(|p| match std::fs::File::create(p) {
        Ok(f) => Some(Arc::new(Mutex::new(f))),
        Err(e) => {
            eprintln!("journal impossible ({}) : {e}", p.display());
            None
        }
    });

    let t0 = Instant::now();
    let mut files = Vec::new();
    let mut walk_errors = Vec::new();
    walk(&args.root, &mut files, &mut walk_errors);
    let n = files.len();
    eprintln!("{} fichiers 3D trouvés en {:.1} s — analyse sur {} threads…", n, t0.elapsed().as_secs_f64(), args.threads);
    if let Some(p) = &log_path {
        eprintln!("Journal de progression : {}", p.display());
    }
    for (d, e) in &walk_errors {
        eprintln!("  dossier illisible : {} ({e})", d.display());
    }
    if let Some(l) = &log {
        if let Ok(mut f) = l.lock() {
            let _ = writeln!(f, "# {} {} — {} — {} fichiers", exe_name(), env!("CARGO_PKG_VERSION"), args.root.display(), n);
            let _ = f.flush();
        }
    }

    let files = Arc::new(files);
    let next = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicUsize::new(0));
    let rows: Arc<Mutex<Vec<Option<Row>>>> = Arc::new(Mutex::new((0..n).map(|_| None).collect()));
    // very big files are read one at a time
    let big = Arc::new(Mutex::new(()));
    let args = Arc::new(args);
    let t_scan = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..args.threads.min(n.max(1)) {
        let (files, next, done, rows, log, big, args) = (files.clone(), next.clone(), done.clone(), rows.clone(), log.clone(), big.clone(), args.clone());
        handles.push(std::thread::spawn(move || loop {
            let i = next.fetch_add(1, Ordering::SeqCst);
            if i >= files.len() {
                break;
            }
            let path = &files[i];
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            let logln = |s: String| {
                if let Some(l) = &log {
                    if let Ok(mut f) = l.lock() {
                        let _ = writeln!(f, "{s}");
                        let _ = f.flush();
                        #[cfg(windows)]
                        let _ = f.sync_data();
                    }
                }
            };
            let row = if size > args.max_size {
                let mut r = new_row(path);
                r.status = Status::TooLarge;
                r.message = format!("{:.1} Mo > limite {:.1} Mo (--max-size)", size as f64 / 1048576.0, args.max_size as f64 / 1048576.0);
                r
            } else {
                let child = needs_child(path, size, args.isolate);
                logln(format!("START\t{}\t{}\t{}\t{}", i + 1, size, if child { "enfant" } else { "direct" }, path.display()));
                let _g = (size > 256 << 20).then(|| big.lock().unwrap_or_else(|e| e.into_inner()));
                if child { check_child(path, &args) } else { check_inproc(path, args.external) }
            };
            logln(format!("END\t{}\t{}\t{:.0} ms\t{}", i + 1, row.status.as_str(), row.ms, path.display()));
            if !args.quiet {
                eprintln!(
                    "[{:>5}/{}] {:<11} {:>8.1} ms  {}{}",
                    done.fetch_add(1, Ordering::SeqCst) + 1,
                    files.len(),
                    row.status.as_str(),
                    row.ms,
                    row.path.display(),
                    if matches!(row.status, Status::Error | Status::Crash | Status::Timeout | Status::TooLarge) { format!("  — {}", row.message) } else { String::new() }
                );
            } else {
                done.fetch_add(1, Ordering::SeqCst);
            }
            rows.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(row);
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    let rows: Vec<Row> = std::mem::take(&mut *rows.lock().unwrap_or_else(|e| e.into_inner())).into_iter().flatten().collect();
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
    let (crash, tout, big) = (count(Status::Crash), count(Status::Timeout), count(Status::TooLarge));
    println!("Plantage lecteur  : {crash:>6}  ({:.1} %)  isolés dans un processus séparé", pc(crash));
    println!("Délai dépassé     : {tout:>6}  ({:.1} %)  plus de {:.0} s", pc(tout), args.timeout);
    println!("Trop volumineux   : {big:>6}  ({:.1} %)  plus de {:.0} Mo, non ouverts", pc(big), args.max_size as f64 / 1048576.0);
    println!(
        "Temps par fichier : médiane {:.1} ms · p95 {:.1} ms · max {:.1} ms   (total {:.1} s)",
        pct(0.5),
        pct(0.95),
        pct(1.0),
        scan_s
    );
    if err + crash + tout > 0 {
        let mut msgs: std::collections::BTreeMap<String, usize> = Default::default();
        for r in rows.iter().filter(|r| matches!(r.status, Status::Error | Status::Crash | Status::Timeout)) {
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
            "  \"summary\": {{\"files\": {n}, \"ok\": {ok}, \"partial\": {partial}, \"no_mesh\": {nomesh}, \"unsupported\": {unsup}, \"error\": {err}, \"crash\": {crash}, \"timeout\": {tout}, \"too_large\": {big}, \"seconds\": {scan_s:.2}}},\n"
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
    if let Some(p) = &log_path {
        println!("Journal           : {}", p.display());
    }
    pause_if(args.interactive);
}

fn write_report(p: &Path, data: &[u8]) {
    match std::fs::File::create(p).and_then(|mut f| f.write_all(data)) {
        Ok(_) => println!("Rapport écrit : {}", p.display()),
        Err(e) => eprintln!("Impossible d'écrire {} : {e}", p.display()),
    }
}

/// Platform helpers (Windows: job object memory limit, no crash dialogs).
mod sys {
    /// Total physical memory in bytes.
    pub fn total_memory() -> Option<u64> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
            let mut s: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
            s.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
            // SAFETY: correctly sized struct.
            if unsafe { GlobalMemoryStatusEx(&mut s) } != 0 {
                return Some(s.ullTotalPhys);
            }
            None
        }
        #[cfg(target_os = "linux")]
        {
            let s = std::fs::read_to_string("/proc/meminfo").ok()?;
            let kb = s.lines().find(|l| l.starts_with("MemTotal:"))?.split_whitespace().nth(1)?.parse::<u64>().ok()?;
            Some(kb * 1024)
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            None
        }
    }

    /// No "program has stopped working" dialog for crashing children (it would hang the scan
    /// until the timeout). The error mode is inherited by child processes.
    pub fn quiet_crashes() {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::System::Diagnostics::Debug::{SetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX};
            SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
        }
    }

    /// Put this process in a job object whose per-process memory limit applies to every child we
    /// start (children inherit the job), so a runaway reader fails instead of exhausting RAM.
    pub fn limit_child_memory(bytes: u64) {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::System::JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
            };
            use windows_sys::Win32::System::Threading::GetCurrentProcess;
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
            info.ProcessMemoryLimit = bytes as usize;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok != 0 && AssignProcessToJobObject(job, GetCurrentProcess()) != 0 {
                eprintln!("Limite mémoire par fichier : {:.1} Go", bytes as f64 / (1u64 << 30) as f64);
            }
            // the handle is kept open for the lifetime of the process
        }
        #[cfg(not(windows))]
        let _ = bytes;
    }
}
