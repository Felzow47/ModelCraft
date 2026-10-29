//! Home tab in the ArtCraft layout (structure ported from PdfCraft's home.rs, MIT OR Apache-2.0,
//! © ArtCraft Team and PdfCraft contributors): launcher header, format cards, pinned folders, recents.

use crate::app::App;
use crate::i18n::{tr, trf};
use crate::theme::{self, Press, Tokens};
use crate::{icons, widgets};
use egui::{vec2, Align2, Color32, CornerRadius, Rect, Sense, Stroke};
use std::path::{Path, PathBuf};

pub(crate) struct Card {
    icon: &'static str,
    pub(crate) title: &'static str,
    pub(crate) blurb: &'static str,
    hue: Color32,
}

pub(crate) const CARDS: [Card; 5] = [
    Card { icon: "file-box", title: "SolidWorks", blurb: "Pièces et assemblages .sldprt · .sldasm", hue: Color32::from_rgb(0xE5, 0x48, 0x4D) },
    Card { icon: "cuboid", title: "STEP · IGES", blurb: "Échange CAO .step · .stp · .iges", hue: Color32::from_rgb(0x3E, 0x8E, 0xDE) },
    Card { icon: "box", title: "Maillages", blurb: "Impression et rendu .stl · .obj · .3mf · .ply · .off", hue: Color32::from_rgb(0x30, 0xA4, 0x6C) },
    Card { icon: "layers", title: "Scènes 3D", blurb: "FBX · glTF · DAE · VRML · exports Blender", hue: Color32::from_rgb(0xE0, 0x8A, 0x2E) },
    Card { icon: "boxes", title: "BIM · eDrawings", blurb: "Visualisation .ifc · .rvt · .3dxml · .easm", hue: Color32::from_rgb(0x8E, 0x4E, 0xC6) },
];

/// One line of text cut with "…" so it never runs under the next card.
fn elided(ui: &egui::Ui, text: &str, font: egui::FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_string(), font, color, width.max(10.0));
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    ui.fonts_mut(|f| f.layout_job(job))
}

fn card_lift(ui: &egui::Ui, t: &Tokens, rect: Rect, press: Press) {
    let lift = press.hover * (1.0 - press.down);
    if lift > 0.04 {
        let shadow = egui::Shadow { offset: [0, 3], blur: (14.0 * lift) as u8, spread: 0, color: t.shadow.gamma_multiply(0.85 * lift) };
        ui.painter().add(shadow.as_shape(rect, CornerRadius::same(10)));
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.pasteboard)).show_inside(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::NONE.inner_margin(egui::Margin { left: 36, right: 36, top: 28, bottom: 28 }).show(ui, |ui| {
                header(app, ui, &t);
                ui.add_space(22.0);
                formats(app, ui, &t);
                ui.add_space(26.0);
                pinned(app, ui, &t);
                ui.add_space(26.0);
                recents(app, ui, &t);
                ui.add_space(20.0);
                widgets::section_title(ui, tr("Confidentialité"));
                ui.label(egui::RichText::new(tr("ModelCraft fonctionne hors ligne. Aucun compte, aucune télémétrie : vos fichiers ne quittent jamais votre ordinateur.")).color(t.text_muted));
            });
        });
    });
}

fn header(_app: &mut App, ui: &mut egui::Ui, t: &Tokens) {
    egui::Frame::NONE.fill(t.card).stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(12)).inner_margin(egui::Margin::same(18)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            widgets::app_icon(ui, 56.0);
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(egui::RichText::new(tr("Bienvenue dans ")).font(theme::semibold(24.0)));
                    widgets::wordmark(ui, 24.0);
                });
                ui.label(egui::RichText::new(tr("La visionneuse 3D ultime - SolidWorks, STEP, IGES, IFC, FBX, glTF, STL, OBJ, 3MF et plus, sans licence ni installation.")).color(t.text_muted).font(theme::regular(14.0)));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    ui.label(widgets::mono_caps(&format!("V{}", crate::branding::VERSION), 11.0, t.accent_text));
                    ui.label(egui::RichText::new(tr("par {a}").replace("{a}", crate::branding::AUTHOR)).font(theme::mono(11.0)).color(t.text_faint));
                });
            });
        });
    });
}

fn formats(app: &mut App, ui: &mut egui::Ui, t: &Tokens) {
    egui::Frame::NONE.fill(t.card).stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(12)).inner_margin(egui::Margin::same(18)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new(tr("Formats pris en charge")).font(theme::semibold(15.0)));
        ui.add_space(10.0);
        // a grid that fills the width: as many ≥ 200 px columns as fit, all the same width,
        // with the "Ouvrir un fichier" tile as the last cell
        let gap = 14.0;
        let n = CARDS.len() + 1;
        let avail = ui.available_width();
        let cols = (((avail + gap) / (200.0 + gap)).floor() as usize).clamp(1, n);
        let w = ((avail - gap * (cols as f32 - 1.0)) / cols as f32).floor();
        let h = 104.0;
        let mut cells: Vec<Option<&Card>> = CARDS.iter().map(Some).collect();
        cells.push(None);
        for row in cells.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(gap, gap);
                for cell in row {
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
                    let press = Press::track(ui, &resp);
                    card_lift(ui, t, rect, press);
                    let body = rect.translate(press.offset());
                    match cell {
                        Some(c) => {
                            ui.painter().rect(rect, CornerRadius::same(10), press.fill(t.card, t.hover, t.pressed), Stroke::new(1.0, t.divider), egui::StrokeKind::Inside);
                            icons::paint(ui, Rect::from_min_size(body.min + vec2(14.0, 14.0), vec2(22.0, 22.0)), c.icon, 21.0, c.hue);
                            let g = elided(ui, tr(c.title), theme::semibold(13.5), t.text, body.width() - 58.0);
                            ui.painter().galley(body.min + vec2(44.0, 25.0 - g.size().y * 0.5), g, t.text);
                            let mut job = egui::text::LayoutJob::simple(tr(c.blurb).to_string(), theme::regular(11.5), t.text_muted, body.width() - 28.0);
                            job.wrap.max_rows = 2;
                            job.wrap.overflow_character = Some('…');
                            let g = ui.fonts_mut(|f| f.layout_job(job));
                            ui.painter().galley(body.min + vec2(14.0, 44.0), g, t.text_muted);
                            ui.painter().text(body.left_bottom() + vec2(14.0, -14.0), Align2::LEFT_CENTER, tr("Ouvrir…"), theme::medium(12.0), t.accent_text);
                            if theme::hand(resp).on_hover_text(tr(c.blurb)).clicked() {
                                app.cmds.push("file.open");
                            }
                        }
                        None => {
                            ui.painter().rect(rect, CornerRadius::same(10), press.fill(t.pasteboard, t.hover, t.pressed), Stroke::new(1.0, t.divider), egui::StrokeKind::Inside);
                            icons::paint(ui, Rect::from_center_size(body.center() - vec2(0.0, 16.0), vec2(28.0, 28.0)), "folder-open", 26.0, t.icon);
                            let g = elided(ui, tr("Ouvrir un fichier"), theme::semibold(13.0), t.text, body.width() - 20.0);
                            ui.painter().galley(body.center() + vec2(-g.size().x * 0.5, 22.0 - g.size().y * 0.5), g, t.text);
                            if theme::hand(resp).on_hover_text(crate::chrome::tip("file.open", ui.ctx())).clicked() {
                                app.cmds.push("file.open");
                            }
                        }
                    }
                }
            });
            ui.add_space(gap);
        }
    });
}

fn supported_files(dir: &Path, max: usize) -> (Vec<(PathBuf, u64)>, usize) {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() && craft_formats::is_supported(&p) {
                v.push((p, e.metadata().map(|m| m.len()).unwrap_or(0)));
            }
        }
    }
    v.sort_by_key(|(p, _)| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    let total = v.len();
    v.truncate(max);
    (v, total)
}

fn pinned(app: &mut App, ui: &mut egui::Ui, t: &Tokens) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(tr("Dossiers épinglés")).font(theme::semibold(17.0)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::ghost_button(ui, "pin", tr("Épingler un dossier")).clicked() {
                if let Some(d) = rfd::FileDialog::new().set_title(tr("Épingler un dossier")).pick_folder() {
                    app.settings.pin(&d);
                    app.expanded_pins.insert(d.display().to_string());
                    app.settings_dirty = true;
                }
            }
        });
    });
    ui.add_space(8.0);
    if app.settings.pinned.is_empty() {
        ui.label(egui::RichText::new(tr("Épinglez vos dossiers de projets pour retrouver leurs modèles en un clic. Vous pouvez aussi y glisser un dossier.")).color(t.text_muted));
        return;
    }
    let mut unpin = None;
    let mut open = None;
    for dir in app.settings.pinned.clone() {
        let path = PathBuf::from(&dir);
        let expanded = app.expanded_pins.contains(&dir);
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
        let press = Press::track(ui, &resp);
        press.wash(ui, rect, 8, false);
        let body = rect.translate(press.offset());
        icons::paint(ui, Rect::from_min_size(body.min + vec2(8.0, 12.0), vec2(16.0, 16.0)), if expanded { "chevron-down" } else { "chevron-right" }, 15.0, t.text_muted);
        icons::paint(ui, Rect::from_min_size(body.min + vec2(30.0, 9.0), vec2(22.0, 22.0)), "folder", 20.0, t.accent);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.clone());
        let exists = path.is_dir();
        widgets::row_text(ui, body.min + vec2(62.0, 13.0), name, theme::medium(13.5), t.text, body.width() - 260.0);
        widgets::row_text(ui, body.min + vec2(62.0, 29.0), dir.clone(), theme::regular(11.0), t.text_faint, body.width() - 260.0);
        if !exists {
            ui.painter().text(body.right_center() - vec2(14.0, 0.0), Align2::RIGHT_CENTER, tr("introuvable"), theme::regular(12.0), t.warning);
        }
        let resp = theme::hand(resp);
        if resp.clicked() {
            if expanded {
                app.expanded_pins.remove(&dir);
            } else {
                app.expanded_pins.insert(dir.clone());
            }
        }
        resp.context_menu(|ui| {
            if widgets::menu_icon_item(ui, "external-link", tr("Afficher dans l'explorateur"), "").clicked() {
                let _ = crate::platform::open_folder(&path);
                ui.close();
            }
            if widgets::menu_icon_item(ui, "pin-off", tr("Désépingler"), "").clicked() {
                unpin = Some(dir.clone());
                ui.close();
            }
        });
        if expanded && exists {
            let (files, total) = supported_files(&path, 40);
            if files.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(62.0);
                    ui.label(egui::RichText::new(tr("Aucun modèle 3D dans ce dossier.")).color(t.text_faint));
                });
            }
            for (p, size) in files {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
                let press = Press::track(ui, &resp);
                press.wash(ui, rect, 6, false);
                let body = rect.translate(press.offset());
                icons::paint(ui, Rect::from_min_size(body.min + vec2(62.0, 7.0), vec2(16.0, 16.0)), crate::chrome::file_icon(&p), 15.0, t.text_muted);
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                widgets::row_text(ui, body.min + vec2(86.0, 15.0), name, theme::regular(13.0), t.text, body.width() - 200.0);
                ui.painter().text(body.right_center() - vec2(14.0, 0.0), Align2::RIGHT_CENTER, widgets::human_size(size), theme::regular(12.0), t.text_muted);
                if theme::hand(resp).clicked() {
                    open = Some(p.clone());
                }
            }
            if total > 40 {
                ui.horizontal(|ui| {
                    ui.add_space(62.0);
                    ui.label(egui::RichText::new(trf("… et {n} autres fichiers", &[("n", &(total - 40).to_string())])).color(t.text_faint));
                });
            }
            ui.add_space(4.0);
        }
    }
    if let Some(d) = unpin {
        app.settings.pinned.retain(|p| *p != d);
        app.settings_dirty = true;
    }
    if let Some(p) = open {
        let ctx = ui.ctx().clone();
        app.open(&p, &ctx);
    }
}

fn recents(app: &mut App, ui: &mut egui::Ui, t: &Tokens) {
    let mut clear = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(tr("Récents")).font(theme::semibold(17.0)));
        if !app.settings.recent.is_empty() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                clear = widgets::ghost_button(ui, "trash-2", tr("Effacer")).on_hover_text(tr("Effacer la liste des fichiers récents")).clicked();
            });
        }
    });
    ui.add_space(8.0);
    if app.settings.recent.is_empty() {
        ui.label(egui::RichText::new(tr("Les fichiers que vous ouvrez apparaissent ici. Glissez un modèle n'importe où pour l'ouvrir.")).color(t.text_muted));
    }
    let mut open = None;
    let mut remove = None;
    for r in app.settings.recent.iter().take(app.settings.recent_max) {
        let path = PathBuf::from(&r.path);
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
        let press = Press::track(ui, &resp);
        let hovered = ui.rect_contains_pointer(rect);
        if hovered && press.hover < 0.04 && press.down < 0.04 {
            ui.painter().rect_filled(rect, CornerRadius::same(8), t.hover);
        } else {
            press.wash(ui, rect, 8, false);
        }
        if hovered {
            let x_rect = Rect::from_center_size(rect.right_center() - vec2(26.0, 0.0), vec2(28.0, 28.0));
            let x = ui.interact(x_rect, ui.id().with(("remove-recent", &r.path)), Sense::click());
            if x.hovered() {
                ui.painter().rect_filled(x_rect, CornerRadius::same(6), t.pressed);
            }
            icons::paint(ui, x_rect.shrink(6.0), "x", 16.0, t.icon);
            if x.on_hover_text(tr("Retirer des récents")).clicked() {
                remove = Some(r.path.clone());
            }
        }
        let body = rect.translate(press.offset());
        let exists = path.exists();
        let hue = ext_hue(&path);
        icons::paint(ui, Rect::from_min_size(body.min + vec2(10.0, 11.0), vec2(24.0, 24.0)), crate::chrome::file_icon(&path), 22.0, if exists { hue } else { t.text_faint });
        let ext = path.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
        let detail = if exists { format!("{ext}  ·  {}", widgets::human_size(r.size)) } else { tr("introuvable").to_string() };
        let d = ui.painter().text(body.right_center() - vec2(48.0, 0.0), Align2::RIGHT_CENTER, detail, theme::regular(12.0), if exists { t.text_muted } else { t.warning });
        let width = d.left() - 16.0 - (body.left() + 46.0);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| r.path.clone());
        widgets::row_text(ui, body.min + vec2(46.0, 15.0), name, theme::medium(13.5), t.text, width);
        widgets::row_text(ui, body.min + vec2(46.0, 32.0), r.path.clone(), theme::regular(11.0), t.text_faint, width);
        let resp = theme::hand(resp).on_hover_text(&r.path);
        if resp.clicked() {
            open = Some(path.clone());
        }
        resp.context_menu(|ui| {
            if widgets::menu_icon_item(ui, "folder-open", tr("Ouvrir"), "").clicked() {
                open = Some(path.clone());
                ui.close();
            }
            if widgets::menu_icon_item(ui, "external-link", tr("Afficher dans l'explorateur"), "").clicked() {
                let _ = crate::platform::reveal(&path);
                ui.close();
            }
            if widgets::menu_icon_item(ui, "copy", tr("Copier le chemin du fichier"), "").clicked() {
                ui.ctx().copy_text(r.path.clone());
                ui.close();
            }
            if let Some(parent) = path.parent() {
                if widgets::menu_icon_item(ui, "pin", tr("Épingler le dossier"), "").clicked() {
                    app.settings_dirty = true;
                    let parent = parent.to_path_buf();
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("pin-request"), parent));
                    ui.close();
                }
            }
            ui.separator();
            if widgets::menu_icon_item(ui, "x", tr("Retirer des récents"), "").clicked() {
                remove = Some(r.path.clone());
                ui.close();
            }
        });
    }
    if let Some(p) = ui.ctx().data_mut(|d| d.remove_temp::<PathBuf>(egui::Id::new("pin-request"))) {
        app.settings.pin(&p);
        app.settings_dirty = true;
    }
    if let Some(p) = open {
        let ctx = ui.ctx().clone();
        app.open(&p, &ctx);
    }
    if let Some(p) = remove {
        app.settings.remove_recent(&p);
        app.settings_dirty = true;
    }
    if clear {
        app.settings.recent.clear();
        app.settings_dirty = true;
    }
}

fn ext_hue(p: &Path) -> Color32 {
    match p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("sldprt") | Some("sldasm") => CARDS[0].hue,
        Some("step") | Some("stp") | Some("iges") | Some("igs") => CARDS[1].hue,
        Some("stl") | Some("obj") | Some("3ds") | Some("3mf") | Some("ply") | Some("off") => CARDS[2].hue,
        Some("fbx") | Some("gltf") | Some("glb") | Some("dae") | Some("wrl") | Some("vrml") | Some("x3d") | Some("blend") => CARDS[3].hue,
        _ => CARDS[4].hue,
    }
}
