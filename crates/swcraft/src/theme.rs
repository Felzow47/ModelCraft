//! Design tokens and egui style.
//!
//! Ported from PdfCraft's `theme.rs` (crates/ui-egui, © ArtCraft Team and the PdfCraft
//! contributors, MIT OR Apache-2.0) so ModelCraft shares the Craft suite's look: neutral chrome,
//! Inter, 6 px radii, eased hover/press washes. The accent is configurable (amber by default).

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Rect, Response, Stroke,
    Vec2, Visuals,
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ThemeKind {
    Light,
    #[default]
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePreference {
    pub fn resolve(self, system: Option<bool>, fallback: ThemeKind) -> ThemeKind {
        match self {
            Self::Light => ThemeKind::Light,
            Self::Dark => ThemeKind::Dark,
            Self::System => match system {
                Some(true) => ThemeKind::Dark,
                Some(false) => ThemeKind::Light,
                None => fallback,
            },
        }
    }
}

/// Accent colours offered in the settings. Amber is ModelCraft's own (the logo colour).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Accent {
    #[default]
    Amber,
    Blue,
    Teal,
    Violet,
    Rose,
    Green,
}

impl Accent {
    pub const ALL: [Accent; 6] = [
        Accent::Amber,
        Accent::Blue,
        Accent::Teal,
        Accent::Violet,
        Accent::Rose,
        Accent::Green,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Accent::Amber => "Ambre",
            Accent::Blue => "Bleu",
            Accent::Teal => "Sarcelle",
            Accent::Violet => "Violet",
            Accent::Rose => "Rose",
            Accent::Green => "Vert",
        }
    }
    /// (dark-theme accent, light-theme accent)
    pub fn colors(self) -> (Color32, Color32) {
        match self {
            Accent::Amber => (
                Color32::from_rgb(0xF2, 0x8C, 0x28),
                Color32::from_rgb(0xC9, 0x62, 0x0E),
            ),
            Accent::Blue => (
                Color32::from_rgb(0x4B, 0x8B, 0xF5),
                Color32::from_rgb(0x1B, 0x63, 0xE0),
            ),
            Accent::Teal => (
                Color32::from_rgb(0x2D, 0xC2, 0xB8),
                Color32::from_rgb(0x0B, 0x84, 0x7C),
            ),
            Accent::Violet => (
                Color32::from_rgb(0xC5, 0x9E, 0xFB),
                Color32::from_rgb(0x6D, 0x47, 0xD9),
            ),
            Accent::Rose => (
                Color32::from_rgb(0xF0, 0x6A, 0x9E),
                Color32::from_rgb(0xC8, 0x2E, 0x6B),
            ),
            Accent::Green => (
                Color32::from_rgb(0x4C, 0xC3, 0x6E),
                Color32::from_rgb(0x1E, 0x8A, 0x45),
            ),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub kind: ThemeKind,
    pub titlebar: Color32,
    pub chrome: Color32,
    pub panel: Color32,
    pub pasteboard: Color32,
    pub card: Color32,
    pub border: Color32,
    pub divider: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    pub icon: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub selected: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub accent_soft: Color32,
    pub field: Color32,
    pub shadow: Color32,
    pub danger: Color32,
    pub warning: Color32,
    pub radius: u8,
}

impl Tokens {
    pub fn for_kind(kind: ThemeKind, accent: Accent) -> Self {
        let (ad, al) = accent.colors();
        match kind {
            ThemeKind::Light => Self {
                kind,
                titlebar: Color32::from_rgb(0xE9, 0xE9, 0xEB),
                chrome: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                panel: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                pasteboard: Color32::from_rgb(0xF1, 0xF1, 0xF3),
                card: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                border: Color32::from_rgb(0xDA, 0xDA, 0xDE),
                divider: Color32::from_rgb(0xE8, 0xE8, 0xEB),
                text: Color32::from_rgb(0x22, 0x22, 0x26),
                text_muted: Color32::from_rgb(0x5E, 0x5E, 0x66),
                text_faint: Color32::from_rgb(0x6B, 0x6B, 0x73),
                icon: Color32::from_rgb(0x44, 0x44, 0x4B),
                hover: Color32::from_rgb(0xF0, 0xF0, 0xF3),
                pressed: Color32::from_rgb(0xE4, 0xE4, 0xE9),
                selected: Color32::WHITE.lerp_to_gamma(al, 0.16),
                accent: al,
                accent_text: al.lerp_to_gamma(Color32::BLACK, 0.12),
                accent_soft: Color32::WHITE.lerp_to_gamma(al, 0.12),
                field: Color32::from_rgb(0xEC, 0xEC, 0xEF),
                shadow: Color32::from_black_alpha(34),
                danger: Color32::from_rgb(0xC8, 0x32, 0x2D),
                warning: Color32::from_rgb(0xA8, 0x5F, 0x00),
                radius: 6,
            },
            ThemeKind::Dark => Self {
                kind,
                titlebar: Color32::from_rgb(0x1B, 0x1B, 0x1E),
                chrome: Color32::from_rgb(0x26, 0x26, 0x2A),
                panel: Color32::from_rgb(0x26, 0x26, 0x2A),
                pasteboard: Color32::from_rgb(0x19, 0x19, 0x1C),
                card: Color32::from_rgb(0x2E, 0x2E, 0x33),
                border: Color32::from_rgb(0x3C, 0x3C, 0x43),
                divider: Color32::from_rgb(0x33, 0x33, 0x39),
                text: Color32::from_rgb(0xEC, 0xEC, 0xEF),
                text_muted: Color32::from_rgb(0xAE, 0xAE, 0xB6),
                text_faint: Color32::from_rgb(0x97, 0x97, 0x9E),
                icon: Color32::from_rgb(0xD4, 0xD4, 0xDA),
                hover: Color32::from_rgb(0x34, 0x34, 0x3A),
                pressed: Color32::from_rgb(0x3E, 0x3E, 0x45),
                selected: Color32::from_rgb(0x26, 0x26, 0x2A).lerp_to_gamma(ad, 0.32),
                accent: ad,
                accent_text: ad.lerp_to_gamma(Color32::WHITE, 0.3),
                accent_soft: Color32::from_rgb(0x26, 0x26, 0x2A).lerp_to_gamma(ad, 0.22),
                field: Color32::from_rgb(0x1E, 0x1E, 0x22),
                shadow: Color32::from_black_alpha(120),
                danger: Color32::from_rgb(0xF0, 0x6E, 0x64),
                warning: Color32::from_rgb(0xF5, 0xB4, 0x5A),
                radius: 6,
            },
        }
    }

    pub fn get(ctx: &egui::Context) -> Self {
        ctx.data(|d| d.get_temp::<Tokens>(egui::Id::new("craft-theme")))
            .unwrap_or_else(|| Self::for_kind(ThemeKind::Dark, Accent::Amber))
    }

    pub fn dark(&self) -> bool {
        self.kind == ThemeKind::Dark
    }

    /// Text drawn on an accent fill.
    pub fn on_accent(&self) -> Color32 {
        if self.dark() {
            Color32::from_rgb(0x16, 0x16, 0x19)
        } else {
            Color32::WHITE
        }
    }
}

/// Hover and press amounts for chrome controls (PdfCraft): the wash eases in; holding the
/// pointer darkens it and drops the content one pixel.
#[derive(Clone, Copy, Debug)]
pub struct Press {
    pub hover: f32,
    pub down: f32,
}

impl Press {
    pub fn track(ui: &egui::Ui, resp: &Response) -> Self {
        let hot = resp.hovered() || resp.is_pointer_button_down_on();
        let ease = egui::emath::easing::quadratic_out;
        Self {
            hover: ui.ctx().animate_bool_with_time_and_easing(
                resp.id.with("chrome-hover"),
                hot,
                0.12,
                ease,
            ),
            down: ui.ctx().animate_bool_with_time_and_easing(
                resp.id.with("chrome-down"),
                resp.is_pointer_button_down_on(),
                0.06,
                ease,
            ),
        }
    }
    pub fn fill(self, rest: Color32, hover: Color32, pressed: Color32) -> Color32 {
        rest.lerp_to_gamma(hover, self.hover)
            .lerp_to_gamma(pressed, self.down)
    }
    pub fn offset(self) -> Vec2 {
        Vec2::new(0.0, self.down)
    }
    pub fn wash_color(self, t: &Tokens, selected: bool) -> Color32 {
        if selected {
            self.fill(
                t.accent_soft,
                t.accent_soft.lerp_to_gamma(t.accent, 0.16),
                t.selected,
            )
        } else {
            self.fill(Color32::TRANSPARENT, t.hover, t.pressed)
        }
    }
    pub fn wash(self, ui: &egui::Ui, rect: Rect, radius: impl Into<CornerRadius>, selected: bool) {
        let fill = self.wash_color(&Tokens::get(ui.ctx()), selected);
        if fill.a() > 0 {
            ui.painter().rect_filled(rect, radius, fill);
        }
    }
}

pub fn hand(resp: Response) -> Response {
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn font_definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    };
    add(
        &mut fonts,
        "Inter",
        include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    );
    add(
        &mut fonts,
        "Inter-Medium",
        include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    );
    add(
        &mut fonts,
        "Inter-SemiBold",
        include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    );
    add(
        &mut fonts,
        "JetBrainsMono",
        include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "Inter".to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "JetBrainsMono".to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(1, "Inter".to_owned());
    let fallback: Vec<String> = fonts.families[&FontFamily::Proportional].clone();
    for (fam, primary) in [("medium", "Inter-Medium"), ("semibold", "Inter-SemiBold")] {
        let mut stack = vec![primary.to_owned()];
        stack.extend(fallback.iter().cloned());
        fonts.families.insert(FontFamily::Name(fam.into()), stack);
    }
    fonts
}

pub fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}
pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

pub fn apply(ctx: &egui::Context, kind: ThemeKind, accent: Accent) {
    ctx.set_theme(egui::ThemePreference::System);
    let t = Tokens::for_kind(kind, accent);
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("craft-theme"), t));
    let mut v = if t.dark() {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.panel_fill = t.panel;
    v.window_fill = t.card;
    v.window_stroke = Stroke::new(1.0, t.border);
    v.extreme_bg_color = t.field;
    v.faint_bg_color = t.hover;
    v.selection.bg_fill = t.accent_soft;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.hyperlink_color = t.accent_text;
    v.override_text_color = Some(t.text);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = egui::Shadow {
        offset: [0, 8],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(if t.dark() { 110 } else { 38 }),
    };
    v.popup_shadow = egui::Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(if t.dark() { 90 } else { 30 }),
    };
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(t.radius);
        w.fg_stroke.color = t.text;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.divider);
    v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    v.widgets.inactive.bg_fill = t.field;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.hovered.weak_bg_fill = t.hover;
    v.widgets.hovered.bg_fill = t.hover;
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.active.weak_bg_fill = t.pressed;
    v.widgets.active.bg_fill = t.pressed;
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    v.widgets.open.weak_bg_fill = t.hover;
    v.text_options.subpixel_binning = false;
    if !t.dark() {
        v.text_options.color_transfer_function =
            egui::epaint::FontColorTransferFunction::Gamma(0.75);
    }
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 5.0);
        s.spacing.menu_margin = egui::Margin::same(6);
        s.spacing.scroll.bar_width = 8.0;
        s.spacing.scroll.floating = true;
        s.spacing.scroll.floating_width = 3.0;
        s.spacing.scroll.foreground_color = true;
        s.text_styles.insert(egui::TextStyle::Body, regular(13.0));
        s.text_styles.insert(egui::TextStyle::Button, regular(13.0));
        s.text_styles.insert(egui::TextStyle::Small, regular(11.0));
        s.text_styles
            .insert(egui::TextStyle::Heading, semibold(17.0));
        s.text_styles.insert(egui::TextStyle::Monospace, mono(12.0));
        s.interaction.tooltip_delay = 0.35;
    });
    let style = ctx.global_style();
    ctx.set_style_of(egui::Theme::Light, style.clone());
    ctx.set_style_of(egui::Theme::Dark, style);
}
