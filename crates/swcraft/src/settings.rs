//! User settings, recent files and pinned folders, saved as JSON in
//! %APPDATA%\ModelCraft\settings.json (Windows) or $XDG_CONFIG_HOME/ModelCraft (elsewhere).

use crate::i18n::LangPref;
use crate::theme::{Accent, ThemePreference};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Units {
    #[default]
    Mm,
    Cm,
    M,
    In,
}

impl Units {
    pub const ALL: [Units; 4] = [Units::Mm, Units::Cm, Units::M, Units::In];
    pub fn suffix(self) -> &'static str {
        match self {
            Units::Mm => "mm",
            Units::Cm => "cm",
            Units::M => "m",
            Units::In => "in",
        }
    }
    /// Metres -> display unit.
    pub fn from_m(self, v: f64) -> f64 {
        match self {
            Units::Mm => v * 1000.0,
            Units::Cm => v * 100.0,
            Units::M => v,
            Units::In => v / 0.0254,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NavStyle {
    /// Left / middle drag rotates, right drag or Ctrl+middle pans, Shift+middle zooms (SolidWorks/eDrawings).
    SolidWorks,
    /// Middle drag orbits, Shift+middle pans, Ctrl+middle zooms (Blender); left drag orbits,
    /// right or Shift+left drag pans. Default.
    #[default]
    Blender,
    /// Middle drag pans, Shift+middle (or left) drag orbits (AutoCAD / Inventor style).
    Cad,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Background {
    #[default]
    Gradient,
    Solid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Lighting {
    #[default]
    Studio,
    Soft,
    Contrast,
    Flat,
}

impl Lighting {
    pub const ALL: [Lighting; 4] = [Lighting::Studio, Lighting::Soft, Lighting::Contrast, Lighting::Flat];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DefaultView {
    #[default]
    Iso,
    Front,
    Top,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Quality {
    Fast,
    #[default]
    Balanced,
    High,
}

impl Quality {
    /// STEP/IGES tessellation fineness (0..1).
    pub fn tessellation(self) -> f32 {
        match self {
            Quality::Fast => 0.15,
            Quality::Balanced => 0.5,
            Quality::High => 0.9,
        }
    }
    /// Offscreen render resolution factor.
    pub fn render_scale(self) -> f32 {
        match self {
            Quality::Fast => 0.75,
            Quality::Balanced => 1.0,
            Quality::High => 1.5,
        }
    }
}

/// How a mouse drag rotates the view.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum OrbitMode {
    /// Trackball around the screen axes: unlimited in every direction (default).
    #[default]
    Free,
    /// Turntable around the vertical axis, stops at the poles.
    Turntable,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RecentFile {
    pub path: String,
    pub size: u64,
    /// Unix seconds of the last opening.
    pub opened: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Settings format version (missing in old files = 0), used to move old defaults forward.
    #[serde(default = "version_zero")]
    pub version: u32,
    pub language: LangPref,
    pub theme: ThemePreference,
    pub accent: Accent,
    pub units: Units,
    pub decimals: u8,
    /// Large areas / volumes / lengths switch to a bigger unit (cm², m², m³…).
    pub auto_units: bool,
    pub nav: NavStyle,
    pub invert_zoom: bool,
    /// Blender keymap: numpad 1/3/7 views (Ctrl = opposite), 5 ortho, 2/4/6/8 turn 15°,
    /// Numpad . frame selected, G move, R rotate, Ctrl+Z / Ctrl+Y.
    pub blender_keys: bool,
    pub orbit: OrbitMode,
    pub ortho: bool,
    pub fov_deg: f32,
    pub background: Background,
    pub bg_top: [u8; 3],
    pub bg_bottom: [u8; 3],
    /// Use the theme's colours for the background instead of `bg_*`.
    pub bg_follow_theme: bool,
    pub lighting: Lighting,
    pub edges: bool,
    pub edge_width: f32,
    pub edge_color: [u8; 3],
    pub msaa: u32,
    pub quality: Quality,
    pub show_axes: bool,
    pub show_grid: bool,
    pub show_viewcube: bool,
    pub default_view: DefaultView,
    pub recent_max: usize,
    pub base_color: [u8; 3],
    pub show_tree: bool,
    pub show_info: bool,
    pub recent: Vec<RecentFile>,
    pub pinned: Vec<String>,
}

fn version_zero() -> u32 {
    0
}

/// Current settings version (4 = ModelCraft V4: brighter viewport, Blender-style navigation).
pub const SETTINGS_VERSION: u32 = 4;

impl Default for Settings {
    fn default() -> Self {
        Settings {
            version: SETTINGS_VERSION,
            language: LangPref::System,
            theme: ThemePreference::System,
            accent: Accent::Amber,
            units: Units::Mm,
            decimals: 2,
            auto_units: true,
            nav: NavStyle::Blender,
            invert_zoom: false,
            blender_keys: true,
            orbit: OrbitMode::Free,
            ortho: false,
            fov_deg: 30.0,
            background: Background::Gradient,
            bg_top: [0x5C, 0x63, 0x71],
            bg_bottom: [0x2A, 0x2D, 0x35],
            bg_follow_theme: true,
            lighting: Lighting::Studio,
            edges: true,
            edge_width: 1.0,
            edge_color: [0x1E, 0x21, 0x28],
            msaa: 4,
            quality: Quality::Balanced,
            show_axes: true,
            show_grid: false,
            show_viewcube: true,
            default_view: DefaultView::Iso,
            recent_max: 12,
            base_color: [0xA8, 0xB4, 0xC6],
            show_tree: true,
            show_info: true,
            recent: Vec::new(),
            pinned: Vec::new(),
        }
    }
}

pub fn config_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MODELCRAFT_CONFIG_DIR") {
        return Some(PathBuf::from(p));
    }
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    base.map(|b| b.join(crate::branding::CONFIG_DIR))
}

pub fn settings_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("settings.json"))
}

impl Settings {
    pub fn load() -> Settings {
        settings_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .map(|mut s: Settings| {
                s.migrate();
                s.sanitize();
                s
            })
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(p) = settings_path() else { return };
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            // write then rename so a crash never leaves a half-written file
            let tmp = p.with_extension("json.tmp");
            if std::fs::write(&tmp, s).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }

    /// Bring a settings file from an older version up to date: values still equal to an old
    /// default take the new default; anything the user chose is kept.
    pub fn migrate(&mut self) {
        if self.version < 4 {
            let d = Settings::default();
            // V4: the old SolidWorks default made the middle button orbit where people expect a
            // pan; the Blender style (middle orbit, Shift+middle pan, left orbit, right pan) is
            // the new default
            if self.nav == NavStyle::SolidWorks {
                self.nav = NavStyle::Blender;
            }
            if self.bg_top == [0x4A, 0x50, 0x5C] && self.bg_bottom == [0x1C, 0x1D, 0x22] {
                self.bg_top = d.bg_top;
                self.bg_bottom = d.bg_bottom;
            }
            if self.edge_color == [0x14, 0x16, 0x1C] {
                self.edge_color = d.edge_color;
            }
        }
        self.version = SETTINGS_VERSION;
    }

    pub fn sanitize(&mut self) {
        self.decimals = self.decimals.min(6);
        self.fov_deg = self.fov_deg.clamp(10.0, 90.0);
        self.edge_width = self.edge_width.clamp(1.0, 4.0);
        if ![1, 2, 4, 8].contains(&self.msaa) {
            self.msaa = 4;
        }
        self.recent_max = self.recent_max.clamp(1, 50);
        self.recent.truncate(self.recent_max);
    }

    /// Everything back to defaults except the lists (recent files, pinned folders).
    pub fn reset_preferences(&mut self) {
        let recent = std::mem::take(&mut self.recent);
        let pinned = std::mem::take(&mut self.pinned);
        *self = Settings { recent, pinned, ..Settings::default() };
    }

    pub fn add_recent(&mut self, path: &Path) {
        let p = path.to_string_lossy().into_owned();
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        self.recent.retain(|r| r.path != p);
        self.recent.insert(0, RecentFile { path: p, size, opened: now });
        self.recent.truncate(self.recent_max);
    }

    pub fn remove_recent(&mut self, path: &str) {
        self.recent.retain(|r| r.path != path);
    }

    pub fn pin(&mut self, dir: &Path) {
        let p = dir.to_string_lossy().into_owned();
        if !self.pinned.contains(&p) {
            self.pinned.push(p);
        }
    }

    /// Format a length given in metres.
    pub fn fmt_len(&self, m: f64) -> String {
        let (v, u) = match self.units {
            Units::Mm | Units::Cm if self.auto_units && m.abs() >= 10.0 => (m, "m"),
            u => (u.from_m(m), u.suffix()),
        };
        format!("{} {u}", fmt_num(v, self.decimals))
    }
    /// Unit used for a group of lengths whose largest is `max_m` metres (same rule as fmt_len).
    fn group_unit(&self, max_m: f64) -> (f64, &'static str) {
        match self.units {
            Units::Mm | Units::Cm if self.auto_units && max_m.abs() >= 10.0 => (1.0, "m"),
            u => (u.from_m(1.0), u.suffix()),
        }
    }
    /// A length shown in the unit chosen for a group (so X, Y, Z of one box share a unit).
    pub fn fmt_len_group(&self, m: f64, max_m: f64) -> String {
        let (f, u) = self.group_unit(max_m);
        format!("{} {u}", fmt_num(m * f, self.decimals))
    }
    /// "a × b × c unit" with one unit for the three values.
    pub fn fmt_dims(&self, x: f64, y: f64, z: f64) -> String {
        let (f, u) = self.group_unit(x.abs().max(y.abs()).max(z.abs()));
        format!("{} × {} × {} {u}", fmt_num(x * f, self.decimals), fmt_num(y * f, self.decimals), fmt_num(z * f, self.decimals))
    }
    pub fn fmt_len_bare(&self, m: f64) -> String {
        fmt_num(self.units.from_m(m), self.decimals)
    }
    /// Area with thousands separators; with automatic units, large values move up to cm² / m²
    /// (ft² for inches) so they stay readable.
    pub fn fmt_area(&self, m2: f64) -> String {
        let f = self.units.from_m(1.0);
        let mut v = m2 * f * f;
        let mut u = format!("{}²", self.units.suffix());
        if self.auto_units {
            let ladder: &[(f64, &str)] = match self.units {
                Units::Mm => &[(100.0, "cm²"), (1e4, "m²")],
                Units::Cm => &[(1e4, "m²")],
                Units::M => &[],
                Units::In => &[(144.0, "ft²")],
            };
            for &(k, name) in ladder {
                if v.abs() >= 1e4 {
                    v /= k;
                    u = name.to_string();
                }
            }
        }
        format!("{} {u}", fmt_num(v, self.decimals))
    }
    pub fn fmt_volume(&self, m3: f64) -> String {
        let f = self.units.from_m(1.0);
        let mut v = m3 * f * f * f;
        let mut u = format!("{}³", self.units.suffix());
        if self.auto_units {
            let ladder: &[(f64, &str)] = match self.units {
                Units::Mm => &[(1e3, "cm³"), (1e6, "m³")],
                Units::Cm => &[(1e6, "m³")],
                Units::M => &[],
                Units::In => &[(1728.0, "ft³")],
            };
            for &(k, name) in ladder {
                if v.abs() >= 1e6 {
                    v /= k;
                    u = name.to_string();
                }
            }
        }
        format!("{} {u}", fmt_num(v, self.decimals))
    }
}

/// Number with `decimals` digits and thousands separators in the interface language
/// (French: `6 115 613,46`, English: `6,115,613.46`).
pub fn fmt_num(v: f64, decimals: u8) -> String {
    let fr = crate::i18n::current() == crate::i18n::Lang::Fr;
    fmt_num_in(v, decimals, fr)
}

pub fn fmt_num_in(v: f64, decimals: u8, fr: bool) -> String {
    if !v.is_finite() {
        return format!("{v}");
    }
    let s = format!("{:.*}", decimals as usize, v.abs());
    let (int, frac) = s.split_once('.').map(|(a, b)| (a.to_string(), Some(b.to_string()))).unwrap_or((s.clone(), None));
    let sep = if fr { '\u{a0}' } else { ',' };
    let mut out = String::new();
    let n = int.len();
    for (k, ch) in int.chars().enumerate() {
        if k > 0 && (n - k) % 3 == 0 {
            out.push(sep);
        }
        out.push(ch);
    }
    if let Some(f) = frac {
        out.push(if fr { ',' } else { '.' });
        out.push_str(&f);
    }
    let neg = v < 0.0 && out.chars().any(|c| c.is_ascii_digit() && c != '0');
    if neg {
        out.insert(0, '-');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_settings_are_migrated() {
        let old = r#"{"nav":"SolidWorks","bg_top":[74,80,92],"bg_bottom":[28,29,34],"edge_color":[20,22,28],"show_tree":false}"#;
        let mut s: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(s.version, 0);
        s.migrate();
        assert_eq!(s.version, SETTINGS_VERSION);
        assert_eq!(s.nav, NavStyle::Blender);
        assert_eq!(s.bg_top, Settings::default().bg_top);
        assert!(!s.show_tree, "user choices are kept");
        let mut custom: Settings = serde_json::from_str(r#"{"bg_top":[1,2,3]}"#).unwrap();
        custom.migrate();
        assert_eq!(custom.bg_top, [1, 2, 3]);
        // a current file is left alone
        let mut cur = Settings { nav: NavStyle::SolidWorks, ..Settings::default() };
        cur.migrate();
        assert_eq!(cur.nav, NavStyle::SolidWorks);
    }

    #[test]
    fn number_and_auto_units() {
        let mut g = Settings::default();
        g.decimals = 2;
        g.auto_units = true;
        g.units = Units::Mm;
        assert_eq!(fmt_num_in(18.0, 2, false), "18.00");
        assert!(g.fmt_dims(18.0, 7.31769, 16.0).ends_with(" m"));
        assert!(g.fmt_dims(0.2, 0.06, 0.01).ends_with(" mm"));

        assert_eq!(fmt_num_in(6115613.456, 2, false), "6,115,613.46");
        assert_eq!(fmt_num_in(6115613.456, 2, true), "6\u{a0}115\u{a0}613,46");
        assert_eq!(fmt_num_in(-1234.5, 1, false), "-1,234.5");
        assert_eq!(fmt_num_in(999.0, 0, false), "999");
        assert_eq!(fmt_num_in(-0.001, 2, false), "0.00");
        crate::i18n::set_current(crate::i18n::Lang::En);
        let s = Settings::default();
        // the screenshot value: 6 115 613.46 mm² -> 6.12 m²
        assert_eq!(s.fmt_area(6115613.46e-6), "6.12 m²");
        assert_eq!(s.fmt_area(977511.84e-6), "9,775.12 cm²");
        assert_eq!(s.fmt_area(500e-6), "500.00 mm²");
        assert_eq!(s.fmt_volume(541097.55e-9), "541,097.55 mm³");
        assert_eq!(s.fmt_volume(5e6 * 1e-9), "5,000.00 cm³");
        assert_eq!(s.fmt_volume(2.0), "2.00 m³");
        assert_eq!(s.fmt_len(12.5), "12.50 m");
        let raw = Settings { auto_units: false, ..Settings::default() };
        assert_eq!(raw.fmt_area(6115613.46e-6), "6,115,613.46 mm²");
    }
    #[test]
    fn roundtrip_and_units() {
        let mut s = Settings::default();
        s.units = Units::In;
        s.decimals = 3;
        let j = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&j).unwrap();
        assert_eq!(back, s);
        crate::i18n::set_current(crate::i18n::Lang::En);
        assert_eq!(s.fmt_len(0.0254), "1.000 in");
        // unknown / missing fields fall back to defaults
        let partial: Settings = serde_json::from_str("{\"decimals\":1}").unwrap();
        assert_eq!(partial.decimals, 1);
        assert_eq!(partial.msaa, 4);
    }
}
