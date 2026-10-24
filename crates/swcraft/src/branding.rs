//! Product name and identity, kept in one place so the app can be renamed in a single edit.
//! The name is two-tone in the UI: [`NAME_HEAD`] in the text colour, [`NAME_TAIL`] in the accent.

pub const NAME_HEAD: &str = "Model";
pub const NAME_TAIL: &str = "Craft";
pub const NAME: &str = "ModelCraft";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Folder name under %APPDATA% / $XDG_CONFIG_HOME.
pub const CONFIG_DIR: &str = "ModelCraft";
/// ProgID prefix for Windows file associations.
pub const PROG_ID: &str = "ModelCraft.Model";
/// The app icon (rounded amber square, isometric part + orbit), 256 px PNG.
pub const ICON_PNG: &[u8] = include_bytes!("../assets/icon_256.png");
pub const ICON_SVG: &[u8] = include_bytes!("../assets/logo.svg");
