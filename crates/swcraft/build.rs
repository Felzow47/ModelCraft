// Embeds the Windows icon + version info when cross-compiling with mingw (windres).
fn main() {
    icons();
    println!("cargo:rerun-if-changed=assets/modelcraft.rc");
    println!("cargo:rerun-if-changed=assets/modelcraft.ico");
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") || !target.contains("gnu") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let obj = out.join("modelcraft_res.o");
    let windres = if cfg!(windows) { "windres" } else { "x86_64-w64-mingw32-windres" };
    let ok = std::process::Command::new(windres)
        .current_dir("assets")
        .args(["modelcraft.rc", "-O", "coff", "-o"])
        .arg(&obj)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        println!("cargo:rustc-link-arg-bins={}", obj.display());
    } else {
        println!("cargo:warning=windres introuvable : exe sans icône");
    }
}

/// Embed every Lucide SVG of assets/icons as (name, bytes).
fn icons() {
    println!("cargo:rerun-if-changed=assets/icons");
    let mut names: Vec<String> = std::fs::read_dir("assets/icons")
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".svg"))
        .collect();
    names.sort();
    let mut out = String::from("pub static ICONS: &[(&str, &[u8])] = &[\n");
    for n in names {
        let stem = n.trim_end_matches(".svg");
        out.push_str(&format!("    (\"{stem}\", include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/assets/icons/{n}\"))),\n"));
    }
    out.push_str("];\n");
    let dst = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("icon_data.rs");
    std::fs::write(dst, out).unwrap();
}
