//! Probe (local only): the 2D content read for the sheets of a Revit file, written as SVG
//! (paper millimetres, drawn the way the viewer draws a sheet up close).
//! Usage: rvtsheet <file.rvt> <out dir> [sheet id or part of its name]
use sw_format::arial_width as text_width;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let t = std::time::Instant::now();
    let s = craft_formats::rvt::load(std::path::Path::new(&a[0]), &Default::default()).unwrap();
    eprintln!("loaded in {:.1}s, {} sheets", t.elapsed().as_secs_f64(), s.sheets.len());
    if std::env::var_os("FRAMES").is_some() {
        for f in &s.view_frames {
            let name = s.browser.iter().find(|r| r.id == f.id).map(|r| format!("{:?} {:?}", r.kind, r.label)).unwrap_or_default();
            eprintln!("frame {} {name}: dir {:?} up {:?} region {:?} clip {:?}", f.id, f.dir, f.up, f.region, f.clip.map(|c| (c.lo, c.hi)));
        }
    }
    if std::env::var_os("MISSING").is_some() {
        // browser rows of 2D views with no drawing
        let have: std::collections::HashSet<i64> = s.sheets.iter().map(|sh| sh.id).collect();
        for r in s.browser.iter().filter(|r| r.id > 0 && !have.contains(&r.id) && matches!(r.kind, sw_format::BrowserKind::Sheet | sw_format::BrowserKind::Schedule | sw_format::BrowserKind::Drafting | sw_format::BrowserKind::Legend)) {
            eprintln!("no drawing: {} {:?} {:?}", r.id, r.kind, r.label);
        }
    }
    // the sheets or views asked for (ids or parts of their names), else all
    let only: Vec<String> = a.iter().skip(2).cloned().collect();
    for sh in &s.sheets {
        let name = s.browser.iter().find(|r| r.id == sh.id).map(|r| r.label.clone()).unwrap_or_default();
        if !only.is_empty() && !only.iter().any(|o| *o == sh.id.to_string() || name.contains(o.as_str())) {
            continue;
        }
        eprintln!("{} {:?}: {} lines, {} fine, {} texts, {} images, {} views, {} strokes, {} fills, extent {:?} views {:?}", sh.id, name, sh.lines.len(), sh.fine.len(), sh.texts.len(), sh.images.len(), sh.views.len(), sh.strokes.len(), sh.fills.len(), sh.extent, sh.views.iter().map(|v| (v.view_id, v.name.as_str())).collect::<Vec<_>>());
        if std::env::var_os("SHEET_TEXTS").is_some() {
            for t in &sh.texts {
                eprintln!("   {:?} h{:.6} a{}{} w{:.4} c{:?} d{:?} {:?}", t.pos, t.height, t.halign, t.valign, t.wrap, t.color, t.dir, t.text);
            }
        }
        let e = sh.extent;
        let k = 1000.0; // mm
        let (w, h) = ((e[2] - e[0]) * k, (e[3] - e[1]) * k);
        let tx = |p: [f32; 2]| ((p[0] - e[0]) * k, (e[3] - p[1]) * k);
        let rgb = |c: [u8; 3]| format!("rgb({},{},{})", c[0], c[1], c[2]);
        let mut svg = format!("<svg xmlns='http://www.w3.org/2000/svg' width='{w:.3}mm' height='{h:.3}mm' viewBox='0 0 {w:.3} {h:.3}'><rect x='0' y='0' width='{w:.3}' height='{h:.3}' fill='white'/>");
        for i in sh.images.iter().filter(|i| !i.front) {
            let (p, q) = (tx([i.rect[0], i.rect[3]]), tx([i.rect[2], i.rect[1]]));
            // the image itself (PNG), turned in its frame by its quarter turns (counter-clockwise
            // on paper); a pale frame when it cannot be written
            let png = image::RgbaImage::from_raw(i.image.width, i.image.height, i.image.pixels.clone()).and_then(|im| {
                let mut v = Vec::new();
                im.write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Png).ok()?;
                Some(v)
            });
            match png {
                Some(png) if std::env::var_os("NO_IMAGES").is_none() => {
                    let (cx, cy) = ((p.0 + q.0) * 0.5, (p.1 + q.1) * 0.5);
                    let (fw, fh) = (q.0 - p.0, q.1 - p.1);
                    let (iw, ih) = if i.turns % 2 == 1 { (fh, fw) } else { (fw, fh) };
                    svg += &format!(
                        "<image x='{:.3}' y='{:.3}' width='{iw:.3}' height='{ih:.3}' preserveAspectRatio='none' transform='rotate({:.1} {cx:.3} {cy:.3})' href='data:image/png;base64,{}'/>",
                        cx - iw * 0.5,
                        cy - ih * 0.5,
                        -90.0 * i.turns as f32,
                        base64(&png)
                    );
                }
                _ => svg += &format!("<rect x='{:.3}' y='{:.3}' width='{:.3}' height='{:.3}' fill='#dde6ee' stroke='none'/>", p.0, p.1, q.0 - p.0, q.1 - p.1),
            }
        }
        for f in &sh.fills {
            for t in &f.tris {
                let (p, q, r) = (tx(t[0]), tx(t[1]), tx(t[2]));
                svg += &format!("<polygon points='{:.3},{:.3} {:.3},{:.3} {:.3},{:.3}' fill='{}' stroke='{}' stroke-width='0.02'/>", p.0, p.1, q.0, q.1, r.0, r.1, rgb(f.color), rgb(f.color));
            }
        }
        for sg in &sh.fine {
            let (p, q) = (tx(sg[0]), tx(sg[1]));
            svg += &format!("<line x1='{:.3}' y1='{:.3}' x2='{:.3}' y2='{:.3}' stroke='rgb(90,90,90)' stroke-width='0.13'/>", p.0, p.1, q.0, q.1);
        }
        for st in &sh.strokes {
            let pts: Vec<String> = st.pts.iter().map(|p| { let (x, y) = tx(*p); format!("{x:.3},{y:.3}") }).collect();
            svg += &format!("<polyline points='{}' fill='none' stroke='{}' stroke-width='{:.3}' stroke-linecap='round' stroke-linejoin='round'/>", pts.join(" "), rgb(st.color), (st.width * k).max(0.05));
        }
        for l in &sh.lines {
            let pts: Vec<String> = l.iter().map(|p| { let (x, y) = tx(*p); format!("{x:.3},{y:.3}") }).collect();
            svg += &format!("<polyline points='{}' fill='none' stroke='rgb(25,25,25)' stroke-width='0.35'/>", pts.join(" "));
        }
        for t in &sh.texts {
            if t.text.is_empty() {
                continue;
            }
            let (x, y) = tx(t.pos);
            // its typeface: Arial Narrow drawn as Arial at 82 % of its width; at the font size
            // Revit gives its height
            let narrow = t.font.to_ascii_lowercase().starts_with("arial narrow");
            let fs = t.font_size() * k;
            let ang = -(t.dir[1].atan2(t.dir[0])).to_degrees();
            let squeeze = if narrow { 0.82 } else { 1.0 };
            // its parts (rich text): byte range, bold, italic, underlined
            let parts: Vec<(std::ops::Range<usize>, bool, bool, bool, i8)> = if t.spans.is_empty() {
                vec![(0..t.text.len(), t.bold, t.italic, t.underline, 0)]
            } else {
                let mut at = 0;
                t.spans
                    .iter()
                    .map(|s| {
                        let r = at.min(t.text.len())..(at + s.len).min(t.text.len());
                        at = r.end;
                        (r, t.bold || s.bold, t.italic || s.italic, t.underline || s.underline, s.script)
                    })
                    .collect()
            };
            let width_of = |r: &std::ops::Range<usize>| -> f32 {
                parts
                    .iter()
                    .map(|(p, b, _, _, sc)| {
                        let (s0, s1) = (p.start.max(r.start), p.end.min(r.end));
                        let f = if *sc != 0 { fs * sw_format::SCRIPT_SIZE } else { fs };
                        if s0 < s1 { text_width(&t.text[s0..s1], f, *b) } else { 0.0 }
                    })
                    .sum::<f32>()
                    * squeeze
            };
            let tab = if t.tab > 0.0 { t.tab * k } else { 12.7 };
            // its rows: its lines, wrapped at its width as the viewer wraps them (byte ranges)
            let mut rows: Vec<std::ops::Range<usize>> = Vec::new();
            let mut start = 0;
            for line in t.text.split('\n') {
                let range = start..start + line.len();
                start = range.end + 1;
                if t.wrap > 0.0 {
                    // (as the viewer breaks it; Revit lets a row run `WRAP_SLACK` past its width)
                    // (its tab stops, fixed where Revit measures its words narrower, as far apart
                    // as the slack makes its width: "\tNECESSARY" past AC-100's box)
                    rows.extend(sw_format::break_line(&t.text, range, t.wrap * k * sw_format::WRAP_SLACK, tab * sw_format::WRAP_SLACK, t.break_words, &mut |r| width_of(&r)));
                } else {
                    rows.push(range);
                }
            }
            // (each line Revit's distance from the one before, by the two lines' weight and
            // slant: their characters' and their paragraph end's)
            let faces: Vec<(bool, bool)> = rows
                .iter()
                .map(|r| {
                    let end = if t.text[r.end..].starts_with('\n') { r.end + 1 } else { r.end.max(r.start + 1) };
                    // (an empty last row: its paragraph ends the text, the break before it its face)
                    let start = if r.start == t.text.len() { r.start.saturating_sub(1) } else { r.start };
                    parts.iter().filter(|(q, ..)| q.start < end && start < q.end).fold((false, false), |(b, i), (_, pb, pi, _, _)| (b || *pb, i || *pi))
                })
                .collect();
            let em = t.font_size();
            let pitch: Vec<f32> = (0..rows.len()).map(|i| if i == 0 { 0.0 } else { sw_format::line_pitch(em, faces[i - 1], faces[i]) * k }).collect();
            // its first baseline below its anchor, as Revit places it: its lines from the top of
            // the first (a regular line's box less the typeface's (Windows) descent above its
            // baseline, more when bold or italic) to the bottom of the last (the descent, more
            // when bold or italic), going down from the anchor (top), centred on it (middle) or
            // up to it (bottom)
            let desc = if narrow { 431.0 / 2048.0 } else { 434.0 / 2048.0 } * fs;
            let extra = |f: Option<&(bool, bool)>| f.map_or((0.0, 0.0), |f| sw_format::line_extra(em, f.0, f.1));
            let above = sw_format::line_pitch(em, (false, false), (false, false)) * k - desc + extra(faces.first()).0 * k;
            let below = desc + extra(faces.last()).1 * k;
            let total: f32 = above + pitch.iter().sum::<f32>() + below;
            let first = y + above
                - match t.valign {
                    0 => 0.0,
                    1 => total * 0.5,
                    _ => total,
                };
            let ax = [0.0, 0.5, 1.0][t.halign.min(2) as usize];
            let mut by = first;
            for (n, row) in rows.iter().enumerate() {
                if n > 0 {
                    by += pitch[n];
                }
                // its pieces between tabs, each at the next tab stop (the spaces it ends with take
                // no room, as the viewer lays it out)
                let row = &(row.start..row.start + t.text[row.clone()].trim_end_matches([' ', '\r']).len());
                let mut pieces = Vec::new();
                let (mut cx, mut at) = (0.0f32, row.start);
                for (i, piece) in t.text[row.clone()].split('\t').enumerate() {
                    if i > 0 {
                        cx = ((cx / tab).floor() + 1.0) * tab;
                    }
                    let pr = at..at + piece.len();
                    at = pr.end + 1;
                    pieces.push((pr.clone(), cx));
                    cx += width_of(&pr);
                }
                // (aligned as Revit aligns it: see `align_scale`)
                let x0 = x - ax * cx * sw_format::align_scale(t.height);
                for (pr, off) in pieces {
                    // its runs, each change of face (bold, italic) or superscript or subscript
                    // (smaller, raised or lowered) a text of its own, as the PDF prints split them
                    // (and the signs Revit writes in Segoe UI Symbol, "∅", "⌀", texts of their own)
                    let mut groups: Vec<((i8, bool, bool, bool), f32, String)> = Vec::new();
                    let mut gx = x0 + off;
                    for (r, b, i, u, sc) in &parts {
                        let (s0, s1) = (r.start.max(pr.start), r.end.min(pr.end));
                        if s0 >= s1 {
                            continue;
                        }
                        let mut cuts: Vec<usize> = t.text[s0..s1].char_indices().zip(t.text[s0..s1].chars().skip(1)).filter(|((_, c), d)| sw_format::fallback_glyph(*c) != sw_format::fallback_glyph(*d)).map(|((o, c), _)| s0 + o + c.len_utf8()).collect();
                        cuts.insert(0, s0);
                        cuts.push(s1);
                        for w in cuts.windows(2) {
                            let (a0, a1) = (w[0], w[1]);
                            let sym = t.text[a0..a1].chars().next().is_some_and(sw_format::fallback_glyph);
                            let esc = t.text[a0..a1].replace('&', "&amp;").replace('<', "&lt;");
                            let run = format!(
                                "<tspan font-weight='{}' font-style='{}' text-decoration='{}'>{esc}</tspan>",
                                if *b { "bold" } else { "normal" },
                                if *i { "italic" } else { "normal" },
                                if *u { "underline" } else { "none" }
                            );
                            match groups.last_mut() {
                                Some(g) if g.0 == (*sc, *b, *i, sym) => g.2 += &run,
                                _ => groups.push(((*sc, *b, *i, sym), gx, run)),
                            }
                            gx += width_of(&(a0..a1));
                        }
                    }
                    for ((sc, _, _, sym), tx0, runs) in groups {
                        let f = if sc != 0 { fs * sw_format::SCRIPT_SIZE } else { fs };
                        let ty = by - if sc > 0 { 0.302 * fs } else if sc < 0 { -0.15 * fs } else { 0.0 };
                        // (narrow: squeezed along its line from where it starts)
                        let sq = if narrow { format!(" translate({tx0:.3} 0) scale({squeeze} 1) translate({:.3} 0)", -tx0) } else { String::new() };
                        svg += &format!(
                            "<text x='{tx0:.3}' y='{ty:.3}' font-family='{}' font-size='{f:.3}' fill='{}' xml:space='preserve' transform='rotate({ang:.2} {x:.3} {y:.3}){sq}'>{runs}</text>",
                            if sym { "Segoe UI Symbol" } else { "Arial" },
                            rgb(t.color)
                        );
                    }
                }
            }
        }
        // pictures in front of the drawing
        for i in sh.images.iter().filter(|i| i.front) {
            let (p, q) = (tx([i.rect[0], i.rect[3]]), tx([i.rect[2], i.rect[1]]));
            let png = image::RgbaImage::from_raw(i.image.width, i.image.height, i.image.pixels.clone()).and_then(|im| {
                let mut v = Vec::new();
                im.write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Png).ok()?;
                Some(v)
            });
            if let (Some(png), None) = (png, std::env::var_os("NO_IMAGES")) {
                let (cx, cy) = ((p.0 + q.0) * 0.5, (p.1 + q.1) * 0.5);
                let (fw, fh) = (q.0 - p.0, q.1 - p.1);
                let (iw, ih) = if i.turns % 2 == 1 { (fh, fw) } else { (fw, fh) };
                svg += &format!(
                    "<image x='{:.3}' y='{:.3}' width='{iw:.3}' height='{ih:.3}' preserveAspectRatio='none' transform='rotate({:.1} {cx:.3} {cy:.3})' href='data:image/png;base64,{}'/>",
                    cx - iw * 0.5,
                    cy - ih * 0.5,
                    -90.0 * i.turns as f32,
                    base64(&png)
                );
            }
        }
        svg += "</svg>";
        std::fs::write(format!("{}/sheet_{}.svg", a[1], sh.id), svg).unwrap();
    }
}

/// Standard base64 (for images embedded in the SVG).
fn base64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len() / 3 * 4 + 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                s.push(A[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

