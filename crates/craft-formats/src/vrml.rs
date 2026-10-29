//! VRML 97 (.wrl) and X3D classic encoding (.x3dv), written from the ISO/IEC 14772 and 19776-2
//! grammars: DEF/USE, Transform / Group / Switch / LOD / Anchor / Billboard / Collision,
//! Shape with Material colour, IndexedFaceSet, IndexedTriangleSet, TriangleSet, Box, Sphere,
//! Cylinder, Cone. Gzip-compressed files (.wrl.gz or compressed .wrl) are inflated first.
//! PROTO bodies, scripts, routes and Inline files are skipped. Units: metres, Y up.
use crate::{component, empty_scene, mat_mul, stem, Loaded};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;
use sw_format::{MeshSource, SceneMesh};

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Num(f64),
    Str(String),
    Open,  // {
    Close, // }
    LBr,   // [
    RBr,   // ]
}

fn tokenize(src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b'#' => {
                while i < b.len() && b[i] != b'\n' && b[i] != b'\r' {
                    i += 1;
                }
            }
            b' ' | b'\t' | b'\n' | b'\r' | b',' => i += 1,
            b'{' => {
                out.push(Tok::Open);
                i += 1
            }
            b'}' => {
                out.push(Tok::Close);
                i += 1
            }
            b'[' => {
                out.push(Tok::LBr);
                i += 1
            }
            b']' => {
                out.push(Tok::RBr);
                i += 1
            }
            b'"' => {
                i += 1;
                let mut s = String::new();
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' && i + 1 < b.len() {
                        i += 1;
                    }
                    s.push(b[i] as char);
                    i += 1;
                }
                i += 1;
                out.push(Tok::Str(s));
            }
            _ => {
                let st = i;
                while i < b.len() && !matches!(b[i], b' ' | b'\t' | b'\n' | b'\r' | b',' | b'{' | b'}' | b'[' | b']' | b'"' | b'#') {
                    i += 1;
                }
                let w = &src[st..i];
                let first = w.as_bytes()[0];
                if first.is_ascii_digit() || first == b'-' || first == b'+' || first == b'.' {
                    if let Ok(v) = w.parse::<f64>() {
                        out.push(Tok::Num(v));
                        continue;
                    }
                    if let Some(h) = w.strip_prefix("0x").or_else(|| w.strip_prefix("0X")) {
                        if let Ok(v) = u64::from_str_radix(h, 16) {
                            out.push(Tok::Num(v as f64));
                            continue;
                        }
                    }
                }
                out.push(Tok::Word(w.to_string()));
            }
        }
    }
    out
}

#[derive(Debug)]
struct Node {
    ty: String,
    def: Option<String>,
    fields: Vec<(String, Val)>,
}

#[derive(Debug, Clone)]
enum Val {
    Nums(Vec<f64>),
    Node(Rc<Node>),
    Nodes(Vec<Rc<Node>>),
    #[allow(dead_code)]
    Strs(Vec<String>),
    Words(Vec<String>),
}

impl Node {
    fn get(&self, k: &str) -> Option<&Val> {
        self.fields.iter().rev().find(|f| f.0 == k).map(|f| &f.1)
    }
    fn nums(&self, k: &str) -> &[f64] {
        match self.get(k) {
            Some(Val::Nums(v)) => v,
            _ => &[],
        }
    }
    fn node(&self, k: &str) -> Option<&Rc<Node>> {
        match self.get(k) {
            Some(Val::Node(n)) => Some(n),
            Some(Val::Nodes(v)) => v.first(),
            _ => None,
        }
    }
    fn nodes(&self, k: &str) -> Vec<Rc<Node>> {
        match self.get(k) {
            Some(Val::Node(n)) => vec![n.clone()],
            Some(Val::Nodes(v)) => v.clone(),
            _ => Vec::new(),
        }
    }
    fn word(&self, k: &str) -> Option<&str> {
        match self.get(k) {
            Some(Val::Words(w)) => w.first().map(|s| s.as_str()),
            _ => None,
        }
    }
}

struct Parser {
    t: Vec<Tok>,
    i: usize,
    defs: HashMap<String, Rc<Node>>,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }
    fn peek2(&self) -> Option<&Tok> {
        self.t.get(self.i + 1)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.t.get(self.i).cloned();
        self.i += 1;
        t
    }
    fn skip_balanced(&mut self, open: Tok, close: Tok) {
        let mut d = 0i32;
        while let Some(t) = self.next() {
            if t == open {
                d += 1;
            } else if t == close {
                d -= 1;
                if d <= 0 {
                    return;
                }
            }
        }
    }
    /// Top-level statements; returns the root nodes.
    fn statements(&mut self, until_close: bool) -> Vec<Rc<Node>> {
        let mut out = Vec::new();
        while let Some(t) = self.peek().cloned() {
            match t {
                Tok::Close | Tok::RBr if until_close => break,
                Tok::Word(w) => match w.as_str() {
                    "PROTO" => {
                        self.i += 2;
                        self.skip_balanced(Tok::LBr, Tok::RBr);
                        self.skip_balanced(Tok::Open, Tok::Close);
                    }
                    "EXTERNPROTO" => {
                        self.i += 2;
                        self.skip_balanced(Tok::LBr, Tok::RBr);
                        // URL string(s)
                        if self.peek() == Some(&Tok::LBr) {
                            self.skip_balanced(Tok::LBr, Tok::RBr);
                        } else {
                            self.i += 1;
                        }
                    }
                    "ROUTE" => self.i += 4,
                    "PROFILE" | "COMPONENT" | "UNIT" | "META" | "IMPORT" | "EXPORT" => {
                        // X3D header statements: keyword plus one to three tokens on the line
                        self.i += 1;
                        let n = match w.as_str() {
                            "UNIT" => 3,
                            "META" => 2,
                            "IMPORT" | "EXPORT" => 3,
                            _ => 1,
                        };
                        self.i += n;
                    }
                    _ => match self.node() {
                        Some(n) => out.push(n),
                        None => self.i += 1,
                    },
                },
                _ => self.i += 1,
            }
        }
        out
    }
    fn node(&mut self) -> Option<Rc<Node>> {
        let Some(Tok::Word(w)) = self.peek().cloned() else { return None };
        match w.as_str() {
            "USE" => {
                self.i += 1;
                let Some(Tok::Word(n)) = self.next() else { return None };
                self.defs.get(&n).cloned()
            }
            "NULL" => {
                self.i += 1;
                None
            }
            "DEF" => {
                self.i += 1;
                let Some(Tok::Word(name)) = self.next() else { return None };
                let n = self.body(Some(name.clone()))?;
                self.defs.insert(name, n.clone());
                Some(n)
            }
            _ => self.body(None),
        }
    }
    fn body(&mut self, def: Option<String>) -> Option<Rc<Node>> {
        let Some(Tok::Word(ty)) = self.next() else { return None };
        if self.peek() != Some(&Tok::Open) {
            return None;
        }
        self.i += 1;
        self.depth += 1;
        let mut fields = Vec::new();
        if self.depth > 200 {
            self.i -= 1;
            self.skip_balanced(Tok::Open, Tok::Close);
            self.depth -= 1;
            return Some(Rc::new(Node { ty, def, fields }));
        }
        let script = ty == "Script";
        loop {
            match self.peek().cloned() {
                None => break,
                Some(Tok::Close) => {
                    self.i += 1;
                    break;
                }
                Some(Tok::Word(f)) => {
                    self.i += 1;
                    match f.as_str() {
                        "ROUTE" => self.i += 3,
                        // interface declarations inside Script / proto instances
                        "eventIn" | "eventOut" | "inputOnly" | "outputOnly" => self.i += 2,
                        "field" | "exposedField" | "initializeOnly" | "inputOutput" if script => {
                            self.i += 2;
                            self.value();
                        }
                        _ => {
                            // IS mapping inside PROTO bodies
                            if self.peek() == Some(&Tok::Word("IS".into())) {
                                self.i += 2;
                                continue;
                            }
                            if let Some(v) = self.value() {
                                fields.push((f, v));
                            }
                        }
                    }
                }
                Some(_) => self.i += 1,
            }
        }
        self.depth -= 1;
        Some(Rc::new(Node { ty, def, fields }))
    }
    fn value(&mut self) -> Option<Val> {
        match self.peek().cloned()? {
            Tok::Num(_) => {
                let mut v = Vec::new();
                while let Some(Tok::Num(x)) = self.peek() {
                    v.push(*x);
                    self.i += 1;
                }
                Some(Val::Nums(v))
            }
            Tok::Str(_) => {
                let mut v = Vec::new();
                while let Some(Tok::Str(s)) = self.peek() {
                    v.push(s.clone());
                    self.i += 1;
                }
                Some(Val::Strs(v))
            }
            Tok::LBr => {
                self.i += 1;
                let mut nums = Vec::new();
                let mut nodes = Vec::new();
                let mut strs = Vec::new();
                let mut words = Vec::new();
                loop {
                    match self.peek().cloned() {
                        None => break,
                        Some(Tok::RBr) => {
                            self.i += 1;
                            break;
                        }
                        Some(Tok::Num(x)) => {
                            nums.push(x);
                            self.i += 1;
                        }
                        Some(Tok::Str(s)) => {
                            strs.push(s);
                            self.i += 1;
                        }
                        Some(Tok::Word(w)) => {
                            if matches!(w.as_str(), "DEF" | "USE") || self.peek2() == Some(&Tok::Open) {
                                if let Some(n) = self.node() {
                                    nodes.push(n);
                                }
                            } else {
                                words.push(w);
                                self.i += 1;
                            }
                        }
                        Some(_) => self.i += 1,
                    }
                }
                Some(if !nodes.is_empty() {
                    Val::Nodes(nodes)
                } else if !strs.is_empty() {
                    Val::Strs(strs)
                } else if !words.is_empty() && nums.is_empty() {
                    Val::Words(words)
                } else {
                    Val::Nums(nums)
                })
            }
            Tok::Word(w) => {
                if matches!(w.as_str(), "DEF" | "USE") || self.peek2() == Some(&Tok::Open) {
                    return Some(match self.node() {
                        Some(n) => Val::Node(n),
                        None => Val::Nodes(Vec::new()),
                    });
                }
                if w == "NULL" {
                    self.i += 1;
                    return Some(Val::Nodes(Vec::new()));
                }
                self.i += 1;
                Some(Val::Words(vec![w]))
            }
            _ => {
                self.i += 1;
                None
            }
        }
    }
}

struct Out {
    meshes: Vec<SceneMesh>,
    comps: Vec<(String, [f32; 16], usize)>,
    cache: HashMap<*const Node, usize>,
    edges: bool,
    inline: usize,
    tris: usize,
}

fn mat_translate(t: &[f64]) -> [f32; 16] {
    let mut m = crate::IDENTITY;
    if t.len() >= 3 {
        m[12] = t[0] as f32;
        m[13] = t[1] as f32;
        m[14] = t[2] as f32;
    }
    m
}

fn mat_rotate(r: &[f64]) -> [f32; 16] {
    if r.len() < 4 {
        return crate::IDENTITY;
    }
    let (x, y, z, a) = (r[0], r[1], r[2], r[3]);
    let l = (x * x + y * y + z * z).sqrt();
    if l < 1e-12 || a == 0.0 {
        return crate::IDENTITY;
    }
    let (x, y, z) = (x / l, y / l, z / l);
    let (s, c) = a.sin_cos();
    let t = 1.0 - c;
    let m = [
        t * x * x + c,
        t * x * y + s * z,
        t * x * z - s * y,
        0.0,
        t * x * y - s * z,
        t * y * y + c,
        t * y * z + s * x,
        0.0,
        t * x * z + s * y,
        t * y * z - s * x,
        t * z * z + c,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ];
    m.map(|v| v as f32)
}

fn mat_scale(s: &[f64]) -> [f32; 16] {
    let mut m = crate::IDENTITY;
    if s.len() >= 3 {
        m[0] = s[0] as f32;
        m[5] = s[1] as f32;
        m[10] = s[2] as f32;
    }
    m
}

fn neg(v: &[f64]) -> Vec<f64> {
    v.iter().map(|x| -x).collect()
}

/// VRML Transform: T × C × R × SR × S × -SR × -C
fn transform_matrix(n: &Node) -> [f32; 16] {
    let t = mat_translate(n.nums("translation"));
    let c = n.nums("center");
    let r = mat_rotate(n.nums("rotation"));
    let s = mat_scale(n.nums("scale"));
    let so = n.nums("scaleOrientation");
    let mut m = mat_mul(&t, &mat_translate(c));
    m = mat_mul(&m, &r);
    if so.len() >= 4 && so[3] != 0.0 {
        let mut inv = so.to_vec();
        inv[3] = -inv[3];
        m = mat_mul(&m, &mat_rotate(so));
        m = mat_mul(&m, &s);
        m = mat_mul(&m, &mat_rotate(&inv));
    } else {
        m = mat_mul(&m, &s);
    }
    mat_mul(&m, &mat_translate(&neg(c)))
}

fn polygons_to_tris(index: &[f64], nv: usize, ccw: bool) -> Vec<[u32; 3]> {
    let mut tris = Vec::new();
    let mut poly: Vec<u32> = Vec::new();
    let mut flush = |poly: &mut Vec<u32>| {
        for k in 1..poly.len().saturating_sub(1) {
            let t = if ccw { [poly[0], poly[k], poly[k + 1]] } else { [poly[0], poly[k + 1], poly[k]] };
            if t.iter().all(|&v| (v as usize) < nv) && t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
                tris.push(t);
            }
        }
        poly.clear();
    };
    for &i in index {
        if i < 0.0 {
            flush(&mut poly);
        } else {
            poly.push(i as u32);
        }
    }
    flush(&mut poly);
    tris
}

fn points(n: Option<&Rc<Node>>) -> Vec<[f32; 3]> {
    n.map(|c| c.nums("point").chunks_exact(3).map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect()).unwrap_or_default()
}

fn ccw(n: &Node) -> bool {
    n.word("ccw") != Some("FALSE")
}

fn geometry(g: &Node) -> Option<(Vec<[f32; 3]>, Vec<[u32; 3]>)> {
    let seg = 32usize;
    match g.ty.as_str() {
        "IndexedFaceSet" => {
            let p = points(g.node("coord"));
            let t = polygons_to_tris(g.nums("coordIndex"), p.len(), ccw(g));
            Some((p, t))
        }
        "IndexedTriangleSet" => {
            let p = points(g.node("coord"));
            let ix = g.nums("index");
            let t = ix.chunks_exact(3).map(|c| [c[0] as u32, c[1] as u32, c[2] as u32]).filter(|t| t.iter().all(|&v| (v as usize) < p.len())).map(|t| if ccw(g) { t } else { [t[0], t[2], t[1]] }).collect();
            Some((p, t))
        }
        "TriangleSet" => {
            let p = points(g.node("coord"));
            let t = (0..p.len() / 3).map(|k| (k * 3) as u32).map(|b| if ccw(g) { [b, b + 1, b + 2] } else { [b, b + 2, b + 1] }).collect();
            Some((p, t))
        }
        "Box" => {
            let s = g.nums("size");
            let (x, y, z) = if s.len() >= 3 { (s[0] as f32 / 2.0, s[1] as f32 / 2.0, s[2] as f32 / 2.0) } else { (1.0, 1.0, 1.0) };
            let p = vec![[-x, -y, -z], [x, -y, -z], [x, y, -z], [-x, y, -z], [-x, -y, z], [x, -y, z], [x, y, z], [-x, y, z]];
            let q = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]];
            let t = q.iter().flat_map(|f| [[f[0], f[1], f[2]], [f[0], f[2], f[3]]]).collect();
            Some((p, t))
        }
        "Sphere" => {
            let r = g.nums("radius").first().copied().unwrap_or(1.0) as f32;
            let (nu, nv) = (seg, seg / 2);
            let mut p = Vec::new();
            for j in 0..=nv {
                let th = std::f32::consts::PI * j as f32 / nv as f32;
                for i in 0..nu {
                    let ph = std::f32::consts::TAU * i as f32 / nu as f32;
                    p.push([r * th.sin() * ph.sin(), r * th.cos(), r * th.sin() * ph.cos()]);
                }
            }
            let mut t = Vec::new();
            for j in 0..nv as u32 {
                for i in 0..nu as u32 {
                    let a = j * nu as u32 + i;
                    let b = j * nu as u32 + (i + 1) % nu as u32;
                    let c = a + nu as u32;
                    let d = b + nu as u32;
                    t.push([a, c, d]);
                    t.push([a, d, b]);
                }
            }
            Some((p, t))
        }
        "Cylinder" | "Cone" => {
            let cone = g.ty == "Cone";
            let r = g.nums(if cone { "bottomRadius" } else { "radius" }).first().copied().unwrap_or(1.0) as f32;
            let h = g.nums("height").first().copied().unwrap_or(2.0) as f32 / 2.0;
            let rt = if cone { 0.0 } else { r };
            let mut p = Vec::new();
            for i in 0..seg {
                let a = std::f32::consts::TAU * i as f32 / seg as f32;
                p.push([r * a.sin(), -h, r * a.cos()]);
                p.push([rt * a.sin(), h, rt * a.cos()]);
            }
            let (cb, ct) = (p.len() as u32, p.len() as u32 + 1);
            p.push([0.0, -h, 0.0]);
            p.push([0.0, h, 0.0]);
            let mut t = Vec::new();
            for i in 0..seg as u32 {
                let j = (i + 1) % seg as u32;
                let (b0, t0, b1, t1) = (2 * i, 2 * i + 1, 2 * j, 2 * j + 1);
                t.push([b0, b1, t1]);
                if !cone {
                    t.push([b0, t1, t0]);
                    t.push([ct, t0, t1]);
                } else {
                    t.push([b0, t1, t0]);
                }
                t.push([cb, b1, b0]);
            }
            Some((p, t))
        }
        _ => None,
    }
}

fn color_of(shape: &Node) -> Option<[f32; 4]> {
    let m = shape.node("appearance")?.node("material")?;
    let d = m.nums("diffuseColor");
    let tr = m.nums("transparency").first().copied().unwrap_or(0.0) as f32;
    if d.len() >= 3 {
        Some([d[0] as f32, d[1] as f32, d[2] as f32, (1.0 - tr).clamp(0.05, 1.0)])
    } else {
        Some([0.8, 0.8, 0.8, (1.0 - tr).clamp(0.05, 1.0)])
    }
}

fn walk(n: &Rc<Node>, world: [f32; 16], label: &str, out: &mut Out, depth: usize) {
    if depth > 100 || out.comps.len() > 200_000 {
        return;
    }
    let label = n.def.as_deref().unwrap_or(label);
    match n.ty.as_str() {
        "Transform" => {
            let w = mat_mul(&world, &transform_matrix(n));
            for c in n.nodes("children") {
                walk(&c, w, label, out, depth + 1);
            }
        }
        "Switch" => {
            let which = n.nums("whichChoice").first().copied().unwrap_or(-1.0);
            let ch = if n.get("choice").is_some() { n.nodes("choice") } else { n.nodes("children") };
            if which >= 0.0 {
                if let Some(c) = ch.get(which as usize) {
                    walk(c, world, label, out, depth + 1);
                }
            }
        }
        "LOD" => {
            let lv = if n.get("level").is_some() { n.nodes("level") } else { n.nodes("children") };
            if let Some(c) = lv.first() {
                walk(c, world, label, out, depth + 1);
            }
        }
        "Inline" => out.inline += 1,
        "Shape" => {
            let Some(g) = n.node("geometry") else { return };
            let key = Rc::as_ptr(g);
            let mi = if let Some(&mi) = out.cache.get(&key) {
                mi
            } else {
                let Some((p, t)) = geometry(g) else { return };
                if t.is_empty() {
                    return;
                }
                out.tris += t.len();
                let mesh = sw_format::mesh::build_from_triangles(&p, &t, 30.0, out.edges);
                out.meshes.push(SceneMesh { name: g.def.clone().unwrap_or_default(), mesh, source: MeshSource::Embedded, color: color_of(n) });
                out.cache.insert(key, out.meshes.len() - 1);
                out.meshes.len() - 1
            };
            out.comps.push((label.to_string(), world, mi));
        }
        _ => {
            // Group, Anchor, Billboard, Collision, StaticGroup, CADAssembly, CADPart… and unknown
            // grouping nodes: walk any child nodes
            for (_, v) in &n.fields {
                let kids: Vec<Rc<Node>> = match v {
                    Val::Node(c) => vec![c.clone()],
                    Val::Nodes(c) => c.clone(),
                    _ => continue,
                };
                for c in kids {
                    if matches!(c.ty.as_str(), "Appearance" | "Material" | "Coordinate" | "Normal" | "TextureCoordinate" | "Color" | "ImageTexture") {
                        continue;
                    }
                    walk(&c, world, label, out, depth + 1);
                }
            }
        }
    }
}

pub fn load(path: &Path, bytes: &[u8]) -> Result<Loaded, String> {
    let t0 = Instant::now();
    let inflated;
    let bytes = if bytes.starts_with(&[0x1f, 0x8b]) {
        use std::io::Read;
        let mut v = Vec::new();
        flate2::read::MultiGzDecoder::new(bytes).read_to_end(&mut v).map_err(|e| format!("gzip : {e}"))?;
        inflated = v;
        &inflated[..]
    } else {
        bytes
    };
    let text = String::from_utf8_lossy(bytes);
    let head = text.lines().next().unwrap_or("");
    let x3d = head.starts_with("#X3D");
    if head.starts_with("#VRML V1.0") {
        return Err("VRML 1.0 n'est pas géré (seulement VRML 97 / 2.0 et X3D)".into());
    }
    if !x3d && !head.starts_with("#VRML") {
        return Err("en-tête « #VRML V2.0 » ou « #X3D » absent".into());
    }
    let mut p = Parser { t: tokenize(&text), i: 0, defs: HashMap::new(), depth: 0 };
    let roots = p.statements(false);
    let mut out = Out { meshes: Vec::new(), comps: Vec::new(), cache: HashMap::new(), edges: true, inline: 0, tris: 0 };
    let name = stem(path);
    for r in &roots {
        walk(r, crate::IDENTITY, &name, &mut out, 0);
    }
    let mut s = empty_scene(path, if x3d { "X3D" } else { "VRML" });
    s.meshes = out.meshes;
    if out.comps.len() == 1 {
        let (_, w, m) = out.comps[0];
        let mut c = component(&name, 0, None, false, Some(m));
        c.world = w;
        s.components.push(c);
    } else if !out.comps.is_empty() {
        s.components.push(component(&name, 0, None, true, None));
        let mut seen: HashMap<String, usize> = HashMap::new();
        for (label, w, m) in out.comps {
            let k = seen.entry(label.clone()).or_insert(0);
            *k += 1;
            let nm = if *k > 1 { format!("{label} ({k})") } else { label };
            let mut c = component(&nm, 1, Some(0), false, Some(m));
            c.world = w;
            s.components.push(c);
        }
    }
    s.up_z = false;
    if out.inline > 0 {
        s.warnings.push(format!("{} fichier(s) Inline référencé(s) non chargé(s)", out.inline));
    }
    if s.meshes.is_empty() {
        s.warnings.push("Aucune géométrie affichable trouvée dans le fichier".into());
    }
    s.timings.mesh = t0.elapsed();
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn def_use_transform_and_primitives() {
        let src = r#"#VRML V2.0 utf8
# test
PROTO Foo [ field SFFloat a 1 ] { Group {} }
DEF Root Transform {
  translation 1 0 0
  children [
    DEF S Shape {
      appearance Appearance { material Material { diffuseColor 1 0 0 transparency 0.5 } }
      geometry IndexedFaceSet { coord Coordinate { point [ 0 0 0, 1 0 0, 1 1 0, 0 1 0 ] } coordIndex [ 0 1 2 3 -1 ] }
    }
    Transform { translation 0 5 0 children USE S }
    Shape { geometry Box { size 2 2 2 } }
  ]
}
ROUTE A.b TO C.d
"#;
        let s = load(Path::new("t.wrl"), src.as_bytes()).unwrap();
        assert_eq!(s.meshes.len(), 2, "IFS shared through USE + box");
        assert_eq!(s.components.len(), 4);
        assert_eq!(s.meshes[0].mesh.triangle_count(), 2);
        assert_eq!(s.meshes[1].mesh.triangle_count(), 12);
        let c = s.meshes[0].color.unwrap();
        assert!((c[0] - 1.0).abs() < 1e-6 && (c[3] - 0.5).abs() < 1e-6);
        assert!((s.components[2].world[13] - 5.0).abs() < 1e-6 && (s.components[2].world[12] - 1.0).abs() < 1e-6);
    }
    #[test]
    fn x3d_classic() {
        let src = "#X3D V3.0 utf8\nPROFILE Immersive\nShape { geometry IndexedTriangleSet { index [0 1 2] coord Coordinate { point [0 0 0 1 0 0 0 1 0] } } }\n";
        let s = load(Path::new("t.x3dv"), src.as_bytes()).unwrap();
        assert_eq!(s.meshes[0].mesh.triangle_count(), 1);
    }
}
