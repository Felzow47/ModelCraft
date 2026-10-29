//! Hidden lines of the views placed on Revit sheets: the faces the view sees are drawn into a
//! depth buffer, then each model edge keeps only the parts that no face hides. View
//! coordinates (u right, v up, w towards the viewer, feet), orthographic projection.

type V3 = [f64; 3];

/// Depth buffer over a rectangle of the view plane.
pub(crate) struct Hlr {
    x0: f64,
    y0: f64,
    /// pixel size (feet)
    px: f64,
    w: usize,
    h: usize,
    /// nearest depth seen at each pixel (larger = closer), -inf where nothing
    z: Vec<f32>,
    pub(crate) faces: usize,
}

impl Hlr {
    /// Buffer over `r` = [umin, vmin, umax, vmax], at most `max_px` pixels on its longer side.
    pub(crate) fn new(r: [f64; 4], max_px: usize) -> Option<Hlr> {
        let (dw, dh) = (r[2] - r[0], r[3] - r[1]);
        if !(dw > 1e-9 && dh > 1e-9 && dw.is_finite() && dh.is_finite()) {
            return None;
        }
        let px = dw.max(dh) / max_px.max(16) as f64;
        let w = (dw / px).ceil() as usize + 3;
        let h = (dh / px).ceil() as usize + 3;
        Some(Hlr {
            x0: r[0] - px,
            y0: r[1] - px,
            px,
            w,
            h,
            z: vec![f32::NEG_INFINITY; w * h],
            faces: 0,
        })
    }

    pub(crate) fn pixel(&self) -> f64 {
        self.px
    }

    /// Draws a triangle (view coordinates).
    pub(crate) fn tri(&mut self, a: V3, b: V3, c: V3) {
        let s = |p: V3| [(p[0] - self.x0) / self.px, (p[1] - self.y0) / self.px, p[2]];
        let (a, b, c) = (s(a), s(b), s(c));
        let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if area.abs() < 1e-9 || !area.is_finite() {
            return; // seen edge-on
        }
        self.faces += 1;
        let xmin = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
        let ymin = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
        let xmax = (a[0].max(b[0]).max(c[0]).ceil() as isize).min(self.w as isize - 1);
        let ymax = (a[1].max(b[1]).max(c[1]).ceil() as isize).min(self.h as isize - 1);
        if xmax < 0 || ymax < 0 {
            return;
        }
        let inv = 1.0 / area;
        for y in ymin..=ymax as usize {
            let py = y as f64 + 0.5;
            for x in xmin..=xmax as usize {
                let px = x as f64 + 0.5;
                let w0 = ((b[0] - px) * (c[1] - py) - (b[1] - py) * (c[0] - px)) * inv;
                let w1 = ((c[0] - px) * (a[1] - py) - (c[1] - py) * (a[0] - px)) * inv;
                let w2 = 1.0 - w0 - w1;
                if w0 < -1e-9 || w1 < -1e-9 || w2 < -1e-9 {
                    continue;
                }
                let d = (w0 * a[2] + w1 * b[2] + w2 * c[2]) as f32;
                let z = &mut self.z[y * self.w + x];
                if d > *z {
                    *z = d;
                }
            }
        }
    }

    /// Nearest face depth around a point: the farthest of the 3×3 pixels about it, so that
    /// an edge on the border of the face in front of it is not hidden by that face.
    fn depth_at(&self, u: f64, v: f64) -> f32 {
        let x = ((u - self.x0) / self.px).floor() as isize;
        let y = ((v - self.y0) / self.px).floor() as isize;
        let mut d = f32::INFINITY;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (xx, yy) = (x + dx, y + dy);
                if xx < 0 || yy < 0 || xx >= self.w as isize || yy >= self.h as isize {
                    return f32::NEG_INFINITY;
                }
                d = d.min(self.z[yy as usize * self.w + xx as usize]);
            }
        }
        d
    }

    /// The parts of segment a-b that no face hides (view coordinates).
    pub(crate) fn visible(&self, a: V3, b: V3, tol: f64) -> Vec<(V3, V3)> {
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let n = ((len / (self.px * 0.5)).ceil() as usize).clamp(1, 20_000);
        let at = |t: f64| -> V3 { std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t) };
        let seen = |t: f64| {
            let p = at(t);
            p[2] as f32 >= self.depth_at(p[0], p[1]) - tol as f32
        };
        let mut out = Vec::new();
        let mut start: Option<f64> = None;
        for i in 0..=n {
            let t = i as f64 / n as f64;
            let s = seen(t);
            match (s, start) {
                (true, None) => start = Some(if i == 0 { 0.0 } else { t - 0.5 / n as f64 }),
                (false, Some(t0)) => {
                    let t1 = t - 0.5 / n as f64;
                    if t1 > t0 {
                        out.push((at(t0), at(t1)));
                    }
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(t0) = start {
            out.push((at(t0), b));
        }
        out
    }
}

/// Sutherland-Hodgman: the part of the convex polygon `poly` where `f(p) >= 0` (`f` affine).
pub(crate) fn clip_poly(poly: &[V3], f: impl Fn(V3) -> f64) -> Vec<V3> {
    let mut out = Vec::with_capacity(poly.len() + 2);
    for i in 0..poly.len() {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        let (fp, fq) = (f(p), f(q));
        if fp >= 0.0 {
            out.push(p);
        }
        if (fp >= 0.0) != (fq >= 0.0) {
            let t = fp / (fp - fq);
            out.push(std::array::from_fn(|k| p[k] + (q[k] - p[k]) * t));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_hides_line_behind() {
        let mut h = Hlr::new([0.0, 0.0, 10.0, 10.0], 200).unwrap();
        // square at depth 1 covering x 2..6
        h.tri([2.0, 0.0, 1.0], [6.0, 0.0, 1.0], [6.0, 10.0, 1.0]);
        h.tri([2.0, 0.0, 1.0], [6.0, 10.0, 1.0], [2.0, 10.0, 1.0]);
        // a line behind it, across
        let v = h.visible([0.0, 5.0, 0.0], [10.0, 5.0, 0.0], 0.01);
        assert_eq!(v.len(), 2);
        assert!((v[0].1[0] - 2.0).abs() < 0.2 && (v[1].0[0] - 6.0).abs() < 0.2);
        // a line in front stays whole
        assert_eq!(h.visible([0.0, 5.0, 2.0], [10.0, 5.0, 2.0], 0.01).len(), 1);
        // the square's own border stays
        assert_eq!(h.visible([2.0, 1.0, 1.0], [2.0, 9.0, 1.0], 0.01).len(), 1);
    }

    #[test]
    fn clip_triangle() {
        let t = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]];
        let c = clip_poly(&t, |p| 1.0 - p[0]);
        assert_eq!(c.len(), 4);
    }
}
