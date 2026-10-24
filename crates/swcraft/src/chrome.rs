//! Window chrome in the ArtCraft layout (ported from PdfCraft's chrome.rs, MIT OR Apache-2.0,
//! © ArtCraft Team and PdfCraft contributors): tab strip, mode bar, status bar.

use crate::app::{App, Dialog, SettingsCat};
use crate::commands;
use crate::i18n::{tr, trf};
use crate::icons;
use crate::measure::Tool;
use crate::theme::{self, Press, Tokens};
use crate::viewer;
use crate::widgets;
use egui::{vec2, Align, Align2, Color32, CornerRadius, Layout, Rect, Sense, Stroke};

const TAB_STRIP: f32 = 38.0;
const TAB_HEIGHT: f32 = 30.0;
const STRIP_BUTTON: f32 = 28.0;
const TAB_CHROME: f32 = 64.0;
const TAB_MAX: f32 = 240.0;

pub fn tab_strip(app: &mut App, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::top("tab_strip")
        .exact_size(TAB_STRIP)
        .frame(egui::Frame::NONE.fill(t.titlebar).inner_margin(egui::Margin { left: 8, right: 10, top: 0, bottom: 0 }))
        .show_inside(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if icons::button(ui, "house", STRIP_BUTTON, app.active.is_none(), tr("Accueil")).clicked() {
                    app.active = None;
                }
                let right_w = 2.0 * STRIP_BUTTON + 16.0;
                let open_w = widgets::text_width(ui, tr("Ouvrir"), &theme::medium(13.0)) + 38.0 + 12.0;
                let budget = (ui.available_width() - right_w - open_w).max(0.0);
                let mut close = None;
                let mut activate = None;
                egui::ScrollArea::horizontal().id_salt("doc_tabs").max_width(budget).auto_shrink([true, true]).show(ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        for i in 0..app.docs.len() {
                            let d = &app.docs[i];
                            let icon = if d.is_loading() { "loader-circle" } else { file_icon(&d.path) };
                            let resp = tab(ui, &t, icon, &d.title, &d.path.display().to_string(), app.active == Some(i), &mut close, i);
                            if resp.clicked() {
                                activate = Some(i);
                            }
                            if resp.middle_clicked() {
                                close = Some(i);
                            }
                            let path = d.path.clone();
                            resp.context_menu(|ui| {
                                if widgets::menu_icon_item(ui, "x", tr("Fermer l'onglet"), &commands::shortcut_text(ui.ctx(), "file.close")).clicked() {
                                    close = Some(i);
                                    ui.close();
                                }
                                if widgets::menu_icon_item(ui, "refresh-cw", tr("Recharger le fichier"), "").clicked() {
                                    activate = Some(i);
                                    app.cmds.push("file.reload");
                                    ui.close();
                                }
                                ui.separator();
                                if widgets::menu_icon_item(ui, "external-link", tr("Afficher dans l'explorateur"), "").clicked() {
                                    let _ = crate::platform::reveal(&path);
                                    ui.close();
                                }
                                if widgets::menu_icon_item(ui, "copy", tr("Copier le chemin du fichier"), "").clicked() {
                                    ui.ctx().copy_text(path.display().to_string());
                                    ui.close();
                                }
                            });
                        }
                    });
                });
                if widgets::ghost_button_sized(ui, "plus", tr("Ouvrir"), STRIP_BUTTON).on_hover_text(tip("file.open", ui.ctx())).clicked() {
                    app.cmds.push("file.open");
                }
                if let Some(i) = activate {
                    app.active = Some(i);
                }
                if let Some(i) = close {
                    app.close(i);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if icons::button(ui, "circle-help", STRIP_BUTTON, false, tr("Aide")).clicked() {
                        app.dialog = Some(Dialog::Shortcuts);
                    }
                    let dark = t.dark();
                    let r = icons::button(ui, if dark { "sun" } else { "moon" }, STRIP_BUTTON, false, if dark { tr("Thème clair") } else { tr("Thème sombre") });
                    if r.clicked() {
                        app.cmds.push("view.theme");
                    }
                    r.context_menu(|ui| {
                        use crate::theme::ThemePreference as P;
                        for (p, l) in [(P::System, tr("Système")), (P::Light, tr("Clair")), (P::Dark, tr("Sombre"))] {
                            if widgets::menu_check(ui, app.settings.theme == p, l, "").clicked() {
                                app.settings.theme = p;
                                app.settings_dirty = true;
                                ui.close();
                            }
                        }
                    });
                });
            });
        });
}

pub fn file_icon(p: &std::path::Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("sldasm") | Some("easm") | Some("3dxml") => "boxes",
        Some("step") | Some("stp") | Some("iges") | Some("igs") => "cuboid",
        Some("stl") | Some("obj") | Some("3ds") => "box",
        _ => "file-box",
    }
}

#[allow(clippy::too_many_arguments)]
fn tab(ui: &mut egui::Ui, t: &Tokens, icon: &str, name: &str, hover: &str, active: bool, close: &mut Option<usize>, index: usize) -> egui::Response {
    let font = theme::regular(13.0);
    let mut label = name.to_string();
    let room = TAB_MAX - TAB_CHROME;
    if widgets::text_width(ui, &label, &font) > room {
        while label.chars().count() > 4 && widgets::text_width(ui, &format!("{label}…"), &font) > room {
            label.pop();
        }
        label.push('…');
    }
    let text_w = widgets::text_width(ui, &label, &font);
    let width = text_w + TAB_CHROME;
    let height = ui.available_height().max(TAB_HEIGHT);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let press = Press::track(ui, &resp);
    let bg = if active { press.fill(t.chrome, t.chrome.lerp_to_gamma(t.hover, 0.65), t.hover) } else { press.fill(Color32::TRANSPARENT, t.hover, t.pressed) };
    let body = Rect::from_min_max(rect.min + vec2(0.0, height - TAB_HEIGHT), rect.max);
    if bg.a() > 0 {
        ui.painter().rect_filled(body, CornerRadius { nw: 7, ne: 7, sw: 0, se: 0 }, bg);
    }
    icons::paint(ui, Rect::from_min_size(body.min + vec2(8.0, (TAB_HEIGHT - 16.0) / 2.0), vec2(16.0, 16.0)), icon, 15.0, if active { t.accent } else { t.text_muted });
    ui.painter().text(body.min + vec2(30.0, TAB_HEIGHT / 2.0), Align2::LEFT_CENTER, label, font, if active { t.text } else { t.text_muted });
    let x_rect = Rect::from_center_size(body.right_center() - vec2(16.0, 0.0), vec2(20.0, 20.0));
    let x = ui.interact(x_rect, resp.id.with("close"), Sense::click());
    let xp = Press::track(ui, &x);
    xp.wash(ui, x_rect, 4, false);
    if active || resp.hovered() || x.hovered() {
        icons::paint(ui, x_rect.translate(xp.offset()), "x", 13.0, t.text_muted);
    }
    if theme::hand(x).on_hover_text(tr("Fermer l'onglet")).clicked() {
        *close = Some(index);
    }
    theme::hand(resp).on_hover_text(hover)
}

pub fn tip(id: &str, ctx: &egui::Context) -> String {
    let c = commands::get(id);
    let k = commands::shortcut_text(ctx, id);
    if k.is_empty() {
        tr(c.label).to_string()
    } else {
        format!("{} ({k})", tr(c.label))
    }
}

pub fn mode_bar(app: &mut App, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::top("mode_bar")
        .exact_size(48.0)
        .frame(egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::symmetric(10, 0)).stroke(Stroke::new(1.0, t.divider)))
        .show_inside(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                main_menu(app, ui);
                ui.add_space(6.0);
                ui.painter().vline(ui.cursor().left(), ui.max_rect().y_range().shrink(12.0), Stroke::new(1.0, t.divider));
                ui.add_space(10.0);
                let has_doc = app.doc().is_some();
                let (tool, section) = app.doc().map(|d| (d.measure.tool, d.section.is_some())).unwrap_or((Tool::Select, false));
                ui.add_enabled_ui(has_doc, |ui| {
                    let viewing = tool == Tool::Select && !section;
                    if widgets::mode_tab(ui, "rotate-3d", tr("Visualiser"), has_doc && viewing).clicked() {
                        if let Some(d) = app.doc_mut() {
                            d.measure.tool = Tool::Select;
                            d.measure.message = None;
                            d.section = None;
                            d.instances_dirty = true;
                        }
                    }
                    if widgets::mode_tab(ui, "ruler", tr("Mesurer"), has_doc && tool != Tool::Select).clicked() && tool == Tool::Select {
                        app.cmds.push("tool.distance");
                    }
                    if widgets::mode_tab(ui, "scissors", tr("Coupe"), has_doc && section).clicked() {
                        app.cmds.push("view.section");
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if icons::button(ui, "settings", 32.0, matches!(app.dialog, Some(Dialog::Settings(_))), &tip("edit.settings", ui.ctx())).clicked() {
                        app.dialog = Some(Dialog::Settings(SettingsCat::General));
                    }
                    ui.add_enabled_ui(has_doc, |ui| {
                        if icons::button(ui, "camera", 32.0, false, &tip("file.screenshot", ui.ctx())).clicked() {
                            app.cmds.push("file.screenshot");
                        }
                        if icons::button(ui, "focus", 32.0, false, &tip("view.fit", ui.ctx())).clicked() {
                            app.cmds.push("view.fit");
                        }
                    });
                    ui.add_space(8.0);
                    let key = commands::shortcut_text(ui.ctx(), "help.palette");
                    if widgets::search_box(ui, tr("Rechercher une commande"), &key, 260.0).clicked() {
                        app.palette_open = true;
                        app.palette_query.clear();
                    }
                });
            });
        });
}

/// "Menu" button (ArtCraft) followed by the classic menus Fichier / Édition / Affichage / Outils / Aide.
fn main_menu(app: &mut App, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    for g in commands::GROUPS {
        let font = theme::medium(13.0);
        let w = widgets::text_width(ui, tr(g), &font) + 20.0;
        let (rect, r) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click());
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&r));
        let press = Press::track(ui, &r);
        press.wash(ui, rect, 6, open);
        ui.painter().text(rect.center() + press.offset(), Align2::CENTER_CENTER, tr(g), font, if open { t.text } else { t.text.gamma_multiply(0.92) });
        let r = theme::hand(r);
        egui::Popup::menu(&r).show(|ui| {
            ui.set_min_width(250.0);
            menu_group(app, ui, g);
        });
    }
}

pub fn menu_group(app: &mut App, ui: &mut egui::Ui, g: &str) {
    let ctx = ui.ctx().clone();
    let has_doc = app.doc().is_some();
    let mut last_sep = "";
    for c in commands::COMMANDS.iter().filter(|c| c.group == g) {
        // separators between families: file.*, view.iso..left, toggles…
        let family = family(c.id);
        if !last_sep.is_empty() && family != last_sep {
            ui.separator();
        }
        last_sep = family;
        let enabled = !needs_doc(c.id) || has_doc;
        let key = commands::shortcut_text(&ctx, c.id);
        let check = checked(app, c.id);
        let resp = ui
            .add_enabled_ui(enabled, |ui| match check {
                Some(on) => widgets::menu_check(ui, on, tr(c.label), &key),
                None => widgets::menu_icon_item(ui, c.icon, tr(c.label), &key),
            })
            .inner;
        if resp.clicked() {
            app.cmds.push(c.id);
            ui.close();
        }
    }
    if g == "Fichier" && !app.settings.recent.is_empty() {
        // recent files submenu just after Open
    }
}

fn family(id: &str) -> &'static str {
    match id {
        "file.open" | "file.home" | "file.close" | "file.reload" => "a",
        "file.reveal" | "file.copy_path" | "file.screenshot" => "b",
        "file.quit" => "c",
        "edit.hide" | "edit.isolate" | "edit.show_all" | "edit.deselect" => "a",
        "edit.copy_dims" => "b",
        "edit.settings" => "c",
        "view.fit" => "a",
        "view.iso" | "view.front" | "view.top" | "view.right" | "view.back" | "view.bottom" | "view.left" => "b",
        "view.ortho" | "view.edges" | "view.bbox" | "view.section" | "view.grid" | "view.axes" | "view.viewcube" => "c",
        "view.tree" | "view.info" | "view.theme" | "view.fullscreen" => "d",
        "tool.select" | "tool.distance" | "tool.edge" | "tool.diameter" | "tool.angle" => "a",
        "tool.clear" => "b",
        "help.palette" | "help.shortcuts" => "a",
        _ => "z",
    }
}

pub fn needs_doc(id: &str) -> bool {
    !(id.starts_with("help.")
        || matches!(
            id,
            "file.open" | "file.home" | "file.quit" | "edit.settings" | "view.theme" | "view.fullscreen" | "view.edges" | "view.grid" | "view.axes" | "view.viewcube" | "view.tree" | "view.info" | "tool.assoc" | "tool.settings_folder"
        ))
}

pub fn checked(app: &App, id: &str) -> Option<bool> {
    let s = &app.settings;
    let d = app.doc();
    Some(match id {
        "view.ortho" => d.is_some_and(|d| d.camera.ortho),
        "view.edges" => s.edges,
        "view.bbox" => d.is_some_and(|d| d.show_bbox),
        "view.section" => d.is_some_and(|d| d.section.is_some()),
        "view.grid" => s.show_grid,
        "view.axes" => s.show_axes,
        "view.viewcube" => s.show_viewcube,
        "view.tree" => s.show_tree,
        "view.info" => s.show_info,
        "tool.select" => d.is_some_and(|d| d.measure.tool == Tool::Select),
        "tool.distance" => d.is_some_and(|d| d.measure.tool == Tool::Distance),
        "tool.edge" => d.is_some_and(|d| d.measure.tool == Tool::Edge),
        "tool.diameter" => d.is_some_and(|d| d.measure.tool == Tool::Diameter),
        "tool.angle" => d.is_some_and(|d| d.measure.tool == Tool::Angle),
        _ => return None,
    })
}

pub fn status_bar(app: &mut App, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::bottom("status_bar")
        .exact_size(26.0)
        .frame(egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::symmetric(12, 0)).stroke(Stroke::new(1.0, t.divider)))
        .show_inside(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let small = |s: String| egui::RichText::new(s).font(theme::regular(11.5)).color(t.text_muted);
                match app.doc() {
                    None => {
                        ui.label(small(trf("{n} formats pris en charge", &[("n", &craft_formats::formats().iter().filter(|f| f.support != craft_formats::Support::None).count().to_string())])));
                        ui.label(small(tr("Glissez un fichier ou un dossier dans la fenêtre").to_string()));
                    }
                    Some(d) => {
                        if let Some((f, stage, secs)) = d.progress() {
                            ui.add(egui::Spinner::new().size(12.0).color(t.accent));
                            ui.label(small(format!("{} {:.0} % · {:.1} s", viewer::tr_stage(&stage), f * 100.0, secs)));
                        } else if let Some(s) = &d.scene {
                            ui.label(small(d.file_kind_label()));
                            ui.label(small(trf("{n} triangles", &[("n", &widgets::fmt_int(d.stats.triangles))])));
                            if d.stats.parts > 1 {
                                ui.label(small(trf("{n} pièces", &[("n", &widgets::fmt_int(d.stats.parts))])));
                            }
                            if let Some((lo, hi)) = d.stats.bbox {
                                let mut e = hi - lo;
                                if s.up_z {
                                    e = glam::vec3(e.x, e.z, e.y);
                                }
                                let st = &app.settings;
                                ui.label(small(format!("{} × {} × {}", st.fmt_len_bare(e.x as f64), st.fmt_len_bare(e.y as f64), st.fmt_len(e.z as f64))));
                            }
                            if let Some(ms) = d.load_time {
                                ui.label(small(trf("chargé en {t} s", &[("t", &format!("{ms:.2}"))])));
                            }
                            let _ = s;
                            if let Some(i) = d.selected {
                                if let Some(c) = d.scene.as_ref().and_then(|s| s.components.get(i)) {
                                    ui.label(egui::RichText::new(format!("▸ {}", c.name)).font(theme::medium(11.5)).color(t.accent_text));
                                }
                            }
                        }
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.label(widgets::mono_caps(&format!("V{}", crate::branding::VERSION), 10.5, t.text_faint));
                    let st = &mut app.settings;
                    let u = ui.add(egui::Button::new(small(st.units.suffix().to_string())).frame(false));
                    egui::Popup::menu(&u).show(|ui| {
                        use crate::settings::Units::*;
                        for un in [Mm, Cm, M, In] {
                            if widgets::menu_check(ui, st.units == un, un.suffix(), "").clicked() {
                                st.units = un;
                                app.settings_dirty = true;
                                ui.close();
                            }
                        }
                    });
                    let u = u.on_hover_text(tr("Unités"));
                    let _ = u;
                    if let Some(d) = app.doc() {
                        if d.scene.is_some() {
                            ui.label(small(if d.camera.ortho { tr("Orthographique").to_string() } else { tr("Perspective").to_string() }));
                        }
                    }
                });
            });
        });
}

/// Tree on the left, properties on the right, 3D view in the middle.
pub fn document_view(app: &mut App, ui: &mut egui::Ui, i: usize) {
    let t = Tokens::get(ui.ctx());
    let mut cmds = std::mem::take(&mut app.cmds);
    let has_tree = app.docs[i].scene.as_ref().is_some_and(|s| s.components.len() > 1);
    if app.settings.show_tree && (has_tree || app.docs[i].is_loading()) {
        egui::Panel::left("tree")
            .default_size(270.0)
            .size_range(180.0..=520.0)
            .resizable(true)
            .frame(egui::Frame::NONE.fill(t.panel).inner_margin(egui::Margin::symmetric(10, 6)).stroke(Stroke::new(1.0, t.divider)))
            .show_inside(ui, |ui| viewer::tree_panel(ui, &mut app.docs[i], &mut cmds));
    }
    if app.settings.show_info {
        egui::Panel::right("info")
            .default_size(300.0)
            .size_range(240.0..=480.0)
            .resizable(true)
            .frame(egui::Frame::NONE.fill(t.panel).inner_margin(egui::Margin::symmetric(12, 6)).stroke(Stroke::new(1.0, t.divider)))
            .show_inside(ui, |ui| viewer::info_panel(ui, &mut app.docs[i], &app.settings, &mut cmds));
    }
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.pasteboard)).show_inside(ui, |ui| {
        let r = ui.max_rect();
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("viewport-rect"), r));
        let gfx = viewer::Gfx { rs: &app.rs, renderer: &mut app.renderer, tex: &mut app.tex };
        viewer::viewport(ui, &mut app.docs[i], &app.settings, gfx, &mut cmds);
    });
    cmds.append(&mut app.cmds);
    app.cmds = cmds;
}

pub fn toast(app: &mut App, ctx: &egui::Context) {
    let Some((msg, age)) = app.toast.clone() else { return };
    let dt = ctx.input(|i| i.stable_dt).min(0.1) as f64;
    let age = age + dt;
    if age > 3.2 {
        app.toast = None;
        return;
    }
    app.toast = Some((msg.clone(), age));
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("toast")).order(egui::Order::Tooltip).pivot(Align2::CENTER_BOTTOM).fixed_pos(egui::pos2(screen.center().x, screen.bottom() - 44.0)).interactable(false).show(ctx, |ui| {
        egui::Frame::new().fill(t.card).stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(18)).inner_margin(egui::Margin::symmetric(16, 9)).shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: t.shadow }).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(icons::image("check", 15.0, t.accent));
                ui.label(egui::RichText::new(msg).font(theme::medium(13.0)).color(t.text));
            });
        });
    });
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
}

pub fn drop_overlay(ctx: &egui::Context) {
    let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
    if !hovering {
        return;
    }
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
    p.rect_filled(screen, 0, Color32::from_black_alpha(140));
    let r = screen.shrink(24.0);
    p.rect_stroke(r, 14, Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
    p.text(r.center(), Align2::CENTER_CENTER, tr("Déposez pour ouvrir"), theme::semibold(22.0), Color32::WHITE);
}
