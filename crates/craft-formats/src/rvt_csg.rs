//! Boolean subtraction of closed triangle meshes (BSP trees, after the csg.js method): the
//! void solids of a Revit family cut its solids.

type V3 = [f64; 3];

const EPS: f64 = 1e-7;

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn lerp(a: V3, b: V3, t: f64) -> V3 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

#[derive(Clone, Copy)]
struct Plane {
    n: V3,
    w: f64,
}

impl Plane {
    fn of(a: V3, b: V3, c: V3) -> Option<Plane> {
        let n = cross(sub(b, a), sub(c, a));
        let l = dot(n, n).sqrt();
        (l > 1e-12).then(|| {
            let n = [n[0] / l, n[1] / l, n[2] / l];
            Plane { n, w: dot(n, a) }
        })
    }
    fn flip(&mut self) {
        self.n = [-self.n[0], -self.n[1], -self.n[2]];
        self.w = -self.w;
    }
}

#[derive(Clone)]
struct Poly {
    v: Vec<V3>,
    plane: Plane,
}

impl Poly {
    fn flip(&mut self) {
        self.v.reverse();
        self.plane.flip();
    }
}

const COPLANAR: u8 = 0;
const FRONT: u8 = 1;
const BACK: u8 = 2;

/// Splits `p` by `pl` into the four lists.
fn split(pl: &Plane, p: &Poly, cf: &mut Vec<Poly>, cb: &mut Vec<Poly>, f: &mut Vec<Poly>, b: &mut Vec<Poly>) {
    let mut kind = 0u8;
    let types: Vec<u8> = p
        .v
        .iter()
        .map(|v| {
            let t = dot(pl.n, *v) - pl.w;
            let k = if t < -EPS { BACK } else if t > EPS { FRONT } else { COPLANAR };
            kind |= k;
            k
        })
        .collect();
    match kind {
        COPLANAR => {
            if dot(pl.n, p.plane.n) > 0.0 {
                cf.push(p.clone())
            } else {
                cb.push(p.clone())
            }
        }
        FRONT => f.push(p.clone()),
        BACK => b.push(p.clone()),
        _ => {
            let (mut fv, mut bv) = (Vec::new(), Vec::new());
            let n = p.v.len();
            for i in 0..n {
                let j = (i + 1) % n;
                let (ti, tj) = (types[i], types[j]);
                let (vi, vj) = (p.v[i], p.v[j]);
                if ti != BACK {
                    fv.push(vi);
                }
                if ti != FRONT {
                    bv.push(vi);
                }
                if (ti | tj) == (FRONT | BACK) {
                    let t = (pl.w - dot(pl.n, vi)) / dot(pl.n, sub(vj, vi));
                    let v = lerp(vi, vj, t);
                    fv.push(v);
                    bv.push(v);
                }
            }
            if fv.len() >= 3 {
                f.push(Poly { v: fv, plane: p.plane });
            }
            if bv.len() >= 3 {
                b.push(Poly { v: bv, plane: p.plane });
            }
        }
    }
}

#[derive(Default)]
struct Node {
    plane: Option<Plane>,
    front: Option<Box<Node>>,
    back: Option<Box<Node>>,
    polys: Vec<Poly>,
}

impl Node {
    fn new(polys: Vec<Poly>) -> Node {
        let mut n = Node::default();
        n.build(polys);
        n
    }
    fn invert(&mut self) {
        for p in &mut self.polys {
            p.flip();
        }
        if let Some(pl) = &mut self.plane {
            pl.flip();
        }
        if let Some(f) = &mut self.front {
            f.invert();
        }
        if let Some(b) = &mut self.back {
            b.invert();
        }
        std::mem::swap(&mut self.front, &mut self.back);
    }
    fn clip_polys(&self, polys: Vec<Poly>) -> Vec<Poly> {
        let Some(pl) = &self.plane else { return polys };
        let (mut f, mut b) = (Vec::new(), Vec::new());
        for p in &polys {
            let (mut cf, mut cb) = (Vec::new(), Vec::new());
            split(pl, p, &mut cf, &mut cb, &mut f, &mut b);
            f.extend(cf);
            b.extend(cb);
        }
        let mut f = match &self.front {
            Some(n) => n.clip_polys(f),
            None => f,
        };
        let b = match &self.back {
            Some(n) => n.clip_polys(b),
            None => Vec::new(),
        };
        f.extend(b);
        f
    }
    fn clip_to(&mut self, bsp: &Node) {
        self.polys = bsp.clip_polys(std::mem::take(&mut self.polys));
        if let Some(f) = &mut self.front {
            f.clip_to(bsp);
        }
        if let Some(b) = &mut self.back {
            b.clip_to(bsp);
        }
    }
    fn all(&self, out: &mut Vec<Poly>) {
        out.extend(self.polys.iter().cloned());
        if let Some(f) = &self.front {
            f.all(out);
        }
        if let Some(b) = &self.back {
            b.all(out);
        }
    }
    fn build(&mut self, polys: Vec<Poly>) {
        if polys.is_empty() {
            return;
        }
        let pl = *self.plane.get_or_insert(polys[0].plane);
        let (mut f, mut b) = (Vec::new(), Vec::new());
        for p in &polys {
            let (mut cf, mut cb) = (Vec::new(), Vec::new());
            split(&pl, p, &mut cf, &mut cb, &mut f, &mut b);
            self.polys.extend(cf);
            self.polys.extend(cb);
        }
        if !f.is_empty() {
            self.front.get_or_insert_with(Default::default).build(f);
        }
        if !b.is_empty() {
            self.back.get_or_insert_with(Default::default).build(b);
        }
    }
}

fn polys_of(pos: &[[f32; 3]], tri: &[[u32; 3]]) -> Vec<Poly> {
    tri.iter()
        .filter_map(|t| {
            let v: Vec<V3> = t.iter().map(|&i| pos[i as usize].map(|x| x as f64)).collect();
            Plane::of(v[0], v[1], v[2]).map(|plane| Poly { v, plane })
        })
        .collect()
}

/// Most triangles of either operand: past it the cut is skipped (the BSP grows fast).
pub const MAX_TRIS: usize = 4000;

/// `a` minus `b` (closed meshes, same frame): the triangles of the result, or None when
/// either is too large or the result is empty.
pub fn subtract(a: (&[[f32; 3]], &[[u32; 3]]), b: (&[[f32; 3]], &[[u32; 3]])) -> Option<(Vec<[f32; 3]>, Vec<[u32; 3]>)> {
    if a.1.len() > MAX_TRIS || b.1.len() > MAX_TRIS || a.1.is_empty() || b.1.is_empty() {
        return None;
    }
    let mut na = Node::new(polys_of(a.0, a.1));
    let mut nb = Node::new(polys_of(b.0, b.1));
    na.invert();
    na.clip_to(&nb);
    nb.clip_to(&na);
    nb.invert();
    nb.clip_to(&na);
    nb.invert();
    let mut rest = Vec::new();
    nb.all(&mut rest);
    na.build(rest);
    na.invert();
    let mut polys = Vec::new();
    na.all(&mut polys);
    // back to triangles, vertices shared
    let mut pos: Vec<[f32; 3]> = Vec::new();
    let mut idx: std::collections::HashMap<[i64; 3], u32> = Default::default();
    let mut at = |v: V3| -> u32 {
        let k = v.map(|x| (x * 1e6).round() as i64);
        *idx.entry(k).or_insert_with(|| {
            pos.push(v.map(|x| x as f32));
            (pos.len() - 1) as u32
        })
    };
    let mut tri = Vec::new();
    for p in &polys {
        let ids: Vec<u32> = p.v.iter().map(|v| at(*v)).collect();
        for i in 1..ids.len().saturating_sub(1) {
            let t = [ids[0], ids[i], ids[i + 1]];
            if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
                tri.push(t);
            }
        }
    }
    (!tri.is_empty()).then_some((pos, tri))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(lo: [f32; 3], hi: [f32; 3]) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let p: Vec<[f32; 3]> = (0..8).map(|k| [if k & 1 == 0 { lo[0] } else { hi[0] }, if k & 2 == 0 { lo[1] } else { hi[1] }, if k & 4 == 0 { lo[2] } else { hi[2] }]).collect();
        let t = vec![[0, 2, 3], [0, 3, 1], [4, 5, 7], [4, 7, 6], [0, 1, 5], [0, 5, 4], [2, 6, 7], [2, 7, 3], [0, 4, 6], [0, 6, 2], [1, 3, 7], [1, 7, 5]];
        (p, t)
    }

    fn volume(p: &[[f32; 3]], t: &[[u32; 3]]) -> f64 {
        t.iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| p[i as usize].map(|x| x as f64));
                dot(a, cross(b, c)) / 6.0
            })
            .sum()
    }

    #[test]
    fn cube_minus_corner() {
        let a = cube([0.0; 3], [1.0; 3]);
        let b = cube([0.5; 3], [2.0; 3]);
        let (p, t) = subtract((&a.0, &a.1), (&b.0, &b.1)).unwrap();
        assert!((volume(&p, &t).abs() - 0.875).abs() < 1e-4, "{}", volume(&p, &t));
    }
}
