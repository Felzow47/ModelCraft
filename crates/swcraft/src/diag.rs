//! `--diag <file>`: headless diagnostic load. Writes the container layout (streams with stored
//! and decompressed sizes), each loading stage with timings and memory use, the resulting mesh
//! sizes and the peak memory to a text file - flushed line by line, so the log is useful even if
//! the process is killed or crashes midway.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// (current, peak) resident memory of this process, in bytes.
pub fn process_memory() -> Option<(u64, u64)> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::ProcessStatus::{
            K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut c: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        // SAFETY: plain Win32 call on our own process filling a correctly sized struct.
        if unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) } != 0 {
            // private commit is what runs out first; report the larger of commit and working set
            return Some((
                (c.PagefileUsage.max(c.WorkingSetSize)) as u64,
                (c.PeakPagefileUsage.max(c.PeakWorkingSetSize)) as u64,
            ));
        }
        None
    }
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/self/status").ok()?;
        let kb = |k: &str| {
            s.lines()
                .find(|l| l.starts_with(k))?
                .split_whitespace()
                .nth(1)?
                .parse::<u64>()
                .ok()
        };
        Some((kb("VmRSS:")? * 1024, kb("VmHWM:")? * 1024))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

fn mb(b: u64) -> String {
    format!("{:.1} Mo", b as f64 / (1024.0 * 1024.0))
}

fn mem_text() -> String {
    match process_memory() {
        Some((cur, peak)) => format!("mém {} (pic {})", mb(cur), mb(peak)),
        None => "mém ?".into(),
    }
}

pub fn default_out(file: &Path) -> PathBuf {
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "fichier".into());
    let dir = crate::settings::config_dir()
        .filter(|d| std::fs::create_dir_all(d).is_ok())
        .unwrap_or_else(std::env::temp_dir);
    dir.join(format!("diag-{stem}.txt"))
}

struct Log {
    f: Option<std::fs::File>,
    t0: Instant,
}

impl Log {
    fn line(&mut self, s: &str) {
        let l = format!("[{:>8.2} s] {s}\n", self.t0.elapsed().as_secs_f64());
        eprint!("{l}");
        if let Some(f) = self.f.as_mut() {
            let _ = f.write_all(l.as_bytes());
            let _ = f.flush();
        }
    }
}

/// Run the diagnostic; returns the process exit code.
pub fn run(file: &Path, out: Option<PathBuf>) -> i32 {
    let out = out.unwrap_or_else(|| default_out(file));
    let f = std::fs::File::create(&out).ok();
    let ok_file = f.is_some();
    let log = Arc::new(Mutex::new(Log {
        f,
        t0: Instant::now(),
    }));
    let say = |s: &str| log.lock().unwrap().line(s);
    say(&format!(
        "{} {} - diagnostic de chargement",
        crate::branding::NAME,
        crate::branding::VERSION
    ));
    say(&format!(
        "système : {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    say(&format!("fichier : {}", file.display()));
    say(&format!("rapport : {}", out.display()));
    match std::fs::metadata(file) {
        Ok(m) => say(&format!("taille : {} octets ({})", m.len(), mb(m.len()))),
        Err(e) => {
            say(&format!("ERREUR : fichier illisible : {e}"));
            return 2;
        }
    }
    let total = sw_format::ctl::total_memory().unwrap_or(0);
    let avail = sw_format::ctl::mem_available().unwrap_or(0);
    say(&format!(
        "mémoire : totale {}, disponible {}",
        mb(total),
        mb(avail)
    ));
    say(&format!(
        "budget d'affichage : {} sommets (MODELCRAFT_MAX_VERTICES pour changer) ; flux > {} décodés en continu",
        sw_format::vertex_budget(),
        mb(sw_format::STREAM_THRESHOLD)
    ));
    say(&mem_text());

    // Container layout (SolidWorks / OLE files).
    let t = Instant::now();
    match sw_format::Container::open(file) {
        Ok(c) => {
            say(&format!(
                "conteneur : {:?}, {} flux, ouvert en {:.2} s - {}",
                c.kind(),
                c.streams().len(),
                t.elapsed().as_secs_f64(),
                mem_text()
            ));
            let mut v: Vec<_> = c.streams().iter().collect();
            v.sort_by_key(|s| std::cmp::Reverse(s.stored_size));
            for s in v.iter().take(60) {
                let raw = s
                    .raw_size()
                    .map(|r| format!("{r} octets décompressés"))
                    .unwrap_or_else(|| "taille décompressée inconnue".into());
                say(&format!(
                    "  flux {:<48} {:>12} octets stockés, {raw}",
                    s.name, s.stored_size
                ));
            }
            if v.len() > 60 {
                say(&format!("  … {} autres flux", v.len() - 60));
            }
        }
        Err(e) => say(&format!("conteneur SolidWorks/OLE : non ({e})")),
    }

    // Memory sampler: one line every 2 s (shows where a long or failing load stands).
    let done = Arc::new(AtomicBool::new(false));
    let stage = Arc::new(Mutex::new((String::new(), 0.0f32, 0u64, 0u64)));
    let sampler = {
        let (log, done, stage) = (log.clone(), done.clone(), stage.clone());
        std::thread::spawn(move || {
            let mut n = 0u32;
            while !done.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(250));
                n += 1;
                if n % 8 == 0 {
                    let (s, f, d, t) = stage.lock().unwrap().clone();
                    let bytes = if t > 0 {
                        format!(", {} / {}", mb(d), mb(t))
                    } else {
                        String::new()
                    };
                    log.lock().unwrap().line(&format!(
                        "… {:>3.0} % {s}{bytes} - {}",
                        f * 100.0,
                        mem_text()
                    ));
                }
            }
        })
    };
    let progress = {
        let (log, stage, stage2) = (log.clone(), stage.clone(), stage.clone());
        sw_format::Progress::new(move |f, s| {
            let mut st = stage.lock().unwrap();
            if st.0 != s {
                st.0 = s.to_string();
                st.2 = 0;
                st.3 = 0;
                drop(st);
                log.lock().unwrap().line(&format!(
                    "étape {:>3.0} % : {s} - {}",
                    f * 100.0,
                    mem_text()
                ));
                stage.lock().unwrap().1 = f;
            } else {
                st.1 = f;
            }
        })
        .with_bytes(move |d, t| {
            let mut st = stage2.lock().unwrap();
            st.2 = d;
            st.3 = t;
        })
    };
    crate::crash::set_current_file(Some(file));
    let opts = craft_formats::Options {
        progress: Some(progress),
        ..Default::default()
    };
    let t = Instant::now();
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        craft_formats::load_any(file, &opts)
    }));
    done.store(true, Ordering::Relaxed);
    let _ = sampler.join();
    crate::crash::set_current_file(None);
    let code = match res {
        Ok(Ok(scene)) => {
            say(&format!(
                "CHARGÉ en {:.2} s - {}",
                t.elapsed().as_secs_f64(),
                mem_text()
            ));
            let tm = &scene.timings;
            say(&format!(
                "temps : ouverture {:.2} s, lecture {:.2} s, analyse {:.2} s, maillage {:.2} s, externes {:.2} s, total {:.2} s",
                tm.open.as_secs_f64(),
                tm.read.as_secs_f64(),
                tm.decode.as_secs_f64(),
                tm.mesh.as_secs_f64(),
                tm.external.as_secs_f64(),
                tm.total.as_secs_f64()
            ));
            if let Some(d) = &tm.dl {
                say(&format!(
                    "DisplayLists : {} stockés, {} décompressés, {} ; {} blocs, {} faces",
                    mb(d.stored_bytes),
                    mb(d.raw_bytes),
                    if d.streamed {
                        "décodé en continu (2 passes)"
                    } else {
                        "décodé en mémoire"
                    },
                    d.blocks,
                    d.faces
                ));
                say(&format!(
                    "  sommets {} → {} affichés, triangles {} → {}{} ; passe 1 {:.2} s, passe 2 {:.2} s",
                    d.src_vertices,
                    d.out_vertices,
                    d.src_triangles,
                    d.out_triangles,
                    if d.simplified { format!(" (simplifié, budget {})", d.budget) } else { String::new() },
                    d.pass1.as_secs_f64(),
                    d.pass2.as_secs_f64()
                ));
            }
            let verts: usize = scene.meshes.iter().map(|m| m.mesh.positions.len()).sum();
            let tris: usize = scene.meshes.iter().map(|m| m.mesh.triangle_count()).sum();
            let edges: usize = scene.meshes.iter().map(|m| m.mesh.edges.len() / 2).sum();
            say(&format!(
                "scène : {:?}, {} maillages, {} composants, {} sommets, {} triangles ({} dessinés), {} arêtes, vignette {}",
                scene.kind,
                scene.meshes.len(),
                scene.components.len(),
                verts,
                tris,
                scene.drawn_triangles(),
                edges,
                if scene.preview.is_some() { "oui" } else { "non" }
            ));
            let gpu: usize = scene
                .meshes
                .iter()
                .map(|m| {
                    m.mesh.positions.len() * 24 + m.mesh.indices.len() * 4 + m.mesh.edges.len() * 4
                })
                .sum();
            say(&format!("mémoire GPU estimée : {}", mb(gpu as u64)));
            for w in &scene.warnings {
                say(&format!("avertissement : {w}"));
            }
            0
        }
        Ok(Err(e)) => {
            say(&format!(
                "ÉCHEC après {:.2} s : {e} - {}",
                t.elapsed().as_secs_f64(),
                mem_text()
            ));
            1
        }
        Err(p) => {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            say(&format!(
                "PANIQUE après {:.2} s : {msg} - {}",
                t.elapsed().as_secs_f64(),
                mem_text()
            ));
            3
        }
    };
    say(&format!("fin - {}", mem_text()));
    if !ok_file {
        eprintln!("(rapport non écrit : {} inaccessible)", out.display());
    }
    code
}
