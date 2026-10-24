//! Lucide icons (ISC, assets/icons/LICENSE-lucide.txt), recoloured to white so they can be
//! tinted per use and rasterised by egui_extras at the on-screen size — the PdfCraft approach.

use crate::theme::{self, Tokens};
use egui::{Color32, Rect, Response, Sense, Stroke, Vec2};
use std::collections::HashMap;
use std::sync::OnceLock;

include!(concat!(env!("OUT_DIR"), "/icon_data.rs"));

fn white() -> &'static HashMap<&'static str, Vec<u8>> {
    static MAP: OnceLock<HashMap<&'static str, Vec<u8>>> = OnceLock::new();
    MAP.get_or_init(|| {
        ICONS
            .iter()
            .map(|(n, b)| {
                let s = String::from_utf8_lossy(b).replace("currentColor", "#ffffff").replace("stroke-width=\"2\"", "stroke-width=\"1.75\"");
                (*n, s.into_bytes())
            })
            .collect()
    })
}

pub fn exists(name: &str) -> bool {
    white().contains_key(name)
}

fn source(name: &str) -> egui::ImageSource<'static> {
    let name = if exists(name) { name } else { "box" };
    let bytes: &'static [u8] = white().get(name).map(|b| b.as_slice()).unwrap_or(&[]);
    egui::ImageSource::Bytes { uri: format!("bytes://icons/{name}.svg").into(), bytes: egui::load::Bytes::Static(bytes) }
}

pub fn image(name: &str, size: f32, tint: Color32) -> egui::Image<'static> {
    egui::Image::new(source(name)).fit_to_exact_size(Vec2::splat(size)).tint(tint)
}

pub fn paint(ui: &egui::Ui, rect: Rect, name: &str, size: f32, tint: Color32) {
    image(name, size, tint).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(size)));
}

/// Square icon button: transparent until hovered; `selected` gets the accent treatment.
pub fn button(ui: &mut egui::Ui, name: &str, box_size: f32, selected: bool, tooltip: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(box_size), Sense::click());
    if ui.is_rect_visible(rect) {
        let press = theme::Press::track(ui, &resp);
        let fill = press.wash_color(&t, selected);
        if fill.a() > 0 {
            ui.painter().rect_filled(rect, t.radius, fill);
        }
        if selected {
            ui.painter().rect_stroke(rect.shrink(0.5), t.radius, Stroke::new(1.0, t.accent.gamma_multiply(0.45)), egui::StrokeKind::Inside);
        }
        let tint = if !ui.is_enabled() { t.text_faint.gamma_multiply(0.6) } else if selected { t.accent_text } else { t.icon };
        let size = (box_size * 0.5).round() * (1.0 - 0.08 * press.down);
        paint(ui, rect.translate(press.offset()), name, size, tint);
    }
    let resp = theme::hand(resp);
    if tooltip.is_empty() { resp } else { resp.on_hover_text(tooltip) }
}
