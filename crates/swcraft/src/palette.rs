//! Command palette (Ctrl+K), ported from PdfCraft's palette.rs (MIT OR Apache-2.0,
//! © ArtCraft Team and PdfCraft contributors).

use crate::app::App;
use crate::commands::{self, COMMANDS};
use crate::i18n::tr;
use crate::icons;
use crate::theme::{self, Press, Tokens};
use egui::{vec2, Align2, CornerRadius, Rect, Sense};

fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'à' | 'â' | 'ä' => 'a',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c => c,
        })
        .collect()
}

pub fn score(hay: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let h = fold(hay);
    if let Some(p) = h.find(needle) {
        return Some(p);
    }
    let mut it = h.chars();
    needle.chars().all(|c| it.any(|x| x == c)).then_some(100)
}

enum Target {
    Cmd(&'static str),
    Recent(std::path::PathBuf),
}

struct Hit {
    label: String,
    detail: String,
    icon: &'static str,
    target: Target,
    ready: bool,
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.palette_open {
        return;
    }
    let rect_id = egui::Id::new("palette_rect");
    let t = Tokens::get(ctx);
    let q = fold(app.palette_query.trim());
    let has_doc = app.doc().is_some();
    let mut hits: Vec<(usize, Hit)> = Vec::new();
    for c in COMMANDS {
        let label = tr(c.label);
        if let Some(s) = score(label, &q)
            .or_else(|| score(c.label, &q))
            .or_else(|| score(c.id, &q).map(|s| s + 50))
        {
            let key = commands::shortcut_text(ctx, c.id);
            hits.push((
                s,
                Hit {
                    label: label.trim_end_matches('…').to_string(),
                    detail: if key.is_empty() {
                        tr(c.group).to_string()
                    } else {
                        key
                    },
                    icon: c.icon,
                    target: Target::Cmd(c.id),
                    ready: !crate::chrome::needs_doc(c.id) || has_doc,
                },
            ));
        }
    }
    if !q.is_empty() {
        for r in &app.settings.recent {
            let p = std::path::PathBuf::from(&r.path);
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if let Some(s) = score(&name, &q) {
                hits.push((
                    s + 10,
                    Hit {
                        label: name,
                        detail: tr("Récent").to_string(),
                        icon: crate::chrome::file_icon(&p),
                        target: Target::Recent(p),
                        ready: true,
                    },
                ));
            }
        }
    }
    hits.sort_by_key(|(s, h)| (*s, !h.ready));
    hits.truncate(12);
    let (down, up, enter) = ctx.input(|i| {
        (
            i.key_pressed(egui::Key::ArrowDown),
            i.key_pressed(egui::Key::ArrowUp),
            i.key_pressed(egui::Key::Enter),
        )
    });
    let selected = ctx.data_mut(|data| {
        let id = egui::Id::new("palette_selected");
        let qid = egui::Id::new("palette_selected_query");
        let prev = data.get_temp::<String>(qid);
        let sel = if prev.as_deref() == Some(&q) {
            data.get_temp::<usize>(id).unwrap_or(0)
        } else {
            0
        }
        .min(hits.len().saturating_sub(1));
        data.insert_temp(qid, q.clone());
        let sel = if down {
            (sel + 1).min(hits.len().saturating_sub(1))
        } else if up {
            sel.saturating_sub(1)
        } else {
            sel
        };
        data.insert_temp(id, sel);
        sel
    });
    let screen = ctx.content_rect();
    let prev_rect = ctx.data(|d| d.get_temp::<Rect>(rect_id));
    let outside = ctx.input(|i| {
        i.pointer.button_pressed(egui::PointerButton::Primary)
            && i.pointer
                .interact_pos()
                .is_some_and(|p| prev_rect.is_some_and(|r| !r.contains(p)))
    });
    if outside {
        app.palette_open = false;
        app.palette_query.clear();
        return;
    }
    let mut chosen: Option<usize> = None;
    let resp = egui::Area::new(egui::Id::new("palette"))
        .order(egui::Order::Foreground)
        .pivot(Align2::CENTER_TOP)
        .fixed_pos(egui::pos2(screen.center().x, screen.top() + 96.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .corner_radius(CornerRadius::same(12))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(560.0);
                    ui.horizontal(|ui| {
                        ui.add(icons::image("search", 18.0, t.text_muted));
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut app.palette_query)
                                .hint_text(tr(
                                    "Rechercher une commande, une vue, un fichier récent…",
                                ))
                                .frame(egui::Frame::NONE)
                                .font(theme::regular(15.0))
                                .desired_width(f32::INFINITY),
                        );
                        if (r.has_focus() || r.lost_focus()) && enter && !hits.is_empty() {
                            chosen = Some(selected);
                        }
                        if !r.has_focus() {
                            r.request_focus();
                        }
                    });
                    ui.separator();
                    if hits.is_empty() {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(tr("Aucun résultat")).color(t.text_muted));
                        ui.add_space(6.0);
                    }
                    for (index, (_, h)) in hits.iter().enumerate() {
                        let (rect, resp) = ui
                            .allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
                        let press = Press::track(ui, &resp);
                        press.wash(ui, rect, 6, index == selected);
                        let body = rect.translate(press.offset());
                        let col = if h.ready { t.text } else { t.text_faint };
                        icons::paint(
                            ui,
                            Rect::from_min_size(body.min + vec2(10.0, 10.0), vec2(16.0, 16.0)),
                            h.icon,
                            16.0,
                            if index == selected { t.accent } else { t.icon },
                        );
                        ui.painter().text(
                            body.min + vec2(36.0, 18.0),
                            Align2::LEFT_CENTER,
                            &h.label,
                            theme::regular(13.5),
                            col,
                        );
                        ui.painter().text(
                            body.right_center() - vec2(10.0, 0.0),
                            Align2::RIGHT_CENTER,
                            &h.detail,
                            theme::mono(11.5),
                            t.text_faint,
                        );
                        if resp.clicked() {
                            chosen = Some(index);
                        }
                    }
                });
        });
    ctx.data_mut(|d| d.insert_temp(rect_id, resp.response.rect));
    if let Some(i) = chosen {
        if let Some((_, h)) = hits.into_iter().nth(i) {
            if h.ready {
                app.palette_open = false;
                app.palette_query.clear();
                match h.target {
                    Target::Cmd(id) => app.cmds.push(id),
                    Target::Recent(p) => app.open(&p, ctx),
                }
            }
        }
    }
}
