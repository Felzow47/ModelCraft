//! SW Craft — fast SolidWorks part/assembly viewer.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod camera;
mod render;

use eframe::egui;
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!(
        "SW Craft {} — visionneuse SolidWorks\n\nUsage: swcraft [fichier.SLDPRT|fichier.SLDASM] [--screenshot out.png] [--view iso|front|top|right] [--size LxH] [--no-edges]",
        env!("CARGO_PKG_VERSION")
    );
    std::process::exit(0)
}

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let mut opts = app::StartupOptions::default();
    let mut size = [1400.0f32, 900.0];
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        match a.to_str() {
            Some("-h") | Some("--help") => usage(),
            Some("--screenshot") => opts.screenshot = args.next().map(PathBuf::from),
            Some("--view") => opts.view = args.next().and_then(|s| s.into_string().ok()),
            Some("--no-edges") => opts.no_edges = true,
            Some("--size") => {
                if let Some(s) = args.next().and_then(|s| s.into_string().ok()) {
                    if let Some((w, h)) = s.split_once('x') {
                        size = [w.parse().unwrap_or(size[0]), h.parse().unwrap_or(size[1])];
                    }
                }
            }
            _ => opts.file = Some(PathBuf::from(a)),
        }
    }
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("SW Craft")
            .with_inner_size(size)
            .with_min_inner_size([640.0, 420.0])
            .with_drag_and_drop(true)
            .with_icon(icon()),
        renderer: eframe::Renderer::Wgpu,
        centered: true,
        ..Default::default()
    };
    let res = eframe::run_native("SW Craft", native, Box::new(move |cc| Ok(Box::new(app::SwCraftApp::new(cc, opts)))));
    if let Err(e) = &res {
        // GPU init failed with the default backend (e.g. broken DX12 driver, VM, Wine):
        // relaunch ourselves with the next backend. Each attempt is a fresh process
        // because a window event loop can only be created once per process.
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

/// Small procedurally generated app icon (blue diamond).
fn icon() -> egui::IconData {
    let n = 64u32;
    let mut rgba = vec![0u8; (n * n * 4) as usize];
    for y in 0..n {
        for x in 0..n {
            let dx = (x as f32 - 31.5).abs();
            let dy = (y as f32 - 31.5).abs();
            let d = dx + dy;
            let i = ((y * n + x) * 4) as usize;
            if d < 28.0 {
                let t = 1.0 - y as f32 / n as f32;
                rgba[i] = (70.0 + 40.0 * t) as u8;
                rgba[i + 1] = (130.0 + 50.0 * t) as u8;
                rgba[i + 2] = 255;
                rgba[i + 3] = 255;
            }
        }
    }
    egui::IconData { rgba, width: n, height: n }
}
