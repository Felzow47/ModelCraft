//! One open file (a document tab): background loading, the scene, visibility, selection,
//! camera and measurements.

use crate::camera::Camera;
use crate::measure::{self, MeasureState};
use crate::render::{GpuScene, InstanceDesc};
use glam::{Mat4, Vec3};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;
use sw_format::Scene;

pub type LoadResult = Result<Scene, String>;

/// Live loading state shared with the loader thread.
#[derive(Clone, Default)]
pub struct LoadProgress {
    pub fraction: f32,
    pub stage: String,
    /// Bytes done / total of the current stage (reading or decompressing).
    pub bytes: Option<(u64, u64)>,
    pub secs: f32,
}

pub enum DocState {
    Loading {
        rx: mpsc::Receiver<LoadResult>,
        progress: Arc<Mutex<LoadProgress>>,
        started: Instant,
    },
    Ready,
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Section {
    /// 0 = X, 1 = Y, 2 = Z (display axes).
    pub axis: usize,
    /// Position across the model's bounding box, 0..1.
    pub pos: f32,
    pub flip: bool,
}

#[derive(Default, Clone, Copy)]
pub struct Stats {
    pub triangles: usize,
    pub vertices: usize,
    pub faces: usize,
    pub parts: usize,
    /// m³ / m², of the visible bodies (closed meshes only give a meaningful volume).
    pub volume: f64,
    pub area: f64,
    pub bbox: Option<(Vec3, Vec3)>,
}

/// A selected face (for highlighting and the info panel).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceSel {
    pub comp: usize,
    pub mesh: usize,
    pub face: Option<usize>,
    pub tri: u32,
}

/// Part of the move gizmo being dragged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Handle {
    /// Translate along world axis 0/1/2.
    Axis(usize),
    /// Rotate around world axis 0/1/2.
    Ring(usize),
    /// Free move in the view plane.
    Free,
    /// Rotate mode, part grabbed away from the rings: turn it like a ball held in the hand.
    Spin,
    /// Alt+drag: place / drag the 3D cursor (custom pivot), snapped to the surface under it.
    Pivot,
}

/// Point the Move / Rotate gizmo turns around (viewer-only, never written to the file).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PivotMode {
    /// Centre of the bounding box of the selection (with its sub-components).
    #[default]
    BBox,
    /// Centre of the selected component's own geometry.
    Object,
    /// A point the user places with Alt+click / Alt+drag (snaps to the surface).
    Cursor,
    /// World origin (0, 0, 0).
    World,
}

#[derive(Clone, Copy, Debug)]
pub struct GizmoDrag {
    pub comp: usize,
    pub handle: Handle,
    /// Pivot / line origin, fixed for the whole drag.
    pub origin: Vec3,
    /// Plane normal (free / ring drags).
    pub normal: Vec3,
    /// Last axis parameter or angle (x) / last plane point.
    pub last: Vec3,
}

pub struct Document {
    pub id: u64,
    pub path: PathBuf,
    pub title: String,
    pub size: u64,
    pub state: DocState,
    pub scene: Option<Scene>,
    pub gpu: Option<GpuScene>,
    pub camera: Camera,
    /// Hidden by the user (per component).
    pub hidden: Vec<bool>,
    /// Effective visibility (own flag and every parent).
    pub visible: Vec<bool>,
    /// Display-space world matrix per component (Z-up formats are turned Y-up).
    pub display: Vec<Mat4>,
    /// Accumulated viewer transform per component (display = acc * loaded matrix).
    pub acc: Vec<Mat4>,
    /// Viewer-only moves, explosion and opacity.
    pub asm: crate::assembly::Assembly,
    pub expanded: HashSet<usize>,
    pub selected: Option<usize>,
    pub face_sel: Option<FaceSel>,
    /// The view was rolled with the view cube's 90° buttons (cleared by a standard view).
    pub rolled: bool,
    pub hover: Option<usize>,
    pub measure: MeasureState,
    pub section: Option<Section>,
    pub show_bbox: bool,
    pub instances_dirty: bool,
    pub highlight_dirty: bool,
    pub stats: Stats,
    pub mesh_va: Vec<(f64, f64)>,
    pub preview: Option<egui::TextureHandle>,
    /// Images placed on the Revit sheets (rendered views), by sheet id, in `Sheet2d.images` order.
    pub sheet_tex: std::collections::HashMap<i64, Vec<egui::TextureHandle>>,
    /// Zoom and pan of the open sheet (2D only; the 3D camera is not touched).
    pub sheet_cam: (f32, egui::Vec2),
    /// The open Revit view's region (crop region × depth, display coordinates): fitting frames
    /// it instead of the whole model, as Revit opens a cropped view on its crop.
    pub view_box: Option<(Vec3, Vec3)>,
    /// What the opened Revit view keeps of the model (crop region, plan view range, depth
    /// clips, 3D section box), display space: (axes, lower bounds, upper bounds).
    pub view_clip: Option<([[f32; 3]; 3], [f32; 3], [f32; 3])>,
    /// Show the whole model while a view is open (the view clip is ignored).
    pub view_clip_off: bool,
    pub needs_fit: bool,
    pub cam_goal: Option<(f32, f32)>,
    pub tree_filter: String,
    /// Left panel shows the document browser (Revit "Navigateur de projet") instead of the
    /// component tree.
    pub show_browser: bool,
    /// Open rows of the document browser.
    pub browser_open: HashSet<usize>,
    pub browser_sel: Option<usize>,
    /// Active Revit view or sheet (browser row), as when it is opened in Revit; `None` = the
    /// whole model in 3D.
    pub view_ctx: Option<usize>,
    /// Scroll the browser to the selected row for this many frames (the list height is only
    /// known after its first frame).
    pub browser_reveal: u8,
    pub load_time: Option<f64>,
    /// Set to stop the loader thread.
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
    /// Scroll the tree to the selected row once.
    pub reveal_in_tree: bool,
    /// Move mode: translate gizmo on the selected component (viewer-only).
    pub gizmo: bool,
    /// Also show the rotation rings of the gizmo.
    pub gizmo_rotate: bool,
    /// Rotation / move pivot of the gizmo.
    pub pivot: PivotMode,
    /// 3D cursor (custom pivot), world space.
    pub cursor3d: Option<Vec3>,
    /// Floating "Vue éclatée" panel open.
    pub explode_panel: bool,
    /// Gizmo / free drag in progress.
    pub drag: Option<GizmoDrag>,
    /// Pan tool: the left button pans the view.
    pub pan_tool: bool,
    /// Where the hover comes from (the tree or the viewport), so one does not clear the other.
    pub hover_from_tree: bool,
    /// Viewer actions that can be undone (Ctrl+Z) / redone (Ctrl+Y).
    pub undo: Vec<ViewState>,
    pub redo: Vec<ViewState>,
    /// Aspect ratio of the viewport (frame selected / frame all from the keyboard).
    pub aspect: f32,
    /// Last coalesced checkpoint (slider drags make one undo step, not hundreds).
    last_ck: Option<(&'static str, std::time::Instant)>,
}

/// Everything a viewer action can change (moves, explosion, opacity, visibility).
#[derive(Clone, PartialEq)]
pub struct ViewState {
    user: Vec<Mat4>,
    explode: f32,
    opacity: Vec<f32>,
    global_opacity: f32,
    others_transparent: bool,
    hidden: Vec<bool>,
}

/// Undo steps kept per document.
const UNDO_MAX: usize = 100;

impl Drop for Document {
    fn drop(&mut self) {
        // closing / reloading a tab stops its loader
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Document {
    pub fn open(path: &Path, opts: craft_formats::Options, ctx: &egui::Context) -> Document {
        let (tx, rx) = mpsc::channel();
        let progress = Arc::new(Mutex::new(LoadProgress {
            stage: "Lecture du fichier…".into(),
            ..Default::default()
        }));
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut opts = opts;
        opts.cancel = Some(cancel.clone());
        {
            let progress = progress.clone();
            let progress2 = progress.clone();
            let ctx = ctx.clone();
            let ctx2 = ctx.clone();
            opts.progress = Some(
                craft_formats::Progress::new(move |f, stage| {
                    if let Ok(mut p) = progress.lock() {
                        if p.stage != stage {
                            p.bytes = None;
                        }
                        p.fraction = f;
                        p.stage = stage.to_string();
                    }
                    ctx.request_repaint();
                })
                .with_bytes(move |done, total| {
                    if let Ok(mut p) = progress2.lock() {
                        p.bytes = Some((done, total));
                    }
                    ctx2.request_repaint();
                }),
            );
        }
        let p = path.to_path_buf();
        let ctx2 = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("loader".into())
            .spawn(move || {
                crate::crash::set_current_file(Some(&p));
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    craft_formats::load_any(&p, &opts).map(|mut s| {
                        // a single part with several bodies: one child per body (explode / opacity by body)
                        if let Some(pr) = &opts.progress {
                            pr.report(0.97, "Séparation des corps…");
                        }
                        if sw_format::bodies::split_single_part(&mut s) == 0 {
                            // CAD assemblies (SolidWorks, STEP, IGES): bodies inside each part too;
                            // mesh formats already come split into objects
                            let cad = match &s.container {
                                sw_format::ContainerKind::Other(f) => matches!(*f, "STEP" | "IGES"),
                                _ => true,
                            };
                            if cad {
                                sw_format::bodies::split_assembly_parts(&mut s);
                            }
                        }
                        s
                    })
                }))
                .unwrap_or_else(|e| {
                    Err(format!(
                        "le lecteur a planté sur ce fichier (erreur interne : {})",
                        crate::crash::panic_text(&*e)
                    ))
                });
                crate::crash::set_current_file(None);
                let _ = tx.send(r);
                ctx2.request_repaint();
            });
        let failed = spawned
            .err()
            .map(|e| format!("impossible de lancer le chargement : {e}"));
        Document {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            path: path.to_path_buf(),
            title: path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            state: match failed {
                Some(e) => DocState::Failed(e),
                None => DocState::Loading {
                    rx,
                    progress,
                    started: Instant::now(),
                },
            },
            cancel,
            scene: None,
            gpu: None,
            camera: Camera::default(),
            hidden: Vec::new(),
            visible: Vec::new(),
            display: Vec::new(),
            acc: Vec::new(),
            asm: crate::assembly::Assembly::empty(),
            expanded: HashSet::new(),
            selected: None,
            face_sel: None,
            rolled: false,
            hover: None,
            measure: MeasureState::default(),
            section: None,
            show_bbox: false,
            instances_dirty: true,
            highlight_dirty: true,
            stats: Stats::default(),
            mesh_va: Vec::new(),
            preview: None,
            sheet_tex: Default::default(),
            sheet_cam: (1.0, egui::Vec2::ZERO),
            view_box: None,
            view_clip: None,
            view_clip_off: false,
            needs_fit: true,
            cam_goal: None,
            tree_filter: String::new(),
            show_browser: false,
            browser_open: HashSet::new(),
            browser_sel: None,
            view_ctx: None,
            browser_reveal: 0,
            load_time: None,
            reveal_in_tree: false,
            gizmo: false,
            gizmo_rotate: false,
            pivot: PivotMode::BBox,
            cursor3d: None,
            explode_panel: false,
            drag: None,
            pan_tool: false,
            hover_from_tree: false,
            undo: Vec::new(),
            redo: Vec::new(),
            aspect: 1.5,
            last_ck: None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.state, DocState::Loading { .. })
    }

    pub fn progress(&self) -> Option<LoadProgress> {
        match &self.state {
            DocState::Loading {
                progress, started, ..
            } => {
                let mut p = progress.lock().map(|p| p.clone()).unwrap_or_default();
                p.secs = started.elapsed().as_secs_f32();
                Some(p)
            }
            _ => None,
        }
    }

    /// Ask the loader to stop (it checks between steps and every few megabytes).
    pub fn cancel_load(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if self.is_loading() {
            self.state = DocState::Failed("Chargement annulé".into());
        }
    }

    /// Check the loader; returns true when the scene just arrived.
    pub fn poll(&mut self, device: &eframe::wgpu::Device, ctx: &egui::Context) -> bool {
        let DocState::Loading { rx, started, .. } = &self.state else {
            return false;
        };
        let r = match rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => Err("le chargement s'est interrompu".into()),
        };
        self.load_time = Some(started.elapsed().as_secs_f64());
        match r {
            Ok(scene) => {
                self.state = DocState::Ready;
                self.set_scene(scene, device, ctx);
                matches!(self.state, DocState::Ready)
            }
            Err(e) => {
                self.state = DocState::Failed(e);
                false
            }
        }
    }

    fn set_scene(&mut self, scene: Scene, device: &eframe::wgpu::Device, ctx: &egui::Context) {
        let n = scene.components.len();
        self.hidden = scene.components.iter().map(|c| c.hidden).collect();
        let rot = if scene.up_z {
            Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2)
        } else {
            Mat4::IDENTITY
        };
        let base: Vec<Mat4> = scene
            .components
            .iter()
            .map(|c| rot * Mat4::from_cols_array(&c.world))
            .collect();
        self.asm = crate::assembly::Assembly::new(&scene, base.clone());
        self.acc = vec![Mat4::IDENTITY; n];
        self.display = base;
        self.mesh_va = scene.meshes.iter().map(|m| m.mesh.volume_area()).collect();
        // expand the first levels of the tree
        for (i, c) in scene.components.iter().enumerate() {
            if c.depth < 1 && c.is_assembly {
                self.expanded.insert(i);
            }
        }
        if n == 1 {
            self.expanded.insert(0);
        }
        // a Revit project opens on its project browser, sections unfolded (as in Revit)
        if !scene.browser.is_empty() && self.browser_open.is_empty() {
            self.show_browser = true;
            self.browser_open = scene
                .browser
                .iter()
                .enumerate()
                .filter(|(_, r)| r.depth == 0)
                .map(|(i, _)| i)
                .collect();
        }
        if let Some(p) = &scene.preview {
            let img = egui::ColorImage::from_rgba_unmultiplied(
                [p.width as usize, p.height as usize],
                &p.pixels,
            );
            self.preview = Some(ctx.load_texture(
                format!("preview-{}", self.id),
                img,
                egui::TextureOptions::LINEAR,
            ));
        }
        self.sheet_tex.clear();
        for sh in &scene.sheets {
            let tex = sh
                .images
                .iter()
                .enumerate()
                .map(|(k, i)| {
                    let img = egui::ColorImage::from_rgba_unmultiplied(
                        [i.image.width as usize, i.image.height as usize],
                        &i.image.pixels,
                    );
                    ctx.load_texture(
                        format!("sheet-{}-{}-{k}", self.id, sh.id),
                        img,
                        egui::TextureOptions::LINEAR,
                    )
                })
                .collect();
            self.sheet_tex.insert(sh.id, tex);
        }
        let before = crate::crash::gpu_errors().0;
        let up = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            GpuScene::upload(device, &scene)
        }));
        let (after, msg) = crate::crash::gpu_errors();
        match up {
            Ok(g) if after == before => self.gpu = Some(g),
            Ok(_) => {
                self.state =
                    DocState::Failed(format!("la carte graphique a refusé le modèle ({msg})"));
                return;
            }
            Err(e) => {
                self.state = DocState::Failed(format!(
                    "envoi au GPU impossible ({})",
                    crate::crash::panic_text(&*e)
                ));
                return;
            }
        }
        let start = scene.browser.iter().position(|r| r.start);
        self.scene = Some(scene);
        self.update_visibility();
        self.needs_fit = true;
        // Revit opens a project on its starting view ("Vue de démarrage"): same here
        if let Some(i) = start {
            self.activate_view(i, false);
        }
    }

    /// 2D-only browser rows (sheets, drafting views, legends, schedules): no 3D camera.
    pub fn view_is_flat(&self, row: usize) -> bool {
        use sw_format::BrowserKind as K;
        self.scene
            .as_ref()
            .and_then(|s| s.browser.get(row))
            .is_some_and(|r| matches!(r.kind, K::Sheet | K::Drafting | K::Legend | K::Schedule))
    }

    /// Can this browser row be opened like a Revit view (double-click in Revit)?
    pub fn view_openable(&self, row: usize) -> bool {
        use sw_format::BrowserKind as K;
        self.scene
            .as_ref()
            .and_then(|s| s.browser.get(row))
            .is_some_and(|r| {
                r.id > 0
                    && matches!(
                        r.kind,
                        K::Plan
                            | K::CeilingPlan
                            | K::View3d
                            | K::Section
                            | K::Elevation
                            | K::Detail
                            | K::Drafting
                            | K::Rendering
                            | K::Legend
                            | K::Schedule
                            | K::Sheet
                    )
            })
    }

    /// Opens a view or sheet of the project browser: selects and reveals its row, then puts
    /// the viewer in that view's context (plans from above, sections and elevations from the
    /// front, both orthographic; 3D views in 3D; sheets and other 2D views on their sheet).
    pub fn activate_view(&mut self, row: usize, animate: bool) {
        use sw_format::BrowserKind as K;
        let Some(b) = self.scene.as_ref().map(|s| &s.browser) else {
            return;
        };
        let Some(r) = b.get(row) else { return };
        let kind = r.kind;
        // unfold the parents
        let mut d = r.depth;
        let mut parents = Vec::new();
        for j in (0..row).rev() {
            if b[j].depth < d {
                parents.push(j);
                d = b[j].depth;
                if d == 0 {
                    break;
                }
            }
        }
        self.browser_open.extend(parents);
        self.browser_sel = Some(row);
        self.view_ctx = Some(row);
        self.sheet_cam = (1.0, egui::Vec2::ZERO);
        self.show_browser = true;
        self.browser_reveal = if animate { 0 } else { 4 };
        let mut goal = match kind {
            K::Plan | K::CeilingPlan => Some(((0.0f32, 90.0f32), true)),
            K::Section | K::Elevation | K::Detail => Some(((0.0, 0.0), true)),
            K::View3d | K::Rendering => Some(((45.0, 35.264), false)),
            _ => None,
        };
        // the view's own direction and crop region when the file gives them
        self.view_box = None;
        self.view_clip = None;
        self.view_clip_off = false;
        let id = self
            .scene
            .as_ref()
            .and_then(|s| s.browser.get(row))
            .map_or(-1, |r| r.id);
        let up_z = self.scene.as_ref().is_some_and(|s| s.up_z);
        let frame = self
            .scene
            .as_ref()
            .and_then(|s| s.view_frames.iter().find(|f| f.id == id))
            .cloned();
        let model_view = matches!(
            kind,
            K::Plan | K::CeilingPlan | K::Section | K::Elevation | K::Detail | K::View3d
        );
        if let (Some(f), Some((_, ortho)), true) = (frame, goal, model_view) {
            // model (z up) to display (y up): (x, y, z) → (x, z, -y)
            let disp = |v: [f32; 3]| {
                if up_z {
                    Vec3::new(v[0], v[2], -v[1])
                } else {
                    Vec3::from(v)
                }
            };
            if let Some(yp) = view_yaw_pitch(disp(f.dir), disp(f.up)) {
                goal = Some((yp, ortho));
            }
            self.view_box = f.region.map(|(a, b)| {
                let (a, b) = (disp(a), disp(b));
                (a.min(b), a.max(b))
            });
            self.view_clip = f.clip.map(|c| {
                let o = disp(c.orig);
                let ax = c.axes.map(|a| disp(a).normalize_or_zero());
                let base: [f32; 3] = std::array::from_fn(|k| o.dot(ax[k]));
                (
                    ax.map(|a| a.to_array()),
                    std::array::from_fn(|k| base[k] + c.lo[k]),
                    std::array::from_fn(|k| base[k] + c.hi[k]),
                )
            });
        }
        if let Some(((y, p), ortho)) = goal {
            if ortho {
                self.camera.ortho = true;
            }
            if animate {
                self.cam_goal = Some((y.to_radians(), p.to_radians()));
            } else {
                self.camera.set_view(y, p);
            }
            self.needs_fit = true;
        }
    }

    /// Back to the whole model in 3D.
    pub fn leave_view(&mut self) {
        self.view_ctx = None;
        self.view_box = None;
        self.view_clip = None;
        self.view_clip_off = false;
        self.cam_goal = Some((45f32.to_radians(), 35.264f32.to_radians()));
        self.needs_fit = true;
    }

    /// A part that is a single body cannot be exploded by bodies: split it into its faces
    /// (viewer-only) so the exploded view still separates something. Returns true if done.
    pub fn split_into_faces(&mut self, device: &eframe::wgpu::Device, ctx: &egui::Context) -> bool {
        if self.asm.can_explode() {
            return false;
        }
        let Some(mut scene) = self.scene.take() else {
            return false;
        };
        if sw_format::bodies::split_single_part_faces(&mut scene) == 0 {
            self.scene = Some(scene);
            return false;
        }
        let cam = self.camera.clone();
        self.gpu = None;
        self.selected = None;
        self.hover = None;
        self.face_sel = None;
        self.set_scene(scene, device, ctx);
        self.camera = cam;
        self.needs_fit = false;
        self.highlight_dirty = true;
        true
    }

    /// Move / Rotate on a selected face: detach that face into its own child component
    /// (viewer-only, the file is never modified) so it moves alone while the rest of the body
    /// stays, even when the whole model is one single body. Moves, opacity, visibility and the
    /// undo history of the other components are kept. Returns true when a face was detached;
    /// a face that is already a whole component just selects that component.
    pub fn detach_selected_face(
        &mut self,
        device: &eframe::wgpu::Device,
        ctx: &egui::Context,
    ) -> bool {
        let Some(f) = self.face_sel else { return false };
        let Some(face) = f.face else { return false };
        let Some(scene) = self.scene.as_ref() else {
            return false;
        };
        let Some(mesh) = scene.components.get(f.comp).and_then(|c| c.mesh) else {
            return false;
        };
        if mesh != f.mesh
            || scene
                .meshes
                .get(mesh)
                .is_none_or(|m| m.mesh.face_start.len() < 2)
        {
            // the face is the whole component (e.g. already detached): move the component
            self.face_sel = None;
            self.select(Some(f.comp));
            self.highlight_dirty = true;
            self.instances_dirty = true;
            return false;
        }
        let mut scene = self.scene.take().unwrap();
        let Some(ins) = sw_format::bodies::detach_face(&mut scene, f.comp, face, "Face") else {
            self.scene = Some(scene);
            return false;
        };
        // keep the viewer state of every component, with a neutral entry for the new one
        let mut user = std::mem::take(&mut self.asm.user);
        let mut opacity = std::mem::take(&mut self.asm.opacity);
        let (explode, goal, lines) = (
            self.asm.explode,
            self.asm.explode_goal,
            self.asm.explode_lines,
        );
        let (global, others) = (self.asm.global_opacity, self.asm.others_transparent);
        let mut hidden = std::mem::take(&mut self.hidden);
        let ok = ins <= user.len() && ins <= opacity.len() && ins <= hidden.len();
        if ok {
            user.insert(ins, Mat4::IDENTITY);
            opacity.insert(ins, 1.0);
            hidden.insert(ins, false);
        }
        for st in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            if ins <= st.user.len() && ins <= st.opacity.len() && ins <= st.hidden.len() {
                st.user.insert(ins, Mat4::IDENTITY);
                st.opacity.insert(ins, 1.0);
                st.hidden.insert(ins, false);
            }
        }
        let expanded: HashSet<usize> = self
            .expanded
            .iter()
            .map(|&i| if i >= ins { i + 1 } else { i })
            .chain([f.comp])
            .collect();
        let cam = self.camera.clone();
        let cursor = self.cursor3d;
        self.gpu = None;
        self.hover = None;
        self.face_sel = None;
        self.drag = None;
        self.set_scene(scene, device, ctx);
        self.camera = cam;
        self.cursor3d = cursor;
        self.needs_fit = false;
        self.expanded = expanded;
        let n = self.scene.as_ref().map(|s| s.components.len()).unwrap_or(0);
        if ok && user.len() == n && opacity.len() == n && hidden.len() == n {
            self.asm.user = user;
            self.asm.opacity = opacity;
            self.hidden = hidden;
        }
        self.asm.explode = explode;
        self.asm.explode_goal = goal;
        self.asm.explode_lines = lines;
        self.asm.global_opacity = global;
        self.asm.others_transparent = others;
        self.update_visibility();
        self.apply_transforms();
        self.select(Some(ins));
        self.reveal_in_tree = true;
        self.highlight_dirty = true;
        self.instances_dirty = true;
        true
    }

    fn view_state(&self) -> ViewState {
        ViewState {
            user: self.asm.user.clone(),
            explode: self.asm.explode_goal,
            opacity: self.asm.opacity.clone(),
            global_opacity: self.asm.global_opacity,
            others_transparent: self.asm.others_transparent,
            hidden: self.hidden.clone(),
        }
    }

    /// Remember the current state before a viewer action (no-op if nothing changed since the
    /// last checkpoint). Call it before moving, exploding, hiding or changing an opacity.
    pub fn checkpoint(&mut self) {
        if self.scene.is_none() {
            return;
        }
        let s = self.view_state();
        if self.undo.last() != Some(&s) {
            self.undo.push(s);
            if self.undo.len() > UNDO_MAX {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_ck = None;
    }

    /// Checkpoint for continuous edits (sliders): successive calls of the same kind less than
    /// a second apart are one undo step.
    pub fn checkpoint_coalesced(&mut self, kind: &'static str) {
        let now = std::time::Instant::now();
        if let Some((k, t)) = self.last_ck {
            if k == kind && now.duration_since(t).as_secs_f32() < 1.0 {
                self.last_ck = Some((kind, now));
                return;
            }
        }
        self.checkpoint();
        self.last_ck = Some((kind, now));
    }

    fn restore(&mut self, s: ViewState) {
        if s.user.len() != self.asm.user.len() || s.hidden.len() != self.hidden.len() {
            return;
        }
        self.asm.user = s.user;
        self.asm.explode_goal = s.explode;
        self.asm.explode = s.explode;
        self.asm.opacity = s.opacity;
        self.asm.global_opacity = s.global_opacity;
        self.asm.others_transparent = s.others_transparent;
        self.hidden = s.hidden;
        self.drag = None;
        self.update_visibility();
        self.apply_transforms();
    }

    /// Undo the last viewer action. Returns false when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let cur = self.view_state();
        // skip checkpoints identical to the current state (an action that changed nothing)
        while let Some(s) = self.undo.pop() {
            if s != cur {
                self.redo.push(cur);
                self.restore(s);
                return true;
            }
        }
        false
    }

    pub fn redo(&mut self) -> bool {
        let Some(s) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.view_state());
        self.restore(s);
        true
    }

    pub fn update_visibility(&mut self) {
        let Some(s) = &self.scene else { return };
        let n = s.components.len();
        let mut vis = vec![true; n];
        for i in 0..n {
            let c = &s.components[i];
            let own = !c.suppressed && !self.hidden.get(i).copied().unwrap_or(false);
            vis[i] = own && c.parent.is_none_or(|p| p < i && vis[p]);
        }
        self.visible = vis;
        self.instances_dirty = true;
        self.compute_stats();
    }

    fn compute_stats(&mut self) {
        let Some(s) = &self.scene else { return };
        let mut st = Stats::default();
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for (i, c) in s.components.iter().enumerate() {
            let Some(mi) = c.mesh else { continue };
            if !self.visible[i] {
                continue;
            }
            let m = &s.meshes[mi].mesh;
            if m.is_empty() {
                continue;
            }
            st.parts += 1;
            st.triangles += m.triangle_count();
            st.vertices += m.positions.len();
            st.faces += m.face_count;
            let w = self.display[i];
            // volume scales by the determinant (exact, also for the sized unit cubes of Revit
            // boxes); area by its 2/3 power (exact for uniform scales, CAD transforms are rigid)
            let det = (glam::Mat3::from_mat4(w).determinant().abs()) as f64;
            let (v, a) = self.mesh_va[mi];
            st.volume += v.abs() * det;
            st.area += a * det.powf(2.0 / 3.0);
            for k in 0..8 {
                let p = Vec3::new(
                    if k & 1 == 0 {
                        m.bbox_min[0]
                    } else {
                        m.bbox_max[0]
                    },
                    if k & 2 == 0 {
                        m.bbox_min[1]
                    } else {
                        m.bbox_max[1]
                    },
                    if k & 4 == 0 {
                        m.bbox_min[2]
                    } else {
                        m.bbox_max[2]
                    },
                );
                let q = w.transform_point3(p);
                lo = lo.min(q);
                hi = hi.max(q);
            }
        }
        st.bbox = (lo.x <= hi.x).then_some((lo, hi));
        self.stats = st;
    }

    /// Bounding box of everything (visible or not), for the section plane range.
    pub fn bbox(&self) -> Option<(Vec3, Vec3)> {
        self.stats.bbox
    }

    pub fn subtree(&self, root: usize) -> Vec<usize> {
        let Some(s) = &self.scene else { return vec![] };
        let mut out = vec![root];
        let d = s.components[root].depth;
        for i in root + 1..s.components.len() {
            if s.components[i].depth <= d {
                break;
            }
            out.push(i);
        }
        out
    }

    pub fn has_children(&self, i: usize) -> bool {
        self.scene
            .as_ref()
            .is_some_and(|s| s.components.get(i + 1).is_some_and(|c| c.parent == Some(i)))
    }

    pub fn hide(&mut self, i: usize) {
        self.checkpoint();
        if let Some(h) = self.hidden.get_mut(i) {
            *h = true;
        }
        if self.selected == Some(i) {
            self.selected = None;
            self.face_sel = None;
            self.highlight_dirty = true;
        }
        self.update_visibility();
    }

    pub fn toggle_hidden(&mut self, i: usize) {
        if self.hidden.get(i).copied().unwrap_or(false) {
            self.checkpoint();
            self.hidden[i] = false;
            self.update_visibility();
        } else {
            self.hide(i);
        }
    }

    pub fn show_all(&mut self) {
        self.checkpoint();
        self.hidden.iter_mut().for_each(|h| *h = false);
        self.update_visibility();
    }

    /// Hide everything except `i`, its ancestors (so it stays reachable) and its subtree.
    pub fn isolate(&mut self, i: usize) {
        self.checkpoint();
        let Some(s) = &self.scene else { return };
        let keep: HashSet<usize> = self.subtree(i).into_iter().collect();
        let mut anc = HashSet::new();
        let mut p = s.components[i].parent;
        while let Some(k) = p {
            anc.insert(k);
            p = s.components[k].parent;
        }
        for (k, h) in self.hidden.iter_mut().enumerate() {
            *h = !(keep.contains(&k) || anc.contains(&k));
        }
        // siblings of ancestors are hidden by the loop above; ancestors themselves stay shown
        self.update_visibility();
        self.needs_fit = true;
    }

    pub fn select(&mut self, i: Option<usize>) {
        if self.selected != i {
            self.selected = i;
            self.instances_dirty = true;
        }
        if let (Some(i), Some(s)) = (i, &self.scene) {
            // open the path to it in the tree
            let mut p = s.components[i].parent;
            while let Some(k) = p {
                self.expanded.insert(k);
                p = s.components[k].parent;
            }
            self.reveal_in_tree = true;
        }
    }

    pub fn set_hover(&mut self, i: Option<usize>) {
        if self.hover != i {
            self.hover = i;
            self.instances_dirty = true;
        }
    }

    /// Rebuild instance / highlight buffers if needed.
    pub fn sync_gpu(&mut self, device: &eframe::wgpu::Device, highlight: [f32; 4]) {
        if self.scene.is_none() || self.gpu.is_none() {
            return;
        }
        let (sel, hov): (HashSet<usize>, HashSet<usize>) = if self.instances_dirty {
            let s = self.scene.as_ref().unwrap();
            (
                self.selected
                    .map(|r| self.subtree_of(s, r))
                    .unwrap_or_default(),
                self.hover
                    .map(|r| self.subtree_of(s, r))
                    .unwrap_or_default(),
            )
        } else {
            Default::default()
        };
        let (Some(s), Some(gs)) = (&self.scene, self.gpu.as_mut()) else {
            return;
        };
        if self.instances_dirty {
            self.instances_dirty = false;
            let mut items = Vec::new();
            for (i, c) in s.components.iter().enumerate() {
                let Some(mi) = c.mesh else { continue };
                if !self.visible.get(i).copied().unwrap_or(true) {
                    continue;
                }
                let tint = if sel.contains(&i) && self.face_sel.is_none() {
                    [highlight[0], highlight[1], highlight[2], 0.55]
                } else if hov.contains(&i) {
                    [highlight[0], highlight[1], highlight[2], 0.25]
                } else {
                    [0.0; 4]
                };
                let color = c
                    .color
                    .or(s.meshes[mi].color)
                    .map(|c| [c[0], c[1], c[2], 1.0])
                    .unwrap_or([0.0; 4]);
                // material transparency (glass in IFC / glTF / VRML) multiplies the viewer opacity
                let mat_alpha = c
                    .color
                    .or(s.meshes[mi].color)
                    .map(|c| c[3])
                    .unwrap_or(1.0)
                    .clamp(0.05, 1.0);
                let opacity = effective_opacity(&self.asm, i, &sel) * mat_alpha;
                let center = self.acc[i].transform_point3(self.asm.center0[i]).to_array();
                items.push(InstanceDesc {
                    mesh: mi,
                    model: self.display[i].to_cols_array(),
                    tint,
                    color,
                    opacity,
                    center,
                });
            }
            gs.set_instances(device, items);
        }
        if self.highlight_dirty {
            self.highlight_dirty = false;
            let mut tris = Vec::new();
            if let Some(f) = self.face_sel {
                if let (Some(m), Some(w)) = (s.meshes.get(f.mesh), self.display.get(f.comp)) {
                    let m = &m.mesh;
                    let nm = Mat4::from_mat3(glam::Mat3::from_mat4(*w).inverse().transpose());
                    for t in measure::face_tris(m, f.face, f.tri) {
                        for k in 0..3 {
                            let vi = m.indices[t as usize * 3 + k] as usize;
                            tris.push(crate::render::Vertex {
                                pos: w.transform_point3(Vec3::from(m.positions[vi])).to_array(),
                                nrm: nm
                                    .transform_vector3(Vec3::from(m.normals[vi]))
                                    .normalize_or_zero()
                                    .to_array(),
                                uv: [0.0, 0.0],
                            });
                        }
                    }
                }
            }
            gs.set_highlight(
                device,
                &tris,
                [highlight[0], highlight[1], highlight[2], 0.8],
            );
        }
    }

    /// Recompute display matrices after a move / explosion change.
    pub fn apply_transforms(&mut self) {
        if self.scene.is_none() {
            return;
        }
        let (d, a) = self.asm.compute();
        self.display = d;
        self.acc = a;
        self.instances_dirty = true;
        self.highlight_dirty = true;
        self.compute_stats();
        // keep moved / exploded parts inside the camera's clipping range
        if let Some((lo, hi)) = self.stats.bbox {
            let r = (lo - self.camera.target)
                .length()
                .max((hi - self.camera.target).length());
            if r.is_finite() && r > self.camera.radius {
                self.camera.radius = r;
            }
        }
    }

    /// Move the subtree of `i` by a world-space transform.
    pub fn move_component(&mut self, i: usize, w: Mat4) {
        if i < self.acc.len() {
            self.asm.apply_world(&self.acc, i, w);
            self.apply_transforms();
        }
    }

    /// Current world-space centre of the subtree of `i` (gizmo pivot).
    pub fn component_center(&self, i: usize) -> Option<Vec3> {
        Some(self.acc.get(i)?.transform_point3(*self.asm.center0.get(i)?))
    }

    /// World-space centre of the own geometry of `i` (subtree centre when it has none).
    pub fn object_center(&self, i: usize) -> Option<Vec3> {
        Some(
            self.acc
                .get(i)?
                .transform_point3(*self.asm.own_center.get(i)?),
        )
    }

    /// Point the gizmo of component `i` turns around, following [`PivotMode`].
    pub fn gizmo_pivot(&self, i: usize) -> Option<Vec3> {
        match self.pivot {
            PivotMode::BBox => self.component_center(i),
            PivotMode::Object => self.object_center(i).or_else(|| self.component_center(i)),
            PivotMode::Cursor => self.cursor3d.or_else(|| self.component_center(i)),
            PivotMode::World => Some(Vec3::ZERO),
        }
    }

    /// Switch the pivot mode; the 3D cursor starts on the selection (or the model centre).
    pub fn set_pivot(&mut self, m: PivotMode) {
        self.pivot = m;
        if m == PivotMode::Cursor && self.cursor3d.is_none() {
            self.cursor_to_selection();
        }
    }

    /// Put the 3D cursor on the bounding-box centre of the selection (whole model if none).
    pub fn cursor_to_selection(&mut self) {
        let c = match self.selected {
            Some(i) => self.component_center(i),
            None => self.bbox().map(|(lo, hi)| (lo + hi) * 0.5),
        };
        if let Some(c) = c {
            self.cursor3d = Some(c);
        }
    }

    pub fn reset_position(&mut self, i: Option<usize>) {
        self.checkpoint();
        match i {
            Some(i) => {
                for k in self.subtree(i) {
                    self.asm.reset(k);
                }
            }
            None => self.asm.reset_all(),
        }
        self.apply_transforms();
    }

    pub fn set_opacity(&mut self, i: Option<usize>, v: f32) {
        self.checkpoint_coalesced("opacity");
        let v = v.clamp(0.02, 1.0);
        match i {
            Some(i) => {
                if let Some(o) = self.asm.opacity.get_mut(i) {
                    *o = v;
                }
            }
            None => self.asm.global_opacity = v,
        }
        self.instances_dirty = true;
    }

    /// Advance animations (explosion). Returns true if the view changed.
    pub fn tick(&mut self, dt: f32) -> bool {
        if self.asm.tick(dt) {
            self.apply_transforms();
            return true;
        }
        false
    }

    /// Explosion lines (world space) for the overlay.
    pub fn explode_lines(&self) -> Vec<(Vec3, Vec3)> {
        let Some(s) = &self.scene else { return vec![] };
        let with_mesh: Vec<bool> = s
            .components
            .iter()
            .enumerate()
            .map(|(i, c)| c.mesh.is_some() && self.visible.get(i).copied().unwrap_or(true))
            .collect();
        self.asm.lines(&self.acc, &with_mesh)
    }

    fn subtree_of(&self, _s: &Scene, r: usize) -> HashSet<usize> {
        self.subtree(r).into_iter().collect()
    }

    /// Instances for picking: (component, mesh, display matrix) of visible components.
    pub fn pick_items(&self) -> Vec<(usize, usize, Mat4)> {
        let Some(s) = &self.scene else { return vec![] };
        s.components
            .iter()
            .enumerate()
            .filter(|(i, _)| self.visible.get(*i).copied().unwrap_or(true))
            .filter_map(|(i, c)| c.mesh.map(|m| (i, m, self.display[i])))
            .collect()
    }

    /// The opened view's clip box when it applies (a view is open and not shown whole).
    pub fn active_view_clip(&self) -> Option<([[f32; 3]; 3], [f32; 3], [f32; 3])> {
        self.view_clip
            .filter(|_| self.view_ctx.is_some() && !self.view_clip_off)
    }

    pub fn pick(&self, o: Vec3, d: Vec3) -> Option<measure::Hit> {
        let s = self.scene.as_ref()?;
        let meshes: Vec<&sw_format::TriMesh> = s.meshes.iter().map(|m| &m.mesh).collect();
        let mut items = self.pick_items();
        // the opened view hides what is outside its box: step past hidden hits
        if let Some((ax, lo, hi)) = self.active_view_clip() {
            let inside = |p: Vec3| {
                (0..3).all(|k| (lo[k] - 1e-4..=hi[k] + 1e-4).contains(&p.dot(Vec3::from(ax[k]))))
            };
            let sec = self.section_plane();
            let ok = |p: Vec3| inside(p) && sec.is_none_or(|s| p.dot(Vec3::from(s.0)) <= s.1);
            let mut from = o;
            for _ in 0..24 {
                let h = measure::pick(&meshes, &items, from, d)?;
                if ok(h.point) {
                    return Some(h);
                }
                from = h.point + d.normalize() * 1e-4;
            }
            return None;
        }
        // the section plane hides part of the model: ignore hits there
        if let Some(sec) = self.section_plane() {
            let h = measure::pick(&meshes, &items, o, d);
            if let Some(h) = h {
                if h.point.dot(Vec3::from(sec.0)) <= sec.1 {
                    return Some(h);
                }
                // step past the cut and retry once from there
                let o2 = h.point + d.normalize() * 1e-6;
                items.retain(|_| true);
                return measure::pick(&meshes, &items, o2, d)
                    .filter(|h| h.point.dot(Vec3::from(sec.0)) <= sec.1);
            }
            return None;
        }
        measure::pick(&meshes, &items, o, d)
    }

    /// Section plane (normal, offset) in display space.
    pub fn section_plane(&self) -> Option<([f32; 3], f32)> {
        let sec = self.section?;
        let (lo, hi) = self.stats.bbox?;
        let a = sec.axis.min(2);
        let v = lo[a] + (hi[a] - lo[a]) * sec.pos.clamp(0.0, 1.0);
        let mut n = [0.0; 3];
        n[a] = 1.0;
        if sec.flip {
            n[a] = -1.0;
            Some((n, -v))
        } else {
            Some((n, v))
        }
    }

    pub fn fit(&mut self, aspect: f32) {
        if let Some((lo, hi)) = self
            .view_box
            .filter(|_| self.view_ctx.is_some())
            .or(self.stats.bbox)
        {
            self.camera.fit(lo, hi, aspect);
        }
        self.needs_fit = false;
    }

    pub fn file_kind_label(&self) -> String {
        craft_formats::ext_of(&self.path).to_uppercase()
    }
}

/// Turntable yaw and pitch (degrees, see `Camera::orientation`) looking along a Revit view:
/// `f` points from the model to the viewer, `up` is the screen up (display coordinates). Top
/// and bottom views take their yaw from `up` (rotated plans keep their orientation).
fn view_yaw_pitch(f: Vec3, up: Vec3) -> Option<(f32, f32)> {
    let f = f.try_normalize()?;
    let pitch = f.y.clamp(-1.0, 1.0).asin();
    let yaw = if f.y.abs() > 0.999 {
        // orientation(yaw, ±90°) has up = ∓(sin yaw, 0, cos yaw)
        let s = -f.y.signum();
        (s * up.x).atan2(s * up.z)
    } else {
        f.x.atan2(f.z)
    };
    Some((yaw.to_degrees(), pitch.to_degrees()))
}

#[cfg(test)]
mod view_tests {
    use super::*;

    #[test]
    fn revit_view_directions() {
        let close = |a: Vec3, b: Vec3| (a - b).length() < 1e-3;
        let check = |f: Vec3, up: Vec3| {
            let (y, p) = view_yaw_pitch(f, up).unwrap();
            let r = Camera::orientation(y.to_radians(), p.to_radians());
            assert!(
                close(r * Vec3::Z, f.normalize()),
                "dir {f:?}: {:?}",
                r * Vec3::Z
            );
            assert!(
                close(r * Vec3::Y, up.normalize()),
                "up {up:?}: {:?}",
                r * Vec3::Y
            );
        };
        // plan, north up: model dir (0,0,1), up (0,1,0) → display (0,1,0), (0,0,-1)
        check(Vec3::Y, -Vec3::Z);
        // plan rotated 30°
        let a = 30f32.to_radians();
        check(Vec3::Y, Vec3::new(a.sin(), 0.0, -a.cos()));
        // south elevation (display +z), east elevation, 3D from above
        check(Vec3::Z, Vec3::Y);
        check(Vec3::X, Vec3::Y);
        let f = Vec3::new(1.0, 1.0, 1.0).normalize();
        check(f, (Vec3::Y - f * f.y).normalize());
        // ceiling plan, from below
        check(-Vec3::Y, -Vec3::Z);
    }
}

/// Opacity of component `i` with the per-part, global and "others" settings.
fn effective_opacity(asm: &crate::assembly::Assembly, i: usize, sel: &HashSet<usize>) -> f32 {
    let mut o = asm.chain_opacity(i);
    if asm.others_transparent && !sel.is_empty() && !sel.contains(&i) {
        o = o.min(crate::assembly::OTHERS_OPACITY);
    }
    o
}
