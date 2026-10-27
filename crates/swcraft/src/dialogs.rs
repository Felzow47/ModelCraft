//! Modal windows: settings (persisted in %APPDATA%\ModelCraft\settings.json), about, shortcuts, formats.

use crate::app::{App, Dialog, SettingsCat};
use crate::commands::{self, COMMANDS, GROUPS};
use crate::i18n::{tr, trf, LangPref};
use crate::settings::*;
use crate::theme::{self, Accent, Press, ThemePreference, Tokens};
use crate::{icons, widgets};
use egui::{vec2, Align, Align2, Color32, CornerRadius, Layout, Rect, Sense, Stroke};

pub fn show(app: &mut App, ctx: &egui::Context) {
    match app.dialog {
        Some(Dialog::Settings(cat)) => settings(app, ctx, cat),
        Some(Dialog::About) => about(app, ctx),
        Some(Dialog::Shortcuts) => shortcuts(app, ctx),
        Some(Dialog::Formats) => formats(app, ctx),
        None => {}
    }
}

fn modal(ctx: &egui::Context, id: &str, size: egui::Vec2, add: impl FnOnce(&mut egui::Ui)) -> bool {
    let r = egui::Modal::new(egui::Id::new(id)).frame(widgets::dialog_frame(ctx)).show(ctx, |ui| {
        widgets::begin_dialog(ui);
        ui.set_width(size.x);
        ui.set_min_height(size.y);
        add(ui);
    });
    r.should_close()
}

fn title_row(ui: &mut egui::Ui, title: &str) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).font(theme::semibold(17.0)));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            close = icons::button(ui, "x", 28.0, false, tr("Fermer")).clicked();
        });
    });
    ui.add_space(6.0);
    close
}

// ------------------------------------------------------------------ settings

pub(crate) const CATS: [(SettingsCat, &str, &str); 6] = [
    (SettingsCat::General, "settings-2", "Général"),
    (SettingsCat::Display, "monitor", "Affichage"),
    (SettingsCat::Navigation, "mouse-pointer-2", "Navigation"),
    (SettingsCat::Measure, "ruler", "Mesures"),
    (SettingsCat::Performance, "gauge", "Performances"),
    (SettingsCat::Files, "folder", "Fichiers"),
];

fn row(ui: &mut egui::Ui, title: &str, desc: &str, control: impl FnOnce(&mut egui::Ui)) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        ui.set_min_height(44.0);
        ui.vertical(|ui| {
            ui.set_max_width((ui.available_width() - 250.0).max(160.0));
            ui.add_space(4.0);
            ui.label(egui::RichText::new(title).font(theme::medium(13.5)).color(t.text));
            if !desc.is_empty() {
                ui.label(egui::RichText::new(desc).font(theme::regular(11.5)).color(t.text_muted));
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), control);
    });
    ui.add_space(2.0);
    let r = ui.available_rect_before_wrap();
    ui.painter().hline(r.x_range(), r.top(), Stroke::new(1.0, t.divider));
    ui.add_space(6.0);
}

fn combo<T: PartialEq + Copy>(ui: &mut egui::Ui, id: &str, value: &mut T, options: &[(T, &str)]) -> bool {
    let cur = options.iter().find(|(v, _)| v == value).map(|(_, l)| *l).unwrap_or("");
    let mut changed = false;
    egui::ComboBox::from_id_salt(id).selected_text(cur).width(190.0).show_ui(ui, |ui| {
        for (v, l) in options {
            if ui.selectable_label(*v == *value, *l).clicked() && *v != *value {
                *value = *v;
                changed = true;
            }
        }
    });
    changed
}

fn swatch(ui: &mut egui::Ui, c: Color32, selected: bool, tip: &str) -> bool {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
    let press = Press::track(ui, &resp);
    ui.painter().circle_filled(rect.center() + press.offset(), 9.0, c);
    if selected {
        ui.painter().circle_stroke(rect.center(), 11.5, Stroke::new(2.0, t.text));
    }
    theme::hand(resp).on_hover_text(tip).clicked()
}

fn settings(app: &mut App, ctx: &egui::Context, cat: SettingsCat) {
    let t = Tokens::get(ctx);
    let before = app.settings.clone();
    let mut cat = cat;
    let mut close = false;
    let mut reset = false;
    let esc = modal(ctx, "settings", vec2(820.0, 560.0), |ui| {
        close = title_row(ui, tr("Paramètres"));
        ui.horizontal_top(|ui| {
            // sidebar
            ui.vertical(|ui| {
                ui.set_width(190.0);
                for (c, icon, label) in CATS {
                    let (rect, resp) = ui.allocate_exact_size(vec2(182.0, 34.0), Sense::click());
                    let press = Press::track(ui, &resp);
                    press.wash(ui, rect, 7, c == cat);
                    let col = if c == cat { t.accent_text } else { t.text_muted };
                    icons::paint(ui, Rect::from_min_size(rect.min + vec2(10.0, 9.0), vec2(16.0, 16.0)), icon, 16.0, col);
                    ui.painter().text(rect.min + vec2(36.0, 17.0), Align2::LEFT_CENTER, tr(label), theme::medium(13.0), if c == cat { t.text } else { t.text_muted });
                    if theme::hand(resp).clicked() {
                        cat = c;
                    }
                }
                ui.add_space(12.0);
                if widgets::ghost_button(ui, "rotate-ccw", tr("Réinitialiser")).on_hover_text(tr("Rétablir tous les paramètres par défaut (les récents et les dossiers épinglés sont conservés)")).clicked() {
                    reset = true;
                }
            });
            let r = ui.available_rect_before_wrap();
            ui.painter().vline(r.left() - 2.0, r.y_range(), Stroke::new(1.0, t.divider));
            ui.add_space(14.0);
            ui.vertical(|ui| {
                egui::ScrollArea::vertical().max_height(500.0).auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_width(ui.available_width() - 8.0);
                    let label = CATS.iter().find(|c| c.0 == cat).map(|c| c.2).unwrap_or("");
                    widgets::section_title(ui, tr(label));
                    page(app, ui, cat);
                });
            });
        });
    });
    if reset {
        app.settings.reset_preferences();
    }
    if esc || close {
        app.dialog = None;
    } else {
        app.dialog = Some(Dialog::Settings(cat));
    }
    if app.settings != before {
        app.settings_dirty = true;
        // camera-related settings apply to the open documents
        let (ortho_changed, ortho) = (app.settings.ortho != before.ortho, app.settings.ortho);
        for d in &mut app.docs {
            if ortho_changed {
                d.camera.ortho = ortho;
            }
            d.instances_dirty = true;
        }
    }
}

fn page(app: &mut App, ui: &mut egui::Ui, cat: SettingsCat) {
    let s = &mut app.settings;
    match cat {
        SettingsCat::General => {
            row(ui, tr("Langue"), tr("Français ou anglais ; « Système » suit la langue de Windows."), |ui| {
                combo(ui, "lang", &mut s.language, &[(LangPref::System, tr("Système")), (LangPref::Fr, "Français"), (LangPref::En, "English")]);
            });
            row(ui, tr("Thème"), tr("Clair, sombre, ou selon le réglage du système."), |ui| {
                widgets::segmented(ui, &mut s.theme, &[(ThemePreference::System, tr("Système")), (ThemePreference::Light, tr("Clair")), (ThemePreference::Dark, tr("Sombre"))]);
            });
            row(ui, tr("Couleur d'accent"), tr("Boutons, sélection et mesures."), |ui| {
                let dark = Tokens::get(ui.ctx()).dark();
                for a in Accent::ALL.iter().rev() {
                    let (d, l) = a.colors();
                    if swatch(ui, if dark { d } else { l }, s.accent == *a, tr(a.label())) {
                        s.accent = *a;
                    }
                }
            });
            row(ui, tr("Fichiers récents"), tr("Nombre de fichiers affichés sur l'accueil."), |ui| {
                ui.add(egui::Slider::new(&mut s.recent_max, 3..=30));
            });
            row(ui, tr("Associations de fichiers"), tr("Ajoute ModelCraft à « Ouvrir avec » pour SLDPRT, SLDASM, STEP, IGES, IFC, FBX, glTF, STL, OBJ, 3MF… (utilisateur courant, sans droits administrateur)."), |ui| {
                if widgets::icon_pill(ui, "link", tr("Associer"), false).clicked() {
                    app.cmds.push("tool.assoc");
                }
            });
            if let Some(m) = &app.assoc_result {
                ui.label(egui::RichText::new(m).color(Tokens::get(ui.ctx()).text_muted));
            }
            row(ui, tr("Dossier des paramètres"), &crate::settings::settings_path().map(|p| p.display().to_string()).unwrap_or_default(), |ui| {
                if widgets::icon_pill(ui, "folder-open", tr("Ouvrir"), false).clicked() {
                    app.cmds.push("tool.settings_folder");
                }
            });
        }
        SettingsCat::Display => {
            row(ui, tr("Arrière-plan"), tr("Dégradé ou couleur unie derrière le modèle."), |ui| {
                widgets::segmented(ui, &mut s.background, &[(Background::Gradient, tr("Dégradé")), (Background::Solid, tr("Uni"))]);
            });
            row(ui, tr("Couleurs du thème"), tr("Utiliser les couleurs d'arrière-plan du thème actif."), |ui| {
                widgets::switch(ui, &mut s.bg_follow_theme);
            });
            if !s.bg_follow_theme {
                row(ui, tr("Couleurs personnalisées"), tr("Haut et bas du dégradé (seule la première sert pour un fond uni)."), |ui| {
                    egui::color_picker::color_edit_button_srgb(ui, &mut s.bg_bottom);
                    egui::color_picker::color_edit_button_srgb(ui, &mut s.bg_top);
                });
            }
            row(ui, tr("Éclairage"), tr("Préréglage de lumière appliqué au modèle."), |ui| {
                combo(ui, "light", &mut s.lighting, &[(Lighting::Studio, tr("Studio")), (Lighting::Soft, tr("Doux")), (Lighting::Contrast, tr("Contrasté")), (Lighting::Flat, tr("Plat (sans ombrage)"))]);
            });
            row(ui, tr("Couleur des pièces sans couleur"), tr("Utilisée quand le fichier ne définit pas de couleur."), |ui| {
                egui::color_picker::color_edit_button_srgb(ui, &mut s.base_color);
            });
            row(ui, tr("Arêtes"), tr("Contours des faces et silhouettes."), |ui| {
                widgets::switch(ui, &mut s.edges);
            });
            if s.edges {
                row(ui, tr("Épaisseur et couleur des arêtes"), "", |ui| {
                    egui::color_picker::color_edit_button_srgb(ui, &mut s.edge_color);
                    ui.add(egui::Slider::new(&mut s.edge_width, 1.0..=4.0).step_by(0.5).suffix(" px"));
                });
            }
            row(ui, tr("Axes"), tr("Trièdre XYZ en bas à gauche."), |ui| {
                widgets::switch(ui, &mut s.show_axes);
            });
            row(ui, tr("Grille"), tr("Grille au sol sous le modèle."), |ui| {
                widgets::switch(ui, &mut s.show_grid);
            });
            row(ui, tr("Cube de vue"), tr("Cube cliquable pour choisir une vue standard."), |ui| {
                widgets::switch(ui, &mut s.show_viewcube);
            });
            row(ui, tr("Arbre de création"), tr("Panneau des composants à gauche."), |ui| {
                widgets::switch(ui, &mut s.show_tree);
            });
            row(ui, tr("Panneau des propriétés"), tr("Dimensions, sélection et mesures à droite."), |ui| {
                widgets::switch(ui, &mut s.show_info);
            });
        }
        SettingsCat::Navigation => {
            row(ui, tr("Style de navigation"), tr("Blender (défaut) : clic molette ou clic gauche pour pivoter, Maj+clic molette ou clic droit pour déplacer. SolidWorks : clic gauche ou molette pour pivoter, Ctrl+molette ou clic droit pour déplacer. CAO : clic molette pour déplacer, Maj+molette pour pivoter. L'outil Panoramique (main) fait déplacer la vue au clic gauche."), |ui| {
                widgets::segmented(ui, &mut s.nav, &[(NavStyle::SolidWorks, "SolidWorks"), (NavStyle::Blender, "Blender"), (NavStyle::Cad, tr("CAO"))]);
            });
            row(ui, tr("Raccourcis Blender"), tr("Pavé numérique 1/3/7 : face, droite, dessus (Ctrl = vue opposée), 5 : ortho, 2/4/6/8 : tourner de 15°, 9 : vue opposée, « . » : cadrer la sélection, Origine : tout cadrer, G : déplacer, R : tourner. Désactivé : anciens raccourcis (0-6 = vues, G = grille, R = diamètre)."), |ui| {
                widgets::switch(ui, &mut s.blender_keys);
            });
            row(ui, tr("Rotation"), tr("Libre : rotation illimitée dans toutes les directions (trackball). Plateau tournant : l'axe Y reste vertical, arrêt aux pôles."), |ui| {
                widgets::segmented(ui, &mut s.orbit, &[(OrbitMode::Free, tr("Libre")), (OrbitMode::Turntable, tr("Plateau tournant"))]);
            });
            row(ui, tr("Inverser le zoom"), tr("Inverse le sens de la molette."), |ui| {
                widgets::switch(ui, &mut s.invert_zoom);
            });
            row(ui, tr("Projection"), tr("Projection par défaut des nouveaux documents (touche O pour basculer)."), |ui| {
                let mut v = s.ortho;
                if widgets::segmented(ui, &mut v, &[(false, tr("Perspective")), (true, tr("Orthographique"))]) {
                    s.ortho = v;
                }
            });
            row(ui, tr("Champ de vision"), tr("Angle vertical de la caméra en perspective."), |ui| {
                ui.add(egui::Slider::new(&mut s.fov_deg, 10.0..=90.0).suffix("°"));
            });
            row(ui, tr("Vue par défaut"), tr("Orientation à l'ouverture d'un fichier."), |ui| {
                combo(ui, "defview", &mut s.default_view, &[(DefaultView::Iso, tr("Isométrique")), (DefaultView::Front, tr("Face")), (DefaultView::Top, tr("Dessus")), (DefaultView::Right, tr("Droite"))]);
            });
        }
        SettingsCat::Measure => {
            row(ui, tr("Unités"), tr("Unité des cotes, distances et diamètres."), |ui| {
                widgets::segmented(ui, &mut s.units, &[(Units::Mm, "mm"), (Units::Cm, "cm"), (Units::M, "m"), (Units::In, "in")]);
            });
            row(ui, tr("Unités automatiques"), tr("Les grandes valeurs passent en cm², m², m³… (ex. 6 115 613 mm² → 6,12 m²)."), |ui| {
                widgets::switch(ui, &mut s.auto_units);
            });
            row(ui, tr("Décimales"), tr("Nombre de chiffres après la virgule."), |ui| {
                ui.add(egui::Slider::new(&mut s.decimals, 0..=6));
            });
            let ex = trf("Exemple : {v}", &[("v", &s.fmt_len(0.123456))]);
            ui.label(egui::RichText::new(ex).color(Tokens::get(ui.ctx()).text_muted));
        }
        SettingsCat::Performance => {
            row(ui, tr("Anticrénelage (MSAA)"), tr("Lisse les arêtes ; 1× désactive l'anticrénelage."), |ui| {
                widgets::segmented(ui, &mut s.msaa, &[(1, "1×"), (4, "4×")]);
            });
            row(ui, tr("Qualité de rendu"), tr("Rapide : résolution réduite et maillage grossier. Haute : suréchantillonnage et maillage fin (STEP/IGES rechargés)."), |ui| {
                widgets::segmented(ui, &mut s.quality, &[(Quality::Fast, tr("Rapide")), (Quality::Balanced, tr("Équilibrée")), (Quality::High, tr("Haute"))]);
            });
        }
        SettingsCat::Files => {
            row(ui, tr("Fichiers récents"), &trf("{n} fichiers mémorisés", &[("n", &s.recent.len().to_string())]), |ui| {
                if widgets::ghost_button(ui, "trash-2", tr("Effacer")).clicked() {
                    s.recent.clear();
                }
            });
            row(ui, tr("Dossiers épinglés"), &trf("{n} dossiers", &[("n", &s.pinned.len().to_string())]), |ui| {
                if widgets::ghost_button(ui, "trash-2", tr("Tout désépingler")).clicked() {
                    s.pinned.clear();
                }
            });
            row(ui, tr("Associations de fichiers"), tr("Ajoute ModelCraft à « Ouvrir avec » (utilisateur courant)."), |ui| {
                if widgets::icon_pill(ui, "link", tr("Associer"), false).clicked() {
                    app.cmds.push("tool.assoc");
                }
            });
        }
    }
}

// ------------------------------------------------------------------ about

fn about(app: &mut App, ctx: &egui::Context) {
    let t = Tokens::get(ctx);
    let mut close = false;
    let esc = modal(ctx, "about", vec2(480.0, 300.0), |ui| {
        close = title_row(ui, tr("À propos"));
        ui.horizontal(|ui| {
            widgets::app_icon(ui, 72.0);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                widgets::wordmark(ui, 26.0);
                ui.label(widgets::mono_caps(&format!("V{}", crate::branding::VERSION), 11.0, t.accent_text));
                ui.label(egui::RichText::new(tr("Visionneuse 3D native pour SolidWorks, STEP, IGES, IFC, FBX, glTF, STL, OBJ, 3MF, 3DS, 3DXML et bien d'autres.")).color(t.text_muted));
            });
        });
        ui.add_space(12.0);
        widgets::section_title(ui, tr("Crédits"));
        for line in [
            tr("Interface inspirée de la suite ArtCraft ; jetons de design et widgets adaptés de PdfCraft (MIT OR Apache-2.0, © ArtCraft Team et contributeurs PdfCraft). Projet indépendant, non affilié."),
            tr("Icônes Lucide (ISC) · Polices Inter et JetBrains Mono (OFL)."),
            tr("STEP/IGES : Open CASCADE Technology 7.8 (LGPL-2.1 avec exception)."),
            tr("Lecteur SolidWorks écrit de zéro, sans code d'eDrawings."),
        ] {
            ui.label(egui::RichText::new(line).font(theme::regular(12.0)).color(t.text_muted));
            ui.add_space(2.0);
        }
    });
    if esc || close {
        app.dialog = None;
    }
}

// ------------------------------------------------------------------ shortcuts

fn shortcuts(app: &mut App, ctx: &egui::Context) {
    let t = Tokens::get(ctx);
    let mut close = false;
    let esc = modal(ctx, "shortcuts", vec2(720.0, 520.0), |ui| {
        close = title_row(ui, tr("Raccourcis clavier"));
        let note = if app.settings.blender_keys {
            tr("Raccourcis façon Blender (Paramètres › Navigation). Les chiffres marchent au pavé numérique comme sur la rangée du haut ; « . » du pavé cadre la sélection, Origine cadre tout.")
        } else {
            tr("Raccourcis classiques ModelCraft. Activez « Raccourcis Blender » dans Paramètres › Navigation pour la disposition Blender.")
        };
        ui.label(egui::RichText::new(note).font(theme::regular(12.0)).color(t.text_muted));
        ui.add_space(6.0);
        egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
            ui.columns(2, |cols| {
                for (gi, g) in GROUPS.iter().enumerate() {
                    let ui = &mut cols[if gi < 2 || gi == 4 { 0 } else { 1 }];
                    widgets::section_title(ui, tr(g));
                    for c in COMMANDS.iter().filter(|c| c.group == *g) {
                        let keys = commands::all_keys_text(ui.ctx(), c.id);
                        if keys.is_empty() {
                            continue;
                        }
                        ui.horizontal(|ui| {
                            ui.add(icons::image(c.icon, 14.0, t.icon));
                            ui.label(egui::RichText::new(tr(c.label)).color(t.text));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(egui::RichText::new(keys).font(theme::mono(11.5)).color(t.accent_text));
                            });
                        });
                    }
                    ui.add_space(8.0);
                }
                let ui = &mut cols[1];
                widgets::section_title(ui, tr("Souris"));
                for (a, b) in nav_lines(app.settings.nav) {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(a).color(t.text));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(egui::RichText::new(b).font(theme::mono(11.5)).color(t.accent_text));
                        });
                    });
                }
            });
        });
    });
    if esc || close {
        app.dialog = None;
    }
}

pub fn nav_lines(nav: NavStyle) -> Vec<(&'static str, &'static str)> {
    match nav {
        NavStyle::SolidWorks => vec![(tr("Pivoter"), tr("Clic gauche / molette")), (tr("Déplacer"), tr("Clic droit / Ctrl+molette")), (tr("Zoom"), tr("Molette / Maj+molette")), (tr("Sélectionner"), tr("Clic")), (tr("Tout cadrer"), tr("Double-clic"))],
        NavStyle::Blender => vec![(tr("Pivoter"), tr("Clic molette / clic gauche")), (tr("Déplacer"), tr("Maj+clic molette / clic droit")), (tr("Zoom"), tr("Molette / Ctrl+clic molette")), (tr("Sélectionner"), tr("Clic")), (tr("Tout cadrer"), tr("Double-clic molette"))],
        NavStyle::Cad => vec![(tr("Pivoter"), tr("Clic gauche / Maj+molette")), (tr("Déplacer"), tr("Clic molette / clic droit")), (tr("Zoom"), tr("Molette")), (tr("Sélectionner"), tr("Clic")), (tr("Tout cadrer"), tr("Double-clic"))],
    }
}

// ------------------------------------------------------------------ formats

fn formats(app: &mut App, ctx: &egui::Context) {
    let t = Tokens::get(ctx);
    let mut close = false;
    let esc = modal(ctx, "formats", vec2(640.0, 420.0), |ui| {
        close = title_row(ui, tr("Formats pris en charge"));
        egui::Grid::new("fmt").num_columns(3).spacing(vec2(18.0, 8.0)).striped(true).show(ui, |ui| {
            ui.label(widgets::mono_caps(tr("Format"), 10.5, t.text_faint));
            ui.label(widgets::mono_caps(tr("Extensions"), 10.5, t.text_faint));
            ui.label(widgets::mono_caps(tr("Prise en charge"), 10.5, t.text_faint));
            ui.end_row();
            for f in craft_formats::formats() {
                ui.label(egui::RichText::new(tr(f.name)).font(theme::medium(13.0)));
                ui.label(egui::RichText::new(f.exts.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join("  ")).font(theme::mono(12.0)).color(t.text_muted));
                let (txt, col) = match f.support {
                    craft_formats::Support::Full => (tr("Complète"), Color32::from_rgb(0x30, 0xA4, 0x6C)),
                    craft_formats::Support::Partial => (tr("Partielle"), t.warning),
                    craft_formats::Support::None => (tr("Non pris en charge"), t.danger),
                };
                ui.horizontal(|ui| {
                    ui.painter().circle_filled(ui.cursor().min + vec2(4.0, 9.0), 4.0, col);
                    ui.add_space(12.0);
                    ui.label(txt).on_hover_text(tr(f.note));
                });
                ui.end_row();
            }
        });
        let _ = CornerRadius::ZERO;
    });
    if esc || close {
        app.dialog = None;
    }
}
