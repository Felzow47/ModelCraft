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
    /// World-space offset of each piece at 100 % explosion (lines start from where it was).
    world_off: Vec<Vec3>,
    parent: Vec<Option<usize>>,
    /// Bounding-box centre of each component subtree (as loaded).
    pub center0: Vec<Vec3>,
    /// Centre of each component's own mesh (as loaded; subtree centre when it has none).
    pub own_center: Vec<Vec3>,
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
/// At 100 %, a piece's distance from the model centre grows by this factor (1 = doubled)…
pub const EXPLODE_SPREAD: f32 = 1.0;
/// …plus this fraction of the model's half-diagonal, so pieces near the centre still move out.
pub const EXPLODE_PUSH: f32 = 0.6;
/// Up to this many pieces the push is full; above, it shrinks as 1/count.
pub const EXPLODE_PUSH_PIECES: f32 = 6.0;
/// From this many pieces on, the explosion centre is the per-axis median of the piece
/// centres (stray far-away elements, common in Revit models, do not drag it off the model).
const MEDIAN_FROM: usize = 16;

/// Point the pieces fly away from: the mean of their centres (few pieces: with two pieces that
/// share almost the same box centre, like a U-bolt in its bracket, the box centre can sit to
/// one side of both and send them the same way; around the mean they always go opposite ways),
/// the per-axis median for many.
fn explode_centre(c: &[Vec3]) -> Vec3 {
    if c.is_empty() {
        return Vec3::ZERO;
    }
    if c.len() < MEDIAN_FROM {
        return c.iter().sum::<Vec3>() / c.len() as f32;
    }
    let med = |f: fn(&Vec3) -> f32| {
        let mut v: Vec<f32> = c.iter().map(f).collect();
        let m = v.len() / 2;
        let (_, x, _) = v.select_nth_unstable_by(m, f32::total_cmp);
        *x
    };
    Vec3::new(med(|p| p.x), med(|p| p.y), med(|p| p.z))
}

impl Assembly {
    pub fn empty() -> Assembly {
        Assembly {
            base: vec![],
            user: vec![],
            explode_dir: vec![],
            world_off: vec![],
            parent: vec![],
            center0: vec![],
            own_center: vec![],
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
        let parent: Vec<Option<usize>> = scene
            .components
            .iter()
            .map(|c| c.parent.filter(|&p| p < n))
            .collect();
        // subtree bounding boxes (children come after their parent: accumulate backwards)
        let mut lo = vec![Vec3::splat(f32::INFINITY); n];
        let mut hi = vec![Vec3::splat(f32::NEG_INFINITY); n];
        for (i, c) in scene.components.iter().enumerate() {
            let Some(m) = c.mesh.and_then(|m| scene.meshes.get(m)) else {
                continue;
            };
            let m = &m.mesh;
            if m.is_empty() || c.suppressed {
                continue;
            }
            let (a, b) = (Vec3::from(m.bbox_min), Vec3::from(m.bbox_max));
            for k in 0..8 {
                let p = Vec3::new(
                    if k & 1 == 0 { a.x } else { b.x },
                    if k & 2 == 0 { a.y } else { b.y },
                    if k & 4 == 0 { a.z } else { b.z },
                );
                let q = base[i].transform_point3(p);
                lo[i] = lo[i].min(q);
                hi[i] = hi[i].max(q);
            }
        }
        let (own_lo, own_hi) = (lo.clone(), hi.clone());
        for i in (0..n).rev() {
            if let Some(p) = parent[i] {
                if lo[i].x <= hi[i].x {
                    lo[p] = lo[p].min(lo[i]);
                    hi[p] = hi[p].max(hi[i]);
                }
            }
        }
        let valid = |i: usize| lo[i].x <= hi[i].x && lo[i].is_finite() && hi[i].is_finite();
        let center0: Vec<Vec3> = (0..n)
            .map(|i| {
                if valid(i) {
                    (lo[i] + hi[i]) * 0.5
                } else {
                    base[i].w_axis.truncate()
                }
            })
            .collect();
        let own_valid =
            |i: usize| own_lo[i].x <= own_hi[i].x && own_lo[i].is_finite() && own_hi[i].is_finite();
        let own_center: Vec<Vec3> = (0..n)
            .map(|i| {
                if own_valid(i) {
                    (own_lo[i] + own_hi[i]) * 0.5
                } else {
                    center0[i]
                }
            })
            .collect();
        // whole-scene box (virtual root for several top-level components)
        let (mut slo, mut shi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for i in (0..n).filter(|&i| parent[i].is_none() && valid(i)) {
            slo = slo.min(lo[i]);
            shi = shi.max(hi[i]);
        }
        let scene_ok = slo.x <= shi.x;
        // Exploded view: every piece that carries geometry moves straight away from the centre
        // of the whole model, along the line from that centre to its own centre, by an amount
        // proportional to the slider. One direction per piece, linear in t: pieces never turn
        // round or come back as the slider goes up, and parents never add their offset twice.
        let sr = if scene_ok {
            ((shi - slo).length() * 0.5).max(1e-9)
        } else {
            1.0
        };
        let pieces: Vec<usize> = (0..n)
            .filter(|&i| {
                scene.components[i]
                    .mesh
                    .and_then(|m| scene.meshes.get(m))
                    .is_some_and(|m| !m.mesh.is_empty())
                    && !scene.components[i].suppressed
                    && own_valid(i)
            })
            .collect();
        let mut world_off = vec![Vec3::ZERO; n];
        if scene_ok && pieces.len() >= 2 {
            let np = pieces.len();
            let sc = explode_centre(&pieces.iter().map(|&i| own_center[i]).collect::<Vec<_>>());
            // A few pieces (a bracket and its bolts) also get a fixed push so they clearly part;
            // with many (a building's elements) the push fades out and the spread does the work:
            // every gap grows in proportion, nothing is thrown onto a hollow shell.
            let fade = (EXPLODE_PUSH_PIECES / np as f32).min(1.0);
            let push = sr * EXPLODE_PUSH * fade;
            // the own-size term is capped too: a site slab or a long main under a building's
            // thousand elements must not be thrown off by half its own length
            let support_cap = sr * fade.max(0.05);
            for (k, &i) in pieces.iter().enumerate() {
                let c = own_center[i];
                let d = c - sc;
                let len = d.length();
                let dir = if len > sr * 0.02 {
                    d / len
                } else {
                    // piece sitting at the model centre: spread on a golden-angle spiral
                    let t = (k as f32 + 0.5) / np as f32;
                    let phi = k as f32 * 2.399_963;
                    let z = 1.0 - 2.0 * t;
                    let r = (1.0 - z * z).max(0.0).sqrt();
                    Vec3::new(r * phi.cos(), z, r * phi.sin())
                };
                // plus the piece's own half-size along its direction, so that at 100 % two
                // interlocked pieces (a bolt through a bracket) come fully out of each other
                let h = (own_hi[i] - own_lo[i]) * 0.5;
                let support = dir.x.abs() * h.x + dir.y.abs() * h.y + dir.z.abs() * h.z;
                world_off[i] = d * EXPLODE_SPREAD + dir * (push + support.min(support_cap));
            }
        }
        // local offset = own world offset minus the nearest ancestor's (rigid translations add up)
        let mut explode_dir = vec![Vec3::ZERO; n];
        let mut acc_off = vec![Vec3::ZERO; n];
        for i in 0..n {
            let pa = parent[i].map(|p| acc_off[p]).unwrap_or(Vec3::ZERO);
            if pieces.binary_search(&i).is_ok() {
                explode_dir[i] = world_off[i] - pa;
                acc_off[i] = world_off[i];
            } else {
                acc_off[i] = pa;
            }
        }
        Assembly {
            user: vec![Mat4::IDENTITY; n],
            explode_dir,
            world_off,
            parent,
            center0,
            own_center,
            opacity: vec![1.0; n],
            base,
            ..Assembly::empty()
        }
    }

    /// Display matrices with the user transforms and the current explosion applied, and the
    /// accumulated transform of each component (display = acc * base).
    pub fn compute(&self) -> (Vec<Mat4>, Vec<Mat4>) {
        let n = self.base.len();
        let mut acc = vec![Mat4::IDENTITY; n];
        let mut out = Vec::with_capacity(n);
        let t = self.explode.clamp(0.0, 1.0);
        for i in 0..n {
            let pa = self.parent[i].map(|p| acc[p]).unwrap_or(Mat4::IDENTITY);
            let pu = pa * self.user[i];
            acc[i] = pu * Mat4::from_translation(self.local_offset(i, &pu, t));
            out.push(acc[i] * self.base[i]);
        }
        (out, acc)
    }

    /// Explosion translation of `i` in the frame of `pa` (parent's accumulated transform and the
    /// component's own user move), so the piece moves along its fixed world-space direction.
    fn local_offset(&self, i: usize, pa: &Mat4, t: f32) -> Vec3 {
        let e = self.explode_dir[i] * t;
        if e == Vec3::ZERO {
            return e;
        }
        let inv = pa.inverse();
        inv.transform_vector3(e)
    }

    /// At least two pieces (components or bodies) move apart in the exploded view.
    pub fn can_explode(&self) -> bool {
        self.explode_dir
            .iter()
            .filter(|d| d.length_squared() > 0.0)
            .count()
            >= 2
    }

    pub fn is_modified(&self) -> bool {
        self.user.iter().any(|m| *m != Mat4::IDENTITY)
            || self.explode_goal > 0.0
            || self.explode > 0.0
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
            let b = acc[i].transform_point3(self.own_center[i]);
            let a = b - self.world_off.get(i).copied().unwrap_or(Vec3::ZERO)
                * self.explode.clamp(0.0, 1.0);
            if (b - a).length_squared() > 1e-12 {
                v.push((a, b));
            }
        }
        v
    }
}

#[cfg(test)]
mod explode_tests {
    use super::*;
    use sw_format::{SceneComponent, SceneMesh, TriMesh};

    fn tri_at(c: [f32; 3]) -> TriMesh {
        let p = |dx: f32, dy: f32| [c[0] + dx, c[1] + dy, c[2]];
        TriMesh {
            positions: vec![p(-0.1, -0.1), p(0.1, -0.1), p(0.0, 0.1)],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            indices: vec![0, 1, 2],
            edges: vec![],
            face_count: 1,
            face_start: vec![0],
            bbox_min: [c[0] - 0.1, c[1] - 0.1, c[2]],
            bbox_max: [c[0] + 0.1, c[1] + 0.1, c[2]],
            ..Default::default()
        }
    }

    /// Two pieces with (almost) the same centre, like a U-bolt in its bracket, go opposite ways.
    #[test]
    fn concentric_pieces_separate() {
        let meshes: Vec<SceneMesh> = [[0.0, -0.0085, 0.0], [0.0, -0.0081, 0.0]]
            .iter()
            .map(|c| SceneMesh {
                name: "m".into(),
                mesh: tri_at(*c),
                source: sw_format::MeshSource::Embedded,
                color: None,
            })
            .collect();
        let id = [
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
        ];
        let comp = |name: &str, mesh: usize| SceneComponent {
            name: name.into(),
            path: String::new(),
            depth: 0,
            parent: None,
            is_assembly: false,
            world: id,
            mesh: Some(mesh),
            suppressed: false,
            hidden: false,
            color: None,
        };
        let scene = sw_format::Scene {
            meshes,
            components: vec![comp("a", 0), comp("b", 1)],
            path: Default::default(),
            kind: sw_format::DocKind::Assembly,
            container: sw_format::ContainerKind::Other("t"),
            stream_count: 0,
            preview: None,
            warnings: vec![],
            properties: vec![],
            timings: Default::default(),
            up_z: false,
            browser: vec![],
            sheets: vec![],
            view_frames: vec![],
        };
        let asm = Assembly::new(&scene, vec![Mat4::IDENTITY; 2]);
        assert!(
            asm.world_off[0].dot(asm.world_off[1]) < 0.0,
            "they must not move the same way"
        );
        assert!((asm.world_off[0] - asm.world_off[1]).length() > 0.05);
    }

    /// root ─ sub ─ {a, b}, c, d: every piece's distance from the model centre grows with the
    /// slider and nothing ever comes back (the V3 bug).
    #[test]
    fn explode_is_monotonic_and_radial() {
        let centres = [
            [1.0, 0.2, 0.0],
            [1.4, -0.3, 0.5],
            [-1.0, 0.0, 0.0],
            [0.05, 0.02, 0.0],
        ];
        let meshes: Vec<SceneMesh> = centres
            .iter()
            .map(|c| SceneMesh {
                name: "m".into(),
                mesh: tri_at(*c),
                source: sw_format::MeshSource::Embedded,
                color: None,
            })
            .collect();
        let id = [
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
        ];
        let comp =
            |name: &str, depth: usize, parent: Option<usize>, mesh: Option<usize>| SceneComponent {
                name: name.into(),
                path: String::new(),
                depth,
                parent,
                is_assembly: mesh.is_none(),
                world: id,
                mesh,
                suppressed: false,
                hidden: false,
                color: None,
            };
        let scene = sw_format::Scene {
            meshes,
            components: vec![
                comp("root", 0, None, None),
                comp("sub", 1, Some(0), None),
                comp("a", 2, Some(1), Some(0)),
                comp("b", 2, Some(1), Some(1)),
                comp("c", 1, Some(0), Some(2)),
                comp("d", 1, Some(0), Some(3)),
            ],
            path: Default::default(),
            kind: sw_format::DocKind::Assembly,
            container: sw_format::ContainerKind::Other("t"),
            stream_count: 0,
            preview: None,
            warnings: vec![],
            properties: vec![],
            timings: Default::default(),
            up_z: false,
            browser: vec![],
            sheets: vec![],
            view_frames: vec![],
        };
        let mut asm = Assembly::new(&scene, vec![Mat4::IDENTITY; 6]);
        assert!(asm.can_explode());
        let pieces = [2usize, 3, 4, 5];
        // explosion centre: mean of the piece centres
        let sc = centres.iter().map(|c| Vec3::from(*c)).sum::<Vec3>() / centres.len() as f32;
        let pos = |asm: &Assembly| {
            let (_, acc) = asm.compute();
            pieces.map(|i| acc[i].transform_point3(asm.own_center[i]))
        };
        let mut last = pos(&asm).map(|p| (p - sc).length());
        let start = pos(&asm);
        for k in 1..=40 {
            asm.explode = k as f32 / 40.0;
            let now = pos(&asm);
            for (j, p) in now.iter().enumerate() {
                let d = (*p - sc).length();
                assert!(
                    d > last[j] - 1e-6,
                    "piece {j} came back at {}%",
                    k * 100 / 40
                );
                last[j] = d;
                // straight line out: the displacement keeps the same direction
                let disp = (*p - start[j]).normalize();
                let dir = asm.world_off[pieces[j]].normalize();
                assert!(disp.dot(dir) > 0.999, "piece {j} left its ray");
            }
        }
        // every piece moves out, even one sitting near the middle
        for j in 0..4 {
            assert!(
                (pos(&asm)[j] - start[j]).length() > 0.1,
                "piece {j} barely moved"
            );
        }
        // a moved (rotated) sub-assembly keeps exploding along fixed world directions
        asm.user[1] = Mat4::from_rotation_z(0.7);
        asm.explode = 0.0;
        let s0 = pos(&asm);
        asm.explode = 1.0;
        let s1 = pos(&asm);
        for j in 0..2 {
            let disp = (s1[j] - s0[j]).normalize();
            assert!(disp.dot(asm.world_off[pieces[j]].normalize()) > 0.999);
        }
    }

    /// A building's worth of elements (a 10×10×4 grid under category groups): at 100 % every
    /// element is further from the centre than before, neighbours part (each gap grows), and
    /// the explosion is a spread, not a throw onto a hollow shell around an empty middle.
    #[test]
    fn many_elements_spread_out() {
        let mut centres = Vec::new();
        for x in 0..10 {
            for y in 0..10 {
                for z in 0..4 {
                    centres.push([x as f32, y as f32 * 0.5, z as f32 * 3.0]);
                }
            }
        }
        let meshes: Vec<SceneMesh> = centres
            .iter()
            .map(|c| SceneMesh {
                name: "m".into(),
                mesh: tri_at(*c),
                source: sw_format::MeshSource::Embedded,
                color: None,
            })
            .collect();
        let id = [
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
        ];
        let mut components = vec![SceneComponent {
            name: "root".into(),
            path: String::new(),
            depth: 0,
            parent: None,
            is_assembly: true,
            world: id,
            mesh: None,
            suppressed: false,
            hidden: false,
            color: None,
        }];
        for g in 0..2 {
            components.push(SceneComponent {
                name: format!("cat{g}"),
                path: String::new(),
                depth: 1,
                parent: Some(0),
                is_assembly: true,
                world: id,
                mesh: None,
                suppressed: false,
                hidden: false,
                color: None,
            });
        }
        for k in 0..centres.len() {
            components.push(SceneComponent {
                name: format!("e{k}"),
                path: String::new(),
                depth: 2,
                parent: Some(1 + k % 2),
                is_assembly: false,
                world: id,
                mesh: Some(k),
                suppressed: false,
                hidden: false,
                color: None,
            });
        }
        let n = components.len();
        let scene = sw_format::Scene {
            meshes,
            components,
            path: Default::default(),
            kind: sw_format::DocKind::Assembly,
            container: sw_format::ContainerKind::Other("t"),
            stream_count: 0,
            preview: None,
            warnings: vec![],
            properties: vec![],
            timings: Default::default(),
            up_z: false,
            browser: vec![],
            sheets: vec![],
            view_frames: vec![],
        };
        let mut asm = Assembly::new(&scene, vec![Mat4::IDENTITY; n]);
        let pos = |asm: &Assembly| {
            let (_, acc) = asm.compute();
            (3..n)
                .map(|i| acc[i].transform_point3(asm.own_center[i]))
                .collect::<Vec<_>>()
        };
        let p0 = pos(&asm);
        asm.explode = 1.0;
        let p1 = pos(&asm);
        let sc = explode_centre(&p0);
        // nearest element to the centre stays near it: no hollow shell
        let near = (0..p0.len())
            .min_by(|&a, &b| (p0[a] - sc).length().total_cmp(&(p0[b] - sc).length()))
            .unwrap();
        assert!((p1[near] - sc).length() < 2.0, "the middle was emptied");
        // every pair of neighbours ends further apart, along the model's own layout
        for a in 0..p0.len() {
            assert!((p1[a] - sc).length() >= (p0[a] - sc).length() - 1e-4);
            for b in a + 1..p0.len() {
                let (d0, d1) = ((p0[a] - p0[b]).length(), (p1[a] - p1[b]).length());
                if d0 < 1.01 {
                    assert!(
                        d1 > d0 * 1.5,
                        "neighbours {a} {b} did not part: {d0} -> {d1}"
                    );
                }
            }
        }
    }

    /// A stray element far away does not move the explosion centre off the model.
    #[test]
    fn centre_ignores_strays() {
        let mut c: Vec<Vec3> = (0..40)
            .map(|k| Vec3::new(k as f32 % 5.0, 0.0, (k / 5) as f32))
            .collect();
        c.push(Vec3::new(5000.0, 0.0, 0.0));
        let m = explode_centre(&c);
        assert!(m.x < 5.0 && m.z < 8.0);
    }
}
