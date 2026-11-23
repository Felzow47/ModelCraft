//! AutoCAD DXF drawings (ASCII, R12 to 2018): the model space of a 2D drawing as a flat sheet,
//! its entities as AutoCAD draws them (lines, polylines with bulges, arcs, circles, ellipses,
//! splines, texts, block references, dimensions, solid fills, hatches with a solid fill,
//! leaders), in their colour (by entity, layer or block), layers switched off or frozen left
//! out. A file of 3D faces and meshes only is left to the mesh reader.

use std::collections::HashMap;
use std::path::Path;

use sw_format::{BrowserKind, BrowserNode, Fill2d, Scene, Sheet2d, Stroke2d, Text2d};

type P2 = [f64; 2];

/// One entity as read: its type and its group codes in order.
#[derive(Debug, Clone, Default)]
struct Ent {
    kind: String,
    groups: Vec<(i32, String)>,
    /// POLYLINE: its VERTEX entities.
    vertices: Vec<Ent>,
}

impl Ent {
    fn s(&self, c: i32) -> Option<&str> {
        self.groups.iter().find(|g| g.0 == c).map(|g| g.1.as_str())
    }
    fn f(&self, c: i32) -> Option<f64> {
        self.s(c).and_then(|v| v.trim().parse().ok())
    }
    fn fd(&self, c: i32, d: f64) -> f64 {
        self.f(c).unwrap_or(d)
    }
    fn i(&self, c: i32) -> Option<i64> {
        self.s(c).and_then(|v| v.trim().parse().ok())
    }
    fn all(&self, c: i32) -> impl Iterator<Item = f64> + '_ {
        self.groups.iter().filter(move |g| g.0 == c).filter_map(|g| g.1.trim().parse().ok())
    }
}

#[derive(Debug, Clone, Copy)]
struct Layer {
    color: [u8; 3],
    shown: bool,
}

struct Block {
    base: P2,
    ents: Vec<Ent>,
}

/// An affine map of the plane: x' = a·x + b·y + e, y' = c·x + d·y + f.
#[derive(Debug, Clone, Copy)]
struct Tf([f64; 6]);

impl Tf {
    const ID: Tf = Tf([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    fn apply(&self, p: P2) -> P2 {
        let m = &self.0;
        [m[0] * p[0] + m[1] * p[1] + m[4], m[2] * p[0] + m[3] * p[1] + m[5]]
    }
    fn lin(&self, p: P2) -> P2 {
        let m = &self.0;
        [m[0] * p[0] + m[1] * p[1], m[2] * p[0] + m[3] * p[1]]
    }
    /// `self` after `o` (o applied first).
    fn after(&self, o: &Tf) -> Tf {
        let (a, b) = (&self.0, &o.0);
        Tf([
            a[0] * b[0] + a[1] * b[2],
            a[0] * b[1] + a[1] * b[3],
            a[2] * b[0] + a[3] * b[2],
            a[2] * b[1] + a[3] * b[3],
            a[0] * b[4] + a[1] * b[5] + a[4],
            a[2] * b[4] + a[3] * b[5] + a[5],
        ])
    }
    fn scale(&self) -> f64 {
        let m = &self.0;
        (m[0] * m[3] - m[1] * m[2]).abs().sqrt()
    }
}

/// AutoCAD's colour index as RGB on a white sheet (7, white on black, prints black).
/// shortcut: 10–249 follow AutoCAD's hue wheel (24 hues, 5 shades, full or half saturation)
/// approximately; replace with AutoCAD's exact table if a drawing shows the difference.
fn aci(i: i64) -> [u8; 3] {
    match i {
        1 => [255, 0, 0],
        2 => [255, 255, 0],
        3 => [0, 255, 0],
        4 => [0, 255, 255],
        5 => [0, 0, 255],
        6 => [255, 0, 255],
        7 | 0 | 256 => [0, 0, 0],
        8 => [128, 128, 128],
        9 => [192, 192, 192],
        250..=255 => {
            let v = [51, 91, 132, 173, 214, 255][(i - 250) as usize];
            [v, v, v]
        }
        10..=249 => {
            let h = ((i / 10) - 1) as f64 * 15.0;
            let k = i % 10;
            let v = [255.0, 189.0, 129.0, 104.0, 79.0][(k / 2) as usize] / 255.0;
            let s = if k % 2 == 0 { 1.0 } else { 0.5 };
            let c = v * s;
            let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
            let (r, g, b) = match (h / 60.0) as i32 {
                0 => (c, x, 0.0),
                1 => (x, c, 0.0),
                2 => (0.0, c, x),
                3 => (0.0, x, c),
                4 => (x, 0.0, c),
                _ => (c, 0.0, x),
            };
            let m = v - c;
            [((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8]
        }
        _ => [0, 0, 0],
    }
}

/// The entity's own colour: true colour (420), else its colour index; `None` by layer.
fn own_color(e: &Ent) -> Option<Option<[u8; 3]>> {
    if let Some(tc) = e.i(420) {
        return Some(Some([((tc >> 16) & 255) as u8, ((tc >> 8) & 255) as u8, (tc & 255) as u8]));
    }
    match e.i(62) {
        None | Some(256) => None,
        Some(0) => Some(None), // by block
        Some(c) if c < 0 => Some(Some(aci(-c))),
        Some(c) => Some(Some(aci(c))),
    }
}

/// Group code / value pairs of the file (text decoded as UTF-8, else as Windows-1252).
fn pairs(data: &[u8]) -> Vec<(i32, String)> {
    let text = match std::str::from_utf8(data) {
        Ok(t) => t.to_string(),
        Err(_) => data.iter().map(|&b| b as char).collect(),
    };
    let mut lines = text.lines();
    let mut out = Vec::new();
    while let (Some(c), Some(v)) = (lines.next(), lines.next()) {
        let Ok(c) = c.trim().parse::<i32>() else { break };
        out.push((c, v.trim_end_matches('\r').to_string()));
    }
    out
}

/// Entities from `ps` (pairs starting at an entity's 0 code) up to the end of the list.
fn entities(ps: &[(i32, String)]) -> Vec<Ent> {
    let mut out: Vec<Ent> = Vec::new();
    let mut cur: Option<Ent> = None;
    let mut in_poly = false;
    for (c, v) in ps {
        if *c == 0 {
            if let Some(e) = cur.take() {
                if e.kind == "VERTEX" && in_poly {
                    if let Some(p) = out.last_mut() {
                        p.vertices.push(e);
                    }
                } else {
                    out.push(e);
                }
            }
            match v.as_str() {
                "SEQEND" => in_poly = false,
                "POLYLINE" => in_poly = true,
                _ => {}
            }
            if v != "SEQEND" {
                cur = Some(Ent { kind: v.clone(), ..Default::default() });
            }
        } else if let Some(e) = cur.as_mut() {
            e.groups.push((*c, v.clone()));
        }
    }
    if let Some(e) = cur {
        out.push(e);
    }
    out
}

/// The sections of the file: name -> its pairs (between SECTION/2 name and ENDSEC).
fn sections(ps: &[(i32, String)]) -> HashMap<String, Vec<(i32, String)>> {
    let mut out = HashMap::new();
    let mut i = 0;
    while i + 1 < ps.len() {
        if ps[i] == (0, "SECTION".to_string()) && ps[i + 1].0 == 2 {
            let name = ps[i + 1].1.clone();
            let mut j = i + 2;
            while j < ps.len() && ps[j] != (0, "ENDSEC".to_string()) {
                j += 1;
            }
            out.insert(name, ps[i + 2..j].to_vec());
            i = j;
        }
        i += 1;
    }
    out
}

/// Points of the arc of centre `c`, radius `r`, from angle `a0` to `a1` (radians,
/// counter-clockwise), within a chord error of a thousandth of its radius.
fn arc(c: P2, r: f64, a0: f64, mut a1: f64) -> Vec<P2> {
    while a1 <= a0 {
        a1 += std::f64::consts::TAU;
    }
    let n = (((a1 - a0) / (2.0 * (1.0 - 1e-3f64).acos())).ceil() as usize).clamp(2, 720);
    (0..=n).map(|k| a0 + (a1 - a0) * k as f64 / n as f64).map(|a| [c[0] + r * a.cos(), c[1] + r * a.sin()]).collect()
}

/// The segment from `a` to `b` with bulge `bulge` (tan of a quarter of its arc's angle,
/// positive counter-clockwise) as points, `a` left out.
fn bulge(a: P2, b: P2, bulge: f64) -> Vec<P2> {
    if bulge.abs() < 1e-9 {
        return vec![b];
    }
    let th = 4.0 * bulge.atan();
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let chord = dx.hypot(dy);
    if chord < 1e-12 {
        return vec![b];
    }
    let r = chord / (2.0 * (th / 2.0).sin());
    // centre: from the chord's middle, along its normal
    let h = r * (th / 2.0).cos();
    let (mx, my) = ((a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0);
    let (nx, ny) = (-dy / chord, dx / chord);
    let c = [mx + nx * h, my + ny * h];
    let a0 = (a[1] - c[1]).atan2(a[0] - c[0]);
    let pts = if th > 0.0 { arc(c, r.abs(), a0, a0 + th) } else { arc(c, r.abs(), a0 + th, a0).into_iter().rev().collect() };
    pts.into_iter().skip(1).collect()
}

/// Points of a B-spline (degree `p`, knots `u`, control points `cp`, weights `w`).
fn bspline(p: usize, u: &[f64], cp: &[P2], w: &[f64]) -> Vec<P2> {
    let n = cp.len();
    if n < 2 || u.len() < n + p + 1 {
        return cp.to_vec();
    }
    let (t0, t1) = (u[p], u[n]);
    let steps = (n * 12).clamp(16, 2000);
    let eval = |t: f64| -> P2 {
        // de Boor on homogeneous points
        let mut k = p;
        while k < n - 1 && t >= u[k + 1] {
            k += 1;
        }
        let mut d: Vec<[f64; 3]> = (0..=p).map(|j| {
            let i = (k + j).saturating_sub(p).min(n - 1);
            let wi = w.get(i).copied().unwrap_or(1.0);
            [cp[i][0] * wi, cp[i][1] * wi, wi]
        }).collect();
        for r in 1..=p {
            for j in (r..=p).rev() {
                let i = k + j - p;
                let den = u[i + p - r + 1] - u[i];
                let a = if den.abs() < 1e-15 { 0.0 } else { (t - u[i]) / den };
                for c in 0..3 {
                    d[j][c] = (1.0 - a) * d[j - 1][c] + a * d[j][c];
                }
            }
        }
        let q = d[p];
        if q[2].abs() < 1e-15 { [q[0], q[1]] } else { [q[0] / q[2], q[1] / q[2]] }
    };
    (0..=steps).map(|k| eval(t0 + (t1 - t0) * k as f64 / steps as f64)).collect()
}

/// MTEXT's text without its formatting codes: paragraphs as line breaks, `\~` as a space,
/// `\U+XXXX` decoded, `{}` groups and `\f…;`, `\H…;`, `\C…;`… codes left out.
fn mtext_plain(s: &str) -> String {
    let mut out = String::new();
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c == '\\' && i + 1 < cs.len() {
            let k = cs[i + 1];
            match k {
                'P' | 'X' => {
                    out.push('\n');
                    i += 2;
                }
                '~' => {
                    out.push(' ');
                    i += 2;
                }
                '\\' | '{' | '}' => {
                    out.push(k);
                    i += 2;
                }
                'U' if i + 6 < cs.len() + 1 && cs.get(i + 2) == Some(&'+') => {
                    let hex: String = cs[i + 3..(i + 7).min(cs.len())].iter().collect();
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                    i += 7;
                }
                'L' | 'l' | 'O' | 'o' | 'K' | 'k' => i += 2,
                'S' => {
                    // stacked fraction "\Sa^b;": written a/b
                    let mut j = i + 2;
                    while j < cs.len() && cs[j] != ';' {
                        out.push(if cs[j] == '^' || cs[j] == '#' { '/' } else { cs[j] });
                        j += 1;
                    }
                    i = j + 1;
                }
                _ => {
                    // a code with an argument up to ';'
                    let mut j = i + 2;
                    while j < cs.len() && cs[j] != ';' {
                        j += 1;
                    }
                    i = j + 1;
                }
            }
        } else if c == '{' || c == '}' {
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// TEXT's `%%` codes: degree, plus-minus, diameter, percent; underline/overline left out.
fn text_plain(s: &str) -> String {
    s.replace("%%d", "°").replace("%%D", "°").replace("%%p", "±").replace("%%P", "±").replace("%%c", "Ø").replace("%%C", "Ø").replace("%%%", "%").replace("%%u", "").replace("%%U", "").replace("%%o", "").replace("%%O", "")
}

struct Ctx<'a> {
    layers: &'a HashMap<String, Layer>,
    blocks: &'a HashMap<String, Block>,
    /// metres a drawing unit
    unit: f64,
    out: Sheet2d,
    budget: usize,
}

impl Ctx<'_> {
    fn color(&self, e: &Ent, parent: [u8; 3]) -> [u8; 3] {
        match own_color(e) {
            Some(Some(c)) => c,
            Some(None) => parent,
            None => self.layers.get(e.s(8).unwrap_or("0")).map_or([0; 3], |l| l.color),
        }
    }
    fn shown(&self, e: &Ent) -> bool {
        self.layers.get(e.s(8).unwrap_or("0")).is_none_or(|l| l.shown) && e.i(60).unwrap_or(0) == 0
    }
    fn m(&self, p: P2) -> [f32; 2] {
        [(p[0] * self.unit) as f32, (p[1] * self.unit) as f32]
    }
    fn stroke(&mut self, tf: &Tf, pts: &[P2], color: [u8; 3]) {
        if pts.len() < 2 || self.budget == 0 {
            return;
        }
        self.budget -= 1;
        let pts = pts.iter().map(|p| self.m(tf.apply(*p))).collect();
        self.out.strokes.push(Stroke2d { pts, width: 0.000_25, color });
    }
    fn fill(&mut self, tf: &Tf, poly: &[P2], color: [u8; 3]) {
        if poly.len() < 3 {
            return;
        }
        let p: Vec<[f32; 2]> = poly.iter().map(|q| self.m(tf.apply(*q))).collect();
        // fan: convex outlines (SOLID, most hatch loops); shortcut: concave hatches fill
        // their fan, switch to an ear-clipping triangulation if one shows wrong
        let tris = (1..p.len() - 1).map(|k| [p[0], p[k], p[k + 1]]).collect();
        self.out.fills.push(Fill2d { tris, color, ..Default::default() });
    }
    fn text(&mut self, tf: &Tf, at: P2, angle: f64, height: f64, text: String, (halign, valign): (u8, u8), color: [u8; 3], wrap: f64) {
        if text.trim().is_empty() {
            return;
        }
        let o = tf.apply(at);
        let d = tf.lin([angle.cos(), angle.sin()]);
        let l = d[0].hypot(d[1]).max(1e-12);
        let k = tf.scale();
        self.out.texts.push(Text2d {
            pos: self.m(o),
            dir: [(d[0] / l) as f32, (d[1] / l) as f32],
            height: (height * k * self.unit) as f32,
            halign,
            valign,
            text,
            wrap: (wrap * k * self.unit) as f32,
            color,
            font: "Arial".into(),
            ..Default::default()
        });
    }

    fn draw(&mut self, e: &Ent, tf: &Tf, parent: [u8; 3], depth: usize) {
        if !self.shown(e) || depth > 16 {
            return;
        }
        let col = self.color(e, parent);
        let p = |x: i32, y: i32| [e.fd(x, 0.0), e.fd(y, 0.0)];
        match e.kind.as_str() {
            "LINE" => self.stroke(tf, &[p(10, 20), p(11, 21)], col),
            "LWPOLYLINE" => {
                // its vertices: 10/20 each, a bulge (42) after the vertex it starts from
                let mut vs: Vec<(P2, f64)> = Vec::new();
                for (c, v) in &e.groups {
                    let x: f64 = v.trim().parse().unwrap_or(0.0);
                    match c {
                        10 => vs.push(([x, 0.0], 0.0)),
                        20 => {
                            if let Some(l) = vs.last_mut() {
                                l.0[1] = x;
                            }
                        }
                        42 => {
                            if let Some(l) = vs.last_mut() {
                                l.1 = x;
                            }
                        }
                        _ => {}
                    }
                }
                let closed = e.i(70).unwrap_or(0) & 1 != 0;
                self.poly(tf, &vs, closed, col);
            }
            "POLYLINE" => {
                let flags = e.i(70).unwrap_or(0);
                if flags & 64 != 0 {
                    // a polyface mesh: the edges of its faces (vertices with flag 128 are faces)
                    let pts: Vec<P2> = e.vertices.iter().filter(|v| v.i(70).unwrap_or(0) & 128 == 0 || v.i(70).unwrap_or(0) & 64 != 0).map(|v| [v.fd(10, 0.0), v.fd(20, 0.0)]).collect();
                    for f in e.vertices.iter().filter(|v| v.i(70).unwrap_or(0) & 128 != 0 && v.i(70).unwrap_or(0) & 64 == 0) {
                        let idx: Vec<usize> = [71, 72, 73, 74].iter().filter_map(|c| f.i(*c)).filter(|i| *i != 0).map(|i| i.unsigned_abs() as usize - 1).collect();
                        let mut loop_: Vec<P2> = idx.iter().filter_map(|i| pts.get(*i).copied()).collect();
                        if let Some(f0) = loop_.first().copied() {
                            loop_.push(f0);
                        }
                        self.stroke(tf, &loop_, col);
                    }
                    return;
                }
                let vs: Vec<(P2, f64)> = e.vertices.iter().map(|v| ([v.fd(10, 0.0), v.fd(20, 0.0)], v.fd(42, 0.0))).collect();
                self.poly(tf, &vs, flags & 1 != 0, col);
            }
            "CIRCLE" => {
                let r = e.fd(40, 0.0);
                self.stroke(tf, &arc(p(10, 20), r, 0.0, std::f64::consts::TAU), col);
            }
            "ARC" => {
                let r = e.fd(40, 0.0);
                self.stroke(tf, &arc(p(10, 20), r, e.fd(50, 0.0).to_radians(), e.fd(51, 360.0).to_radians()), col);
            }
            "ELLIPSE" => {
                let (c, mj) = (p(10, 20), p(11, 21));
                let ratio = e.fd(40, 1.0);
                let mn = [-mj[1] * ratio, mj[0] * ratio];
                let (t0, mut t1) = (e.fd(41, 0.0), e.fd(42, std::f64::consts::TAU));
                while t1 <= t0 {
                    t1 += std::f64::consts::TAU;
                }
                let n = 128;
                let pts: Vec<P2> = (0..=n).map(|k| t0 + (t1 - t0) * k as f64 / n as f64).map(|t| [c[0] + mj[0] * t.cos() + mn[0] * t.sin(), c[1] + mj[1] * t.cos() + mn[1] * t.sin()]).collect();
                self.stroke(tf, &pts, col);
            }
            "SPLINE" => {
                let deg = e.i(71).unwrap_or(3).max(1) as usize;
                let knots: Vec<f64> = e.all(40).collect();
                let weights: Vec<f64> = e.all(41).collect();
                let cp: Vec<P2> = e.all(10).zip(e.all(20)).map(|(x, y)| [x, y]).collect();
                let pts = if cp.len() >= 2 && knots.len() >= cp.len() + deg + 1 {
                    bspline(deg, &knots, &cp, &weights)
                } else {
                    e.all(11).zip(e.all(21)).map(|(x, y)| [x, y]).collect()
                };
                let closed = e.i(70).unwrap_or(0) & 1 != 0;
                let mut pts = pts;
                if closed {
                    if let Some(f) = pts.first().copied() {
                        pts.push(f);
                    }
                }
                self.stroke(tf, &pts, col);
            }
            "SOLID" | "TRACE" => {
                // corners 1, 2, 4, 3 in order around it
                let q = [p(10, 20), p(11, 21), p(13, 23), p(12, 22)];
                self.fill(tf, &q, col);
            }
            "3DFACE" => {
                let q = [p(10, 20), p(11, 21), p(12, 22), p(13, 23), p(10, 20)];
                self.stroke(tf, &q, col);
            }
            "TEXT" | "ATTRIB" => {
                if e.kind == "ATTRIB" && e.i(70).unwrap_or(0) & 1 != 0 {
                    return; // invisible attribute
                }
                let h = e.fd(40, 2.5);
                let (j, v) = (e.i(72).unwrap_or(0), e.i(73).unwrap_or(0));
                // aligned by its alignment point unless left on its baseline
                let at = if j == 0 && v == 0 { p(10, 20) } else { p(11, 21) };
                let halign = match j {
                    1 | 4 => 1,
                    2 => 2,
                    _ => 0,
                };
                let valign = match v {
                    3 => 0,
                    2 => 1,
                    _ => 2,
                };
                self.text(tf, at, e.fd(50, 0.0).to_radians(), h, text_plain(e.s(1).unwrap_or("")), (halign, valign), col, 0.0);
            }
            "MTEXT" => {
                let mut raw = String::new();
                for (c, v) in &e.groups {
                    if *c == 3 {
                        raw += v;
                    }
                }
                raw += e.s(1).unwrap_or("");
                let att = e.i(71).unwrap_or(1).clamp(1, 9) - 1;
                let angle = match (e.f(11), e.f(21)) {
                    (Some(x), Some(y)) if x.hypot(y) > 1e-12 => y.atan2(x),
                    _ => e.fd(50, 0.0).to_radians(),
                };
                let (halign, valign) = ((att % 3) as u8, (att / 3) as u8);
                self.text(tf, p(10, 20), angle, e.fd(40, 2.5), mtext_plain(&raw), (halign, valign), col, e.fd(41, 0.0));
            }
            "INSERT" | "DIMENSION" => {
                let Some(name) = e.s(2) else { return };
                let Some(b) = self.blocks.get(name) else { return };
                let (sx, sy) = if e.kind == "INSERT" { (e.fd(41, 1.0), e.fd(42, 1.0)) } else { (1.0, 1.0) };
                let rot = if e.kind == "INSERT" { e.fd(50, 0.0).to_radians() } else { 0.0 };
                let (cols, rows) = (e.i(70).unwrap_or(1).max(1), e.i(71).unwrap_or(1).max(1));
                let (dc, dr) = (e.fd(44, 0.0), e.fd(45, 0.0));
                let ins = if e.kind == "INSERT" { p(10, 20) } else { [0.0, 0.0] };
                let ents = b.ents.clone();
                let base = b.base;
                for r in 0..rows {
                    for c in 0..cols {
                        let (co, si) = (rot.cos(), rot.sin());
                        // block space: minus its base, scaled, turned, moved to the insertion
                        // (array cells along the turned axes)
                        let off = [c as f64 * dc, r as f64 * dr];
                        let t = Tf([co * sx, -si * sy, si * sx, co * sy, ins[0] + co * off[0] - si * off[1] - (co * sx * base[0] - si * sy * base[1]), ins[1] + si * off[0] + co * off[1] - (si * sx * base[0] + co * sy * base[1])]);
                        let t = tf.after(&t);
                        for be in &ents {
                            self.draw(be, &t, col, depth + 1);
                        }
                    }
                }
            }
            "HATCH" => {
                // solid fills only (pattern hatches: shortcut, drawn as nothing; add their
                // lines from the pattern definition, codes 53/43/44/45/46/49, when needed)
                if e.i(70).unwrap_or(0) & 1 == 0 {
                    return;
                }
                for lp in hatch_loops(e) {
                    self.fill(tf, &lp, col);
                }
            }
            "LEADER" => {
                let pts: Vec<P2> = e.all(10).zip(e.all(20)).map(|(x, y)| [x, y]).collect();
                self.stroke(tf, &pts, col);
            }
            _ => {}
        }
    }

    fn poly(&mut self, tf: &Tf, vs: &[(P2, f64)], closed: bool, col: [u8; 3]) {
        if vs.is_empty() {
            return;
        }
        let mut pts = vec![vs[0].0];
        let n = vs.len();
        for k in 0..(if closed { n } else { n - 1 }) {
            let (a, b) = (vs[k], vs[(k + 1) % n]);
            pts.extend(bulge(a.0, b.0, a.1));
        }
        self.stroke(tf, &pts, col);
    }
}

/// A hatch's boundary loops as points (polyline loops with bulges, edge loops of lines and
/// arcs).
fn hatch_loops(e: &Ent) -> Vec<Vec<P2>> {
    let g = &e.groups;
    let mut out = Vec::new();
    let mut i = g.iter().position(|x| x.0 == 91).map_or(g.len(), |k| k + 1);
    let num = |k: usize| g.get(k).and_then(|x| x.1.trim().parse::<f64>().ok()).unwrap_or(0.0);
    while i < g.len() && g[i].0 == 92 {
        let flag = num(i) as i64;
        i += 1;
        let mut lp: Vec<P2> = Vec::new();
        if flag & 2 != 0 {
            // polyline loop: 72 has bulges, 73 closed, 93 count, then 10/20(/42)
            let mut has_bulge = false;
            let mut count = 0;
            while i < g.len() && g[i].0 != 10 {
                match g[i].0 {
                    72 => has_bulge = num(i) != 0.0,
                    93 => count = num(i) as usize,
                    _ => {}
                }
                i += 1;
            }
            let mut vs: Vec<(P2, f64)> = Vec::new();
            while vs.len() < count && i + 1 < g.len() && g[i].0 == 10 {
                let p = [num(i), num(i + 1)];
                i += 2;
                let b = if has_bulge && i < g.len() && g[i].0 == 42 {
                    i += 1;
                    num(i - 1)
                } else {
                    0.0
                };
                vs.push((p, b));
            }
            if let Some(f) = vs.first() {
                lp.push(f.0);
                for k in 0..vs.len() {
                    lp.extend(bulge(vs[k].0, vs[(k + 1) % vs.len()].0, vs[k].1));
                }
            }
        } else {
            // edge loop: 93 edges, each 72 type then its data
            let edges = if g.get(i).is_some_and(|x| x.0 == 93) { num(i) as usize } else { 0 };
            i += 1;
            for _ in 0..edges {
                if !g.get(i).is_some_and(|x| x.0 == 72) {
                    break;
                }
                let kind = num(i) as i64;
                i += 1;
                let mut vals: HashMap<i32, Vec<f64>> = HashMap::new();
                while i < g.len() && ![72, 97, 92, 75].contains(&g[i].0) {
                    vals.entry(g[i].0).or_default().push(num(i));
                    i += 1;
                }
                let v = |c: i32, k: usize| vals.get(&c).and_then(|x| x.get(k)).copied().unwrap_or(0.0);
                match kind {
                    1 => {
                        if lp.is_empty() {
                            lp.push([v(10, 0), v(20, 0)]);
                        }
                        lp.push([v(11, 0), v(21, 0)]);
                    }
                    2 => {
                        let (a0, a1) = (v(50, 0).to_radians(), v(51, 0).to_radians());
                        let ccw = vals.get(&73).is_none_or(|x| x.first().copied().unwrap_or(1.0) != 0.0);
                        // (a clockwise one keeps its angles mirrored: from -start to -end)
                        let c = [v(10, 0), v(20, 0)];
                        let pts: Vec<P2> = if ccw { arc(c, v(40, 0), a0, a1) } else { arc(c, v(40, 0), -a1, -a0).into_iter().rev().collect() };
                        lp.extend(pts);
                    }
                    _ => {}
                }
            }
        }
        // its source objects (97 count, 330 handles)
        while i < g.len() && g[i].0 != 92 && g[i].0 != 75 {
            i += 1;
        }
        if lp.len() >= 3 {
            out.push(lp);
        }
    }
    out
}

/// Metres per drawing unit (`$INSUNITS`; unitless drawings read as millimetres).
fn unit_of(header: &[(i32, String)]) -> f64 {
    let v = header.iter().position(|g| g.1 == "$INSUNITS").and_then(|k| header.get(k + 1)).and_then(|g| g.1.trim().parse::<i64>().ok()).unwrap_or(4);
    match v {
        1 => 0.0254,
        2 => 0.3048,
        5 => 0.01,
        6 => 1.0,
        7 => 1000.0,
        8 => 0.0254e-6,
        9 => 0.0254e-3,
        10 => 0.9144,
        14 => 0.1,
        _ => 0.001,
    }
}

/// Whether the drawing has 2D content (anything but 3D faces and meshes).
pub fn has_2d(data: &[u8]) -> bool {
    let ps = pairs(data);
    let sec = sections(&ps);
    let ents = entities(sec.get("ENTITIES").map_or(&[][..], |v| v.as_slice()));
    ents.iter().any(|e| !matches!(e.kind.as_str(), "3DFACE" | "MESH" | "3DSOLID" | "BODY" | "REGION" | "POINT") && !(e.kind == "POLYLINE" && e.i(70).unwrap_or(0) & (16 | 64) != 0))
}

/// The drawing's model space as a sheet.
pub fn load(path: &Path, data: &[u8]) -> Result<Scene, String> {
    let ps = pairs(data);
    if ps.is_empty() {
        return Err("DXF : fichier vide ou binaire".into());
    }
    let sec = sections(&ps);
    let header = sec.get("HEADER").cloned().unwrap_or_default();
    let unit = unit_of(&header);
    // layers: colour (negative: switched off), flags 70 bit 1 frozen
    let mut layers = HashMap::new();
    if let Some(t) = sec.get("TABLES") {
        for e in entities(t).into_iter().filter(|e| e.kind == "LAYER") {
            let c = e.i(62).unwrap_or(7);
            let color = e.i(420).map(|tc| [((tc >> 16) & 255) as u8, ((tc >> 8) & 255) as u8, (tc & 255) as u8]).unwrap_or_else(|| aci(c.abs()));
            let shown = c >= 0 && e.i(70).unwrap_or(0) & 1 == 0 && e.i(290).unwrap_or(1) != 0 || e.s(2) == Some("0") && c >= 0;
            layers.insert(e.s(2).unwrap_or("").to_string(), Layer { color, shown });
        }
    }
    // blocks
    let mut blocks = HashMap::new();
    if let Some(b) = sec.get("BLOCKS") {
        let all = entities(b);
        let mut k = 0;
        while k < all.len() {
            if all[k].kind == "BLOCK" {
                let name = all[k].s(2).unwrap_or("").to_string();
                let base = [all[k].fd(10, 0.0), all[k].fd(20, 0.0)];
                let mut j = k + 1;
                let mut ents = Vec::new();
                while j < all.len() && all[j].kind != "ENDBLK" {
                    ents.push(all[j].clone());
                    j += 1;
                }
                blocks.insert(name, Block { base, ents });
                k = j;
            }
            k += 1;
        }
    }
    let ents = entities(sec.get("ENTITIES").map_or(&[][..], |v| v.as_slice()));
    // the model space (paper space entities, code 67 = 1, left out)
    let mut ctx = Ctx { layers: &layers, blocks: &blocks, unit, out: Sheet2d { id: 1, ..Default::default() }, budget: 3_000_000 };
    for e in ents.iter().filter(|e| e.i(67).unwrap_or(0) == 0) {
        ctx.draw(e, &Tf::ID, [0; 3], 0);
    }
    let mut out = ctx.out;
    // its extent: what it draws
    let mut b = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    let mut grow = |p: [f32; 2]| {
        if p[0].is_finite() && p[1].is_finite() {
            b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
        }
    };
    for s in &out.strokes {
        s.pts.iter().for_each(|p| grow(*p));
    }
    for f in &out.fills {
        f.tris.iter().flatten().for_each(|p| grow(*p));
    }
    for t in &out.texts {
        grow(t.pos);
    }
    if !b[0].is_finite() {
        return Err("DXF : le dessin ne contient rien à afficher".into());
    }
    let m = ((b[2] - b[0]).max(b[3] - b[1]) * 0.02).max(1e-6);
    out.extent = [b[0] - m, b[1] - m, b[2] + m, b[3] + m];
    let mut scene = crate::empty_scene(path, "DXF");
    let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    scene.browser = vec![BrowserNode { label: name, depth: 0, kind: BrowserKind::Sheet, id: 1, start: true }];
    scene.sheets = vec![out];
    scene.properties.push(("Unité".into(), format!("{} m", unit)));
    Ok(scene)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_bulge_of_one_is_a_half_circle() {
        let pts = super::bulge([0.0, 0.0], [2.0, 0.0], 1.0);
        // counter-clockwise from (0,0) to (2,0): a half circle below the chord, radius 1
        let low = pts.iter().fold(0.0f64, |m, p| m.min(p[1]));
        assert!((low + 1.0).abs() < 1e-3, "{low}");
        assert!(pts.iter().all(|p| ((p[0] - 1.0).hypot(p[1]) - 1.0).abs() < 1e-9));
        assert!((pts.last().unwrap()[0] - 2.0).abs() < 1e-9);
    }

    #[test]
    fn mtext_codes_are_left_out() {
        assert_eq!(super::mtext_plain("{\\fArial|b1;LIMITE}\\PDE ZONE\\~1"), "LIMITE\nDE ZONE 1");
    }

    #[test]
    fn a_small_drawing_reads() {
        let dxf = "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0\n20\n0\n11\n100\n21\n0\n0\nCIRCLE\n8\n0\n62\n1\n10\n50\n20\n50\n40\n10\n0\nTEXT\n8\n0\n10\n0\n20\n10\n40\n2.5\n1\nHELLO\n0\nENDSEC\n0\nEOF\n";
        let s = super::load(std::path::Path::new("t.dxf"), dxf.as_bytes()).unwrap();
        let sh = &s.sheets[0];
        assert_eq!(sh.strokes.len(), 2);
        assert_eq!(sh.strokes[1].color, [255, 0, 0]);
        assert_eq!(sh.texts[0].text, "HELLO");
        assert!((sh.strokes[0].pts[1][0] - 0.1).abs() < 1e-6);
    }
}
