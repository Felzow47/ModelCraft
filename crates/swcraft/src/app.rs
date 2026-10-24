//! UI (egui) for SW Craft.

use crate::camera::Camera;
use crate::render::{Renderer, ViewSettings};
use eframe::egui::{self, Color32, CornerRadius, RichText, Sense, Stroke, Vec2};
use eframe::{egui_wgpu, wgpu};
use glam::{Vec2 as GVec2, Vec3};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use sw_format::{DocKind, LoadOptions, Scene};

pub const ACCENT: Color32 = Color32::from_rgb(86, 156, 255);
const PANEL_BG: Color32 = Color32::from_rgb(24, 26, 31);
const BAR_BG: Color32 = Color32::from_rgb(30, 33, 39);
const TEXT_DIM: Color32 = Color32::from_rgb(140, 148, 160);
pub const DEFAULT_BASE: [f32; 3] = [0.66, 0.72, 0.82];

#[derive(Default, Clone)]
pub struct StartupOptions {
    pub file: Option<PathBuf>,
    pub screenshot: Option<PathBuf>,
    pub view: Option<String>,
    pub no_edges: bool,
}

struct Loaded {
    scene: Scene,
    /// user visibility toggle per component
    user_visible: Vec<bool>,
    expanded: Vec<bool>,
    has_children: Vec<bool>,
    preview_tex: Option<egui::TextureHandle>,
    gpu_upload: Duration,
    first_frame: Option<Duration>,
    open_started: Instant,
}

enum LoadState {
    Idle,
    Loading { path: PathBuf, rx: mpsc::Receiver<Result<Scene, String>>, started: Instant },
    Failed { path: PathBuf, error: String },
}

pub struct SwCraftApp {
    rs: egui_wgpu::RenderState,
    renderer: Renderer,
    tex_id: Option<egui::TextureId>,
    cam: Camera,
    edges: bool,
    show_tree: bool,
    base_color: [f32; 3],
    loaded: Option<Loaded>,
    state: LoadState,
    selected: Option<usize>,
    hovered: Option<usize>,
    instances_dirty: bool,
    frame_dirty: bool,
    viewport_px: [u32; 2],
    want_fit: bool,
    startup: StartupOptions,
    shot_frames: u32,
    shot_requested: bool,
    shot_done: Arc<AtomicBool>,
}

impl SwCraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>, startup: StartupOptions) -> Self {
        let rs = cc.wgpu_render_state.clone().expect("wgpu backend required");
        setup_style(&cc.egui_ctx);
        let renderer = Renderer::new(&rs.device);
        let mut app = SwCraftApp {
            rs,
            renderer,
            tex_id: None,
            cam: Camera::default(),
            edges: !startup.no_edges,
            show_tree: true,
            base_color: DEFAULT_BASE,
            loaded: None,
            state: LoadState::Idle,
            selected: None,
            hovered: None,
            instances_dirty: false,
            frame_dirty: true,
            viewport_px: [0, 0],
            want_fit: false,
            startup: startup.clone(),
            shot_frames: 0,
            shot_requested: false,
            shot_done: Arc::new(AtomicBool::new(false)),
        };
        if let Some(f) = startup.file {
            app.open(f, &cc.egui_ctx);
        }
        app
    }

    fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let p = path.clone();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(|| sw_format::load_scene(&p, &LoadOptions::default()));
            let r = match r {
                Ok(Ok(s)) => Ok(s),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err("erreur interne pendant la lecture".into()),
            };
            let _ = tx.send(r);
            ctx2.request_repaint();
        });
        self.state = LoadState::Loading { path, rx, started: Instant::now() };
    }

    fn poll_loading(&mut self, ctx: &egui::Context) {
        let LoadState::Loading { rx, path, started } = &self.state else { return };
        match rx.try_recv() {
            Ok(Ok(scene)) => {
                let started = *started;
                let t = Instant::now();
                self.renderer.upload_scene(&self.rs.device, &scene);
                let n = scene.components.len();
                let user_visible: Vec<bool> = scene.components.iter().map(|c| !c.suppressed && !c.hidden).collect();
                let mut has_children = vec![false; n];
                for c in &scene.components {
                    if let Some(p) = c.parent {
                        has_children[p] = true;
                    }
                }
                let preview_tex = scene.preview.as_ref().map(|img| {
                    let ci = egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.pixels);
                    ctx.load_texture("preview", ci, egui::TextureOptions::LINEAR)
                });
                let title = format!(
                    "{} — SW Craft",
                    scene.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
                );
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
                self.loaded = Some(Loaded {
                    expanded: has_children.iter().map(|_| true).collect(),
                    has_children,
                    user_visible,
                    preview_tex,
                    gpu_upload: t.elapsed(),
                    first_frame: None,
                    open_started: started,
                    scene,
                });
                self.selected = None;
                self.instances_dirty = true;
                self.want_fit = true;
                self.cam.set_iso();
                if let Some(v) = self.startup.view.clone() {
                    self.apply_view(&v);
                }
                self.state = LoadState::Idle;
            }
            Ok(Err(e)) => {
                self.state = LoadState::Failed { path: path.clone(), error: e };
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(30)),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.state = LoadState::Failed { path: path.clone(), error: "lecture interrompue".into() };
            }
        }
    }

    fn apply_view(&mut self, v: &str) {
        match v {
            "front" | "face" => self.cam.set_view(0.0, 0.0),
            "back" | "arriere" => self.cam.set_view(180.0, 0.0),
            "top" | "dessus" => self.cam.set_view(0.0, 90.0),
            "bottom" | "dessous" => self.cam.set_view(0.0, -90.0),
            "right" | "droite" => self.cam.set_view(90.0, 0.0),
            "left" | "gauche" => self.cam.set_view(-90.0, 0.0),
            _ => self.cam.set_iso(),
        }
        self.want_fit = true;
        self.frame_dirty = true;
    }

    fn effective_visibility(&self) -> Vec<bool> {
        let Some(l) = &self.loaded else { return Vec::new() };
        let mut eff = vec![true; l.scene.components.len()];
        for (i, c) in l.scene.components.iter().enumerate() {
            let parent_ok = c.parent.map(|p| eff[p]).unwrap_or(true);
            eff[i] = parent_ok && l.user_visible[i];
        }
        eff
    }

    fn subtree_contains(&self, root: usize, i: usize) -> bool {
        let Some(l) = &self.loaded else { return false };
        let mut cur = Some(i);
        while let Some(c) = cur {
            if c == root {
                return true;
            }
            cur = l.scene.components[c].parent;
        }
        false
    }

    fn sync_instances(&mut self) {
        if !self.instances_dirty {
            return;
        }
        self.instances_dirty = false;
        let vis = self.effective_visibility();
        let Some(l) = &self.loaded else { return };
        let tints: Vec<[f32; 4]> = (0..l.scene.components.len())
            .map(|i| {
                if self.selected.is_some_and(|s| self.subtree_contains(s, i)) {
                    [0.32, 0.58, 1.0, 0.7]
                } else if self.hovered.is_some_and(|h| self.subtree_contains(h, i)) {
                    [0.55, 0.75, 1.0, 0.35]
                } else {
                    [0.0; 4]
                }
            })
            .collect();
        self.renderer.update_instances(&self.rs.device, &l.scene, &vis, &tints);
        self.frame_dirty = true;
    }

    fn fit(&mut self) {
        let aspect = self.viewport_px[0].max(1) as f32 / self.viewport_px[1].max(1) as f32;
        let Some(l) = &self.loaded else { return };
        // fit to visible components
        let vis = self.effective_visibility();
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for (i, c) in l.scene.components.iter().enumerate() {
            let Some(mi) = c.mesh else { continue };
            if !vis[i] {
                continue;
            }
            let m = &l.scene.meshes[mi].mesh;
            if m.is_empty() {
                continue;
            }
            let w = glam::Mat4::from_cols_array(&c.world);
            for k in 0..8 {
                let p = Vec3::new(
                    if k & 1 == 0 { m.bbox_min[0] } else { m.bbox_max[0] },
                    if k & 2 == 0 { m.bbox_min[1] } else { m.bbox_max[1] },
                    if k & 4 == 0 { m.bbox_min[2] } else { m.bbox_max[2] },
                );
                let q = w.transform_point3(p);
                lo = lo.min(q);
                hi = hi.max(q);
            }
        }
        if lo.x.is_finite() {
            self.cam.fit(lo, hi, aspect);
            self.frame_dirty = true;
        }
    }

    // ---------------------------------------------------------------- UI pieces

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top")
            .exact_size(44.0)
            .frame(egui::Frame::new().fill(BAR_BG).inner_margin(egui::Margin::symmetric(12, 6)))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                    diamond(ui.painter(), r.center(), 8.0, ACCENT);
                    ui.label(RichText::new("SW Craft").strong().size(16.0).color(Color32::from_gray(235)));
                    ui.add_space(14.0);
                    if ui.add(egui::Button::new(RichText::new("Ouvrir…").size(14.0))).on_hover_text("Ctrl+O").clicked() {
                        self.open_dialog(ui.ctx());
                    }
                    if let Some(l) = &self.loaded {
                        ui.add_space(8.0);
                        let name = l.scene.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        ui.label(RichText::new(name).color(Color32::from_gray(220)).size(14.0))
                            .on_hover_text(l.scene.path.display().to_string());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let has = self.loaded.as_ref().is_some_and(|l| l.scene.has_geometry());
                        ui.add_enabled_ui(has, |ui| {
                            if toggle(ui, "Arbre", self.show_tree).on_hover_text("Afficher l'arbre des composants (T)").clicked() {
                                self.show_tree = !self.show_tree;
                            }
                            if toggle(ui, "Arêtes", self.edges).on_hover_text("Afficher les arêtes (E)").clicked() {
                                self.edges = !self.edges;
                                self.frame_dirty = true;
                            }
                            if toggle(ui, "Ortho", self.cam.ortho).on_hover_text("Projection orthographique (O)").clicked() {
                                self.cam.ortho = !self.cam.ortho;
                                self.frame_dirty = true;
                            }
                            ui.separator();
                            for (label, v, tip) in [
                                ("Droite", "right", "Vue de droite (3)"),
                                ("Dessus", "top", "Vue de dessus (2)"),
                                ("Face", "front", "Vue de face (1)"),
                                ("Iso", "iso", "Vue isométrique (0)"),
                            ] {
                                if ui.button(label).on_hover_text(tip).clicked() {
                                    self.apply_view(v);
                                }
                            }
                            if ui.button("Ajuster").on_hover_text("Ajuster à la vue (F, double-clic)").clicked() {
                                self.want_fit = true;
                            }
                        });
                    });
                });
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status")
            .exact_size(26.0)
            .frame(egui::Frame::new().fill(BAR_BG).inner_margin(egui::Margin::symmetric(12, 4)))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let dim = |s: String| RichText::new(s).color(TEXT_DIM).size(12.0);
                    match (&self.state, &self.loaded) {
                        (LoadState::Loading { path, started, .. }, _) => {
                            ui.spinner();
                            ui.label(dim(format!(
                                "Lecture de {}… {:.1} s",
                                path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                                started.elapsed().as_secs_f32()
                            )));
                        }
                        (LoadState::Failed { error, .. }, _) => {
                            ui.label(RichText::new(format!("Erreur : {error}")).color(Color32::from_rgb(255, 140, 120)).size(12.0));
                        }
                        (_, Some(l)) => {
                            let s = &l.scene;
                            let kind = match s.kind {
                                DocKind::Part => "Pièce",
                                DocKind::Assembly => "Assemblage",
                            };
                            let faces: usize = s.meshes.iter().map(|m| m.mesh.face_count).sum();
                            let t = &s.timings;
                            ui.label(dim(format!(
                                "{kind}  ·  {}  ·  {} faces  ·  {} triangles  ·  lecture {:.1} ms  ·  GPU {:.1} ms{}",
                                s.container.label(),
                                fmt_int(faces),
                                fmt_int(s.drawn_triangles()),
                                t.total.as_secs_f64() * 1000.0,
                                l.gpu_upload.as_secs_f64() * 1000.0,
                                l.first_frame.map(|d| format!("  ·  1re image {:.0} ms", d.as_secs_f64() * 1000.0)).unwrap_or_default()
                            )));
                            if !s.warnings.is_empty() {
                                ui.label(RichText::new(format!("{} avertissement(s)", s.warnings.len())).color(Color32::from_rgb(255, 196, 100)).size(12.0))
                                    .on_hover_text(s.warnings.join("\n"));
                            }
                        }
                        _ => {
                            ui.label(dim("Prêt  ·  glissez un fichier .SLDPRT ou .SLDASM dans la fenêtre".into()));
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(dim(format!("v{}", env!("CARGO_PKG_VERSION"))));
                    });
                });
            });
    }

    fn tree_panel(&mut self, ui: &mut egui::Ui) {
        let Some(l) = &self.loaded else { return };
        if !self.show_tree || !l.scene.has_geometry() {
            return;
        }
        let mut toggle_vis: Option<usize> = None;
        let mut toggle_exp: Option<usize> = None;
        let mut select: Option<Option<usize>> = None;
        let mut hover: Option<usize> = None;
        let mut set_all: Option<bool> = None;
        egui::Panel::left("tree")
            .resizable(true)
            .default_size(270.0)
            .min_size(180.0)
            .frame(egui::Frame::new().fill(PANEL_BG).inner_margin(egui::Margin::same(10)))
            .show(ui, |ui| {
                let s = &l.scene;
                ui.label(RichText::new(if s.kind == DocKind::Assembly { "COMPOSANTS" } else { "PIÈCE" }).size(11.0).color(TEXT_DIM).strong());
                if s.components.len() > 1 {
                    ui.horizontal(|ui| {
                        if ui.small_button("Tout afficher").clicked() {
                            set_all = Some(true);
                        }
                        if ui.small_button("Tout masquer").clicked() {
                            set_all = Some(false);
                        }
                    });
                }
                ui.add_space(4.0);
                let avail = ui.available_height();
                egui::ScrollArea::vertical().max_height((avail - 190.0).max(120.0)).auto_shrink([false, true]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let mut hidden_below: Option<usize> = None; // depth of collapsed ancestor
                    for (i, c) in s.components.iter().enumerate() {
                        if let Some(d) = hidden_below {
                            if c.depth > d {
                                continue;
                            }
                            hidden_below = None;
                        }
                        if l.has_children[i] && !l.expanded[i] {
                            hidden_below = Some(c.depth);
                        }
                        let selected = self.selected == Some(i);
                        let row_h = 22.0;
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click());
                        let resp = resp.on_hover_text(if c.path.is_empty() { c.name.clone() } else { c.path.clone() });
                        if resp.hovered() {
                            hover = Some(i);
                        }
                        let bg = if selected {
                            ACCENT.gamma_multiply(0.35)
                        } else if resp.hovered() {
                            Color32::from_white_alpha(10)
                        } else {
                            Color32::TRANSPARENT
                        };
                        ui.painter().rect_filled(rect, CornerRadius::same(4), bg);
                        let mut x = rect.left() + 4.0 + c.depth as f32 * 14.0;
                        let cy = rect.center().y;
                        // expander
                        if l.has_children[i] {
                            let r = egui::Rect::from_center_size(egui::pos2(x + 6.0, cy), Vec2::splat(14.0));
                            let er = ui.interact(r, ui.scope_id().with(("exp", i)), Sense::click());
                            chevron(ui.painter(), r.center(), l.expanded[i], if er.hovered() { Color32::WHITE } else { TEXT_DIM });
                            if er.clicked() {
                                toggle_exp = Some(i);
                            }
                        }
                        x += 16.0;
                        // eye
                        let vis = l.user_visible[i];
                        let r = egui::Rect::from_center_size(egui::pos2(x + 8.0, cy), Vec2::splat(16.0));
                        let vr = ui.interact(r, ui.scope_id().with(("vis", i)), Sense::click());
                        eye(ui.painter(), r.center(), vis, vr.hovered());
                        if vr.clicked() {
                            toggle_vis = Some(i);
                        }
                        x += 22.0;
                        let col = if c.suppressed {
                            Color32::from_gray(100)
                        } else if c.mesh.is_none() && !c.is_assembly {
                            Color32::from_rgb(220, 160, 90)
                        } else if vis {
                            Color32::from_gray(225)
                        } else {
                            Color32::from_gray(120)
                        };
                        comp_icon(ui.painter(), egui::pos2(x + 5.0, cy), c.is_assembly, c.mesh.is_some());
                        let text = if c.suppressed { format!("{} (supprimé)", c.name) } else { c.name.clone() };
                        ui.painter().with_clip_rect(rect).text(
                            egui::pos2(x + 16.0, cy),
                            egui::Align2::LEFT_CENTER,
                            text,
                            egui::FontId::proportional(13.0),
                            col,
                        );
                        if resp.clicked() && !vr.clicked() {
                            select = Some(if selected { None } else { Some(i) });
                        }
                    }
                });
                ui.add_space(8.0);
                ui.separator();
                info_block(ui, l, self.selected);
            });
        if let Some(i) = toggle_exp {
            if let Some(l) = &mut self.loaded {
                l.expanded[i] = !l.expanded[i];
            }
        }
        if let Some(i) = toggle_vis {
            if let Some(l) = &mut self.loaded {
                l.user_visible[i] = !l.user_visible[i];
            }
            self.instances_dirty = true;
        }
        if let Some(v) = set_all {
            if let Some(l) = &mut self.loaded {
                for (k, c) in l.scene.components.iter().enumerate() {
                    l.user_visible[k] = v && !c.suppressed;
                }
            }
            self.instances_dirty = true;
        }
        if let Some(s) = select {
            self.selected = s;
            self.instances_dirty = true;
        }
        if hover != self.hovered {
            self.hovered = hover;
            self.instances_dirty = true;
        }
    }

    fn open_dialog(&mut self, ctx: &egui::Context) {
        if let Some(p) = rfd::FileDialog::new()
            .set_title("Ouvrir un fichier SolidWorks")
            .add_filter("SolidWorks (*.sldprt, *.sldasm)", &["sldprt", "SLDPRT", "sldasm", "SLDASM"])
            .add_filter("Tous les fichiers", &["*"])
            .pick_file()
        {
            self.open(p, ctx);
        }
    }

    fn viewport(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let rect = ui.max_rect();
            let ppp = ui.ctx().pixels_per_point();
            let px = [(rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32];
            let geometry = self.loaded.as_ref().is_some_and(|l| l.scene.has_geometry());

            if !geometry {
                self.empty_or_preview(ui, rect);
                return;
            }

            if px != self.viewport_px {
                self.viewport_px = px;
                self.frame_dirty = true;
                if self.renderer.ensure_target(&self.rs.device, px) {
                    let view = &self.renderer.target().unwrap().resolve_view;
                    let mut r = self.rs.renderer.write();
                    match self.tex_id {
                        Some(id) => r.update_egui_texture_from_wgpu_texture(&self.rs.device, view, wgpu::FilterMode::Linear, id),
                        None => self.tex_id = Some(r.register_native_texture(&self.rs.device, view, wgpu::FilterMode::Linear)),
                    }
                }
            }
            if self.want_fit {
                self.want_fit = false;
                self.fit();
            }

            let resp = ui.allocate_rect(rect, Sense::click_and_drag());
            let aspect = px[0] as f32 / px[1] as f32;
            // --- input
            let (scroll, zoom2, pointer) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta(), i.pointer.hover_pos()));
            let primary = ui.input(|i| i.pointer.primary_down());
            let shift = ui.input(|i| i.modifiers.shift || i.modifiers.command);
            if resp.dragged() {
                let d = resp.drag_delta();
                let pan = resp.dragged_by(egui::PointerButton::Secondary)
                    || resp.dragged_by(egui::PointerButton::Middle)
                    || (primary && shift);
                if pan {
                    self.cam.pan(GVec2::new(d.x, d.y), rect.height());
                } else {
                    self.cam.orbit(GVec2::new(d.x, d.y));
                }
                self.frame_dirty = true;
            }
            if resp.hovered() {
                let mut factor = 1.0f32;
                if scroll != 0.0 {
                    factor *= (-scroll * 0.0025).exp();
                }
                if zoom2 != 1.0 {
                    factor /= zoom2;
                }
                if factor != 1.0 {
                    let ndc = pointer
                        .map(|p| GVec2::new((p.x - rect.center().x) / (rect.width() * 0.5), -(p.y - rect.center().y) / (rect.height() * 0.5)))
                        .unwrap_or(GVec2::ZERO);
                    self.cam.zoom(factor, ndc, aspect);
                    self.frame_dirty = true;
                }
            }
            if resp.double_clicked() {
                self.fit();
            }
            if resp.clicked() && !resp.dragged() {
                // click on empty space clears selection
                if self.selected.is_some() {
                    self.selected = None;
                    self.instances_dirty = true;
                }
            }

            self.sync_instances();
            if self.frame_dirty {
                self.frame_dirty = false;
                let vs = ViewSettings {
                    view_proj: self.cam.proj(aspect) * self.cam.view(),
                    view: self.cam.view(),
                    ortho: self.cam.ortho,
                    edges: self.edges,
                    base_color: self.base_color,
                };
                self.renderer.render(&self.rs.device, &self.rs.queue, &vs);
                if let Some(l) = &mut self.loaded {
                    if l.first_frame.is_none() {
                        l.first_frame = Some(l.open_started.elapsed());
                    }
                }
            }
            if let Some(id) = self.tex_id {
                ui.painter().image(id, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            }
            self.draw_triad(ui, rect);
            ui.painter().text(
                rect.right_bottom() + Vec2::new(-12.0, -10.0),
                egui::Align2::RIGHT_BOTTOM,
                "Gauche : tourner  ·  Droit/milieu : déplacer  ·  Molette : zoom  ·  Double-clic : ajuster",
                egui::FontId::proportional(11.0),
                Color32::from_white_alpha(70),
            );
        });
    }

    fn draw_triad(&self, ui: &egui::Ui, rect: egui::Rect) {
        let c = rect.left_bottom() + Vec2::new(46.0, -46.0);
        let view = self.cam.view();
        let p = ui.painter();
        p.circle_filled(c, 34.0, Color32::from_black_alpha(60));
        let mut axes = [
            (Vec3::X, "X", Color32::from_rgb(235, 90, 90)),
            (Vec3::Y, "Y", Color32::from_rgb(110, 210, 110)),
            (Vec3::Z, "Z", Color32::from_rgb(100, 150, 255)),
        ];
        axes.sort_by(|a, b| view.transform_vector3(a.0).z.partial_cmp(&view.transform_vector3(b.0).z).unwrap());
        for (a, name, col) in axes {
            let v = view.transform_vector3(a);
            let end = c + Vec2::new(v.x, -v.y) * 26.0;
            p.line_segment([c, end], Stroke::new(2.0, col));
            p.text(end + Vec2::new(v.x, -v.y) * 7.0, egui::Align2::CENTER_CENTER, name, egui::FontId::proportional(11.0), col);
        }
    }

    fn empty_or_preview(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let p = ui.painter();
        // background gradient
        let top = Color32::from_rgb(52, 58, 68);
        let bot = Color32::from_rgb(22, 24, 29);
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(rect.left_top(), top);
        mesh.colored_vertex(rect.right_top(), top);
        mesh.colored_vertex(rect.right_bottom(), bot);
        mesh.colored_vertex(rect.left_bottom(), bot);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        p.add(egui::Shape::mesh(mesh));

        let mut open_clicked = false;
        let center = rect.center();
        match (&self.state, &self.loaded) {
            (LoadState::Loading { path, .. }, _) => {
                p.text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    format!("Lecture de {}…", path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()),
                    egui::FontId::proportional(18.0),
                    Color32::from_gray(210),
                );
            }
            (_, Some(l)) => {
                // No mesh: show the thumbnail with a clear message.
                let msg = "Ce fichier ne contient pas de maillage d'affichage enregistré.\nSeule la vignette intégrée peut être affichée.";
                if let Some(tex) = &l.preview_tex {
                    let size = tex.size_vec2();
                    let max = Vec2::new(rect.width() * 0.7, rect.height() * 0.65);
                    let scale = (max.x / size.x).min(max.y / size.y).min(2.0);
                    let r = egui::Rect::from_center_size(center - Vec2::new(0.0, 30.0), size * scale);
                    p.rect_filled(r.expand(6.0), CornerRadius::same(8), Color32::WHITE);
                    p.image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                    p.text(
                        egui::pos2(center.x, r.bottom() + 30.0),
                        egui::Align2::CENTER_TOP,
                        msg,
                        egui::FontId::proportional(14.0),
                        Color32::from_rgb(255, 205, 130),
                    );
                } else {
                    p.text(center, egui::Align2::CENTER_CENTER, format!("{msg}\n(aucune vignette non plus)"), egui::FontId::proportional(15.0), Color32::from_rgb(255, 205, 130));
                }
            }
            (st, None) => {
                diamond(p, center - Vec2::new(0.0, 74.0), 24.0, ACCENT);
                p.text(center - Vec2::new(0.0, 18.0), egui::Align2::CENTER_CENTER, "Glissez un fichier .SLDPRT ou .SLDASM ici", egui::FontId::proportional(20.0), Color32::from_gray(225));
                if let LoadState::Failed { path, error } = st {
                    p.text(
                        center + Vec2::new(0.0, 70.0),
                        egui::Align2::CENTER_CENTER,
                        format!("Impossible d'ouvrir {} :\n{error}", path.display()),
                        egui::FontId::proportional(13.0),
                        Color32::from_rgb(255, 140, 120),
                    );
                }
                let br = egui::Rect::from_center_size(center + Vec2::new(0.0, 22.0), Vec2::new(200.0, 34.0));
                open_clicked = ui
                    .place(br, egui::Button::new(RichText::new("Ouvrir un fichier…").size(15.0)).fill(ACCENT.gamma_multiply(0.85)).corner_radius(CornerRadius::same(8)))
                    .clicked();
            }
        }
        if open_clicked {
            self.open_dialog(ui.ctx());
        }
    }

    fn handle_shortcuts_and_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if let Some(p) = dropped.into_iter().next() {
            self.open(p, ctx);
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (o, f, e, t, orth, v0, v1, v2, v3) = ctx.input(|i| {
            (
                i.modifiers.command && i.key_pressed(egui::Key::O),
                i.key_pressed(egui::Key::F),
                i.key_pressed(egui::Key::E),
                i.key_pressed(egui::Key::T),
                !i.modifiers.command && i.key_pressed(egui::Key::O),
                i.key_pressed(egui::Key::Num0),
                i.key_pressed(egui::Key::Num1),
                i.key_pressed(egui::Key::Num2),
                i.key_pressed(egui::Key::Num3),
            )
        });
        if o {
            self.open_dialog(ctx);
        }
        if f {
            self.want_fit = true;
        }
        if e {
            self.edges = !self.edges;
            self.frame_dirty = true;
        }
        if t {
            self.show_tree = !self.show_tree;
        }
        if orth {
            self.cam.ortho = !self.cam.ortho;
            self.frame_dirty = true;
        }
        for (k, v) in [(v0, "iso"), (v1, "front"), (v2, "top"), (v3, "right")] {
            if k {
                self.apply_view(v);
            }
        }
    }

    fn drop_overlay(&self, ctx: &egui::Context, rect: egui::Rect) {
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if hovering {
            let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("drop")));
            p.rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(150));
            p.rect_stroke(rect.shrink(16.0), CornerRadius::same(12), Stroke::new(2.0, ACCENT), egui::StrokeKind::Inside);
            p.text(rect.center(), egui::Align2::CENTER_CENTER, "Déposez pour ouvrir", egui::FontId::proportional(24.0), Color32::WHITE);
        }
    }

    fn screenshot_logic(&mut self, ctx: &egui::Context) {
        let Some(out) = self.startup.screenshot.clone() else { return };
        if self.shot_done.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let ready = self.loaded.is_some() || matches!(self.state, LoadState::Failed { .. }) || self.startup.file.is_none();
        if !ready {
            ctx.request_repaint_after(Duration::from_millis(30));
            return;
        }
        self.shot_frames += 1;
        if self.shot_frames == 12 && !self.shot_requested {
            self.shot_requested = true;
            let done = self.shot_done.clone();
            let ctx2 = ctx.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                save_color_image(&img, &out);
                done.store(true, Ordering::SeqCst);
                ctx2.request_repaint();
            })));
        }
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

fn save_color_image(img: &egui::ColorImage, out: &Path) {
    let [w, h] = img.size;
    let mut buf = Vec::with_capacity(w * h * 4);
    for c in &img.pixels {
        buf.extend_from_slice(&c.to_srgba_unmultiplied());
    }
    match image::save_buffer(out, &buf, w as u32, h as u32, image::ExtendedColorType::Rgba8) {
        Ok(_) => eprintln!("screenshot saved: {}", out.display()),
        Err(e) => eprintln!("screenshot failed: {e}"),
    }
}

fn info_block(ui: &mut egui::Ui, l: &Loaded, selected: Option<usize>) {
    let s = &l.scene;
    let row = |ui: &mut egui::Ui, k: &str, v: String| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(k).color(TEXT_DIM).size(12.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(v).size(12.0).color(Color32::from_gray(215)));
            });
        });
    };
    ui.label(RichText::new("INFORMATIONS").size(11.0).color(TEXT_DIM).strong());
    if let Some(i) = selected {
        let c = &s.components[i];
        row(ui, "Sélection", c.name.clone());
        if let Some(m) = c.mesh {
            let mm = &s.meshes[m];
            row(ui, "Faces / triangles", format!("{} / {}", fmt_int(mm.mesh.face_count), fmt_int(mm.mesh.triangle_count())));
            if let sw_format::MeshSource::External(p) = &mm.source {
                row(ui, "Source", p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
            }
        }
    }
    let (lo, hi) = s.bounds().unwrap_or(([0.0; 3], [0.0; 3]));
    row(
        ui,
        "Encombrement (mm)",
        format!("{:.1} × {:.1} × {:.1}", (hi[0] - lo[0]) * 1000.0, (hi[1] - lo[1]) * 1000.0, (hi[2] - lo[2]) * 1000.0),
    );
    row(ui, "Conteneur", format!("{} · {} flux", s.container.label(), s.stream_count));
    let parts = s.components.iter().filter(|c| !c.is_assembly).count();
    if s.kind == DocKind::Assembly {
        row(ui, "Composants", format!("{} (maillages : {})", parts, s.meshes.len()));
    }
    let t = &s.timings;
    row(ui, "Lecture fichier", format!("{:.1} ms", (t.open + t.read).as_secs_f64() * 1000.0));
    row(ui, "Décodage + maillage", format!("{:.1} ms", (t.decode + t.mesh).as_secs_f64() * 1000.0));
    if !t.external.is_zero() {
        row(ui, "Pièces externes", format!("{:.1} ms", t.external.as_secs_f64() * 1000.0));
    }
    if let Some(tex) = &l.preview_tex {
        ui.add_space(6.0);
        let w = ui.available_width().min(220.0);
        let sz = tex.size_vec2();
        let img = egui::Image::new((tex.id(), Vec2::new(w, w * sz.y / sz.x))).corner_radius(CornerRadius::same(6)).bg_fill(Color32::WHITE);
        ui.add(img).on_hover_text("Vignette enregistrée par SolidWorks");
    }
}

fn toggle(ui: &mut egui::Ui, label: &str, on: bool) -> egui::Response {
    let b = egui::Button::new(RichText::new(label).color(if on { Color32::WHITE } else { Color32::from_gray(170) }))
        .fill(if on { ACCENT.gamma_multiply(0.55) } else { Color32::from_gray(45) });
    ui.add(b)
}

pub fn fmt_int(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

fn setup_style(ctx: &egui::Context) {
    // Always dark, regardless of the OS light/dark setting.
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(10.0, 4.0);
        let v = &mut style.visuals;
        v.panel_fill = PANEL_BG;
        v.window_fill = PANEL_BG;
        v.selection.bg_fill = ACCENT.gamma_multiply(0.6);
        v.widgets.inactive.weak_bg_fill = Color32::from_gray(45);
        v.widgets.inactive.corner_radius = CornerRadius::same(6);
        v.widgets.hovered.corner_radius = CornerRadius::same(6);
        v.widgets.active.corner_radius = CornerRadius::same(6);
        v.widgets.hovered.weak_bg_fill = Color32::from_gray(60);
        v.widgets.inactive.bg_stroke = Stroke::NONE;
    });
}

impl eframe::App for SwCraftApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_loading(&ctx);
        self.handle_shortcuts_and_drops(&ctx);
        self.top_bar(ui);
        self.status_bar(ui);
        self.tree_panel(ui);
        self.viewport(ui);
        self.drop_overlay(&ctx, ctx.content_rect());
        self.screenshot_logic(&ctx);
    }
}

fn diamond(p: &egui::Painter, c: egui::Pos2, r: f32, col: Color32) {
    let pts = vec![c + Vec2::new(0.0, -r), c + Vec2::new(r, 0.0), c + Vec2::new(0.0, r), c + Vec2::new(-r, 0.0)];
    p.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
    let s = r * 0.45;
    let pts = vec![c + Vec2::new(0.0, -s), c + Vec2::new(s, 0.0), c + Vec2::new(0.0, s), c + Vec2::new(-s, 0.0)];
    p.add(egui::Shape::convex_polygon(pts, Color32::from_white_alpha(90), Stroke::NONE));
}

fn chevron(p: &egui::Painter, c: egui::Pos2, open: bool, col: Color32) {
    let s = 3.5;
    let pts = if open {
        vec![c + Vec2::new(-s, -s * 0.6), c + Vec2::new(s, -s * 0.6), c + Vec2::new(0.0, s * 0.8)]
    } else {
        vec![c + Vec2::new(-s * 0.6, -s), c + Vec2::new(s * 0.8, 0.0), c + Vec2::new(-s * 0.6, s)]
    };
    p.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
}

fn eye(p: &egui::Painter, c: egui::Pos2, on: bool, hovered: bool) {
    let col = if on { Color32::from_gray(if hovered { 255 } else { 200 }) } else { Color32::from_gray(if hovered { 160 } else { 85 }) };
    // almond outline
    let w = 6.5;
    let h = 3.6;
    let n = 12;
    let mut pts = Vec::new();
    for k in 0..=n {
        let t = k as f32 / n as f32 * std::f32::consts::PI;
        pts.push(c + Vec2::new(-w * t.cos(), -h * t.sin()));
    }
    for k in 1..n {
        let t = k as f32 / n as f32 * std::f32::consts::PI;
        pts.push(c + Vec2::new(w * t.cos(), h * t.sin()));
    }
    p.add(egui::Shape::closed_line(pts, Stroke::new(1.2, col)));
    if on {
        p.circle_filled(c, 2.0, col);
    } else {
        p.line_segment([c + Vec2::new(-6.0, 5.0), c + Vec2::new(6.0, -5.0)], Stroke::new(1.2, col));
    }
}

fn comp_icon(p: &egui::Painter, c: egui::Pos2, assembly: bool, has_mesh: bool) {
    let r = egui::Rect::from_center_size(c, Vec2::splat(9.0));
    if assembly {
        p.rect_stroke(r, CornerRadius::same(2), Stroke::new(1.2, ACCENT), egui::StrokeKind::Inside);
        p.rect_filled(r.shrink(2.5), CornerRadius::same(1), ACCENT);
    } else if has_mesh {
        p.rect_filled(r, CornerRadius::same(2), ACCENT.gamma_multiply(0.85));
    } else {
        p.rect_stroke(r, CornerRadius::same(2), Stroke::new(1.2, Color32::from_rgb(220, 160, 90)), egui::StrokeKind::Inside);
    }
}
