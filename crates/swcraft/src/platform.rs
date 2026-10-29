//! OS integration: reveal a file in the file manager, file associations (HKCU, no admin rights).

use std::path::Path;

/// Open the file manager with the file selected.
pub fn reveal(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        std::process::Command::new("xdg-open")
            .arg(dir)
            .spawn()
            .map(|_| ())
    }
}

pub fn open_folder(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
}

/// Extensions offered for association.
pub const ASSOC_EXTS: &[&str] = &[
    "sldprt", "sldasm", "step", "stp", "iges", "igs", "stl", "obj", "3ds", "3dxml", "easm", "rvt",
    "rfa", "ifc", "ifczip", "fbx", "gltf", "glb", "dae", "3mf", "ply", "off", "amf", "wrl", "x3d",
    "usdz", "dxf",
];

/// Extensions grouped for the settings page (every ASSOC_EXTS entry appears once).
pub const ASSOC_GROUPS: &[(&str, &[&str])] = &[
    ("SolidWorks / eDrawings", &["sldprt", "sldasm", "easm"]),
    ("STEP / IGES", &["step", "stp", "iges", "igs"]),
    ("CATIA", &["3dxml"]),
    ("BIM", &["ifc", "ifczip", "rvt", "rfa"]),
    (
        "Maillages",
        &[
            "stl", "obj", "3ds", "fbx", "gltf", "glb", "dae", "3mf", "ply", "off", "amf", "wrl",
            "x3d", "usdz",
        ],
    ),
    ("2D", &["dxf"]),
];

/// Register ModelCraft for `selected` extensions under HKCU\Software\Classes (per user,
/// reversible) and withdraw it from the other offered extensions.
/// Returns (registered, withdrawn).
#[cfg(windows)]
pub fn associate(selected: &[String]) -> Result<(usize, usize), String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.display().to_string();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    // create_subkey opens the key when it exists (always the case on real Windows)
    let (classes, _) = hkcu
        .create_subkey("Software\\Classes")
        .map_err(|e| e.to_string())?;
    let prog = crate::branding::PROG_ID;
    let (k, _) = classes.create_subkey(prog).map_err(|e| e.to_string())?;
    k.set_value("", &format!("{} 3D", crate::branding::NAME))
        .map_err(|e| e.to_string())?;
    let (icon, _) = k.create_subkey("DefaultIcon").map_err(|e| e.to_string())?;
    icon.set_value("", &format!("\"{exe}\",0"))
        .map_err(|e| e.to_string())?;
    let (cmd, _) = k
        .create_subkey("shell\\open\\command")
        .map_err(|e| e.to_string())?;
    cmd.set_value("", &format!("\"{exe}\" \"%1\""))
        .map_err(|e| e.to_string())?;
    let mut n = 0;
    let mut removed = 0;
    for ext in ASSOC_EXTS {
        if selected.iter().any(|s| s == ext) {
            let (e, _) = classes
                .create_subkey(format!(".{ext}\\OpenWithProgids"))
                .map_err(|e| e.to_string())?;
            e.set_value(prog, &"").map_err(|e| e.to_string())?;
            n += 1;
        } else if let Ok(e) = classes.open_subkey_with_flags(
            format!(".{ext}\\OpenWithProgids"),
            winreg::enums::KEY_ALL_ACCESS,
        ) {
            // only our own entry: other programs' associations are left alone
            if e.delete_value(prog).is_ok() {
                removed += 1;
            }
        }
    }
    // Applications\modelcraft.exe makes it appear in "Open with"
    let (app, _) = classes
        .create_subkey("Applications\\modelcraft.exe\\shell\\open\\command")
        .map_err(|e| e.to_string())?;
    app.set_value("", &format!("\"{exe}\" \"%1\""))
        .map_err(|e| e.to_string())?;
    notify_shell();
    Ok((n, removed))
}

#[cfg(windows)]
fn notify_shell() {
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHChangeNotify(
            wEventId: i32,
            uFlags: u32,
            dwItem1: *const std::ffi::c_void,
            dwItem2: *const std::ffi::c_void,
        );
    }
    // SHCNE_ASSOCCHANGED, SHCNF_IDLIST
    unsafe { SHChangeNotify(0x0800_0000, 0, std::ptr::null(), std::ptr::null()) };
}

#[cfg(not(windows))]
pub fn associate(_selected: &[String]) -> Result<(usize, usize), String> {
    Err("disponible uniquement sous Windows".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn groups_cover_every_extension_once() {
        let mut all: Vec<&str> = super::ASSOC_GROUPS
            .iter()
            .flat_map(|(_, e)| e.iter().copied())
            .collect();
        all.sort();
        let mut exts = super::ASSOC_EXTS.to_vec();
        exts.sort();
        assert_eq!(all, exts);
    }
}
