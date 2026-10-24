//! Viewer-only assembly manipulation: moved / rotated components, exploded view and opacity.
//! Nothing here touches the file; it only changes the display matrices of a document.

use glam::{Mat4, Vec3};
use sw_format::Scene;

pub struct Assembly {
    /// Display matrix of each component as loaded.
    pub base: Vec<Mat4>,
    /// User transform per component (world space, applied to its whole subtree).
    pub user: Vec<Mat4>,
    /// World-space offset of each component (relative to its parent) at 100 % explosion.
    explode_dir: Vec<Vec3>,
    /// Hierarchy level used to stage the explosion (sub-assemblies separate first).
    level: Vec<u32>,
    levels: u32,
    parent: Vec<Option<usize>>,
    /// Bounding-box centre of each component subtree (as loaded).
    pub center0: Vec<Vec3>,
    /// Opacity per component (multiplied down the hierarchy).
    pub opacity: Vec<f32>,
    pub global_opacity: f32,
    /// Show every non-selected component faded.
    pub others_transparent: bool,
    /// Current and target explosion (0..1); the current value animates towards the goal.
    pub explode: f32,
    pub explode_goal: f32,
    pub explode_lines: bool,
}

/// Faded opacity of the other components when "transparency of the others" is on.
pub const OTHERS_OPACITY: f32 = 0.18;

impl Assembly {
    pub fn empty() -> Assembly {
        Assembly {
            base: vec![],
            user: vec![],
            explode_dir: vec![],
            level: vec![],
            levels: 1,
            parent: vec![],
            center0: vec![],
            opacity: vec![],
            global_opacity: 1.0,
            others_transparent: false,
            explode: 0.0,
            explode_goal: 0.0,
            explode_lines: true,
        }
    }

    pub fn new(scene: &Scene, base: Vec<Mat4>) -> Assembly {
        let n = scene.components.len();
        let parent: Vec<Option<usize>> = scene.components.iter().map(|c| c.parent.filter(|&p| p < n)).collect();
        // subtree bounding boxes (children come after their parent: accumulate backwards)
        let mut lo = vec![Vec3::splat(f32::INFINITY); n];
        let mut hi = vec![Vec3::splat(f32::NEG_INFINITY); n];
        for (i, c) in scene.components.iter().enumerate() {
            let Some(m) = c.mesh.and_then(|m| scene.meshes.get(m)) else { continue };
            let m = &m.mesh;
            if m.is_empty() || c.suppressed {
                continue;
            }
            let (a, b) = (Vec3::from(m.bbox_min), Vec3::from(m.bbox_max));
            for k in 0..8 {
                let p = Vec3::new(if k & 1 == 0 { a.x } else { b.x }, if k & 2 == 0 { a.y } else { b.y }, if k & 4 == 0 { a.z } else { b.z });
                let q = base[i].transform_point3(p);
                lo[i] = lo[i].min(q);
                hi[i] = hi[i].max(q);
            }
        }
        for i in (0..n).rev() {
            if let Some(p) = parent[i] {
                if lo[i].x <= hi[i].x {
                    lo[p] = lo[p].min(lo[i]);
                    hi[p] = hi[p].max(hi[i]);
                }
            }
        }
        let valid = |i: usize| lo[i].x <= hi[i].x && lo[i].is_finite() && hi[i].is_finite();
        let center0: Vec<Vec3> = (0..n).map(|i| if valid(i) { (lo[i] + hi[i]) * 0.5 } else { base[i].w_axis.truncate() }).collect();
        // whole-scene box (virtual root for several top-level components)
        let (mut slo, mut shi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for i in (0..n).filter(|&i| parent[i].is_none() && valid(i)) {
            slo = slo.min(lo[i]);
            shi = shi.max(hi[i]);
        }
        let scene_ok = slo.x <= shi.x;
        // siblings (with geometry) of each group
        let mut kids: Vec<Vec<usize>> = vec![vec![]; n + 1];
        for i in 0..n {
            if valid(i) {
                kids[parent[i].unwrap_or(n)].push(i);
            }
        }
        let mut explode_dir = vec![Vec3::ZERO; n];
        let mut level = vec![0u32; n];
        for i in 0..n {
            level[i] = parent[i].map(|p| level[p] + 1).unwrap_or(0);
        }
        for (g, ks) in kids.iter().enumerate() {
            if ks.len() < 2 {
                continue;
            }
            let (pc, pr) = if g == n {
                if !scene_ok {
                    continue;
                }
                ((slo + shi) * 0.5, (shi - slo).length() * 0.5)
            } else {
                (center0[g], (hi[g] - lo[g]).length() * 0.5)
            };
            for (k, &i) in ks.iter().enumerate() {
                let d = center0[i] - pc;
                let len = d.length();
                let dir = if len > pr * 1e-3 {
                    d / len
                } else {
                    // part sitting at the group centre: spread on a golden-angle spiral
                    let t = (k as f32 + 0.5) / ks.len() as f32;
                    let phi = k as f32 * 2.399_963;
                    let z = 1.0 - 2.0 * t;
                    let r = (1.0 - z * z).max(0.0).sqrt();
                    Vec3::new(r * phi.cos(), z, r * phi.sin())
                };
                explode_dir[i] = dir * (len * 1.1 + pr * 0.45);
            }
        }
        let levels = level.iter().copied().max().unwrap_or(0) + 1;
        Assembly {
            user: vec![Mat4::IDENTITY; n],
            explode_dir,
            level,
            levels,
            parent,
            center0,
            opacity: vec![1.0; n],
            base,
            ..Assembly::empty()
        }
    }

    /// Explosion factor of a hierarchy level for the overall factor `t` (outer levels move first).
    fn level_factor(&self, lvl: u32, t: f32) -> f32 {
        let l = self.levels.max(1) as f32;
        let x = (t * l - lvl as f32).clamp(0.0, 1.0);
        x * x * (3.0 - 2.0 * x)
    }

    /// Display matrices with the user transforms and the current explosion applied, and the
    /// accumulated transform of each component (display = acc * base).
    pub fn compute(&self) -> (Vec<Mat4>, Vec<Mat4>) {
        let n = self.base.len();
        let mut acc = vec![Mat4::IDENTITY; n];
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let pa = self.parent[i].map(|p| acc[p]).unwrap_or(Mat4::IDENTITY);
            let e = self.explode_dir[i] * self.level_factor(self.level[i], self.explode);
            acc[i] = pa * self.user[i] * Mat4::from_translation(e);
            out.push(acc[i] * self.base[i]);
        }
        (out, acc)
    }

    pub fn is_modified(&self) -> bool {
        self.user.iter().any(|m| *m != Mat4::IDENTITY) || self.explode_goal > 0.0 || self.explode > 0.0
    }

    pub fn is_moved(&self, i: usize) -> bool {
        self.user.get(i).is_some_and(|m| *m != Mat4::IDENTITY)
    }

    /// Apply a world-space transform to the subtree of `i` (on top of what it already has).
    pub fn apply_world(&mut self, acc: &[Mat4], i: usize, w: Mat4) {
        // user[i] sits between the parent's accumulated transform P and the rest:
        // new display = P * U' * … with P * U' = W * P * U  =>  U' = P⁻¹ W P U
        let p = self.parent[i].map(|p| acc[p]).unwrap_or(Mat4::IDENTITY);
        self.user[i] = p.inverse() * w * p * self.user[i];
    }

    pub fn reset(&mut self, i: usize) {
        if let Some(u) = self.user.get_mut(i) {
            *u = Mat4::IDENTITY;
        }
    }

    pub fn reset_all(&mut self) {
        self.user.iter_mut().for_each(|u| *u = Mat4::IDENTITY);
        self.explode_goal = 0.0;
    }

    /// Advance the explosion animation. Returns true while it is moving.
    pub fn tick(&mut self, dt: f32) -> bool {
        let d = self.explode_goal - self.explode;
        if d.abs() < 1e-4 {
            if self.explode != self.explode_goal {
                self.explode = self.explode_goal;
                return true;
            }
            return false;
        }
        let step = (dt * 2.2).min(1.0);
        self.explode += d.signum() * (d.abs() * step * 3.0).max(dt * 0.4).min(d.abs());
        true
    }

    /// Own opacity of `i` combined down the hierarchy (and the global slider).
    pub fn chain_opacity(&self, i: usize) -> f32 {
        let mut o = self.opacity.get(i).copied().unwrap_or(1.0);
        let mut p = self.parent.get(i).copied().flatten();
        while let Some(k) = p {
            o *= self.opacity.get(k).copied().unwrap_or(1.0);
            p = self.parent[k];
        }
        (o * self.global_opacity).clamp(0.0, 1.0)
    }

    /// Explosion lines: from each moved part's original centre to where it is now.
    pub fn lines(&self, acc: &[Mat4], with_mesh: &[bool]) -> Vec<(Vec3, Vec3)> {
        if !self.explode_lines || self.explode <= 1e-3 {
            return vec![];
        }
        let mut v = Vec::new();
        for i in 0..self.base.len().min(acc.len()) {
            if !with_mesh.get(i).copied().unwrap_or(false) {
                continue;
            }
            let a = self.center0[i];
            let b = acc[i].transform_point3(a);
            if (b - a).length_squared() > 1e-12 {
                v.push((a, b));
            }
        }
        v
    }
}
