// Embeds the Windows icon + version info when cross-compiling with mingw (windres).
fn main() {
    println!("cargo:rerun-if-changed=assets/swcraft.rc");
    println!("cargo:rerun-if-changed=assets/swcraft.ico");
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") || !target.contains("gnu") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let obj = out.join("swcraft_res.o");
    let windres = if cfg!(windows) { "windres" } else { "x86_64-w64-mingw32-windres" };
    let ok = std::process::Command::new(windres)
        .current_dir("assets")
        .args(["swcraft.rc", "-O", "coff", "-o"])
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
