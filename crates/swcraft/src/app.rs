//! The application shell: tabs, menus, home, settings, command palette, dialogs.

use crate::document::Document;
use crate::i18n::{self, tr, trf};
use crate::measure::Tool;
use crate::render::Renderer;
use crate::settings::Settings;
use crate::theme::{self, ThemeKind, Tokens};
use crate::viewer;
use glam::Vec3;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Default, Clone)]
pub struct StartupOptions {
    pub files: Vec<PathBuf>,
    pub screenshot: Option<PathBuf>,
    pub view: Option<String>,
    pub no_edges: bool,
    pub lang: Option<i18n::Lang>,
    pub theme: Option<ThemeKind>,
    /// Open a window for the screenshot: settings, about, palette, menu, ctx (tree context menu), measure.
    pub show: Option<String>,
    /// Don't read or write the settings file (screenshots, tests).
    pub ephemeral: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsCat {
    General,
    Display,
    Navigation,
    Measure,
    Performance,
    Files,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dialog {
    Settings(SettingsCat),
    About,
    Shortcuts,
    Formats,
}

pub struct App {
    pub rs: egui_wgpu::RenderState,
    pub renderer: Renderer,
    pub tex: Option<egui::TextureId>,
    pub settings: Settings,
    pub docs: Vec<Document>,
    /// Index of the shown document; None = home.
    pub active: Option<usize>,
    pub dialog: Option<Dialog>,
    pub palette_open: bool,
    pub palette_query: String,
    pub toast: Option<(String, f64)>,
    pub startup: StartupOptions,
    pub applied: Option<(ThemeKind, theme::Accent)>,
    pub cmds: Vec<&'static str>,
    pub expanded_pins: std::collections::HashSet<String>,
    pub settings_dirty: bool,
    shot_frames: u32,
    shot_requested: bool,
    shot_done: Arc<AtomicBool>,
    shot_slot: Arc<std::sync::Mutex<Option<Arc<egui::ColorImage>>>>,
    pending_png: Option<PathBuf>,
    pub assoc_result: Option<String>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, startup: StartupOptions) -> App {
        let rs = cc.wgpu_render_state.clone().expect("wgpu backend required");
        // never let a GPU validation / out-of-memory error abort the app: log it and carry on
        rs.device.on_uncaptured_error(std::sync::Arc::new(|e: eframe::wgpu::Error| crate::crash::gpu_error(e.to_string())));
        egui_extras::install_image_loaders(&cc.egui_ctx);
        cc.egui_ctx.set_fonts(theme::font_definitions());
        let mut settings = if startup.ephemeral { Settings::default() } else { Settings::load() };
        if startup.no_edges {
            settings.edges = false;
        }
        if let Some(l) = startup.lang {
            settings.language = match l {
                i18n::Lang::Fr => i18n::LangPref::Fr,
                i18n::Lang::En => i18n::LangPref::En,
            };
        }
        if let Some(t) = startup.theme {
            settings.theme = match t {
                ThemeKind::Dark => theme::ThemePreference::Dark,
                ThemeKind::Light => theme::ThemePreference::Light,
            };
        }
        i18n::set_current(settings.language.resolve());
        let renderer = Renderer::new(&rs.device, settings.msaa);
        let mut app = App {
            rs,
            renderer,
            tex: None,
            settings,
            docs: Vec::new(),
            active: None,
            dialog: None,
            palette_open: false,
            palette_query: String::new(),
            toast: None,
            startup: startup.clone(),
            applied: None,
            cmds: Vec::new(),
            expanded_pins: Default::default(),
            settings_dirty: false,
            shot_frames: 0,
            shot_requested: false,
            shot_done: Arc::new(AtomicBool::new(false)),
            shot_slot: Default::default(),
            pending_png: None,
            assoc_result: None,
        };
        for f in &startup.files {
            app.open(f, &cc.egui_ctx);
        }
        match startup.show.as_deref() {
            Some("settings") => app.dialog = Some(Dialog::Settings(SettingsCat::General)),
            Some("settings-display") => app.dialog = Some(Dialog::Settings(SettingsCat::Display)),
            Some("about") => app.dialog = Some(Dialog::About),
            Some("shortcuts") => app.dialog = Some(Dialog::Shortcuts),
            Some("formats") => app.dialog = Some(Dialog::Formats),
            Some("palette") => app.palette_open = true,
            _ => {}
        }
        app
    }

    pub fn doc(&self) -> Option<&Document> {
        self.active.and_then(|i| self.docs.get(i))
    }
    pub fn doc_mut(&mut self) -> Option<&mut Document> {
        self.active.and_then(|i| self.docs.get_mut(i))
    }

    pub fn load_options(&self) -> craft_formats::Options {
        craft_formats::Options { edges: true, quality: self.settings.quality.tessellation(), ..Default::default() }
    }

    pub fn open(&mut self, path: &Path, ctx: &egui::Context) {
        // already open: switch to it
        if let Some(i) = self.docs.iter().position(|d| d.path == path) {
            self.active = Some(i);
            return;
        }
        let mut doc = Document::open(path, self.load_options(), ctx);
        doc.camera.ortho = self.settings.ortho;
        apply_default_view(&mut doc, self.settings.default_view);
        self.docs.push(doc);
        self.active = Some(self.docs.len() - 1);
        if !self.startup.ephemeral {
            self.settings.add_recent(path);
            self.settings_dirty = true;
        }
    }

    pub fn open_dialog(&mut self, ctx: &egui::Context) {
        let exts = craft_formats::open_extensions();
        let mut all: Vec<String> = exts.iter().map(|e| e.to_string()).collect();
        all.extend(exts.iter().map(|e| e.to_uppercase()));
        let mut d = rfd::FileDialog::new().set_title(tr("Ouvrir un modèle 3D")).add_filter(tr("Modèles 3D"), &all);
        for f in craft_formats::formats() {
            if f.support == craft_formats::Support::None && f.exts[0] != "x_t" {
                continue;
            }
            let mut e: Vec<String> = f.exts.iter().map(|e| e.to_string()).collect();
            e.extend(f.exts.iter().map(|x| x.to_uppercase()));
            d = d.add_filter(format!("{} (*.{})", tr_format_name(f.name), f.exts.join(", *.")), &e);
        }
        d = d.add_filter(tr("Tous les fichiers"), &["*"]);
        if let Some(dir) = self.doc().and_then(|d| d.path.parent().map(|p| p.to_path_buf())) {
            d = d.set_directory(dir);
        }
        if let Some(files) = d.pick_files() {
            for f in files {
                self.open(&f, ctx);
            }
        }
    }

    pub fn close(&mut self, i: usize) {
        if i >= self.docs.len() {
            return;
        }
        self.docs.remove(i);
        self.active = match self.active {
            Some(a) if a == i => {
                if self.docs.is_empty() {
                    None
                } else {
                    Some(i.min(self.docs.len() - 1))
                }
            }
            Some(a) if a > i => Some(a - 1),
            a => a,
        };
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), 0.0));
    }

    pub fn save_settings(&mut self) {
        if !self.startup.ephemeral {
            self.settings.save();
        }
        self.settings_dirty = false;
    }

    /// Run a command from the menus, keyboard or palette.
    pub fn run(&mut self, id: &str, ctx: &egui::Context) {
        let st = &mut self.settings;
        match id {
            "file.open" => self.open_dialog(ctx),
            "file.home" => self.active = None,
            "file.close" => {
                if let Some(i) = self.active {
                    self.close(i)
                }
            }
            "file.cancel_load" => {
                if let Some(i) = self.active {
                    if self.docs[i].is_loading() {
                        self.docs[i].cancel_load();
                        self.close(i);
                        self.toast(tr("Chargement annulé"));
                    }
                }
            }
            "file.reload" => {
                if let Some(i) = self.active {
                    let p = self.docs[i].path.clone();
                    let mut d = Document::open(&p, self.load_options(), ctx);
                    d.camera = self.docs[i].camera.clone();
                    d.needs_fit = false;
                    self.docs[i] = d;
                }
            }
            "file.reveal" => {
                if let Some(d) = self.doc() {
                    let _ = crate::platform::reveal(&d.path.clone());
                }
            }
            "file.copy_path" => {
                if let Some(d) = self.doc() {
                    ctx.copy_text(d.path.display().to_string());
                    self.toast(tr("Chemin copié"));
                }
            }
            "file.screenshot" => {
                if let Some(d) = self.doc() {
                    let name = format!("{}.png", d.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "capture".into()));
                    if let Some(out) = rfd::FileDialog::new().set_file_name(name).add_filter("PNG", &["png"]).save_file() {
                        self.pending_png = Some(out);
                        self.request_shot(ctx);
                    }
                }
            }
            "file.quit" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            "edit.settings" => self.dialog = Some(Dialog::Settings(SettingsCat::General)),
            "edit.hide" => {
                if let Some(d) = self.doc_mut() {
                    if let Some(i) = d.selected {
                        d.hide(i)
                    }
                }
            }
            "edit.isolate" => {
                if let Some(d) = self.doc_mut() {
                    if let Some(i) = d.selected {
                        d.isolate(i)
                    }
                }
            }
            "asm.move" => {
                if let Some(d) = self.doc_mut() {
                    d.gizmo = !d.gizmo;
                    d.drag = None;
                    if d.gizmo {
                        d.measure.tool = crate::measure::Tool::Select;
                        d.measure.pending = None;
                    }
                    let on = d.gizmo;
                    let none = d.selected.is_none();
                    self.toast(if !on {
                        tr("Déplacement terminé (le fichier n'est jamais modifié)")
                    } else if none {
                        tr("Déplacer : glissez une pièce, ou sélectionnez-la puis tirez une flèche")
                    } else {
                        tr("Déplacer : tirez une flèche, l'anneau ou le centre ; glissez une pièce pour la bouger librement")
                    });
                }
            }
            "asm.rotate" => {
                if let Some(d) = self.doc_mut() {
                    d.gizmo_rotate = !d.gizmo_rotate;
                    if d.gizmo_rotate {
                        d.gizmo = true;
                        d.measure.tool = crate::measure::Tool::Select;
                    }
                }
            }
            "asm.reset_sel" => {
                if let Some(d) = self.doc_mut() {
                    if let Some(i) = d.selected {
                        d.reset_position(Some(i));
                    }
                }
            }
            "asm.reset_all" => {
                if let Some(d) = self.doc_mut() {
                    d.reset_position(None);
                    d.asm.opacity.iter_mut().for_each(|o| *o = 1.0);
                    d.asm.global_opacity = 1.0;
                    d.asm.others_transparent = false;
                    d.instances_dirty = true;
                }
            }
            "asm.explode" => {
                if let Some(d) = self.doc_mut() {
                    d.asm.explode_goal = if d.asm.explode_goal > 0.0 { 0.0 } else { 1.0 };
                    ctx.request_repaint();
                }
            }
            "asm.explode_lines" => {
                if let Some(d) = self.doc_mut() {
                    d.asm.explode_lines = !d.asm.explode_lines;
                }
            }
            "asm.transparent" => {
                if let Some(d) = self.doc_mut() {
                    if let Some(i) = d.selected {
                        let o = d.asm.opacity.get(i).copied().unwrap_or(1.0);
                        d.set_opacity(Some(i), if o < 0.999 { 1.0 } else { 0.3 });
                    }
                }
            }
            "asm.others" => {
                if let Some(d) = self.doc_mut() {
                    d.asm.others_transparent = !d.asm.others_transparent;
                    d.instances_dirty = true;
                }
            }
            "asm.xray" => {
                if let Some(d) = self.doc_mut() {
                    let g = d.asm.global_opacity;
                    d.set_opacity(None, if g < 0.999 { 1.0 } else { 0.4 });
                }
            }
            "edit.show_all" => {
                if let Some(d) = self.doc_mut() {
                    d.show_all()
                }
            }
            "edit.deselect" => {
                if self.palette_open {
                    self.palette_open = false;
                } else if let Some(d) = self.doc_mut() {
                    if d.measure.pending.is_some() {
                        d.measure.pending = None;
                        d.measure.message = None;
                    } else if d.measure.tool != Tool::Select {
                        d.measure.tool = Tool::Select;
                    } else {
                        d.select(None);
                        d.face_sel = None;
                        d.highlight_dirty = true;
                    }
                }
            }
            "edit.copy_dims" => {
                if let Some(t) = self.doc().and_then(|d| viewer::dims_text(d, &self.settings)) {
                    ctx.copy_text(t.clone());
                    self.toast(trf("Copié : {t}", &[("t", &t)]));
                }
            }
            "view.fit" => {
                if let Some(d) = self.doc_mut() {
                    d.needs_fit = true
                }
            }
            "view.iso" | "view.front" | "view.top" | "view.right" | "view.back" | "view.bottom" | "view.left" => {
                let (y, p) = view_angles(&id[5..]);
                if let Some(d) = self.doc_mut() {
                    d.cam_goal = Some((y.to_radians(), p.to_radians()));
                }
            }
            "view.ortho" => {
                if let Some(d) = self.doc_mut() {
                    d.camera.ortho = !d.camera.ortho
                }
            }
            "view.edges" => {
                st.edges = !st.edges;
                self.settings_dirty = true;
            }
            "view.grid" => {
                st.show_grid = !st.show_grid;
                self.settings_dirty = true;
            }
            "view.axes" => {
                st.show_axes = !st.show_axes;
                self.settings_dirty = true;
            }
            "view.viewcube" => {
                st.show_viewcube = !st.show_viewcube;
                self.settings_dirty = true;
            }
            "view.tree" => {
                st.show_tree = !st.show_tree;
                self.settings_dirty = true;
            }
            "view.info" => {
                st.show_info = !st.show_info;
                self.settings_dirty = true;
            }
            "view.bbox" => {
                if let Some(d) = self.doc_mut() {
                    d.show_bbox = !d.show_bbox
                }
            }
            "view.section" => {
                if let Some(d) = self.doc_mut() {
                    viewer::toggle_section(d)
                }
            }
            "view.theme" => {
                let dark = Tokens::get(ctx).dark();
                st.theme = if dark { theme::ThemePreference::Light } else { theme::ThemePreference::Dark };
                self.settings_dirty = true;
            }
            "view.fullscreen" => {
                let fs = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fs));
            }
            "tool.select" | "tool.distance" | "tool.edge" | "tool.diameter" | "tool.angle" => {
                let tool = match id {
                    "tool.distance" => Tool::Distance,
                    "tool.edge" => Tool::Edge,
                    "tool.diameter" => Tool::Diameter,
                    "tool.angle" => Tool::Angle,
                    _ => Tool::Select,
                };
                if let Some(d) = self.doc_mut() {
                    d.measure.tool = if d.measure.tool == tool { Tool::Select } else { tool };
                    d.measure.pending = None;
                    d.measure.message = (d.measure.tool != Tool::Select).then(|| viewer::tool_hint(d.measure.tool).to_string());
                }
            }
            "tool.clear" => {
                if let Some(d) = self.doc_mut() {
                    d.measure.results.clear();
                    d.measure.pending = None;
                    d.measure.message = None;
                    d.face_sel = None;
                    d.highlight_dirty = true;
                    d.instances_dirty = true;
                }
            }
            "tool.assoc" => {
                let msg = match crate::platform::associate() {
                    Ok(n) => trf("{n} extensions associées à ModelCraft (clic droit › Ouvrir avec)", &[("n", &n.to_string())]),
                    Err(e) => trf("Association impossible : {e}", &[("e", &e)]),
                };
                self.assoc_result = Some(msg.clone());
                self.toast(msg);
            }
            "tool.settings_folder" => {
                if let Some(d) = crate::settings::config_dir() {
                    let _ = std::fs::create_dir_all(&d);
                    let _ = crate::platform::open_folder(&d);
                }
            }
            "help.palette" => {
                self.palette_open = !self.palette_open;
                self.palette_query.clear();
            }
            "help.shortcuts" => self.dialog = Some(Dialog::Shortcuts),
            "help.formats" => self.dialog = Some(Dialog::Formats),
            "help.about" => self.dialog = Some(Dialog::About),
            _ => log::warn!("commande inconnue {id}"),
        }
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let sys = ctx.input(|i| i.raw.system_theme).map(|t| t == egui::Theme::Dark);
        let kind = self.settings.theme.resolve(sys, ThemeKind::Dark);
        let want = (kind, self.settings.accent);
        if self.applied != Some(want) {
            theme::apply(ctx, kind, self.settings.accent);
            self.applied = Some(want);
        }
        i18n::set_current(self.settings.language.resolve());
        if self.renderer.samples != self.settings.msaa {
            self.renderer = Renderer::new(&self.rs.device, self.settings.msaa);
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() && !self.palette_open {
            return;
        }
        let mut fire = Vec::new();
        ctx.input_mut(|i| {
            for c in crate::commands::COMMANDS {
                if let Some(s) = c.shortcut {
                    // single keys only when no text field has focus (checked above) and the palette is closed
                    if self.palette_open && c.id != "help.palette" && c.id != "edit.deselect" {
                        continue;
                    }
                    if i.consume_shortcut(&s) {
                        fire.push(c.id);
                    }
                }
            }
        });
        for id in fire {
            self.run(id, ctx);
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        for p in dropped {
            if p.is_dir() {
                self.settings.pin(&p);
                self.settings_dirty = true;
                self.toast(tr("Dossier épinglé sur l'accueil"));
            } else {
                self.open(&p, ctx);
            }
        }
    }

    fn request_shot(&self, ctx: &egui::Context) {
        let slot = self.shot_slot.clone();
        let c = ctx.clone();
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
            *slot.lock().unwrap() = Some(img);
            c.request_repaint();
        })));
    }

    fn handle_screenshots(&mut self, ctx: &egui::Context) {
        if self.pending_png.is_none() {
            return;
        }
        let Some(img) = self.shot_slot.lock().unwrap().take() else { return };
        let out = self.pending_png.take().unwrap();
        let crop = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("viewport-rect")));
        let img = match crop {
            Some(r) => img.region(&r, Some(ctx.pixels_per_point())),
            None => (*img).clone(),
        };
        match save_color_image(&img, &out) {
            Ok(()) => self.toast(trf("Capture enregistrée : {p}", &[("p", &out.display().to_string())])),
            Err(e) => self.toast(trf("Échec de la capture : {e}", &[("e", &e)])),
        }
    }

    fn screenshot_logic(&mut self, ctx: &egui::Context) {
        let Some(out) = self.startup.screenshot.clone() else { return };
        if self.shot_done.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if self.docs.iter().any(|d| d.is_loading()) {
            ctx.request_repaint_after(Duration::from_millis(30));
            return;
        }
        self.shot_frames += 1;
        if self.shot_frames == 3 {
            if let Some(v) = self.startup.view.clone() {
                let (y, p) = view_angles(&v);
                if let Some(d) = self.doc_mut() {
                    d.camera.set_view(y, p);
                    d.needs_fit = true;
                }
            }
            self.scripted_scene(ctx);
        }
        if self.shot_frames == 24 && !self.shot_requested {
            self.shot_requested = true;
            self.request_shot(ctx);
        }
        if self.shot_requested {
            if let Some(img) = self.shot_slot.lock().unwrap().take() {
                match save_color_image(&img, &out) {
                    Ok(()) => eprintln!("screenshot saved: {}", out.display()),
                    Err(e) => eprintln!("screenshot failed: {e}"),
                }
                self.shot_done.store(true, Ordering::SeqCst);
            }
        }
        ctx.request_repaint_after(Duration::from_millis(16));
    }

    /// Scripted states for documentation screenshots (`--show`).
    fn scripted_scene(&mut self, ctx: &egui::Context) {
        let show = self.startup.show.clone().unwrap_or_default();
        let st = self.settings.clone();
        let Some(d) = self.doc_mut() else { return };
        let _ = ctx;
        match show.as_str() {
            "measure" => {
                // pick a hole if there is one: probe the model with rays and keep a cylinder hit
                crate::script::demo_measures(d, &st);
                d.measure.tool = crate::measure::Tool::Distance;
            }
            "ctx" | "tree" => {
                if let Some(s) = &d.scene {
                    let i = (1..s.components.len()).find(|&i| s.components[i].mesh.is_some()).unwrap_or(0);
                    d.select(Some(i));
                }
            }
            "section" => {
                viewer::toggle_section(d);
            }
            "bbox" => d.show_bbox = true,
            "explode" => {
                d.asm.explode_goal = 1.0;
                d.asm.explode = 1.0;
                d.apply_transforms();
                d.needs_fit = true;
            }
            "transparent" | "others" => {
                // the biggest part becomes see-through (or everything but it is faded)
                if let Some(s) = &d.scene {
                    let big = (0..s.components.len())
                        .filter(|&i| s.components[i].mesh.is_some())
                        .max_by(|&a, &b| {
                            let sz = |i: usize| {
                                let m = &s.meshes[s.components[i].mesh.unwrap()].mesh;
                                (Vec3::from(m.bbox_max) - Vec3::from(m.bbox_min)).length()
                            };
                            sz(a).partial_cmp(&sz(b)).unwrap_or(std::cmp::Ordering::Equal)
                        });
                    if let Some(i) = big {
                        if show == "others" {
                            let small = (0..s.components.len()).filter(|&k| s.components[k].mesh.is_some() && k != i).last().unwrap_or(i);
                            d.select(Some(small));
                            d.asm.others_transparent = true;
                            d.instances_dirty = true;
                        } else {
                            d.set_opacity(Some(i), 0.28);
                        }
                    }
                }
            }
            "move" => {
                if let Some(s) = &d.scene {
                    let i = (1..s.components.len()).filter(|&i| s.components[i].mesh.is_some()).last().unwrap_or(0);
                    d.select(Some(i));
                    d.gizmo = true;
                    d.gizmo_rotate = true;
                    if let Some(c) = d.component_center(i) {
                        let r = d.stats.bbox.map(|(lo, hi)| (hi - lo).length()).unwrap_or(1.0);
                        d.move_component(i, glam::Mat4::from_translation(Vec3::new(0.0, r * 0.25, r * 0.1)));
                        let _ = c;
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn view_angles(v: &str) -> (f32, f32) {
    match v {
        "front" => (0.0, 0.0),
        "back" => (180.0, 0.0),
        "top" => (0.0, 90.0),
        "bottom" => (0.0, -90.0),
        "right" => (90.0, 0.0),
        "left" => (-90.0, 0.0),
        _ => (45.0, 35.264),
    }
}

fn apply_default_view(doc: &mut Document, v: crate::settings::DefaultView) {
    use crate::settings::DefaultView::*;
    let name = match v {
        Iso => "iso",
        Front => "front",
        Top => "top",
        Right => "right",
    };
    let (y, p) = view_angles(name);
    doc.camera.set_view(y, p);
}

pub fn tr_format_name(n: &'static str) -> &'static str {
    tr(n)
}

pub fn save_color_image(img: &egui::ColorImage, out: &Path) -> Result<(), String> {
    let [w, h] = img.size;
    let mut buf = Vec::with_capacity(w * h * 4);
    for c in &img.pixels {
        buf.extend_from_slice(&c.to_srgba_unmultiplied());
    }
    image::save_buffer(out, &buf, w as u32, h as u32, image::ExtendedColorType::Rgba8).map_err(|e| e.to_string())
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);
        // background loading
        let mut arrived = false;
        for d in &mut self.docs {
            arrived |= d.poll(&self.rs.device, &ctx);
        }
        if arrived {
            ctx.request_repaint();
        }
        // explosion animation of the visible document
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
        if let Some(i) = self.active {
            if let Some(d) = self.docs.get_mut(i) {
                if d.tick(dt) {
                    ctx.request_repaint();
                }
            }
        }
        self.handle_drops(&ctx);
        self.shortcuts(&ctx);
        self.handle_screenshots(&ctx);

        crate::chrome::tab_strip(self, ui);
        crate::chrome::mode_bar(self, ui);
        crate::chrome::status_bar(self, ui);
        match self.active {
            Some(i) if i < self.docs.len() => crate::chrome::document_view(self, ui, i),
            _ => crate::home::show(self, ui),
        }
        crate::dialogs::show(self, &ctx);
        crate::palette::show(self, &ctx);
        crate::chrome::toast(self, &ctx);
        crate::chrome::drop_overlay(&ctx);

        let cmds = std::mem::take(&mut self.cmds);
        for c in cmds {
            self.run(c, &ctx);
        }
        // title
        let title = match self.doc() {
            Some(d) => format!("{} — {}", d.title, crate::branding::NAME),
            None => crate::branding::NAME.to_string(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        if self.settings_dirty {
            self.save_settings();
        }
        self.screenshot_logic(&ctx);
        if self.docs.iter().any(|d| d.is_loading()) {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}
