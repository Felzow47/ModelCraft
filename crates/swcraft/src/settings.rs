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
    /// Left drag rotates, right/middle drag pans, Ctrl+middle pans, wheel zooms (SolidWorks/eDrawings).
    #[default]
    SolidWorks,
    /// Middle drag orbits, Shift+middle pans, Ctrl+middle zooms; left click only selects.
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
    pub language: LangPref,
    pub theme: ThemePreference,
    pub accent: Accent,
    pub units: Units,
    pub decimals: u8,
    pub nav: NavStyle,
    pub invert_zoom: bool,
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

impl Default for Settings {
    fn default() -> Self {
        Settings {
            language: LangPref::System,
            theme: ThemePreference::System,
            accent: Accent::Amber,
            units: Units::Mm,
            decimals: 2,
            nav: NavStyle::SolidWorks,
            invert_zoom: false,
            ortho: false,
            fov_deg: 30.0,
            background: Background::Gradient,
            bg_top: [0x4A, 0x50, 0x5C],
            bg_bottom: [0x1C, 0x1D, 0x22],
            bg_follow_theme: true,
            lighting: Lighting::Studio,
            edges: true,
            edge_width: 1.0,
            edge_color: [0x14, 0x16, 0x1C],
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
        format!("{:.*} {}", self.decimals as usize, self.units.from_m(m), self.units.suffix())
    }
    pub fn fmt_len_bare(&self, m: f64) -> String {
        format!("{:.*}", self.decimals as usize, self.units.from_m(m))
    }
    pub fn fmt_area(&self, m2: f64) -> String {
        let f = self.units.from_m(1.0);
        format!("{:.*} {}²", self.decimals as usize, m2 * f * f, self.units.suffix())
    }
    pub fn fmt_volume(&self, m3: f64) -> String {
        let f = self.units.from_m(1.0);
        format!("{:.*} {}³", self.decimals as usize, m3 * f * f * f, self.units.suffix())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_and_units() {
        let mut s = Settings::default();
        s.units = Units::In;
        s.decimals = 3;
        let j = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&j).unwrap();
        assert_eq!(back, s);
        assert_eq!(s.fmt_len(0.0254), "1.000 in");
        // unknown / missing fields fall back to defaults
        let partial: Settings = serde_json::from_str("{\"decimals\":1}").unwrap();
        assert_eq!(partial.decimals, 1);
        assert_eq!(partial.msaa, 4);
    }
}
