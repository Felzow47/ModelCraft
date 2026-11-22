//! # sw-format
//!
//! Clean-room reader for SolidWorks part (`.SLDPRT`) and assembly (`.SLDASM`)
//! files, written from format observations only (no vendor code).
//!
//! * [`container`]: OLE (pre-2015) and disguised-ZIP (2015+) containers
//! * [`dl`]: cached display tessellation (`Contents/DisplayLists`)
//! * [`mesh`]: indexed triangle meshes + B-rep edges
//! * [`assembly`]: component tree and transforms (`COMPINSTANCETREE`)
//! * [`preview`]: embedded thumbnails
//! * [`Scene`]: everything a viewer needs, in one call.

pub mod assembly;
pub mod bodies;
pub mod container;
pub mod ctl;
pub mod dl;
pub mod mesh;
pub mod meshdata;
pub mod gfxstrips;
pub mod pointcloud;
pub mod preview;

pub use container::{Container, ContainerKind, StreamInfo};
pub use ctl::{Ctl, Progress};
pub use mesh::TriMesh;
pub use preview::Rgba;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a SolidWorks file: {0}")]
    NotSolidWorks(String),
    #[error("corrupt data: {0}")]
    Corrupt(String),
    #[error("chargement annulé")]
    Cancelled,
    #[error("fichier trop volumineux - {0}")]
    TooLarge(String),
    #[error("mémoire insuffisante - {0}")]
    OutOfMemory(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub const DISPLAY_LISTS: &str = "Contents/DisplayLists";
pub const COMP_TREE: &str = "swXmlContents/COMPINSTANCETREE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    Part,
    Assembly,
    /// Generic model from another format (STL, OBJ, STEP…), possibly with several objects.
    Model,
}

/// Read the display list of an opened container (decompressed), if any.
pub fn read_display_lists(c: &Container) -> Result<Option<Vec<u8>>> {
    c.read(DISPLAY_LISTS)
}

/// Best thumbnail: `PreviewPNG`, any `*PreviewPNG`, then `Preview` (DIB).
pub fn read_preview(c: &Container) -> Option<Rgba> {
    let mut names: Vec<&StreamInfo> = c.streams().iter().filter(|s| s.name.ends_with("PreviewPNG")).collect();
    names.sort_by_key(|s| (s.name != "PreviewPNG", s.name.len()));
    for s in names {
        if let Ok(b) = c.read_info(s) {
            if let Some(img) = preview::decode_png(&b) {
                return Some(img);
            }
        }
    }
    for s in c.streams().iter().filter(|s| s.name == "Preview" || s.name.ends_with("-Preview")) {
        if let Ok(b) = c.read_info(s) {
            if let Some(img) = preview::decode_dib(&b) {
                return Some(img);
            }
        }
    }
    None
}

#[derive(Debug, Clone, Default)]
pub struct Timings {
    /// Open container + list streams.
    pub open: Duration,
    /// Read + decompress DisplayLists (and XML tree).
    pub read: Duration,
    /// Decode tessellation records.
    pub decode: Duration,
    /// Build indexed meshes + edges.
    pub mesh: Duration,
    /// Loading external component files.
    pub external: Duration,
    pub total: Duration,
    /// How the DisplayLists stream was turned into meshes (diagnostics).
    pub dl: Option<DlStats>,
}

/// Statistics of the display-list → mesh conversion.
#[derive(Debug, Clone, Default)]
pub struct DlStats {
    /// Decompressed size of the DisplayLists stream.
    pub raw_bytes: u64,
    /// Stored (compressed) size.
    pub stored_bytes: u64,
    /// Decompressed piece by piece (two passes) instead of being held in memory.
    pub streamed: bool,
    pub faces: usize,
    /// Vertices in the file's tessellation.
    pub src_vertices: usize,
    pub src_triangles: usize,
    /// Vertices actually kept for display (lower if simplified).
    pub out_vertices: usize,
    pub out_triangles: usize,
    pub simplified: bool,
    pub blocks: usize,
    /// Vertex budget used to decide on simplification.
    pub budget: usize,
    pub pass1: Duration,
    pub pass2: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeshSource {
    /// Cached in the opened file itself.
    Embedded,
    /// Loaded from a sibling component file on disk.
    External(PathBuf),
}

#[derive(Debug, Clone)]
pub struct SceneMesh {
    pub name: String,
    pub mesh: TriMesh,
    pub source: MeshSource,
    /// Display colour (linear-ish sRGB 0..1, alpha) if the file defines one.
    pub color: Option<[f32; 4]>,
}

#[derive(Debug, Clone)]
pub struct SceneComponent {
    pub name: String,
    pub path: String,
    pub depth: usize,
    pub parent: Option<usize>,
    pub is_assembly: bool,
    /// Column-major 4x4 world transform (metres).
    pub world: [f32; 16],
    pub mesh: Option<usize>,
    pub suppressed: bool,
    pub hidden: bool,
    /// Colour override for this component (takes precedence over the mesh colour).
    pub color: Option<[f32; 4]>,
}

#[derive(Debug, Clone)]
pub struct Scene {
    pub path: PathBuf,
    pub kind: DocKind,
    pub container: ContainerKind,
    pub stream_count: usize,
    pub meshes: Vec<SceneMesh>,
    pub components: Vec<SceneComponent>,
    pub preview: Option<Rgba>,
    pub warnings: Vec<String>,
    /// Descriptive metadata shown in the info panel (label, value), e.g. Revit project data.
    pub properties: Vec<(String, String)>,
    pub timings: Timings,
    /// The model's vertical axis is +Z (most CAD exchange formats); SolidWorks is Y-up.
    pub up_z: bool,
    /// Read-only document browser (Revit "Navigateur de projet"), flattened in display order.
    pub browser: Vec<BrowserNode>,
    /// 2D content of the sheets (Revit "Feuilles"), by sheet element id (`BrowserNode::id`).
    pub sheets: Vec<Sheet2d>,
    /// How each Revit view looks at the model (direction, up, cropped region), by view id.
    pub view_frames: Vec<ViewFrame>,
    /// Edge colour of some meshes (by index into `meshes`; the others use the viewer's).
    pub edge_colors: std::collections::HashMap<usize, [u8; 3]>,
}

/// A Revit view's camera as stored in the file, in model coordinates (metres, z up): `dir`
/// points from the model towards the viewer (Revit `ViewDirection`), `up` is the screen up
/// (`UpDirection`), `region` the box the view's crop region and depth clip enclose (None:
/// the whole model).
#[derive(Debug, Clone, Default)]
pub struct ViewFrame {
    pub id: i64,
    pub dir: [f32; 3],
    pub up: [f32; 3],
    pub region: Option<([f32; 3], [f32; 3])>,
    /// What the view hides (crop region, plan view range, depth clip, 3D section box), when
    /// the file gives it: only the model inside this box is drawn.
    pub clip: Option<ViewClip>,
    /// Components the view does not show (categories switched off in its visibility settings,
    /// elements hidden in the view), as indices into [`Scene::components`].
    pub hide: Vec<usize>,
}

/// An oriented box in model coordinates (metres, z up): a point `p` is kept when, for each
/// axis `k`, `lo[k] <= dot(p - orig, axes[k]) <= hi[k]`. Unlimited sides are ±[`CLIP_FAR`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewClip {
    pub orig: [f32; 3],
    pub axes: [[f32; 3]; 3],
    pub lo: [f32; 3],
    pub hi: [f32; 3],
}

/// Bound of an unlimited side of a [`ViewClip`] (metres).
pub const CLIP_FAR: f32 = 1.0e7;

/// What is drawn on a sheet, in paper metres (x right, y up): title block lines and texts,
/// the placed views (frame, title, their model edges or saved image), texts and schedules.
#[derive(Debug, Clone, Default)]
pub struct Sheet2d {
    pub id: i64,
    /// Paper extents: min x, min y, max x, max y.
    pub extent: [f32; 4],
    /// Title block, annotation and frame lines (polylines).
    pub lines: Vec<Vec<[f32; 2]>>,
    /// Model edges inside the placed views (segments), drawn thinner.
    pub fine: Vec<[[f32; 2]; 2]>,
    pub texts: Vec<Text2d>,
    pub images: Vec<Image2d>,
    pub views: Vec<Viewport2d>,
    /// View and sheet annotations drawn with their line style: detail lines, filled region
    /// boundaries and hatch lines, dimensions, tags (dash patterns already cut).
    pub strokes: Vec<Stroke2d>,
    /// Filled regions with a solid fill (triangles).
    pub fills: Vec<Fill2d>,
}

/// A polyline with its weight (`width`, paper metres) and colour.
#[derive(Debug, Clone, Default)]
pub struct Stroke2d {
    pub pts: Vec<[f32; 2]>,
    pub width: f32,
    pub color: [u8; 3],
}

/// A filled area (triangles, paper metres) and its colour.
#[derive(Debug, Clone, Default)]
pub struct Fill2d {
    pub tris: Vec<[[f32; 2]; 3]>,
    pub color: [u8; 3],
}

/// A text on a sheet: `pos` is the anchor given by `halign` (0 left, 1 centre, 2 right) and
/// `valign` (0 top, 1 middle, 2 bottom); `dir` the base line direction; `height` the letter
/// height (paper metres).
#[derive(Debug, Clone, Default)]
pub struct Text2d {
    pub pos: [f32; 2],
    pub dir: [f32; 2],
    pub height: f32,
    pub halign: u8,
    pub valign: u8,
    pub text: String,
    pub bold: bool,
    /// Width its lines wrap at (paper metres; 0: no wrapping).
    pub wrap: f32,
    pub color: [u8; 3],
    /// Its font ("Arial", "Arial Narrow"…; empty: the viewer's own).
    pub font: String,
    pub italic: bool,
    pub underline: bool,
    /// Parts of `text` written their own way (a rich text's bold, italic or underlined
    /// words), in order, covering the whole text; empty: all of it as the text says.
    pub spans: Vec<TextSpan>,
    /// Distance between tab stops (paper metres; 0: unknown).
    pub tab: f32,
    /// A word wider than a row is broken between characters (a text note's), else it runs
    /// past (a label's: Revit never breaks "103" or "AC-401" in a tag).
    pub break_words: bool,
}

impl Text2d {
    /// The font size it is drawn at (paper metres): see [`font_size`].
    pub fn font_size(&self) -> f32 {
        font_size(self.height)
    }
}

/// The font size of a text `height` high (paper metres, its capitals' height), as Revit sizes
/// it: its typeface's capitals that high (Arial's 'H': 1466 of its 2048 units, 3.96 points a
/// millimetre), rounded to half a point (on Revit's PDF prints: 2.5 mm → 10 pt, 2.6 mm →
/// 10.5 pt, 5.2 mm → 20.5 pt, 7 mm → 27.5 pt; Arial Narrow alike).
pub fn font_size(height: f32) -> f32 {
    const POINT: f32 = 0.0254 / 72.0;
    let pt = text_em(height) / POINT;
    ((pt * 2.0).round() / 2.0).max(0.5) * POINT
}

/// The font size of a text `height` high before its rounding (see [`font_size`]): Revit cuts a
/// schedule cell's text measured at this size (a 7.84/96" text, 8.2 pt written at 8).
pub fn text_em(height: f32) -> f32 {
    height / (1466.0 / 2048.0)
}

/// How much of its width Revit counts when it centres or right-aligns a line of a text `height`
/// high: its width measured at its size before rounding, when that is smaller than the size it
/// is written at (a 2.853 mm text, 11.3 pt written at 11.5, centred 1.1 % of its width to the
/// right of its middle, P-101; an 8.2 pt one written at 8 centred on its middle, AC-701).
pub fn align_scale(height: f32) -> f32 {
    (text_em(height) / font_size(height)).min(1.0)
}

/// How much higher above its baseline (font sizes) and lower below it a bold or italic line of a
/// text reaches than a regular one, as Revit lays them out (measured on its PDF prints: bold
/// 0.024 and 0.019, italic 0.006 and 0.019).
fn face_extra(bold: bool, italic: bool) -> (f32, f32) {
    if bold {
        (0.024, 0.019)
    } else if italic {
        (0.006, 0.019)
    } else {
        (0.0, 0.0)
    }
}

/// The distance from the baseline of a line of a text to the next one's, `size` its font size
/// (paper metres), `prev` and `next` the two lines' (bold, italic) (any of their characters, or
/// their paragraph's end), as Revit spaces them: 1.125 font sizes between regular lines, more
/// below a bold or italic line and above one (1.15 between italic lines, 1.168 between bold
/// ones), on a grid of 1/384 inch.
pub fn line_pitch(size: f32, prev: (bool, bool), next: (bool, bool)) -> f32 {
    const UNIT: f32 = 0.0254 / 384.0;
    let k = 1.125 + face_extra(prev.0, prev.1).1 + face_extra(next.0, next.1).0;
    ((size * k / UNIT).round() * UNIT).max(UNIT)
}

/// How much higher above its baseline and lower below it (paper metres) a line of a text `size`
/// large reaches than a regular one when it is bold or italic: for the first line's top and the
/// last line's bottom.
pub fn line_extra(size: f32, bold: bool, italic: bool) -> (f32, f32) {
    let (a, d) = face_extra(bold, italic);
    (a * size, d * size)
}

/// How far past its box width a row of a wrapped text may run, its font size rounded as Revit
/// prints it (measured on Revit's sheet PDFs: rows fit up to 1.0178 of their box, break from
/// 1.0183; 1.01 got 11 of 1727 rows wrong, this 4).
pub const WRAP_SLACK: f32 = 1.018;

/// Where `line` (byte range `r` of `text`, one paragraph, no line break) breaks into rows when
/// it wraps at `width` (the units `measure` gives a byte range's width in), as Revit breaks it:
/// before a word that would run past it, and before a tab when the tab's stop (`tab` apart) and
/// the word after it would (the tab then starting the row, at its first stop). With
/// `break_words` (a text note's), a word also breaks after '-', '=' or ')' ("Pression =" /
/// "6 Bars", "AUTO-" / "ÉQUILIBRANTE") and a word wider than a row between characters; else it
/// stays whole. Rows end with their spaces.
pub fn break_line(text: &str, r: std::ops::Range<usize>, width: f32, tab: f32, break_words: bool, measure: &mut dyn FnMut(std::ops::Range<usize>) -> f32) -> Vec<std::ops::Range<usize>> {
    let b = text.as_bytes();
    let end = r.end.min(b.len());
    let mut rows = Vec::new();
    if width <= 0.0 || tab <= 0.0 || r.start >= end {
        rows.push(r);
        return rows;
    }
    let gap = |c: u8| c == b' ' || c == b'\t';
    let word_end = |i: usize| {
        let mut j = i;
        while j < end && !gap(b[j]) {
            j += 1;
            if break_words && j < end && matches!(b[j - 1], b'-' | b'=' | b')') && !gap(b[j]) {
                break;
            }
        }
        j
    };
    let space_end = |i: usize| (i..end).find(|&j| b[j] != b' ').unwrap_or(end);
    let (mut start, mut x, mut i) = (r.start, 0.0f32, r.start);
    while i < end {
        match b[i] {
            b'\t' => {
                let stop = ((x / tab).floor() + 1.0) * tab;
                // with the spaces and the word after it
                let (s1, w1) = (space_end(i + 1), word_end(space_end(i + 1)));
                let after = if s1 > i + 1 { measure(i + 1..s1) } else { 0.0 } + if w1 > s1 { measure(s1..w1) } else { 0.0 };
                if i > start && stop + after > width {
                    rows.push(start..i);
                    start = i;
                    x = tab;
                } else {
                    x = stop;
                }
                i += 1;
            }
            b' ' => {
                let j = space_end(i);
                x += measure(i..j);
                i = j;
            }
            _ => {
                let j = word_end(i);
                let w = measure(i..j);
                if i > start && x + w > width {
                    rows.push(start..i);
                    start = i;
                    x = 0.0;
                }
                if break_words && start == i && w > width {
                    // a word wider than a row: broken between characters, each piece filling a
                    // row, the rest going on (Revit's notes: "AUTOVIDANGEAB" / "LE EKSOL 50")
                    let mut s = i;
                    while measure(s..j) > width {
                        let mut e = s + text[s..j].chars().next().map_or(1, char::len_utf8);
                        for (o, _) in text[s..j].char_indices().skip(1) {
                            if measure(s..s + o) > width {
                                break;
                            }
                            e = s + o;
                        }
                        rows.push(s..e);
                        s = e;
                    }
                    start = s;
                    x = measure(s..j);
                } else {
                    x += w;
                }
                i = j;
            }
        }
    }
    rows.push(start..r.end);
    rows
}

/// Advance widths of Arial (Helvetica metrics, 1/1000 em) for ASCII 32..126, regular and bold.
const ARIAL: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];
const ARIAL_BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, 975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584, 556, 333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

/// Width of `s` in Arial at font size `fs` (same unit as `fs`), from its advance widths (accented
/// letters as their base letter, others as a digit).
pub fn arial_width(s: &str, fs: f32, bold: bool) -> f32 {
    let w = if bold { &ARIAL_BOLD } else { &ARIAL };
    s.chars()
        .map(|c| {
            // accented letters as their base letter
            let base = match c.to_ascii_lowercase() {
                'à' | 'á' | 'â' | 'ä' => 'a',
                'ç' => 'c',
                'è' | 'é' | 'ê' | 'ë' => 'e',
                'ì' | 'í' | 'î' | 'ï' => 'i',
                'ò' | 'ó' | 'ô' | 'ö' => 'o',
                'ù' | 'ú' | 'û' | 'ü' => 'u',
                _ => c,
            };
            let base = match c {
                'À' | 'Á' | 'Â' | 'Ä' => 'A',
                'Ç' => 'C',
                'È' | 'É' | 'Ê' | 'Ë' => 'E',
                'Ì' | 'Í' | 'Î' | 'Ï' => 'I',
                'Ò' | 'Ó' | 'Ô' | 'Ö' => 'O',
                'Ù' | 'Ú' | 'Û' | 'Ü' => 'U',
                _ => base,
            };
            let i = base as u32;
            if (32..127).contains(&i) {
                return w[(i - 32) as usize] as f32;
            }
            // (Arial's other common signs, regular; bold as regular but "°", "•")
            match base {
                '\u{a0}' => 278.0,
                '²' | '³' | '¹' => 333.0,
                '°' => 400.0,
                '±' | '×' | '÷' => 584.0,
                '·' => 278.0,
                '¼' | '½' | '¾' => 834.0,
                '‘' | '’' | '‚' => if bold { 278.0 } else { 222.0 },
                '“' | '”' | '„' => if bold { 500.0 } else { 333.0 },
                '•' => 350.0,
                '…' | '—' | '‰' => 1000.0,
                'Ø' => 778.0,
                'ø' => 611.0,
                'æ' => 889.0,
                'Æ' | 'Œ' => 1000.0,
                'œ' => 944.0,
                'ß' => 611.0,
                // (not in Arial: Revit's PDF writes them in Segoe UI Symbol, `fallback_glyph`)
                '∅' => 965.0,
                '⌀' => 845.0,
                _ => 556.0,
            }
        })
        .sum::<f32>()
        / 1000.0
        * fs
}

/// Whether Revit writes `c` in Segoe UI Symbol rather than Arial (its PDF prints it as a text
/// of its own): the empty set and diameter signs.
pub fn fallback_glyph(c: char) -> bool {
    matches!(c, '∅' | '⌀')
}

/// `text` as Revit shows it in a box `rows` lines high and `width` wide (Arial at font size
/// `size`, bold or not, all in the same unit): its lines wrapped at the box's width (a word
/// wider than it broken between its characters: "Embarquemen" / "t"), one per line; when they
/// do not all fit, its first `rows` rows, the last one cut to end with "..." within the width
/// ("Local EER 1 / PE...").
pub fn cell_lines(text: &str, size: f32, bold: bool, width: f32, rows: usize) -> String {
    let w = |s: &str| arial_width(s, size, bold);
    let mut lines: Vec<String> = Vec::new();
    // (its line breaks at its end left out: "isolation" / "thermique", not "thermiq...")
    for para in text.trim_end_matches(['\n', '\r']).split('\n').map(|p| p.trim_end_matches('\r')) {
        // each word with the spaces before it (all of them counted, as Revit counts them; those
        // where the line breaks dropped)
        let mut cur = String::new();
        let mut rest_para = para;
        while !rest_para.is_empty() {
            let after = rest_para.trim_start_matches(' ');
            let gap = &rest_para[..rest_para.len() - after.len()];
            let end = after.find(' ').unwrap_or(after.len());
            let word = &after[..end];
            rest_para = &after[end..];
            let joined = format!("{cur}{gap}{word}");
            if word.is_empty() || w(&joined) <= width {
                cur = joined;
                continue;
            }
            if !cur.trim_end().is_empty() {
                lines.push(cur.trim_end().to_string());
            }
            // a word wider than a line: as many of its characters as fit on each, its rest
            // starting the next one
            let mut rest = word;
            while w(rest) > width && rest.chars().count() > 1 {
                let mut n = rest.chars().next().map_or(0, char::len_utf8);
                for (i, c) in rest.char_indices().skip(1) {
                    if w(&rest[..i + c.len_utf8()]) > width {
                        break;
                    }
                    n = i + c.len_utf8();
                }
                lines.push(rest[..n].to_string());
                rest = &rest[n..];
            }
            cur = rest.to_string();
        }
        lines.push(cur.trim_end().to_string());
    }
    let rows = rows.max(1);
    if lines.len() > rows {
        lines.truncate(rows);
        let last = &mut lines[rows - 1];
        let mut kept = last.trim_end().to_string();
        while !kept.is_empty() && w(&format!("{kept}...")) > width {
            kept.pop();
        }
        *last = kept + "...";
    }
    lines.join("\n")
}

/// How many rows `text` takes in Arial at font size `size`, bold or not, its lines wrapping at
/// `width` (0: not wrapped; tab stops `tab` apart; all in the same unit), as `break_line` breaks
/// them.
pub fn wrapped_rows(text: &str, size: f32, bold: bool, width: f32, tab: f32) -> usize {
    let mut start = 0;
    let mut n = 0;
    for line in text.split('\n') {
        let r = start..start + line.len();
        start = r.end + 1;
        n += if width > 0.0 && !line.is_empty() { break_line(text, r, width, tab, false, &mut |q| arial_width(&text[q], size, bold)).len() } else { 1 };
    }
    n.max(1)
}

/// A part of a text written its own way: its length in bytes of the text, and its style
/// (`script`: 1 superscript, -1 subscript, written [`SCRIPT_SIZE`] as large).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TextSpan {
    pub len: usize,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub script: i8,
}

/// How large a superscript or subscript is written, against its line (Revit: 8 pt in 12).
pub const SCRIPT_SIZE: f32 = 2.0 / 3.0;

/// A raster image placed on a sheet (rendered views, imported images), `rect` in paper metres;
/// `turns`: quarter turns counter-clockwise of the image in it; `front`: drawn over the
/// sheet's lines and texts (a picture in the foreground), else under them.
#[derive(Debug, Clone)]
pub struct Image2d {
    pub rect: [f32; 4],
    pub image: std::sync::Arc<Rgba>,
    pub turns: u8,
    pub front: bool,
}

/// A view placed on a sheet: its frame (paper metres), the view element id and its name.
#[derive(Debug, Clone, Default)]
pub struct Viewport2d {
    pub rect: [f32; 4],
    pub view_id: i64,
    pub name: String,
}

/// What a browser row stands for (drives its icon).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserKind {
    Views,
    Legends,
    Schedules,
    Sheets,
    Folder,
    Plan,
    CeilingPlan,
    View3d,
    Section,
    Elevation,
    Detail,
    Drafting,
    Rendering,
    Legend,
    Schedule,
    Sheet,
    /// "Liens Revit" section, a linked model loaded at the last save, one that was not
    /// (unloaded or not found), and a placed instance of a link.
    Links,
    Link,
    /// "Familles" section (category folders use `Folder`), a loadable family, one of its types.
    Families,
    Family,
    FamilyType,
    /// "Groupes" section ("Détail" / "Modèle" use `Folder`), a group type.
    Groups,
    Group,
    LinkUnloaded,
    LinkInstance,
    Other,
}

/// One row of the document browser; `depth` 0 rows are the top-level sections.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserNode {
    pub label: String,
    pub depth: u16,
    pub kind: BrowserKind,
    /// Element id of the view or sheet the row stands for, -1 for folders and other rows.
    pub id: i64,
    /// The project's starting view (Revit "Vue de démarrage"): the file opens on it.
    pub start: bool,
}

impl Scene {
    /// Number of triangles actually drawn (instances counted).
    pub fn drawn_triangles(&self) -> usize {
        self.components
            .iter()
            .filter(|c| !c.suppressed)
            .filter_map(|c| c.mesh)
            .map(|m| self.meshes[m].mesh.triangle_count())
            .sum()
    }
    pub fn has_geometry(&self) -> bool {
        self.components.iter().any(|c| c.mesh.is_some_and(|m| !self.meshes[m].mesh.is_empty()))
    }
    /// Components that should have a mesh (parts) but none was found.
    pub fn missing_components(&self) -> usize {
        self.components.iter().filter(|c| !c.is_assembly && !c.suppressed && c.mesh.is_none()).count()
    }
    /// World-space bounding box of visible geometry.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        let mut any = false;
        for c in self.components.iter().filter(|c| !c.suppressed && !c.hidden) {
            let Some(mi) = c.mesh else { continue };
            let m = &self.meshes[mi].mesh;
            if m.is_empty() {
                continue;
            }
            let w = glam::Mat4::from_cols_array(&c.world);
            for k in 0..8 {
                let p = glam::Vec3::new(
                    if k & 1 == 0 { m.bbox_min[0] } else { m.bbox_max[0] },
                    if k & 2 == 0 { m.bbox_min[1] } else { m.bbox_max[1] },
                    if k & 4 == 0 { m.bbox_min[2] } else { m.bbox_max[2] },
                );
                let q = w.transform_point3(p);
                for i in 0..3 {
                    lo[i] = lo[i].min(q[i]);
                    hi[i] = hi[i].max(q[i]);
                }
                any = true;
            }
        }
        any.then_some((lo, hi))
    }
}

#[derive(Debug, Clone)]
pub struct LoadOptions {
    /// Compute B-rep edge lines.
    pub edges: bool,
    /// For assemblies: load component meshes from sibling files when not cached.
    pub resolve_external: bool,
    /// Decode the thumbnail.
    pub preview: bool,
    /// Progress reporting and cancellation.
    pub ctl: Ctl,
}

impl Default for LoadOptions {
    fn default() -> Self {
        LoadOptions { edges: true, resolve_external: true, preview: true, ctl: Ctl::default() }
    }
}

fn identity() -> [f32; 16] {
    glam::Mat4::IDENTITY.to_cols_array()
}

fn stem_of(path: &str) -> String {
    let f = path.rsplit(['\\', '/']).next().unwrap_or(path);
    match f.rfind('.') {
        Some(i) => f[..i].to_string(),
        None => f.to_string(),
    }
}

/// Locate a referenced component file on disk (all comparisons case-insensitive):
/// 1. the stored absolute path;
/// 2. the stored path's trailing folders re-rooted at the assembly folder and its ancestors
///    (a project tree that was moved or copied to another drive);
/// 3. the assembly's folder and its parent, by file name;
/// 4. the whole folder tree around the assembly (up to two levels above it, bounded walk),
///    preferring the candidate whose folders best match the stored path.
pub fn resolve_component_path(asm_dir: &Path, stored: &str, cache: &mut HashMap<PathBuf, Vec<PathBuf>>) -> Option<PathBuf> {
    let p = PathBuf::from(stored);
    if (cfg!(windows) || !stored.contains('\\')) && p.is_file() {
        return Some(p);
    }
    let parts: Vec<&str> = stored.split(['\\', '/']).filter(|s| !s.is_empty() && !s.ends_with(':')).collect();
    let fname = parts.last()?.to_lowercase();
    let ancestors: Vec<PathBuf> = asm_dir.ancestors().take(4).map(Path::to_path_buf).collect();
    // 2. re-rooted tails, longest first (only folders inside the tail are tried, not the drive)
    for k in (2..=parts.len().min(5)).rev() {
        let tail = &parts[parts.len() - k..];
        for root in &ancestors {
            if let Some(f) = walk_ci(root, tail, cache) {
                return Some(f);
            }
        }
    }
    // 3. same folder, parent
    for d in ancestors.iter().take(2) {
        if let Some(f) = dir_listing(d, cache).iter().find(|q| name_lc(q) == fname) {
            return Some(f.clone());
        }
    }
    // 4. the tree under the grandparent (or the highest ancestor available)
    let root = ancestors.get(2).or(ancestors.last())?.clone();
    let key = root.join("\0tree");
    let index = cache.entry(key).or_insert_with(|| tree_index(&root));
    let stored_dirs: Vec<String> = parts[..parts.len() - 1].iter().map(|s| s.to_lowercase()).collect();
    index
        .iter()
        .filter(|q| name_lc(q) == fname)
        .max_by_key(|q| {
            // trailing folder names in common with the stored path, then shallower paths
            let dirs: Vec<String> = q.parent().map(|d| d.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect()).unwrap_or_default();
            let common = dirs.iter().rev().zip(stored_dirs.iter().rev()).take_while(|(a, b)| a == b).count();
            (common, usize::MAX - dirs.len())
        })
        .cloned()
}

fn name_lc(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Entries (files and folders) of one folder, cached.
fn dir_listing<'a>(d: &Path, cache: &'a mut HashMap<PathBuf, Vec<PathBuf>>) -> &'a Vec<PathBuf> {
    cache.entry(d.to_path_buf()).or_insert_with(|| std::fs::read_dir(d).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect()).unwrap_or_default())
}

/// Follow `tail` (folder names then the file name) from `root`, case-insensitively.
fn walk_ci(root: &Path, tail: &[&str], cache: &mut HashMap<PathBuf, Vec<PathBuf>>) -> Option<PathBuf> {
    let mut cur = root.to_path_buf();
    for (i, seg) in tail.iter().enumerate() {
        let lc = seg.to_lowercase();
        let last = i + 1 == tail.len();
        let next = dir_listing(&cur, cache).iter().find(|q| name_lc(q) == lc && if last { q.is_file() } else { q.is_dir() })?.clone();
        cur = next;
    }
    Some(cur)
}

/// SolidWorks / 3D files below `root` (bounded: depth 6, 200 000 entries).
fn tree_index(root: &Path) -> Vec<PathBuf> {
    const EXTS: &[&str] = &["sldprt", "sldasm", "prt", "asm", "step", "stp", "igs", "iges", "x_t", "x_b"];
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            seen += 1;
            if seen > 200_000 {
                return out;
            }
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                if depth < 6 {
                    stack.push((p, depth + 1));
                }
            } else if p.extension().is_some_and(|x| EXTS.contains(&x.to_string_lossy().to_lowercase().as_str())) {
                out.push(p);
            }
        }
    }
    out
}

/// Load just the faces of a file's display list (used for external components).
fn load_faces(path: &Path) -> Result<Vec<dl::FacePatch>> {
    let c = Container::open(path)?;
    let Some(d) = read_display_lists(&c)? else { return Ok(Vec::new()) };
    let list = dl::decode(&d);
    // For a part file, take all faces.
    Ok(list.blocks.into_iter().flat_map(|b| b.faces).collect())
}

/// Above this decompressed size, the DisplayLists stream is decoded straight from the
/// decompressor (two passes) instead of being held in memory.
pub const STREAM_THRESHOLD: u64 = 128 << 20;

/// Largest number of display vertices kept for one file before the mesh is simplified
/// (vertex clustering). `MODELCRAFT_MAX_VERTICES` overrides it.
pub fn vertex_budget() -> usize {
    if let Some(v) = std::env::var("MODELCRAFT_MAX_VERTICES").ok().and_then(|v| v.trim().parse::<usize>().ok()) {
        return v.max(10_000);
    }
    // ~60 bytes per kept vertex on the CPU side (positions, normals, indices, ...) and as much
    // again transiently for the GPU upload: stay well inside the available memory.
    let by_mem = ctl::mem_available().map(|a| (a / 200) as usize).unwrap_or(usize::MAX);
    by_mem.clamp(2_000_000, 40_000_000)
}

/// Meshes decoded from a DisplayLists stream, one per document block (in order of first
/// appearance) or a single merged one.
struct DlMeshes {
    blocks: Vec<(Option<String>, mesh::TriMesh)>,
    stats: DlStats,
}

/// Decode a DisplayLists stream into meshes with bounded memory: a first pass only counts
/// (faces are dropped as soon as they are measured), the second builds exactly sized meshes,
/// freeing each face right after it is copied. Large streams are never held decompressed:
/// both passes decode straight from the decompressor. If the tessellation is larger than the
/// vertex budget (or the available memory), it is simplified by vertex clustering.
fn build_dl_meshes(c: &Container, info: &container::StreamInfo, merge: bool, opts: &LoadOptions, warnings: &mut Vec<String>) -> Result<DlMeshes> {
    let ctl = &opts.ctl;
    let est = info.raw_size().unwrap_or(info.stored_size.saturating_mul(4));
    let streamed = est > STREAM_THRESHOLD || std::env::var_os("MODELCRAFT_STREAM").is_some();
    let mut stats = DlStats { stored_bytes: info.stored_size, streamed, budget: vertex_budget(), ..Default::default() };
    let whole = if streamed { None } else { Some(c.read_info_ctl(info, ctl, (0.03, 0.35))?) };
    if let Some(d) = &whole {
        stats.raw_bytes = d.len() as u64;
    }
    let (r1, r2) = if streamed { ((0.03, 0.45), (0.45, 0.9)) } else { ((0.35, 0.5), (0.5, 0.9)) };

    // One decoding pass over the stream.
    let run_pass = |range: (f32, f32), stage: &str, visit: &mut dyn FnMut(Option<&str>, dl::FacePatch) -> Result<()>| -> Result<u64> {
        match &whole {
            Some(d) => {
                let len = d.len().max(1);
                let mut prog = |pos: usize| -> Result<()> {
                    ctl.report(range.0 + (range.1 - range.0) * (pos as f32 / len as f32), stage);
                    ctl.check()
                };
                dl::Decoder::default().run(d, 0, true, visit, &mut prog)?;
                Ok(d.len() as u64)
            }
            None => {
                let mut sd = dl::StreamDecoder::new(ctl, visit);
                let n = c.stream_info(info, ctl, range, stage, &mut |chunk| sd.feed(chunk))?;
                sd.finish()?;
                Ok(n)
            }
        }
    };

    // Pass 1: statistics per block.
    let t = Instant::now();
    let mut keys: Vec<Option<String>> = Vec::new();
    let mut paths: Vec<Option<String>> = Vec::new();
    let mut fstats: Vec<mesh::FaceStats> = Vec::new();
    {
        let mut visit = |path: Option<&str>, face: dl::FacePatch| -> Result<()> {
            let key = if merge { None } else { path.map(dl::file_key) };
            let k = match keys.iter().position(|x| *x == key) {
                Some(k) => k,
                None => {
                    keys.push(key);
                    paths.push(if merge { None } else { path.map(str::to_string) });
                    fstats.push(mesh::FaceStats::default());
                    keys.len() - 1
                }
            };
            fstats[k].add(&face);
            Ok(())
        };
        let n = run_pass(r1, "Analyse du maillage (1/2)…", &mut visit)?;
        stats.raw_bytes = n;
    }
    stats.pass1 = t.elapsed();
    stats.blocks = keys.len();
    stats.faces = fstats.iter().map(|s| s.faces).sum();
    stats.src_vertices = fstats.iter().map(|s| s.vertices).sum();
    stats.src_triangles = fstats.iter().map(|s| s.triangles).sum();
    if stats.faces == 0 {
        return Ok(DlMeshes { blocks: Vec::new(), stats });
    }

    // Decide: exact meshes, or simplified to fit the budget / the memory.
    let mut budget = stats.budget;
    let bytes_per_vertex = 24.0 + 12.0 * stats.src_triangles as f64 / stats.src_vertices.max(1) as f64 + 4.0;
    if let Some(avail) = ctl::mem_available() {
        // keep the meshes under ~45 % of the available memory (GPU upload copies come on top)
        let fit = (avail as f64 * 0.45 / bytes_per_vertex) as usize;
        budget = budget.min(fit.max(500_000));
    }
    let simplify = stats.src_vertices > budget;
    stats.simplified = simplify;
    let edge_limit_hit = opts.edges && !simplify && fstats.iter().any(|s| s.vertices > mesh::EDGE_VERTEX_LIMIT);
    if edge_limit_hit {
        warnings.push(format!("Maillage très dense ({} M sommets) : arêtes non calculées", stats.src_vertices / 1_000_000));
    }

    // Pass 2: build.
    let t = Instant::now();
    let mut builders: Vec<mesh::MeshBuilder> = Vec::with_capacity(keys.len());
    for s in &fstats {
        let cluster = simplify.then(|| {
            let target = ((budget as f64) * s.vertices as f64 / stats.src_vertices as f64).max(1000.0) as usize;
            (s.bbox_min, mesh::cluster_cell(s.area, target))
        });
        builders.push(mesh::MeshBuilder::new(s.vertices, s.faces, opts.edges, cluster)?);
    }
    {
        let mut visit = |path: Option<&str>, face: dl::FacePatch| -> Result<()> {
            let key = if merge { None } else { path.map(dl::file_key) };
            if let Some(k) = keys.iter().position(|x| *x == key) {
                builders[k].add_face(&face)?;
            }
            Ok(())
        };
        run_pass(r2, "Construction du maillage (2/2)…", &mut visit)?;
    }
    drop(whole);
    let blocks: Vec<(Option<String>, mesh::TriMesh)> = paths.into_iter().zip(builders.into_iter().map(|b| b.finish())).collect();
    stats.pass2 = t.elapsed();
    stats.out_vertices = blocks.iter().map(|(_, m)| m.positions.len()).sum();
    stats.out_triangles = blocks.iter().map(|(_, m)| m.triangle_count()).sum();
    if simplify {
        warnings.push(format!(
            "Maillage simplifié pour l'affichage : {:.1} M → {:.1} M sommets (le fichier n'est pas modifié)",
            stats.src_vertices as f64 / 1e6,
            stats.out_vertices as f64 / 1e6
        ));
    }
    Ok(DlMeshes { blocks, stats })
}

/// Mesh bodies stored in `MeshData/Config-N-Mesh-*` streams (graphics/mesh bodies, often the
/// only geometry of big imported scans or STL conversions). Only the first configuration
/// that has mesh streams is read.
fn build_meshdata_mesh(c: &Container, opts: &LoadOptions, warnings: &mut Vec<String>) -> Result<Option<mesh::TriMesh>> {
    let ctl = &opts.ctl;
    let mut streams: Vec<&container::StreamInfo> = c.streams().iter().filter(|s| s.name.starts_with("MeshData/Config-")).collect();
    if streams.is_empty() {
        return Ok(None);
    }
    let config = |n: &str| n["MeshData/Config-".len()..].split('-').next().and_then(|x| x.parse::<u32>().ok()).unwrap_or(u32::MAX);
    let first = streams.iter().map(|s| config(&s.name)).min().unwrap_or(0);
    streams.retain(|s| config(&s.name) == first);
    let total: u64 = streams.iter().map(|s| s.stored_size).sum::<u64>().max(1);
    let mut done = 0u64;
    let mut dec = meshdata::Decoder::new();
    for s in &streams {
        let a = 0.03 + 0.62 * done as f32 / total as f32;
        done += s.stored_size;
        let b = 0.03 + 0.62 * done as f32 / total as f32;
        c.stream_info(s, ctl, (a, b), "Lecture des corps maillés…", &mut |chunk| dec.feed(chunk))?;
    }
    let bodies = dec.finish()?;
    if bodies.triangles.is_empty() {
        return Ok(None);
    }
    if bodies.bad > 0 {
        warnings.push(format!("{} facette(s) de corps maillé ignorée(s) (indices invalides)", bodies.bad));
    }
    bodies_to_mesh(&bodies, opts, warnings).map(Some)
}

/// Weld, smooth and split a [`meshdata::MeshBodies`] soup into a display mesh, body by body.
fn bodies_to_mesh(bodies: &meshdata::MeshBodies, opts: &LoadOptions, warnings: &mut Vec<String>) -> Result<mesh::TriMesh> {
    let ctl = &opts.ctl;
    let edges = opts.edges && bodies.positions.len() <= mesh::EDGE_VERTEX_LIMIT;
    if opts.edges && !edges {
        warnings.push(format!("Maillage très dense ({} M sommets) : arêtes non calculées", bodies.positions.len() / 1_000_000));
    }
    // Build body by body (bounded hash maps), smoothing normals inside each body.
    let mut m = mesh::TriMesh::new_empty();
    let nb = bodies.body_start.len();
    for i in 0..nb {
        ctl.check()?;
        ctl.report(0.65 + 0.3 * i as f32 / nb.max(1) as f32, "Construction du maillage…");
        let t0 = bodies.body_start[i] as usize;
        let t1 = bodies.body_start.get(i + 1).map(|&x| x as usize).unwrap_or(bodies.triangles.len());
        let tris = &bodies.triangles[t0..t1];
        if tris.is_empty() {
            continue;
        }
        let lo = tris.iter().flat_map(|t| t.iter()).copied().min().unwrap_or(0);
        let hi = tris.iter().flat_map(|t| t.iter()).copied().max().unwrap_or(0);
        let local: Vec<[u32; 3]> = tris.iter().map(|t| [t[0] - lo, t[1] - lo, t[2] - lo]).collect();
        let body = mesh::build_from_triangles(&bodies.positions[lo as usize..=hi as usize], &local, 35.0, edges);
        m.append(body);
    }
    m.finish();
    Ok(m)
}

/// Graphics bodies kept as triangle strips in `Contents/Config-N-ResolvedFeatures` (older
/// files with an imported STL as a graphics body, see [`gfxstrips`]).
fn build_feature_strips_mesh(c: &Container, opts: &LoadOptions, warnings: &mut Vec<String>) -> Result<Option<mesh::TriMesh>> {
    let ctl = &opts.ctl;
    let mut streams: Vec<&container::StreamInfo> =
        c.streams().iter().filter(|s| s.name.starts_with("Contents/Config-") && s.name.ends_with("-ResolvedFeatures") && s.stored_size >= 4096).collect();
    if streams.is_empty() {
        return Ok(None);
    }
    let config = |n: &str| n["Contents/Config-".len()..].split('-').next().and_then(|x| x.parse::<u32>().ok()).unwrap_or(u32::MAX);
    streams.sort_by_key(|s| config(&s.name));
    for s in streams {
        let d = c.read_info_ctl(s, ctl, (0.03, 0.4))?;
        ctl.check()?;
        ctl.report(0.45, "Lecture des corps graphiques…");
        let bodies = gfxstrips::decode(&d);
        drop(d);
        if bodies.triangles.is_empty() {
            continue;
        }
        return bodies_to_mesh(&bodies, opts, warnings).map(Some);
    }
    Ok(None)
}

/// ScanTo3D point cloud (`ThirdPty/SWNGREStream`) shown as splats.
fn build_point_cloud(c: &Container, opts: &LoadOptions, warnings: &mut Vec<String>) -> Result<Option<mesh::TriMesh>> {
    let Some(info) = c.find(pointcloud::STREAM) else { return Ok(None) };
    let ctl = &opts.ctl;
    let d = c.read_info_ctl(info, ctl, (0.05, 0.6))?;
    ctl.check()?;
    ctl.report(0.65, "Nuage de points…");
    let Some(cloud) = pointcloud::decode(&d) else { return Ok(None) };
    drop(d);
    // 9 vertices per splat; also keep the GPU load reasonable
    let max_points = (vertex_budget() / 9).min(2_000_000);
    let m = pointcloud::splats(&cloud, max_points);
    let shown = m.triangle_count() / 3;
    if shown < cloud.points.len() {
        warnings.push(format!("Nuage de points : {} points, {} affichés (sous-échantillonné)", cloud.points.len(), shown));
    } else {
        warnings.push(format!("Nuage de points : {} points", cloud.points.len()));
    }
    if cloud.declared > cloud.points.len() {
        warnings.push(format!("Nuage de points incomplet : {} points annoncés", cloud.declared));
    }
    Ok(Some(m))
}

/// Open a file and build a renderable scene.
pub fn load_scene(path: &Path, opts: &LoadOptions) -> Result<Scene> {
    let ctl = &opts.ctl;
    let t0 = Instant::now();
    let mut timings = Timings::default();
    ctl.report(0.02, "Ouverture du conteneur…");
    let c = Container::open_ctl(path, ctl)?;
    timings.open = t0.elapsed();
    ctl.check()?;

    let ext_asm = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("sldasm"));
    let t1 = Instant::now();
    let mut warnings = Vec::new();
    let tree_xml = c.read(COMP_TREE).ok().flatten();
    let kind = if ext_asm || (tree_xml.is_some() && !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("sldprt"))) {
        DocKind::Assembly
    } else {
        DocKind::Part
    };
    let dl_info = c.find(DISPLAY_LISTS);
    let had_dl = dl_info.is_some();
    let built = match dl_info {
        None => None,
        Some(info) => match build_dl_meshes(&c, info, kind == DocKind::Part, opts, &mut warnings) {
            Ok(b) => Some(b),
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) => {
                warnings.push(format!("Maillage illisible ({e}) : seule la vignette est affichée"));
                None
            }
        },
    };
    if let Some(b) = &built {
        timings.read = t1.elapsed().saturating_sub(b.stats.pass1 + b.stats.pass2);
        timings.decode = b.stats.pass1;
        timings.dl = Some(b.stats.clone());
    }
    let mut blocks = built.map(|b| b.blocks).unwrap_or_default();
    let mut from_meshdata = false;
    if blocks.is_empty() && kind == DocKind::Part {
        let t = Instant::now();
        match build_meshdata_mesh(&c, opts, &mut warnings) {
            Ok(Some(m)) => {
                blocks.push((None, m));
                from_meshdata = true;
                timings.decode += t.elapsed();
            }
            Ok(None) => {}
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) => warnings.push(format!("Corps maillés illisibles ({e})")),
        }
    }
    // Graphics bodies (imported STL...) are kept in ResolvedFeatures even when DisplayLists
    // holds the B-rep faces of the same part (e.g. 9 k B-rep triangles + 4 M graphics ones):
    // add them when the feature data is big and holds more triangles than the display lists.
    let dl_tris: usize = blocks.iter().map(|(_, m)| m.triangle_count()).sum();
    let big_features = c.streams().iter().any(|s| s.name.starts_with("Contents/Config-") && s.name.ends_with("-ResolvedFeatures") && s.stored_size >= 16 << 20);
    if kind == DocKind::Part && (blocks.is_empty() || big_features) {
        let t = Instant::now();
        match build_feature_strips_mesh(&c, opts, &mut warnings) {
            Ok(Some(m)) if blocks.is_empty() => {
                blocks.push((None, m));
                from_meshdata = true;
                timings.decode += t.elapsed();
            }
            Ok(Some(m)) if m.triangle_count() > dl_tris => {
                warnings.push(format!("Corps graphiques ajoutés : {} triangles (en plus des {} des DisplayLists)", m.triangle_count(), dl_tris));
                blocks[0].1.append(m);
                timings.decode += t.elapsed();
            }
            Ok(Some(_)) => {}
            Ok(None) => {}
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) => warnings.push(format!("Corps graphiques illisibles ({e})")),
        }
    }
    let mut from_cloud = false;
    if blocks.is_empty() && kind == DocKind::Part {
        let t = Instant::now();
        match build_point_cloud(&c, opts, &mut warnings) {
            Ok(Some(m)) => {
                blocks.push((None, m));
                from_cloud = true;
                timings.decode += t.elapsed();
            }
            Ok(None) => {}
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) => warnings.push(format!("Nuage de points illisible ({e})")),
        }
    }
    if from_meshdata || from_cloud {
        // geometry found in MeshData / point cloud: no "no mesh" warning
    } else if !had_dl && warnings.is_empty() {
        warnings.push("Pas de flux DisplayLists (fichier enregistré sans données graphiques)".into());
    } else if had_dl && blocks.is_empty() && warnings.is_empty() {
        warnings.push("DisplayLists présent mais aucun maillage décodé".into());
    }

    let t3 = Instant::now();
    let mut meshes: Vec<SceneMesh> = Vec::new();
    let mut components: Vec<SceneComponent> = Vec::new();
    let file_stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut ext_time = Duration::ZERO;

    match kind {
        DocKind::Model => {}
        DocKind::Part => {
            if let Some((_, m)) = blocks.into_iter().next() {
                meshes.push(SceneMesh { name: file_stem.clone(), mesh: m, source: MeshSource::Embedded, color: None });
            }
            components.push(SceneComponent {
                name: file_stem.clone(),
                path: path.to_string_lossy().into_owned(),
                depth: 0,
                parent: None,
                is_assembly: false,
                world: identity(),
                mesh: (!meshes.is_empty()).then_some(0),
                suppressed: false,
                hidden: false,
                color: None,
            });
        }
        DocKind::Assembly => {
            // Index cached meshes by component file name.
            let mut by_key: HashMap<String, usize> = HashMap::new();
            let mut unkeyed: Vec<usize> = Vec::new();
            for (bpath, m) in blocks {
                let name = bpath.as_deref().map(stem_of).unwrap_or_else(|| file_stem.clone());
                meshes.push(SceneMesh { name, mesh: m, source: MeshSource::Embedded, color: None });
                match bpath.as_deref().map(dl::file_key) {
                    Some(k) => {
                        by_key.insert(k, meshes.len() - 1);
                    }
                    None => unkeyed.push(meshes.len() - 1),
                }
            }
            let tree = match tree_xml.as_deref().map(assembly::parse) {
                Some(Ok(t)) => Some(t),
                Some(Err(e)) => {
                    warnings.push(e);
                    None
                }
                None => None,
            };
            let flat = tree.map(|t| t.flatten()).unwrap_or_default();
            let mut used = vec![false; meshes.len()];
            let asm_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            let mut dir_cache = HashMap::new();
            let mut ext_cache: HashMap<String, Option<usize>> = HashMap::new();
            if opts.resolve_external {
                ctl.report(0.9, "Composants externes…");
            }
            for comp in &flat {
                ctl.check()?;
                let mut mesh_idx = None;
                if !comp.is_assembly {
                    let key = dl::file_key(&comp.path);
                    mesh_idx = by_key.get(&key).copied();
                    if mesh_idx.is_none() && opts.resolve_external && !comp.suppressed && !key.is_empty() {
                        mesh_idx = *ext_cache.entry(key.clone()).or_insert_with(|| {
                            let te = Instant::now();
                            let r = resolve_component_path(&asm_dir, &comp.path, &mut dir_cache).and_then(|p| {
                                let faces = load_faces(&p).ok()?;
                                if faces.is_empty() {
                                    return None;
                                }
                                let m = mesh::try_build(&faces, opts.edges, ctl).ok()?;
                                meshes.push(SceneMesh {
                                    name: stem_of(&comp.path),
                                    mesh: m,
                                    source: MeshSource::External(p),
                                    color: None,
                                });
                                used.push(true);
                                Some(meshes.len() - 1)
                            });
                            ext_time += te.elapsed();
                            r
                        });
                    }
                    if let Some(i) = mesh_idx {
                        used[i] = true;
                    }
                }
                components.push(SceneComponent {
                    name: comp.name.clone(),
                    path: comp.path.clone(),
                    depth: comp.depth,
                    parent: comp.parent,
                    is_assembly: comp.is_assembly,
                    world: comp.world.as_mat4().to_cols_array(),
                    mesh: mesh_idx,
                    suppressed: comp.suppressed,
                    hidden: comp.hidden,
                    color: None,
                });
            }
            // Cached meshes not matched by any component: show them untransformed.
            for (i, u) in used.iter().enumerate() {
                if !*u {
                    if !flat.is_empty() && !unkeyed.contains(&i) {
                        warnings.push(format!("Maillage « {} » sans composant correspondant (affiché sans transformation)", meshes[i].name));
                    }
                    components.push(SceneComponent {
                        name: meshes[i].name.clone(),
                        path: String::new(),
                        depth: 0,
                        parent: None,
                        is_assembly: false,
                        world: identity(),
                        mesh: Some(i),
                        suppressed: false,
                        hidden: false,
                        color: None,
                    });
                }
            }
            let missing = components.iter().filter(|c| !c.is_assembly && !c.suppressed && c.mesh.is_none()).count();
            if missing > 0 {
                warnings.push(format!("{missing} composant(s) sans maillage (ni en cache, ni trouvé à côté de l'assemblage)"));
            }
        }
    }
    timings.external = ext_time;
    timings.mesh = t3.elapsed().saturating_sub(ext_time);

    ctl.check()?;
    ctl.report(0.95, "Vignette…");
    let preview = if opts.preview { read_preview(&c) } else { None };
    timings.total = t0.elapsed();
    Ok(Scene {
        path: path.to_path_buf(),
        kind,
        container: c.kind(),
        stream_count: c.streams().len(),
        meshes,
        components,
        preview,
        warnings,
        properties: Vec::new(),
        timings,
        up_z: false,
        browser: Vec::new(),
        sheets: Vec::new(),
        view_frames: Vec::new(),
        edge_colors: Default::default(),
    })
}

#[cfg(test)]
mod resolve_tests {
    #[test]
    fn component_search_case_insensitive_tree() {
        let root = std::env::temp_dir().join(format!("mc_res_{}", std::process::id()));
        let asm = root.join("Projet").join("Assemblages");
        let sub = root.join("Projet").join("Pièces").join("Visserie");
        std::fs::create_dir_all(&asm).unwrap();
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("VIS M6.SLDPRT"), b"x").unwrap();
        let want = sub.join("VIS M6.SLDPRT");
        // moved project tree: the stored folders re-rooted next to the assembly
        let mut c = std::collections::HashMap::new();
        let r = super::resolve_component_path(&asm, r"D:\Old\Projet\pièces\visserie\vis m6.sldprt", &mut c);
        assert_eq!(r.as_deref(), Some(want.as_path()));
        // unrelated stored folders: found by the bounded tree walk
        let mut c = std::collections::HashMap::new();
        let r = super::resolve_component_path(&asm, r"C:\X\Y\Vis M6.sldprt", &mut c);
        assert_eq!(r.as_deref(), Some(want.as_path()));
        // absent file
        let mut c = std::collections::HashMap::new();
        assert!(super::resolve_component_path(&asm, r"C:\X\nope.sldprt", &mut c).is_none());
        std::fs::remove_dir_all(&root).ok();
    }
}

#[cfg(test)]
mod text_tests {
    const MM: f32 = 0.001;
    const UNIT: f32 = 0.0254 / 384.0;

    #[test]
    fn font_sizes_round_to_half_points() {
        let pt = |h_mm: f32| super::font_size(h_mm * MM) / (0.0254 / 72.0);
        // as Revit prints them
        for (h, size) in [(2.5, 10.0), (2.6, 10.5), (3.0, 12.0), (5.2, 20.5), (6.0, 24.0), (7.0, 27.5)] {
            assert!((pt(h) - size).abs() < 1e-3, "{h} mm: {} pt", pt(h));
        }
    }

    #[test]
    fn line_pitches_on_the_384th_inch() {
        let em = super::font_size(3.0 * MM);
        let u = |p: f32| (p / UNIT).round() as i32;
        let (r, b, i) = ((false, false), (true, false), (false, true));
        // measured on Revit's prints of 12 pt notes: 72, 75 and 74 units
        assert_eq!(u(super::line_pitch(em, r, r)), 72);
        assert_eq!(u(super::line_pitch(em, b, b)), 75);
        assert_eq!(u(super::line_pitch(em, i, i)), 74);
        assert_eq!(u(super::line_pitch(em, b, i)), 74);
    }

    #[test]
    fn a_word_wider_than_a_row_is_broken() {
        // one unit a character, rows 13 wide (P-403: "AUTOVIDANGEAB" / "LE EKSOL 50")
        let text = "STATION SOLAIRE AUTOVIDANGEABLE EKSOL 50";
        let rows = super::break_line(text, 0..text.len(), 13.0, 4.0, true, &mut |r| r.len() as f32);
        let got: Vec<&str> = rows.iter().map(|r| text[r.clone()].trim_end()).collect();
        assert_eq!(got, ["STATION", "SOLAIRE", "AUTOVIDANGEAB", "LE EKSOL 50"]);
        let text = "CANIVEAU 01";
        let rows = super::break_line(text, 0..text.len(), 5.0, 4.0, true, &mut |r| r.len() as f32);
        assert_eq!(rows.iter().map(|r| text[r.clone()].trim_end()).collect::<Vec<_>>(), ["CANIV", "EAU", "01"]);
        // a label's word runs past its row
        let rows = super::break_line(text, 0..text.len(), 5.0, 4.0, false, &mut |r| r.len() as f32);
        assert_eq!(rows.iter().map(|r| text[r.clone()].trim_end()).collect::<Vec<_>>(), ["CANIVEAU", "01"]);
    }

    #[test]
    fn a_note_breaks_a_word_after_a_dash_equals_or_parenthesis() {
        // one unit a character (P-606/607, AC-607 on Revit's prints)
        let rows = |text: &str, w: f32, notes: bool| {
            let r = super::break_line(text, 0..text.len(), w, 4.0, notes, &mut |r| r.len() as f32);
            r.iter().map(|r| text[r.clone()].trim_end().to_string()).collect::<Vec<_>>()
        };
        assert_eq!(rows("Pression =6 Bars", 10.0, true), ["Pression =", "6 Bars"]);
        assert_eq!(rows("( P-611 )Pression =7.4 Bars", 10.0, true), ["( P-611 )", "Pression =", "7.4 Bars"]);
        assert_eq!(rows("AUTO-EQUILIBRANTE TYPE", 12.0, true), ["AUTO-", "EQUILIBRANTE", "TYPE"]);
        // a label keeps "AC-401" whole
        assert_eq!(rows("VOIR AC-401", 5.0, false), ["VOIR", "AC-401"]);
    }

    #[test]
    fn a_tab_that_does_not_fit_starts_the_row() {
        // one unit a character, tab stops 4 apart, rows 11 wide
        let text = "1.\tAAAA BB\tCCCC";
        let rows = super::break_line(text, 0..text.len(), 11.0, 4.0, false, &mut |r| r.len() as f32);
        let got: Vec<&str> = rows.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(got, ["1.\tAAAA BB", "\tCCCC"]);
        assert_eq!(super::wrapped_rows("AAAA BBBB\nCC", 1.0, false, 0.0, 1.0), 2);
    }

    #[test]
    fn cut_cells_end_with_dots() {
        // Revit's EMS schedules (8.2 and 7.2 pt laid out, in cells 27.519 and 25.138 mm wide
        // less 1/32")
        let em = |pt: f32| pt * 25.4 / 72.0;
        let cut = |t: &str, pt: f32, w: f32| super::cell_lines(t, em(pt), false, w, 1);
        assert_eq!(cut("Local EER 1 / PER 3", 8.218, 27.519 - 0.794), "Local EER 1 / PE...");
        assert_eq!(cut("Synthese Défaut DCR2", 7.19, 25.138 - 0.794), "Synthese Défaut...");
        assert_eq!(cut("DCR2", 7.19, 25.138 - 0.794), "DCR2");
        // wrapped narrower than at the printed size; a word wider than its cell broken between
        // its characters (AC-701: 19.846 mm cells)
        let lines = |t: &str| super::cell_lines(t, em(8.218), false, 19.846 - 0.794, 3);
        assert_eq!(lines("RIDEAU D'AIR RA 05"), "RIDEAU\nD'AIR RA 05");
        assert_eq!(lines("CTA 02 Embarquement /"), "CTA 02\nEmbarquemen\nt /");
        // spaces kept as written: a run of them pushes the next word down (P-101)
        assert_eq!(lines("RA 05          FIN"), "RA 05\nFIN");
        assert_eq!(lines("RA 05 FIN\r\nX"), "RA 05 FIN\nX");
    }
}
