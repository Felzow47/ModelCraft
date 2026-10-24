//! The document view: model tree, 3D viewport (navigation, picking, measurements, view cube,
//! overlays) and the information panel.

use crate::document::{DocState, Document, FaceSel, Section};
use crate::i18n::{tr, trf};
use crate::measure::{self, Measure, Tool};
use crate::render::{Light, LineVertex, Renderer, ViewSettings};
use crate::settings::{Background, NavStyle, Settings};
use crate::theme::{self, Tokens};
use crate::{icons, widgets};
use eframe::wgpu;
use egui::{vec2, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke};
use glam::{Mat4, Vec2 as GVec2, Vec3};

pub struct Gfx<'a> {
    pub rs: &'a egui_wgpu::RenderState,
    pub renderer: &'a mut Renderer,
    pub tex: &'a mut Option<egui::TextureId>,
}

/// Commands requested by the view (run by the app after the frame).
pub type Cmds = Vec<&'static str>;

fn c32(c: [u8; 3]) -> [f32; 3] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0]
}
fn col_f(c: Color32) -> [f32; 3] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0]
}

// ---------------------------------------------------------------- tree

pub fn tree_panel(ui: &mut egui::Ui, doc: &mut Document, cmds: &mut Cmds) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(widgets::mono_caps(tr("Arbre de création"), 10.5, t.text_faint));
    });
    ui.add_space(4.0);
    ui.add(
        egui::TextEdit::singleline(&mut doc.tree_filter)
            .hint_text(tr("Filtrer les composants…"))
            .desired_width(f32::INFINITY)
            .margin(egui::Margin::symmetric(10, 6)),
    );
    ui.add_space(6.0);
    let Some(scene) = &doc.scene else {
        if doc.is_loading() {
            ui.label(egui::RichText::new(tr("Chargement…")).color(t.text_muted));
        }
        return;
    };
    let filter = doc.tree_filter.to_lowercase();
    let n = scene.components.len();
    // rows to show
    let mut rows: Vec<usize> = Vec::new();
    if filter.is_empty() {
        let mut skip_depth: Option<usize> = None;
        for i in 0..n {
            let c = &scene.components[i];
            if let Some(d) = skip_depth {
                if c.depth > d {
                    continue;
                }
                skip_depth = None;
            }
            rows.push(i);
            if !doc.expanded.contains(&i) && doc.has_children(i) {
                skip_depth = Some(c.depth);
            }
        }
    } else {
        rows = (0..n).filter(|&i| scene.components[i].name.to_lowercase().contains(&filter)).collect();
    }
    let row_h = 26.0;
    let mut action: Option<(usize, &'static str)> = None;
    let mut hovered_row = None;
    let reveal = std::mem::take(&mut doc.reveal_in_tree);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_h, rows.len(), |ui, range| {
        for &i in &rows[range] {
            let c = &scene.components[i];
            let depth = if filter.is_empty() { c.depth } else { 0 };
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
            let vis = doc.visible.get(i).copied().unwrap_or(true);
            let selected = doc.selected == Some(i);
            let press = theme::Press::track(ui, &resp);
            if selected {
                ui.painter().rect_filled(rect, CornerRadius::same(6), t.accent_soft);
            } else {
                press.wash(ui, rect, 6, false);
            }
            if resp.hovered() {
                hovered_row = Some(i);
            }
            if reveal && selected {
                resp.scroll_to_me(Some(egui::Align::Center));
            }
            let x0 = rect.left() + 6.0 + depth as f32 * 14.0;
            let has_kids = filter.is_empty() && doc.has_children(i);
            if has_kids {
                let open = doc.expanded.contains(&i);
                let r = Rect::from_center_size(egui::pos2(x0 + 7.0, rect.center().y), vec2(16.0, 16.0));
                icons::paint(ui, r, if open { "chevron-down" } else { "chevron-right" }, 14.0, t.text_muted);
                let cr = ui.interact(r.expand(3.0), resp.id.with("chev"), Sense::click());
                if cr.clicked() {
                    action = Some((i, "toggle_expand"));
                }
            }
            let icon = if c.is_assembly { "boxes" } else if c.mesh.is_some() { "box" } else { "square-dashed" };
            let col = if vis { if selected { t.accent_text } else { t.icon } } else { t.text_faint };
            icons::paint(ui, Rect::from_center_size(egui::pos2(x0 + 25.0, rect.center().y), vec2(16.0, 16.0)), icon, 15.0, col);
            let text_col = if !vis { t.text_faint } else if selected { t.text } else { t.text };
            let font = if selected { theme::medium(13.0) } else { theme::regular(13.0) };
            let eye_w = if resp.hovered() || doc.hidden.get(i).copied().unwrap_or(false) { 26.0 } else { 0.0 };
            widgets::row_text(ui, egui::pos2(x0 + 38.0, rect.center().y), c.name.clone(), font, text_col, rect.right() - x0 - 44.0 - eye_w);
            if eye_w > 0.0 {
                let er = Rect::from_center_size(egui::pos2(rect.right() - 16.0, rect.center().y), vec2(20.0, 20.0));
                let hid = doc.hidden.get(i).copied().unwrap_or(false);
                let eresp = ui.interact(er, resp.id.with("eye"), Sense::click());
                icons::paint(ui, er.shrink(2.0), if hid { "eye-off" } else { "eye" }, 15.0, if eresp.hovered() { t.text } else { t.text_muted });
                if eresp.clicked() {
                    action = Some((i, "toggle_hidden"));
                }
                eresp.on_hover_text(if hid { tr("Afficher") } else { tr("Masquer") });
            }
            if resp.clicked() && action.is_none() {
                action = Some((i, "select"));
            }
            if resp.double_clicked() {
                action = Some((i, "zoom"));
            }
            resp.context_menu(|ui| {
                ui.set_min_width(210.0);
                ui.label(egui::RichText::new(&c.name).font(theme::semibold(12.5)).color(t.text));
                ui.separator();
                let hid = doc.hidden.get(i).copied().unwrap_or(false);
                if widgets::menu_icon_item(ui, if hid { "eye" } else { "eye-off" }, if hid { tr("Afficher") } else { tr("Masquer") }, "H").clicked() {
                    action = Some((i, "toggle_hidden"));
                    ui.close();
                }
                if widgets::menu_icon_item(ui, "focus", tr("Isoler"), "I").clicked() {
                    action = Some((i, "isolate"));
                    ui.close();
                }
                if widgets::menu_icon_item(ui, "eye", tr("Afficher tout"), &ui.ctx().format_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::H))).clicked() {
                    action = Some((i, "show_all"));
                    ui.close();
                }
                if widgets::menu_icon_item(ui, "zoom-in", tr("Zoom sur"), "").clicked() {
                    action = Some((i, "zoom"));
                    ui.close();
                }
                ui.separator();
                if widgets::menu_icon_item(ui, "copy", tr("Copier le nom"), "").clicked() {
                    ui.ctx().copy_text(c.name.clone());
                    ui.close();
                }
                if has_kids {
                    if widgets::menu_icon_item(ui, "list-tree", tr("Déplier tout"), "").clicked() {
                        action = Some((i, "expand_all"));
                        ui.close();
                    }
                }
            });
        }
    });
    doc.set_hover(hovered_row);
    if let Some((i, a)) = action {
        match a {
            "toggle_expand" => {
                if !doc.expanded.remove(&i) {
                    doc.expanded.insert(i);
                }
            }
            "expand_all" => {
                for k in doc.subtree(i) {
                    doc.expanded.insert(k);
                }
            }
            "toggle_hidden" => doc.toggle_hidden(i),
            "isolate" => doc.isolate(i),
            "show_all" => doc.show_all(),
            "select" => {
                doc.face_sel = None;
                doc.highlight_dirty = true;
                let same = doc.selected == Some(i);
                doc.select(if same { None } else { Some(i) });
                doc.reveal_in_tree = false;
            }
            "zoom" => {
                doc.select(Some(i));
                doc.reveal_in_tree = false;
                zoom_to(doc, i);
            }
            _ => {}
        }
    }
    let _ = cmds;
}

/// Frame the camera on a component's subtree.
pub fn zoom_to(doc: &mut Document, i: usize) {
    let Some(s) = &doc.scene else { return };
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for k in doc.subtree(i) {
        let Some(mi) = s.components[k].mesh else { continue };
        let m = &s.meshes[mi].mesh;
        if m.is_empty() {
            continue;
        }
        for c in 0..8 {
            let p = Vec3::new(
                if c & 1 == 0 { m.bbox_min[0] } else { m.bbox_max[0] },
                if c & 2 == 0 { m.bbox_min[1] } else { m.bbox_max[1] },
                if c & 4 == 0 { m.bbox_min[2] } else { m.bbox_max[2] },
            );
            let q = doc.display[k].transform_point3(p);
            lo = lo.min(q);
            hi = hi.max(q);
        }
    }
    if lo.x <= hi.x {
        let r = doc.camera.radius;
        doc.camera.fit(lo, hi, 1.5);
        doc.camera.radius = doc.camera.radius.max(r);
    }
}

// ---------------------------------------------------------------- viewport

pub fn viewport(ui: &mut egui::Ui, doc: &mut Document, st: &Settings, gfx: Gfx<'_>, cmds: &mut Cmds) {
    let t = Tokens::get(ui.ctx());
    let rect = ui.max_rect();
    let ppp = ui.ctx().pixels_per_point() * st.quality.render_scale();
    let px = [(rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32];
    let aspect = px[0] as f32 / px[1] as f32;
    let resp = ui.allocate_rect(rect, Sense::click_and_drag());

    let geometry = doc.scene.as_ref().is_some_and(|s| s.has_geometry());
    if !geometry {
        empty_or_preview(ui, doc, rect);
        loading_overlay(ui, doc, rect);
        return;
    }
    if gfx.renderer.ensure_target(&gfx.rs.device, px) {
        let view = &gfx.renderer.target().unwrap().resolve_view;
        let mut r = gfx.rs.renderer.write();
        match *gfx.tex {
            Some(id) => r.update_egui_texture_from_wgpu_texture(&gfx.rs.device, view, wgpu::FilterMode::Linear, id),
            None => *gfx.tex = Some(r.register_native_texture(&gfx.rs.device, view, wgpu::FilterMode::Linear)),
        }
    }
    doc.camera.fov_y = st.fov_deg.to_radians();
    if doc.needs_fit {
        doc.fit(aspect);
    }
    if let Some((yaw, pitch)) = doc.cam_goal {
        doc.camera.lerp_towards(yaw, pitch, 0.25);
        if (doc.camera.pitch - pitch).abs() < 1e-3 && ((doc.camera.yaw - yaw).rem_euclid(std::f32::consts::TAU)).min((yaw - doc.camera.yaw).rem_euclid(std::f32::consts::TAU)) < 1e-3 {
            doc.camera.yaw = yaw;
            doc.camera.pitch = pitch;
            doc.cam_goal = None;
        }
        ui.ctx().request_repaint();
    }

    // ---- navigation
    let (scroll, zoom2, pointer, mods) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta(), i.pointer.hover_pos(), i.modifiers));
    let to_ndc = |p: Pos2| GVec2::new((p.x - rect.center().x) / (rect.width() * 0.5), -(p.y - rect.center().y) / (rect.height() * 0.5));
    if resp.dragged() {
        let d = resp.drag_delta();
        let gd = GVec2::new(d.x, d.y);
        use egui::PointerButton::*;
        let (l, m, r) = (resp.dragged_by(Primary), resp.dragged_by(Middle), resp.dragged_by(Secondary));
        enum Op {
            Orbit,
            Pan,
            Zoom,
            None,
        }
        let op = match st.nav {
            NavStyle::SolidWorks => {
                if r || (m && mods.command) || (l && mods.shift) {
                    Op::Pan
                } else if m && mods.shift {
                    Op::Zoom
                } else if l || m {
                    Op::Orbit
                } else {
                    Op::None
                }
            }
            NavStyle::Blender => {
                if m && mods.shift {
                    Op::Pan
                } else if m && mods.command {
                    Op::Zoom
                } else if m || (l && mods.alt) {
                    Op::Orbit
                } else {
                    Op::None
                }
            }
            NavStyle::Cad => {
                if m && mods.shift {
                    Op::Orbit
                } else if m || r {
                    Op::Pan
                } else if l {
                    Op::Orbit
                } else {
                    Op::None
                }
            }
        };
        match op {
            Op::Orbit => {
                doc.cam_goal = None;
                doc.camera.orbit(gd)
            }
            Op::Pan => doc.camera.pan(gd, rect.height()),
            Op::Zoom => doc.camera.dolly((d.y * 0.005).exp()),
            Op::None => {}
        }
    }
    if resp.hovered() {
        let mut factor = 1.0f32;
        if scroll != 0.0 {
            let s = if st.invert_zoom { scroll } else { -scroll };
            factor *= (s * 0.0025).exp();
        }
        if zoom2 != 1.0 {
            factor /= zoom2;
        }
        if factor != 1.0 {
            let ndc = pointer.map(to_ndc).unwrap_or(GVec2::ZERO);
            doc.camera.zoom(factor, ndc, aspect);
        }
    }
    if resp.double_clicked() && doc.measure.tool == Tool::Select {
        doc.needs_fit = true;
    }

    // ---- picking
    let hover_hit = if doc.measure.tool != Tool::Select && resp.hovered() && !resp.dragged() {
        pointer.and_then(|p| {
            let (o, d) = doc.camera.ray(to_ndc(p), aspect);
            doc.pick(o, d)
        })
    } else {
        None
    };
    let clicked = resp.clicked_by(egui::PointerButton::Primary);
    let right_clicked = resp.secondary_clicked();
    if clicked || right_clicked {
        if let Some(p) = resp.interact_pointer_pos() {
            let (o, d) = doc.camera.ray(to_ndc(p), aspect);
            let hit = doc.pick(o, d);
            if clicked {
                on_click(doc, hit, st, rect, to_ndc(p));
            } else if let Some(h) = hit {
                if doc.selected != Some(h.comp) {
                    doc.face_sel = None;
                    doc.highlight_dirty = true;
                    doc.select(Some(h.comp));
                }
            }
        }
    }

    // ---- render
    let accent = col_f(t.accent);
    doc.sync_gpu(&gfx.rs.device, [accent[0], accent[1], accent[2], 1.0]);
    let view = doc.camera.view();
    let proj = doc.camera.proj(aspect);
    let vp = proj * view;
    let (bg_top, bg_bottom) = background(st, &t);
    let mut lines = Vec::new();
    if st.show_grid {
        grid_lines(doc, &t, &mut lines);
    }
    if doc.show_bbox {
        bbox_lines(doc, t.accent, &mut lines);
    }
    let vs = ViewSettings {
        view_proj: vp,
        view,
        ortho: doc.camera.ortho,
        edges: st.edges,
        edge_width: st.edge_width,
        edge_color: { let c = c32(st.edge_color); [c[0], c[1], c[2], 0.9] },
        base_color: c32(st.base_color),
        bg_top,
        bg_bottom,
        light: Light::preset(st.lighting),
        section: doc.section_plane(),
        cap_color: col_f(t.accent.lerp_to_gamma(Color32::BLACK, 0.25)),
        lines,
    };
    gfx.renderer.render(&gfx.rs.device, &gfx.rs.queue, doc.gpu.as_ref(), &vs);
    if let Some(id) = *gfx.tex {
        ui.painter().image(id, rect, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
    }

    // ---- overlays
    let project = |p: Vec3| -> Option<Pos2> {
        let c = vp * p.extend(1.0);
        if c.w <= 1e-9 {
            return None;
        }
        let n = c.truncate() / c.w;
        Some(egui::pos2(rect.center().x + n.x * rect.width() * 0.5, rect.center().y - n.y * rect.height() * 0.5))
    };
    let painter = ui.painter_at(rect);
    if doc.show_bbox {
        bbox_labels(&painter, doc, st, &t, &project);
    }
    draw_measures(&painter, doc, st, &t, &project, hover_hit);
    if st.show_axes {
        triad(&painter, rect, view, &t);
    }
    if st.show_viewcube {
        viewcube(ui, rect, doc);
    }
    toolbar(ui, rect, doc, st, cmds);
    if let Some(msg) = &doc.measure.message {
        let pos = rect.center_bottom() - vec2(0.0, 26.0);
        let g = painter.layout_no_wrap(msg.clone(), theme::medium(12.5), t.text);
        let r = Rect::from_center_size(pos, g.size() + vec2(24.0, 12.0));
        painter.rect(r, CornerRadius::same(14), t.card.gamma_multiply(0.94), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        painter.galley(r.center() - g.size() * 0.5, g, t.text);
    }
    if doc.measure.tool != Tool::Select && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }

    // ---- context menu
    resp.context_menu(|ui| viewport_menu(ui, doc, st, cmds));
    loading_overlay(ui, doc, rect);
}

fn background(st: &Settings, t: &Tokens) -> ([f32; 3], [f32; 3]) {
    let (top, bottom) = if st.bg_follow_theme {
        if t.dark() {
            ([0x3A, 0x3E, 0x47], [0x17, 0x18, 0x1C])
        } else {
            ([0xF4, 0xF5, 0xF7], [0xC9, 0xCE, 0xD6])
        }
    } else {
        (st.bg_top, st.bg_bottom)
    };
    match st.background {
        Background::Gradient => (c32(top), c32(bottom)),
        Background::Solid => (c32(bottom), c32(bottom)),
    }
}

fn on_click(doc: &mut Document, hit: Option<measure::Hit>, st: &Settings, _rect: Rect, _ndc: GVec2) {
    let tool = doc.measure.tool;
    doc.measure.message = None;
    let Some(h) = hit else {
        if tool == Tool::Select {
            doc.select(None);
            doc.face_sel = None;
            doc.highlight_dirty = true;
            doc.instances_dirty = true;
        }
        return;
    };
    let Some(s) = &doc.scene else { return };
    let m = &s.meshes[h.mesh].mesh;
    let w = doc.display[h.comp];
    let tris = measure::face_tris(m, h.face, h.tri);
    let fs = FaceSel { comp: h.comp, mesh: h.mesh, face: h.face, tri: h.tri };
    match tool {
        Tool::Select => {
            // first click selects the component, a second click on it selects the face
            if doc.selected == Some(h.comp) && doc.face_sel.is_none_or(|f| f != fs) {
                doc.face_sel = Some(fs);
                let (n, area, c) = measure::face_props(m, tris, w);
                doc.measure.results.retain(|r| !matches!(r, Measure::Face { .. }));
                doc.measure.results.push(Measure::Face { at: c, area, normal: n });
            } else {
                doc.face_sel = None;
                doc.select(Some(h.comp));
            }
            doc.highlight_dirty = true;
            doc.instances_dirty = true;
        }
        Tool::Distance => {
            let diag = doc.stats.bbox.map(|(lo, hi)| (hi - lo).length()).unwrap_or(1.0);
            let v = measure::nearest_tri_vertex(m, h.tri, w, h.point);
            let p = if v.distance(h.point) < diag * 0.015 { v } else { h.point };
            match doc.measure.pending.take() {
                None => {
                    doc.measure.pending = Some((p, h.normal));
                    doc.measure.message = Some(tr("Cliquez le second point").into());
                }
                Some((a, _)) => doc.measure.results.push(Measure::Distance { a, b: p }),
            }
        }
        Tool::Edge => {
            let diag = doc.stats.bbox.map(|(lo, hi)| (hi - lo).length()).unwrap_or(1.0);
            match measure::edge_chain_near(m, w, h.point, diag * 0.03) {
                Some(chain) => doc.measure.results.push(Measure::Edge { chain }),
                None => doc.measure.message = Some(tr("Aucune arête près du clic").into()),
            }
        }
        Tool::Diameter => match measure::fit_cylinder(m, tris.clone(), w) {
            Some(cyl) => {
                doc.face_sel = Some(fs);
                doc.highlight_dirty = true;
                doc.measure.results.push(Measure::Diameter { cyl, at: h.point })
            }
            None => {
                // a circular edge near the click also gives a diameter
                let diag = doc.stats.bbox.map(|(lo, hi)| (hi - lo).length()).unwrap_or(1.0);
                match measure::edge_chain_near(m, w, h.point, diag * 0.02).filter(|c| c.circle().is_some()) {
                    Some(chain) => doc.measure.results.push(Measure::Edge { chain }),
                    None => doc.measure.message = Some(tr("Cette face n'est pas cylindrique").into()),
                }
            }
        },
        Tool::Angle => {
            let (n, _, c) = measure::face_props(m, tris, w);
            doc.face_sel = Some(fs);
            doc.highlight_dirty = true;
            match doc.measure.pending.take() {
                None => {
                    doc.measure.pending = Some((c, n));
                    doc.measure.message = Some(tr("Cliquez la seconde face").into());
                }
                Some((a, na)) => doc.measure.results.push(Measure::Angle { a, na, b: c, nb: n }),
            }
        }
    }
    let _ = st;
}

fn label_pill(p: &egui::Painter, at: Pos2, text: &str, t: &Tokens) {
    let g = p.layout_no_wrap(text.to_owned(), theme::semibold(12.0), t.on_accent());
    let r = Rect::from_center_size(at, g.size() + vec2(14.0, 8.0));
    p.rect_filled(r.translate(vec2(0.0, 1.5)), CornerRadius::same(9), Color32::from_black_alpha(60));
    p.rect_filled(r, CornerRadius::same(9), t.accent);
    p.galley(r.center() - g.size() * 0.5, g, t.on_accent());
}

fn draw_measures(p: &egui::Painter, doc: &Document, st: &Settings, t: &Tokens, project: &dyn Fn(Vec3) -> Option<Pos2>, hover: Option<measure::Hit>) {
    let line = Stroke::new(2.0, t.accent);
    let halo = Stroke::new(4.0, Color32::from_black_alpha(90));
    let seg = |a: Pos2, b: Pos2| {
        p.line_segment([a, b], halo);
        p.line_segment([a, b], line);
    };
    let dot = |a: Pos2| {
        p.circle_filled(a, 4.5, Color32::WHITE);
        p.circle_filled(a, 3.0, t.accent);
    };
    for r in &doc.measure.results {
        match r {
            Measure::Distance { a, b } => {
                if let (Some(pa), Some(pb)) = (project(*a), project(*b)) {
                    seg(pa, pb);
                    dot(pa);
                    dot(pb);
                    label_pill(p, pa.lerp(pb, 0.5) - vec2(0.0, 14.0), &st.fmt_len(a.distance(*b) as f64), t);
                }
            }
            Measure::Edge { chain } => {
                let pts: Vec<Pos2> = chain.points.iter().filter_map(|q| project(*q)).collect();
                for w in pts.windows(2) {
                    seg(w[0], w[1]);
                }
                if chain.closed && pts.len() > 2 {
                    seg(pts[pts.len() - 1], pts[0]);
                }
                if let Some(mid) = pts.get(pts.len() / 2) {
                    let mut s = st.fmt_len(chain.length() as f64);
                    if let Some((_, r, _)) = chain.circle() {
                        s = format!("{s}  ·  Ø {}", st.fmt_len(2.0 * r as f64));
                    }
                    label_pill(p, *mid - vec2(0.0, 16.0), &s, t);
                }
            }
            Measure::Diameter { cyl, at } => {
                // circle through the clicked point, around the axis
                let along = (*at - cyl.center).dot(cyl.axis);
                let c = cyl.center + cyl.axis * along;
                let u = cyl.axis.any_orthonormal_vector();
                let w = cyl.axis.cross(u);
                let pts: Vec<Pos2> = (0..=64)
                    .filter_map(|k| {
                        let a = k as f32 / 64.0 * std::f32::consts::TAU;
                        project(c + (u * a.cos() + w * a.sin()) * cyl.radius)
                    })
                    .collect();
                for w2 in pts.windows(2) {
                    seg(w2[0], w2[1]);
                }
                if let (Some(pc), Some(pe)) = (project(c - u * cyl.radius), project(c + u * cyl.radius)) {
                    seg(pc, pe);
                    let kind = if cyl.inward { tr("Perçage") } else { tr("Cylindre") };
                    label_pill(p, pc.lerp(pe, 0.5) - vec2(0.0, 16.0), &format!("{kind} Ø {}", st.fmt_len(2.0 * cyl.radius as f64)), t);
                }
            }
            Measure::Angle { a, na, b, nb } => {
                let ang = na.dot(*nb).clamp(-1.0, 1.0).acos().to_degrees();
                if let (Some(pa), Some(pb)) = (project(*a), project(*b)) {
                    dot(pa);
                    dot(pb);
                    p.line_segment([pa, pb], Stroke::new(1.5, t.accent.gamma_multiply(0.7)));
                    label_pill(p, pa.lerp(pb, 0.5) - vec2(0.0, 14.0), &format!("{:.*}°", st.decimals.min(2) as usize, ang), t);
                }
            }
            Measure::Face { at, area, .. } => {
                if let Some(pa) = project(*at) {
                    label_pill(p, pa, &st.fmt_area(*area as f64), t);
                }
            }
        }
    }
    if let Some((a, _)) = doc.measure.pending {
        if let Some(pa) = project(a) {
            dot(pa);
            if let (Some(h), Tool::Distance) = (hover, doc.measure.tool) {
                if let Some(pb) = project(h.point) {
                    p.line_segment([pa, pb], Stroke::new(1.5, t.accent.gamma_multiply(0.6)));
                    label_pill(p, pa.lerp(pb, 0.5) - vec2(0.0, 14.0), &st.fmt_len(a.distance(h.point) as f64), t);
                }
            }
        }
    }
    if let Some(h) = hover {
        if let Some(ph) = project(h.point) {
            p.circle_stroke(ph, 6.0, Stroke::new(1.5, t.accent));
        }
    }
}

fn grid_lines(doc: &Document, t: &Tokens, out: &mut Vec<LineVertex>) {
    let Some((lo, hi)) = doc.stats.bbox else { return };
    let size = (hi - lo).max_element().max(1e-6);
    let raw = size / 10.0;
    let mag = 10f32.powf(raw.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0].iter().map(|k| k * mag).find(|s| *s >= raw).unwrap_or(raw);
    let c = (lo + hi) * 0.5;
    let half = (size * 1.2 / step).ceil() * step;
    let y = lo.y;
    let col = if t.dark() { [1.0, 1.0, 1.0, 0.10] } else { [0.0, 0.0, 0.0, 0.12] };
    let strong = if t.dark() { [1.0, 1.0, 1.0, 0.22] } else { [0.0, 0.0, 0.0, 0.25] };
    let cx = (c.x / step).round() * step;
    let cz = (c.z / step).round() * step;
    let n = (half / step) as i32;
    for k in -n..=n {
        let o = k as f32 * step;
        let cc = if k == 0 { strong } else { col };
        out.push(LineVertex { pos: [cx + o, y, cz - half], col: cc });
        out.push(LineVertex { pos: [cx + o, y, cz + half], col: cc });
        out.push(LineVertex { pos: [cx - half, y, cz + o], col: cc });
        out.push(LineVertex { pos: [cx + half, y, cz + o], col: cc });
    }
}

fn bbox_lines(doc: &Document, accent: Color32, out: &mut Vec<LineVertex>) {
    let Some((lo, hi)) = doc.stats.bbox else { return };
    let col = { let a = col_f(accent); [a[0], a[1], a[2], 0.9] };
    let c = |k: usize| Vec3::new(if k & 1 == 0 { lo.x } else { hi.x }, if k & 2 == 0 { lo.y } else { hi.y }, if k & 4 == 0 { lo.z } else { hi.z });
    for (a, b) in [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)] {
        out.push(LineVertex { pos: c(a).to_array(), col });
        out.push(LineVertex { pos: c(b).to_array(), col });
    }
}

fn bbox_labels(p: &egui::Painter, doc: &Document, st: &Settings, t: &Tokens, project: &dyn Fn(Vec3) -> Option<Pos2>) {
    let Some((lo, hi)) = doc.stats.bbox else { return };
    let d = hi - lo;
    let mids = [
        (Vec3::new((lo.x + hi.x) * 0.5, lo.y, hi.z), d.x),
        (Vec3::new(hi.x, (lo.y + hi.y) * 0.5, hi.z), d.y),
        (Vec3::new(hi.x, lo.y, (lo.z + hi.z) * 0.5), d.z),
    ];
    for (m, v) in mids {
        if let Some(pt) = project(m) {
            label_pill(p, pt, &st.fmt_len(v as f64), t);
        }
    }
}

fn triad(p: &egui::Painter, rect: Rect, view: Mat4, t: &Tokens) {
    let c = rect.left_bottom() + vec2(46.0, -46.0);
    p.circle_filled(c, 34.0, if t.dark() { Color32::from_black_alpha(70) } else { Color32::from_white_alpha(120) });
    let mut axes = [
        (Vec3::X, "X", Color32::from_rgb(235, 90, 90)),
        (Vec3::Y, "Y", Color32::from_rgb(100, 200, 100)),
        (Vec3::Z, "Z", Color32::from_rgb(90, 140, 250)),
    ];
    axes.sort_by(|a, b| view.transform_vector3(a.0).z.partial_cmp(&view.transform_vector3(b.0).z).unwrap());
    for (a, name, col) in axes {
        let v = view.transform_vector3(a);
        let end = c + vec2(v.x, -v.y) * 26.0;
        p.line_segment([c, end], Stroke::new(2.0, col));
        p.text(end + vec2(v.x, -v.y) * 7.0, Align2::CENTER_CENTER, name, theme::semibold(11.0), col);
    }
}

/// Orientation cube in the top-right corner; click a face to look at it.
fn viewcube(ui: &mut egui::Ui, rect: Rect, doc: &mut Document) {
    let t = Tokens::get(ui.ctx());
    let size = 44.0;
    let c = rect.right_top() + vec2(-70.0, 112.0);
    let view = doc.camera.view();
    let faces: [(Vec3, &str, f32, f32); 6] = [
        (Vec3::Z, tr("FACE"), 0.0, 0.0),
        (Vec3::NEG_Z, tr("ARRIÈRE"), 180.0, 0.0),
        (Vec3::X, tr("DROITE"), 90.0, 0.0),
        (Vec3::NEG_X, tr("GAUCHE"), -90.0, 0.0),
        (Vec3::Y, tr("DESSUS"), 0.0, 90.0),
        (Vec3::NEG_Y, tr("DESSOUS"), 0.0, -90.0),
    ];
    let to2 = |v: Vec3| -> (Pos2, f32) {
        let q = view.transform_vector3(v);
        (c + vec2(q.x, -q.y) * size * 0.5, q.z)
    };
    let mut polys = Vec::new();
    for (n, label, yaw, pitch) in faces {
        let (u, w) = if n.y.abs() > 0.5 { (Vec3::X, Vec3::Z * -n.y) } else { (n.cross(Vec3::Y).normalize() * -1.0, Vec3::Y) };
        let corners = [n - u - w, n + u - w, n + u + w, n - u + w].map(|k| to2(k).0);
        let depth = view.transform_vector3(n).z;
        polys.push((depth, corners, label, yaw, pitch, n, u, w));
    }
    polys.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let area = Rect::from_center_size(c, vec2(size * 1.9, size * 1.9));
    let resp = ui.interact(area, ui.id().with(("viewcube", doc.id)), Sense::click());
    let painter = ui.painter_at(rect);
    let mut hovered = None;
    for (k, (depth, corners, ..)) in polys.iter().enumerate() {
        if *depth > 0.05 {
            if let Some(pp) = pointer {
                if inside(pp, corners) {
                    hovered = Some(k);
                }
            }
        }
    }
    for (k, (depth, corners, label, _, _, n, u, _w)) in polys.iter().enumerate() {
        if *depth <= 0.02 {
            continue;
        }
        let shade = 0.55 + 0.45 * depth.clamp(0.0, 1.0);
        let base = if t.dark() { Color32::from_rgb(0x2E, 0x31, 0x38) } else { Color32::from_rgb(0xF4, 0xF5, 0xF7) };
        let fill = if hovered == Some(k) { t.accent } else { base.gamma_multiply(0.75 + 0.25 * shade) };
        painter.add(egui::Shape::convex_polygon(corners.to_vec(), fill, Stroke::new(1.0, t.border.gamma_multiply(1.4))));
        if *depth > 0.35 {
            // label drawn along the face's horizontal direction, scaled by its tilt
            let cen = to2(*n).0;
            let ux = to2(*n + *u).0 - cen;
            let a = ux.y.atan2(ux.x);
            let fs = (8.5 * depth.sqrt()).max(6.0);
            let g = painter.layout_no_wrap(label.to_string(), theme::semibold(fs), if hovered == Some(k) { t.on_accent() } else { t.text_muted });
            let mut shape = egui::epaint::TextShape::new(cen, g, t.text_muted);
            let half = shape.galley.size() * 0.5;
            let rot = egui::emath::Rot2::from_angle(a);
            shape.pos = cen - rot * half;
            shape.angle = a;
            painter.add(shape);
        }
    }
    if resp.clicked() {
        if let Some(k) = hovered {
            let (_, _, _, yaw, pitch, ..) = polys[k];
            doc.cam_goal = Some((yaw.to_radians(), pitch.to_radians()));
        }
    }
    // home button: isometric view
    let hb = Rect::from_center_size(c + vec2(-size * 0.95, -size * 0.95), vec2(22.0, 22.0));
    let hresp = ui.interact(hb, ui.id().with(("cube-home", doc.id)), Sense::click());
    painter.rect_filled(hb, CornerRadius::same(6), if hresp.hovered() { t.hover } else { Color32::TRANSPARENT });
    icons::paint(ui, hb.shrink(4.0), "house", 13.0, t.text_muted);
    if hresp.on_hover_text(tr("Vue isométrique")).clicked() {
        doc.cam_goal = Some((45f32.to_radians(), 35.264f32.to_radians()));
    }
}

fn inside(p: Pos2, poly: &[Pos2; 4]) -> bool {
    let mut sign = 0.0f32;
    for k in 0..4 {
        let a = poly[k];
        let b = poly[(k + 1) % 4];
        let cr = (b - a).x * (p - a).y - (b - a).y * (p - a).x;
        if cr.abs() < 1e-6 {
            continue;
        }
        if sign == 0.0 {
            sign = cr.signum();
        } else if cr.signum() != sign {
            return false;
        }
    }
    true
}

/// Floating tool strip at the top of the viewport.
fn toolbar(ui: &mut egui::Ui, rect: Rect, doc: &mut Document, st: &Settings, cmds: &mut Cmds) {
    let t = Tokens::get(ui.ctx());
    let tools: [(Tool, &str, &str, &'static str); 5] = [
        (Tool::Select, "mouse-pointer-2", tr("Sélection"), "tool.select"),
        (Tool::Distance, "ruler", tr("Distance"), "tool.distance"),
        (Tool::Edge, "spline-pointer", tr("Arête"), "tool.edge"),
        (Tool::Diameter, "diameter", tr("Diamètre"), "tool.diameter"),
        (Tool::Angle, "drafting-compass", tr("Angle"), "tool.angle"),
    ];
    let toggles: [(&str, bool, &str, &'static str); 5] = [
        ("maximize", false, tr("Ajuster à l'écran"), "view.fit"),
        ("cuboid", doc.camera.ortho, tr("Projection orthographique"), "view.ortho"),
        ("spline-pointer", st.edges, tr("Arêtes"), "view.edges"),
        ("scan", doc.show_bbox, tr("Boîte englobante"), "view.bbox"),
        ("scissors", doc.section.is_some(), tr("Plan de coupe"), "view.section"),
    ];
    let b = 30.0;
    let w = b * 10.0 + 4.0 * 9.0 + 14.0 + 16.0;
    let r = Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + 28.0), vec2(w, b + 10.0));
    egui::Area::new(ui.id().with(("toolbar", doc.id))).fixed_pos(r.min).order(egui::Order::Middle).show(ui.ctx(), |ui| {
        egui::Frame::new()
            .fill(t.card.gamma_multiply(0.96))
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(egui::Margin::same(5))
            .shadow(egui::Shadow { offset: [0, 4], blur: 14, spread: 0, color: t.shadow })
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.horizontal(|ui| {
                    for (tool, icon, tip, cmd) in tools {
                        let sc = crate::commands::shortcut_text(ui.ctx(), cmd);
                        if icons::button(ui, icon, b, doc.measure.tool == tool, &format!("{tip}  ({sc})")).clicked() {
                            cmds.push(cmd);
                        }
                    }
                    ui.add_space(3.0);
                    let (sep, _) = ui.allocate_exact_size(vec2(1.0, b - 8.0), Sense::hover());
                    ui.painter().rect_filled(sep, CornerRadius::ZERO, t.divider);
                    ui.add_space(3.0);
                    for (icon, on, tip, cmd) in toggles {
                        let sc = crate::commands::shortcut_text(ui.ctx(), cmd);
                        let tip = if sc.is_empty() { tip.to_string() } else { format!("{tip}  ({sc})") };
                        if icons::button(ui, icon, b, on, &tip).clicked() {
                            cmds.push(cmd);
                        }
                    }
                });
            });
    });
}

fn viewport_menu(ui: &mut egui::Ui, doc: &mut Document, st: &Settings, cmds: &mut Cmds) {
    ui.set_min_width(230.0);
    let ctx = ui.ctx().clone();
    let sc = |id: &str| crate::commands::shortcut_text(&ctx, id);
    let item = |ui: &mut egui::Ui, cmds: &mut Cmds, id: &'static str| {
        let c = crate::commands::get(id);
        if widgets::menu_icon_item(ui, c.icon, tr(c.label), &crate::commands::shortcut_text(ui.ctx(), id)).clicked() {
            cmds.push(id);
            ui.close();
        }
    };
    let sel = doc.selected.and_then(|i| doc.scene.as_ref().map(|s| (i, s.components[i].name.clone(), s.components[i].parent)));
    if let Some((i, name, parent)) = sel {
        let t = Tokens::get(ui.ctx());
        ui.label(egui::RichText::new(name).font(theme::semibold(12.5)).color(t.text));
        ui.separator();
        item(ui, cmds, "edit.hide");
        item(ui, cmds, "edit.isolate");
        if widgets::menu_icon_item(ui, "zoom-in", tr("Zoom sur"), "").clicked() {
            zoom_to(doc, i);
            ui.close();
        }
        if let Some(pi) = parent {
            if widgets::menu_icon_item(ui, "list-tree", tr("Sélectionner le parent"), "").clicked() {
                doc.select(Some(pi));
                ui.close();
            }
        }
        ui.separator();
    }
    item(ui, cmds, "edit.show_all");
    item(ui, cmds, "view.fit");
    ui.menu_button(tr("Vues standard"), |ui| {
        for id in ["view.iso", "view.front", "view.top", "view.right", "view.back", "view.bottom", "view.left"] {
            let c = crate::commands::get(id);
            if widgets::menu_item(ui, tr(c.label), &sc(id)).clicked() {
                cmds.push(id);
                ui.close();
            }
        }
    });
    ui.menu_button(tr("Mesurer"), |ui| {
        for id in ["tool.distance", "tool.edge", "tool.diameter", "tool.angle", "tool.clear"] {
            let c = crate::commands::get(id);
            if widgets::menu_icon_item(ui, c.icon, tr(c.label), &sc(id)).clicked() {
                cmds.push(id);
                ui.close();
            }
        }
    });
    ui.separator();
    for (id, on) in [("view.ortho", doc.camera.ortho), ("view.edges", st.edges), ("view.bbox", doc.show_bbox), ("view.section", doc.section.is_some()), ("view.grid", st.show_grid)] {
        let c = crate::commands::get(id);
        if widgets::menu_check(ui, on, tr(c.label), &sc(id)).clicked() {
            cmds.push(id);
            ui.close();
        }
    }
    ui.separator();
    item(ui, cmds, "edit.copy_dims");
    item(ui, cmds, "file.screenshot");
}

fn empty_or_preview(ui: &mut egui::Ui, doc: &Document, rect: Rect) {
    let t = Tokens::get(ui.ctx());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::ZERO, t.pasteboard);
    if let Some(tex) = &doc.preview {
        let s = tex.size_vec2();
        let k = (rect.width() * 0.6 / s.x).min(rect.height() * 0.6 / s.y).min(3.0);
        let r = Rect::from_center_size(rect.center() - vec2(0.0, 20.0), s * k);
        p.image(tex.id(), r, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        p.text(r.center_bottom() + vec2(0.0, 18.0), Align2::CENTER_TOP, tr("Aperçu enregistré dans le fichier (pas de géométrie 3D lisible)"), theme::regular(12.5), t.text_muted);
    } else if let DocState::Failed(e) = &doc.state {
        icons::paint(ui, Rect::from_center_size(rect.center() - vec2(0.0, 46.0), vec2(40.0, 40.0)), "triangle-alert", 38.0, t.warning);
        p.text(rect.center(), Align2::CENTER_CENTER, tr("Impossible d'ouvrir ce fichier"), theme::semibold(17.0), t.text);
        let g = p.layout(e.clone(), theme::regular(13.0), t.text_muted, (rect.width() - 80.0).clamp(200.0, 620.0));
        p.galley(rect.center() + vec2(-g.size().x * 0.5, 22.0), g, t.text_muted);
    } else if !doc.is_loading() {
        p.text(rect.center(), Align2::CENTER_CENTER, tr("Ce fichier ne contient pas de géométrie affichable"), theme::medium(14.0), t.text_muted);
    }
}

fn loading_overlay(ui: &mut egui::Ui, doc: &Document, rect: Rect) {
    let Some((f, stage, secs)) = doc.progress() else { return };
    let t = Tokens::get(ui.ctx());
    let p = ui.painter_at(rect);
    if doc.scene.is_none() {
        p.rect_filled(rect, CornerRadius::ZERO, t.pasteboard);
    }
    let card = Rect::from_center_size(rect.center(), vec2(360.0, 112.0));
    p.rect(card, CornerRadius::same(12), t.card, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let time = ui.input(|i| i.time) as f32;
    // spinner
    let sc = card.left_top() + vec2(34.0, 36.0);
    for k in 0..12 {
        let a = k as f32 / 12.0 * std::f32::consts::TAU + time * 6.0;
        let alpha = ((k as f32) / 12.0).powf(1.5);
        p.line_segment([sc + vec2(a.cos(), a.sin()) * 6.0, sc + vec2(a.cos(), a.sin()) * 11.0], Stroke::new(2.0, t.accent.gamma_multiply(alpha)));
    }
    widgets::row_text(ui, card.left_top() + vec2(58.0, 30.0), trf("Ouverture de {name}", &[("name", &doc.title)]), theme::semibold(13.5), t.text, 290.0);
    widgets::row_text(ui, card.left_top() + vec2(58.0, 50.0), format!("{}  ·  {:.1} s", tr_stage(&stage), secs), theme::regular(12.0), t.text_muted, 290.0);
    let track = Rect::from_min_size(card.left_top() + vec2(20.0, 76.0), vec2(320.0, 6.0));
    p.rect_filled(track, CornerRadius::same(3), t.hover);
    let shown = ui.ctx().animate_value_with_time(egui::Id::new(("load-bar", doc.id)), f.clamp(0.0, 1.0), 0.25);
    if shown > 0.001 {
        p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * shown, 6.0)), CornerRadius::same(3), t.accent);
    } else {
        // indeterminate: a sliding band
        let x = (time * 0.8).fract();
        let r = Rect::from_min_size(track.min + vec2(track.width() * x * 0.75, 0.0), vec2(track.width() * 0.25, 6.0));
        p.rect_filled(r, CornerRadius::same(3), t.accent);
    }
    p.text(track.right_bottom() + vec2(0.0, 6.0), Align2::RIGHT_TOP, format!("{:.0} %", shown * 100.0), theme::mono(10.5), t.text_faint);
    ui.ctx().request_repaint();
}

/// Stage labels come from the loaders in French.
/// Progress stage labels reported by the loaders (French), translated for display.
pub const STAGES: &[&str] = &[
    "Lecture du fichier…",
    "Lecture STEP…",
    "Lecture IGES…",
    "Conversion de la géométrie…",
    "Maillage des surfaces…",
    "Construction de la scène…",
];

pub fn tr_stage(s: &str) -> String {
    for k in STAGES {
        if s == *k {
            return tr(k).to_string();
        }
    }
    s.to_string()
}

// ---------------------------------------------------------------- info panel

pub fn info_panel(ui: &mut egui::Ui, doc: &mut Document, st: &Settings, cmds: &mut Cmds) {
    let t = Tokens::get(ui.ctx());
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(6.0);
        let card = |ui: &mut egui::Ui, title: &str, add: &mut dyn FnMut(&mut egui::Ui)| {
            ui.label(widgets::mono_caps(title, 10.5, t.text_faint));
            ui.add_space(4.0);
            egui::Frame::new()
                .fill(t.card)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    add(ui)
                });
            ui.add_space(12.0);
        };
        let kv = |ui: &mut egui::Ui, k: &str, v: String| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(k).font(theme::regular(12.5)).color(t.text_muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(v).font(theme::medium(12.5)).color(t.text));
                });
            });
        };
        card(ui, tr("Fichier"), &mut |ui| {
            ui.label(egui::RichText::new(&doc.title).font(theme::semibold(13.5)).color(t.text));
            ui.add_space(2.0);
            kv(ui, tr("Format"), doc.scene.as_ref().map(|s| scene_format(s, doc)).unwrap_or_else(|| doc.file_kind_label()));
            kv(ui, tr("Taille"), widgets::human_size(doc.size));
            if let Some(lt) = doc.load_time {
                kv(ui, tr("Ouverture"), format!("{lt:.2} s"));
            }
        });
        let Some(s) = &doc.scene else { return };
        let stt = doc.stats;
        card(ui, tr("Dimensions"), &mut |ui| {
            if let Some((lo, hi)) = stt.bbox {
                let d = hi - lo;
                // the model's own axes (Z-up formats are shown Y-up)
                let (x, y, z) = if s.up_z { (d.x, d.z, d.y) } else { (d.x, d.y, d.z) };
                ui.label(
                    egui::RichText::new(format!("{} × {} × {} {}", st.fmt_len_bare(x as f64), st.fmt_len_bare(y as f64), st.fmt_len_bare(z as f64), st.units.suffix()))
                        .font(theme::semibold(15.0))
                        .color(t.text),
                );
                ui.label(egui::RichText::new(tr("Longueur × largeur × hauteur (boîte englobante)")).font(theme::regular(11.0)).color(t.text_faint));
                ui.add_space(6.0);
                kv(ui, "X", st.fmt_len(x as f64));
                kv(ui, "Y", st.fmt_len(y as f64));
                kv(ui, "Z", st.fmt_len(z as f64));
                ui.add_space(4.0);
                kv(ui, tr("Volume"), st.fmt_volume(stt.volume));
                kv(ui, tr("Surface"), st.fmt_area(stt.area));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if widgets::icon_pill(ui, "copy", tr("Copier"), false).clicked() {
                        cmds.push("edit.copy_dims");
                    }
                    if widgets::icon_pill(ui, "scan", if doc.show_bbox { tr("Masquer la boîte") } else { tr("Afficher la boîte") }, false).clicked() {
                        cmds.push("view.bbox");
                    }
                });
            } else {
                ui.label(egui::RichText::new(tr("Rien de visible")).color(t.text_muted));
            }
        });
        card(ui, tr("Géométrie"), &mut |ui| {
            kv(ui, tr("Composants"), widgets::fmt_int(s.components.len()));
            kv(ui, tr("Corps visibles"), widgets::fmt_int(stt.parts));
            kv(ui, tr("Faces"), widgets::fmt_int(stt.faces));
            kv(ui, tr("Triangles"), widgets::fmt_int(stt.triangles));
            kv(ui, tr("Sommets"), widgets::fmt_int(stt.vertices));
        });
        if let Some(i) = doc.selected {
            let c = &s.components[i];
            card(ui, tr("Sélection"), &mut |ui| {
                ui.label(egui::RichText::new(&c.name).font(theme::semibold(13.0)).color(t.text));
                if !c.path.is_empty() && c.path != c.name {
                    ui.label(egui::RichText::new(&c.path).font(theme::regular(11.0)).color(t.text_faint));
                }
                if let Some(mi) = c.mesh {
                    let m = &s.meshes[mi].mesh;
                    let w = doc.display[i];
                    let lo = w.transform_point3(Vec3::from(m.bbox_min));
                    let hi = w.transform_point3(Vec3::from(m.bbox_max));
                    let d = (hi - lo).abs();
                    let (x, y, z) = if s.up_z { (d.x, d.z, d.y) } else { (d.x, d.y, d.z) };
                    kv(ui, tr("Dimensions"), format!("{} × {} × {}", st.fmt_len_bare(x as f64), st.fmt_len_bare(y as f64), st.fmt_len_bare(z as f64)));
                    let (v, a) = doc.mesh_va[mi];
                    kv(ui, tr("Volume"), st.fmt_volume(v.abs()));
                    kv(ui, tr("Surface"), st.fmt_area(a));
                    kv(ui, tr("Triangles"), widgets::fmt_int(m.triangle_count()));
                }
                if let Some(f) = doc.face_sel {
                    if let Some(sm) = s.meshes.get(f.mesh) {
                        let tris = measure::face_tris(&sm.mesh, f.face, f.tri);
                        let w = doc.display[f.comp];
                        let (_, area, _) = measure::face_props(&sm.mesh, tris.clone(), w);
                        ui.separator();
                        kv(ui, tr("Face — surface"), st.fmt_area(area as f64));
                        if let Some(cyl) = measure::fit_cylinder(&sm.mesh, tris, w) {
                            kv(ui, if cyl.inward { tr("Perçage Ø") } else { tr("Cylindre Ø") }, st.fmt_len(2.0 * cyl.radius as f64));
                        }
                    }
                }
            });
        }
        if !doc.measure.results.is_empty() || doc.measure.tool != Tool::Select {
            let mut remove = None;
            card(ui, tr("Mesures"), &mut |ui| {
                if doc.measure.results.is_empty() {
                    ui.label(egui::RichText::new(tool_hint(doc.measure.tool)).font(theme::regular(12.0)).color(t.text_muted));
                }
                for (k, r) in doc.measure.results.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let (icon, text, sub) = describe(r, st);
                        ui.add(icons::image(icon, 15.0, t.accent_text));
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(text).font(theme::semibold(12.5)).color(t.text));
                            if !sub.is_empty() {
                                ui.label(egui::RichText::new(sub).font(theme::regular(11.0)).color(t.text_faint));
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icons::button(ui, "x", 22.0, false, tr("Retirer")).clicked() {
                                remove = Some(k);
                            }
                        });
                    });
                }
                if !doc.measure.results.is_empty() {
                    ui.add_space(4.0);
                    if widgets::ghost_button(ui, "eraser", tr("Effacer les mesures")).clicked() {
                        cmds.push("tool.clear");
                    }
                }
            });
            if let Some(k) = remove {
                doc.measure.results.remove(k);
            }
        }
        if let Some(mut sec) = doc.section {
            card(ui, tr("Plan de coupe"), &mut |ui| {
                ui.horizontal(|ui| {
                    widgets::segmented(ui, &mut sec.axis, &[(0, "X"), (1, "Y"), (2, "Z")]);
                    ui.add_space(6.0);
                    if icons::button(ui, "refresh-cw", 26.0, sec.flip, tr("Inverser le côté")).clicked() {
                        sec.flip = !sec.flip;
                    }
                });
                ui.add_space(4.0);
                ui.add(egui::Slider::new(&mut sec.pos, 0.0..=1.0).show_value(false));
                if let Some((lo, hi)) = stt.bbox {
                    let a = sec.axis.min(2);
                    let v = lo[a] + (hi[a] - lo[a]) * sec.pos;
                    ui.label(egui::RichText::new(trf("Position : {v}", &[("v", &st.fmt_len(v as f64))])).font(theme::regular(11.5)).color(t.text_muted));
                }
            });
            if Some(sec) != doc.section {
                doc.section = Some(sec);
            }
        }
        if !s.warnings.is_empty() {
            card(ui, tr("Avertissements"), &mut |ui| {
                for w in s.warnings.iter().take(8) {
                    ui.label(egui::RichText::new(w).font(theme::regular(11.5)).color(t.warning));
                }
            });
        }
    });
}

fn scene_format(s: &sw_format::Scene, doc: &Document) -> String {
    let e = doc.file_kind_label();
    match s.kind {
        sw_format::DocKind::Part => format!("{e} · {}", tr("pièce")),
        sw_format::DocKind::Assembly => format!("{e} · {}", tr("assemblage")),
        sw_format::DocKind::Model => e,
    }
}

pub fn tool_hint(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => tr("Clic : sélectionner un composant · second clic : la face"),
        Tool::Distance => tr("Cliquez deux points (accroche aux sommets)"),
        Tool::Edge => tr("Cliquez près d'une arête pour sa longueur"),
        Tool::Diameter => tr("Cliquez dans un perçage ou sur un cylindre"),
        Tool::Angle => tr("Cliquez deux faces planes"),
    }
}

fn describe(r: &Measure, st: &Settings) -> (&'static str, String, String) {
    match r {
        Measure::Distance { a, b } => {
            let d = *b - *a;
            (
                "ruler",
                st.fmt_len(d.length() as f64),
                format!("ΔX {}  ΔY {}  ΔZ {}", st.fmt_len_bare(d.x.abs() as f64), st.fmt_len_bare(d.y.abs() as f64), st.fmt_len_bare(d.z.abs() as f64)),
            )
        }
        Measure::Edge { chain } => {
            let sub = match chain.circle() {
                Some((_, r, _)) => trf("Arc / cercle · Ø {d} · R {r}", &[("d", &st.fmt_len(2.0 * r as f64)), ("r", &st.fmt_len(r as f64))]),
                None => {
                    if chain.closed {
                        tr("Arête fermée").to_string()
                    } else {
                        trf("{n} segments", &[("n", &chain.points.len().saturating_sub(1).to_string())])
                    }
                }
            };
            ("spline-pointer", trf("Arête : {l}", &[("l", &st.fmt_len(chain.length() as f64))]), sub)
        }
        Measure::Diameter { cyl, .. } => (
            "diameter",
            format!("Ø {}", st.fmt_len(2.0 * cyl.radius as f64)),
            format!(
                "{} · R {} · {:.0}°",
                if cyl.inward { tr("Perçage") } else { tr("Cylindre") },
                st.fmt_len(cyl.radius as f64),
                cyl.sweep.to_degrees()
            ),
        ),
        Measure::Angle { na, nb, .. } => {
            let a = na.dot(*nb).clamp(-1.0, 1.0).acos().to_degrees();
            ("drafting-compass", format!("{:.2}°", a), trf("Supplément {s}°", &[("s", &format!("{:.2}", 180.0 - a))]))
        }
        Measure::Face { area, .. } => ("square-mouse-pointer", st.fmt_area(*area as f64), tr("Surface de la face").to_string()),
    }
}

/// Text copied by "Copier les dimensions".
pub fn dims_text(doc: &Document, st: &Settings) -> Option<String> {
    let s = doc.scene.as_ref()?;
    let (lo, hi) = doc.stats.bbox?;
    let d = hi - lo;
    let (x, y, z) = if s.up_z { (d.x, d.z, d.y) } else { (d.x, d.y, d.z) };
    Some(format!(
        "{}: {} × {} × {} {}",
        doc.title,
        st.fmt_len_bare(x as f64),
        st.fmt_len_bare(y as f64),
        st.fmt_len_bare(z as f64),
        st.units.suffix()
    ))
}

pub fn toggle_section(doc: &mut Document) {
    doc.section = match doc.section {
        Some(_) => None,
        None => Some(Section { axis: 0, pos: 0.5, flip: false }),
    };
}
