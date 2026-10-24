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
    Loading { rx: mpsc::Receiver<LoadResult>, progress: Arc<Mutex<LoadProgress>>, started: Instant },
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
    pub hover: Option<usize>,
    pub measure: MeasureState,
    pub section: Option<Section>,
    pub show_bbox: bool,
    pub instances_dirty: bool,
    pub highlight_dirty: bool,
    pub stats: Stats,
    pub mesh_va: Vec<(f64, f64)>,
    pub preview: Option<egui::TextureHandle>,
    pub needs_fit: bool,
    pub cam_goal: Option<(f32, f32)>,
    pub tree_filter: String,
    pub load_time: Option<f64>,
    /// Set to stop the loader thread.
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
    /// Scroll the tree to the selected row once.
    pub reveal_in_tree: bool,
    /// Move mode: translate gizmo on the selected component (viewer-only).
    pub gizmo: bool,
    /// Also show the rotation rings of the gizmo.
    pub gizmo_rotate: bool,
    /// Gizmo / free drag in progress.
    pub drag: Option<GizmoDrag>,
}

impl Drop for Document {
    fn drop(&mut self) {
        // closing / reloading a tab stops its loader
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Document {
    pub fn open(path: &Path, opts: craft_formats::Options, ctx: &egui::Context) -> Document {
        let (tx, rx) = mpsc::channel();
        let progress = Arc::new(Mutex::new(LoadProgress { stage: "Lecture du fichier…".into(), ..Default::default() }));
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
        let spawned = std::thread::Builder::new().name("loader".into()).spawn(move || {
            crate::crash::set_current_file(Some(&p));
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| craft_formats::load_any(&p, &opts)))
                .unwrap_or_else(|e| Err(format!("le lecteur a planté sur ce fichier (erreur interne : {})", crate::crash::panic_text(&*e))));
            crate::crash::set_current_file(None);
            let _ = tx.send(r);
            ctx2.request_repaint();
        });
        let failed = spawned.err().map(|e| format!("impossible de lancer le chargement : {e}"));
        Document {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            path: path.to_path_buf(),
            title: path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            state: match failed {
                Some(e) => DocState::Failed(e),
                None => DocState::Loading { rx, progress, started: Instant::now() },
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
            hover: None,
            measure: MeasureState::default(),
            section: None,
            show_bbox: false,
            instances_dirty: true,
            highlight_dirty: true,
            stats: Stats::default(),
            mesh_va: Vec::new(),
            preview: None,
            needs_fit: true,
            cam_goal: None,
            tree_filter: String::new(),
            load_time: None,
            reveal_in_tree: false,
            gizmo: false,
            gizmo_rotate: false,
            drag: None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.state, DocState::Loading { .. })
    }

    pub fn progress(&self) -> Option<LoadProgress> {
        match &self.state {
            DocState::Loading { progress, started, .. } => {
                let mut p = progress.lock().map(|p| p.clone()).unwrap_or_default();
                p.secs = started.elapsed().as_secs_f32();
                Some(p)
            }
            _ => None,
        }
    }

    /// Ask the loader to stop (it checks between steps and every few megabytes).
    pub fn cancel_load(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        if self.is_loading() {
            self.state = DocState::Failed("Chargement annulé".into());
        }
    }

    /// Check the loader; returns true when the scene just arrived.
    pub fn poll(&mut self, device: &eframe::wgpu::Device, ctx: &egui::Context) -> bool {
        let DocState::Loading { rx, started, .. } = &self.state else { return false };
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
        let rot = if scene.up_z { Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2) } else { Mat4::IDENTITY };
        let base: Vec<Mat4> = scene.components.iter().map(|c| rot * Mat4::from_cols_array(&c.world)).collect();
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
        if let Some(p) = &scene.preview {
            let img = egui::ColorImage::from_rgba_unmultiplied([p.width as usize, p.height as usize], &p.pixels);
            self.preview = Some(ctx.load_texture(format!("preview-{}", self.id), img, egui::TextureOptions::LINEAR));
        }
        let before = crate::crash::gpu_errors().0;
        let up = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| GpuScene::upload(device, &scene)));
        let (after, msg) = crate::crash::gpu_errors();
        match up {
            Ok(g) if after == before => self.gpu = Some(g),
            Ok(_) => {
                self.state = DocState::Failed(format!("la carte graphique a refusé le modèle ({msg})"));
                return;
            }
            Err(e) => {
                self.state = DocState::Failed(format!("envoi au GPU impossible ({})", crate::crash::panic_text(&*e)));
                return;
            }
        }
        self.scene = Some(scene);
        self.update_visibility();
        self.needs_fit = true;
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
            // uniform scale of the instance (CAD transforms are rigid, mirrored at most)
            let sc = w.transform_vector3(Vec3::X).length() as f64;
            let (v, a) = self.mesh_va[mi];
            st.volume += v.abs() * sc * sc * sc;
            st.area += a * sc * sc;
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
        self.scene.as_ref().is_some_and(|s| s.components.get(i + 1).is_some_and(|c| c.parent == Some(i)))
    }

    pub fn hide(&mut self, i: usize) {
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
            self.hidden[i] = false;
            self.update_visibility();
        } else {
            self.hide(i);
        }
    }

    pub fn show_all(&mut self) {
        self.hidden.iter_mut().for_each(|h| *h = false);
        self.update_visibility();
    }

    /// Hide everything except `i`, its ancestors (so it stays reachable) and its subtree.
    pub fn isolate(&mut self, i: usize) {
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
            (self.selected.map(|r| self.subtree_of(s, r)).unwrap_or_default(), self.hover.map(|r| self.subtree_of(s, r)).unwrap_or_default())
        } else {
            Default::default()
        };
        let (Some(s), Some(gs)) = (&self.scene, self.gpu.as_mut()) else { return };
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
                let color = c.color.or(s.meshes[mi].color).map(|c| [c[0], c[1], c[2], 1.0]).unwrap_or([0.0; 4]);
                let opacity = effective_opacity(&self.asm, i, &sel);
                let center = self.acc[i].transform_point3(self.asm.center0[i]).to_array();
                items.push(InstanceDesc { mesh: mi, model: self.display[i].to_cols_array(), tint, color, opacity, center });
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
                                nrm: nm.transform_vector3(Vec3::from(m.normals[vi])).normalize_or_zero().to_array(),
                            });
                        }
                    }
                }
            }
            gs.set_highlight(device, &tris, [highlight[0], highlight[1], highlight[2], 0.8]);
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
            let r = (lo - self.camera.target).length().max((hi - self.camera.target).length());
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

    pub fn reset_position(&mut self, i: Option<usize>) {
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
        let with_mesh: Vec<bool> = s.components.iter().enumerate().map(|(i, c)| c.mesh.is_some() && self.visible.get(i).copied().unwrap_or(true)).collect();
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

    pub fn pick(&self, o: Vec3, d: Vec3) -> Option<measure::Hit> {
        let s = self.scene.as_ref()?;
        let meshes: Vec<&sw_format::TriMesh> = s.meshes.iter().map(|m| &m.mesh).collect();
        let mut items = self.pick_items();
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
                return measure::pick(&meshes, &items, o2, d).filter(|h| h.point.dot(Vec3::from(sec.0)) <= sec.1);
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
        if let Some((lo, hi)) = self.stats.bbox {
            self.camera.fit(lo, hi, aspect);
        }
        self.needs_fit = false;
    }

    pub fn file_kind_label(&self) -> String {
        craft_formats::ext_of(&self.path).to_uppercase()
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
