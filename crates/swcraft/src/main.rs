//! ModelCraft — the ultimate native 3D viewer (SolidWorks, STEP, IGES, STL, OBJ, 3DS, 3DXML).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod assembly;
mod branding;
mod camera;
mod chrome;
mod commands;
mod crash;

#[global_allocator]
static ALLOC: crash::LoggingAlloc = crash::LoggingAlloc;
mod diag;
mod dialogs;
mod document;
mod home;
mod i18n;
mod i18n_en;
mod icons;
mod measure;
mod palette;
mod platform;
mod render;
mod script;
mod settings;
mod theme;
mod viewer;
mod widgets;

use eframe::egui;
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!(
        "{} {} — visionneuse 3D\n\nUsage: modelcraft [fichiers…] [--lang fr|en] [--theme dark|light] [--screenshot out.png] [--view iso|front|top|right|back|bottom|left]\n       [--size LxH] [--no-edges] [--show settings|settings-display|about|shortcuts|formats|palette|measure|section|bbox|tree] [--ephemeral]\n       modelcraft --diag <fichier> [--diag-out rapport.txt]   (chargement sans fenêtre, journal détaillé)\n       modelcraft --associate   (associe les extensions 3D pour l'utilisateur courant, Windows)",
        branding::NAME,
        branding::VERSION
    );
    std::process::exit(0)
}

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    crash::install();
    // Headless diagnostic: modelcraft --diag <file> [--diag-out report.txt]
    {
        let a: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        if let Some(i) = a.iter().position(|x| x == "--diag") {
            let Some(file) = a.get(i + 1).map(PathBuf::from) else {
                eprintln!("--diag : fichier manquant");
                std::process::exit(2)
            };
            let out = a.iter().position(|x| x == "--diag-out").and_then(|j| a.get(j + 1)).map(PathBuf::from);
            std::process::exit(diag::run(&file, out));
        }
    }
    let mut opts = app::StartupOptions::default();
    let mut size = [1440.0f32, 900.0];
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        let next = |args: &mut std::iter::Skip<std::env::ArgsOs>| args.next().and_then(|s| s.into_string().ok());
        match a.to_str() {
            Some("-h") | Some("--help") => usage(),
            Some("--associate") => match platform::associate() {
                Ok(n) => {
                    println!("{n} extensions associées à {}", branding::NAME);
                    std::process::exit(0)
                }
                Err(e) => {
                    eprintln!("association impossible : {e}");
                    std::process::exit(1)
                }
            },
            Some("--screenshot") => {
                opts.screenshot = args.next().map(PathBuf::from);
                opts.ephemeral = true;
            }
            Some("--view") => opts.view = next(&mut args),
            Some("--no-edges") => opts.no_edges = true,
            Some("--ephemeral") => opts.ephemeral = true,
            Some("--show") => opts.show = next(&mut args),
            Some("--lang") => opts.lang = next(&mut args).map(|l| if l.starts_with("en") { i18n::Lang::En } else { i18n::Lang::Fr }),
            Some("--theme") => opts.theme = next(&mut args).map(|l| if l == "light" { theme::ThemeKind::Light } else { theme::ThemeKind::Dark }),
            Some("--size") => {
                if let Some(s) = next(&mut args) {
                    if let Some((w, h)) = s.split_once('x') {
                        size = [w.parse().unwrap_or(size[0]), h.parse().unwrap_or(size[1])];
                    }
                }
            }
            _ => opts.files.push(PathBuf::from(a)),
        }
    }
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(branding::NAME)
            .with_app_id("modelcraft")
            .with_inner_size(size)
            .with_min_inner_size([760.0, 480.0])
            .with_drag_and_drop(true)
            .with_icon(icon()),
        renderer: eframe::Renderer::Wgpu,
        centered: true,
        ..Default::default()
    };
    let res = eframe::run_native(branding::NAME, native, Box::new(move |cc| Ok(Box::new(app::App::new(cc, opts)))));
    if let Err(e) = &res {
        // GPU init failed with the default backend (broken DX12 driver, VM, Wine):
        // relaunch with the next backend in a fresh process.
        if matches!(e, eframe::Error::Wgpu(_)) {
            if let Some(code) = relaunch_with_next_backend() {
                std::process::exit(code);
            }
        }
    }
    res
}

fn relaunch_with_next_backend() -> Option<i32> {
    const ORDER: [&str; 3] = ["primary", "vulkan", "gl"];
    let current = std::env::var("WGPU_BACKEND").unwrap_or_else(|_| "primary".into());
    let idx = ORDER.iter().position(|b| *b == current)?;
    let next = ORDER.get(idx + 1)?;
    log::warn!("échec d'initialisation GPU avec '{current}', nouvel essai avec '{next}'");
    let exe = std::env::current_exe().ok()?;
    let status = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env("WGPU_BACKEND", next)
        .status()
        .ok()?;
    Some(status.code().unwrap_or(1))
}

fn icon() -> egui::IconData {
    let img = image::load_from_memory(branding::ICON_PNG).expect("icon").to_rgba8();
    let (w, h) = img.dimensions();
    egui::IconData { rgba: img.into_raw(), width: w, height: h }
}
