//! Orbit camera. The orientation is a quaternion. The turntable mode (default) orbits around +Y
//! like Blender and keeps any 90° roll from the view cube; the free (trackball) mode has no limit.

use glam::{Mat3, Mat4, Quat, Vec2, Vec3};

#[derive(Clone, Debug)]
pub struct Camera {
    pub target: Vec3,
    pub distance: f32,
    /// Camera-to-world rotation: local +Z points from the target to the eye, +Y is screen up.
    pub rot: Quat,
    pub fov_y: f32,
    pub ortho: bool,
    /// Scene radius, used for clip planes.
    pub radius: f32,
}

impl Default for Camera {
    fn default() -> Self {
        let mut c = Camera {
            target: Vec3::ZERO,
            distance: 1.0,
            rot: Quat::IDENTITY,
            fov_y: 30f32.to_radians(),
            ortho: false,
            radius: 1.0,
        };
        c.set_iso();
        c
    }
}

impl Camera {
    pub fn set_iso(&mut self) {
        self.rot = Self::orientation(45f32.to_radians(), 35.264f32.to_radians());
    }
    pub fn set_view(&mut self, yaw_deg: f32, pitch_deg: f32) {
        self.rot = Self::orientation(yaw_deg.to_radians(), pitch_deg.to_radians());
    }

    /// Orientation looking at the target from azimuth `yaw` (around +Y, 0 = from +Z) and elevation
    /// `pitch` (radians), with no roll. Exact at ±90° (top / bottom views).
    pub fn orientation(yaw: f32, pitch: f32) -> Quat {
        let f = Vec3::new(pitch.cos() * yaw.sin(), pitch.sin(), pitch.cos() * yaw.cos());
        let u = Vec3::new(-pitch.sin() * yaw.sin(), pitch.cos(), -pitch.sin() * yaw.cos());
        let r = u.cross(f).normalize();
        let u = f.cross(r).normalize();
        Quat::from_mat3(&Mat3::from_cols(r, u, f)).normalize()
    }

    pub fn forward_dir(&self) -> Vec3 {
        // direction from target to eye
        self.rot * Vec3::Z
    }
    pub fn up(&self) -> Vec3 {
        self.rot * Vec3::Y
    }
    pub fn right(&self) -> Vec3 {
        self.rot * Vec3::X
    }
    pub fn eye(&self) -> Vec3 {
        self.target + self.forward_dir() * self.distance
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.target, self.up())
    }

    pub fn proj(&self, aspect: f32) -> Mat4 {
        let r = self.radius.max(1e-6);
        let near = (self.distance - 2.0 * r).max(self.distance * 1e-3).max(r * 1e-4);
        let far = self.distance + 2.0 * r;
        if self.ortho {
            let h = self.distance * (self.fov_y * 0.5).tan();
            // reversed-Z: near and far swapped so depth runs 1 → 0 (see render.rs DEPTH_FORMAT)
            Mat4::orthographic_rh(-h * aspect, h * aspect, -h, h, far, near.min(self.distance * 0.01))
        } else {
            Mat4::perspective_rh(self.fov_y, aspect, far, near)
        }
    }

    /// Fit a bounding sphere.
    pub fn fit(&mut self, lo: Vec3, hi: Vec3, aspect: f32) {
        self.target = (lo + hi) * 0.5;
        let r = ((hi - lo).length() * 0.5).max(1e-5);
        self.radius = r;
        let half_v = self.fov_y * 0.5;
        let half_h = (half_v.tan() * aspect.max(0.05)).atan();
        let half = half_v.min(half_h);
        self.distance = r / half.sin() * 1.08;
    }

    /// Rotate by a mouse delta in pixels. Turntable (default): around world +Y and the screen
    /// horizontal axis, stopping at the poles so +Y stays up and the view never tilts. Free:
    /// trackball around the screen axes, unlimited in every direction (circular drags roll).
    pub fn orbit(&mut self, delta: Vec2, turntable: bool) {
        const K: f32 = 0.008;
        if turntable {
            // Split the orientation into a level (yaw, pitch) part and a roll around the viewing
            // axis (set by the view cube's 90° buttons), orbit the level part, then put the roll
            // back so the view keeps the orientation the user chose instead of snapping upright.
            let (yaw, pitch, roll) = self.yaw_pitch_roll();
            let (s, c) = roll.sin_cos();
            // the drag is in screen space: express it in the un-rolled frame
            let d = Vec2::new(delta.x * c + delta.y * s, -delta.x * s + delta.y * c);
            let lim = 89.9f32.to_radians();
            let yaw = yaw - d.x * K;
            let pitch = if pitch.abs() > lim && d.y == 0.0 { pitch } else { (pitch + d.y * K).clamp(-lim, lim) };
            self.rot = (Self::orientation(yaw, pitch) * Quat::from_rotation_z(roll)).normalize();
        } else {
            let q = Quat::from_axis_angle(self.up(), -delta.x * K) * Quat::from_axis_angle(self.right(), -delta.y * K);
            self.rot = (q * self.rot).normalize();
        }
    }

    /// Decompose the orientation as `orientation(yaw, pitch) * roll around local Z`. At the poles
    /// (top / bottom views) roll and yaw are the same motion, so the roll is folded into the yaw.
    pub fn yaw_pitch_roll(&self) -> (f32, f32, f32) {
        let f = self.forward_dir();
        let pitch = f.y.clamp(-1.0, 1.0).asin();
        // only exact top / bottom views (orbit stops at 89.9°, so a drag never folds the roll)
        if pitch.abs() > 89.95f32.to_radians() {
            let u = self.up();
            let yaw = if f.y > 0.0 { (-u.x).atan2(-u.z) } else { u.x.atan2(u.z) };
            return (yaw, pitch.signum() * std::f32::consts::FRAC_PI_2, 0.0);
        }
        let yaw = f.x.atan2(f.z);
        let rq = Self::orientation(yaw, pitch).inverse() * self.rot;
        let roll = 2.0 * rq.z.atan2(rq.w);
        (yaw, pitch, roll)
    }

    /// Roll around the viewing axis (degrees, positive = counter-clockwise on screen).
    pub fn roll(&mut self, deg: f32) {
        self.rot = (Quat::from_axis_angle(self.forward_dir(), deg.to_radians()) * self.rot).normalize();
    }

    /// Pan by a pixel delta, given the viewport height in pixels.
    pub fn pan(&mut self, delta: Vec2, viewport_h: f32) {
        let world_per_px = 2.0 * self.distance * (self.fov_y * 0.5).tan() / viewport_h.max(1.0);
        self.target += (-self.right() * delta.x + self.up() * delta.y) * world_per_px;
    }

    /// Zoom by `factor` (<1 = closer) toward the point under the cursor.
    /// `ndc` is the cursor position in [-1,1]².
    pub fn zoom(&mut self, factor: f32, ndc: Vec2, aspect: f32) {
        let h = self.distance * (self.fov_y * 0.5).tan();
        let p = self.target + self.right() * (ndc.x * h * aspect) + self.up() * (ndc.y * h);
        let new_dist = (self.distance * factor).clamp(self.radius * 1e-4, self.radius * 1e4);
        let f = new_dist / self.distance;
        self.target = p + (self.target - p) * f;
        self.distance = new_dist;
    }

    /// World-space ray through a point given in NDC ([-1,1]², y up).
    pub fn ray(&self, ndc: Vec2, aspect: f32) -> (Vec3, Vec3) {
        let h = (self.fov_y * 0.5).tan();
        let fwd = -self.forward_dir();
        if self.ortho {
            let half = self.distance * h;
            let o = self.eye() + self.right() * (ndc.x * half * aspect) + self.up() * (ndc.y * half);
            (o - fwd * self.radius * 4.0, fwd)
        } else {
            let d = (fwd + self.right() * (ndc.x * h * aspect) + self.up() * (ndc.y * h)).normalize();
            (self.eye(), d)
        }
    }

    /// Smoothly move towards another orientation (view cube, standard views). Returns true once
    /// the goal is reached (and snaps to it).
    pub fn lerp_towards(&mut self, goal: Quat, k: f32) -> bool {
        let goal = if self.rot.dot(goal) < 0.0 { -goal } else { goal };
        if self.rot.angle_between(goal) < 1e-3 {
            self.rot = goal;
            return true;
        }
        self.rot = self.rot.slerp(goal, k).normalize();
        false
    }

    /// Distance multiplier (dolly) without moving the target.
    pub fn dolly(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(self.radius * 1e-4, self.radius * 1e4);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn projection_is_reversed_z() {
        // depth 1 at the near plane, 0 at the far plane: required by the Greater depth test
        let mut c = Camera::default();
        c.fit(glam::Vec3::splat(-5.0), glam::Vec3::splat(5.0), 1.5);
        let vp = c.proj(1.5) * c.view();
        let depth = |p: glam::Vec3| { let q = vp * p.extend(1.0); q.z / q.w };
        let near = c.target + (c.eye() - c.target).normalize() * 8.0;
        let far = c.target;
        assert!(depth(near) > depth(far), "{} {}", depth(near), depth(far));
        assert!((0.0..=1.0).contains(&depth(far)) && (0.0..=1.0).contains(&depth(near)));
    }

    use super::*;

    #[test]
    fn free_orbit_full_turn_any_axis() {
        let mut c = Camera::default();
        c.set_view(0.0, 0.0);
        let f0 = c.forward_dir();
        // vertical drag: 2*pi / 0.008 px goes over the top and back with no clamp
        let steps = 200;
        let px = std::f32::consts::TAU / 0.008 / steps as f32;
        let mut max_y: f32 = 0.0;
        for _ in 0..steps {
            c.orbit(Vec2::new(0.0, px), false);
            max_y = max_y.max(c.forward_dir().y);
        }
        assert!(max_y > 0.999, "went over the pole");
        assert!((c.forward_dir() - f0).length() < 1e-2, "full vertical turn returns to start");
        for _ in 0..steps {
            c.orbit(Vec2::new(px, 0.0), false);
        }
        assert!((c.forward_dir() - f0).length() < 1e-2);
    }

    #[test]
    fn standard_views_exact() {
        let mut c = Camera::default();
        c.set_view(0.0, 90.0);
        assert!((c.forward_dir() - Vec3::Y).length() < 1e-5);
        assert!((c.up() - Vec3::NEG_Z).length() < 1e-5);
        c.set_view(90.0, 0.0);
        assert!((c.forward_dir() - Vec3::X).length() < 1e-5);
        assert!((c.up() - Vec3::Y).length() < 1e-5);
        let g = Camera::orientation(0.0, -90f32.to_radians());
        for _ in 0..200 {
            if c.lerp_towards(g, 0.25) {
                break;
            }
        }
        assert!((c.forward_dir() - Vec3::NEG_Y).length() < 1e-4);
    }

    #[test]
    fn turntable_keeps_y_up() {
        let mut c = Camera::default();
        for _ in 0..500 {
            c.orbit(Vec2::new(13.0, 7.0), true);
        }
        assert!(c.forward_dir().y < 1.0 && c.up().y >= -1e-3);
        assert!(c.right().y.abs() < 1e-4);
    }

    #[test]
    fn turntable_circular_drag_never_tilts() {
        // a circular mouse drag (the user's "the view tilts by itself"): the horizon stays level
        let mut c = Camera::default();
        for k in 0..2000 {
            let a = k as f32 * 0.05;
            c.orbit(Vec2::new(a.cos() * 9.0, a.sin() * 9.0), true);
            assert!(c.right().y.abs() < 1e-4, "rolled at step {k}: {:?}", c.right());
            assert!(c.up().y > 0.0);
        }
        // the trackball does roll on the same drag (kept as the optional free mode)
        let mut f = Camera::default();
        for k in 0..2000 {
            let a = k as f32 * 0.05;
            f.orbit(Vec2::new(a.cos() * 9.0, a.sin() * 9.0), false);
        }
        assert!(f.right().y.abs() > 1e-3);
    }

    #[test]
    fn turntable_keeps_view_cube_roll() {
        let mut c = Camera::default();
        c.roll(90.0);
        let (_, _, r0) = c.yaw_pitch_roll();
        assert!((r0 - 90f32.to_radians()).abs() < 1e-3, "roll {r0}");
        for _ in 0..200 {
            c.orbit(Vec2::new(11.0, -6.0), true);
            let (_, _, r) = c.yaw_pitch_roll();
            assert!((r - r0).abs() < 1e-3, "roll lost: {r}");
        }
        // a pan right after must not touch the orientation either
        let before = c.rot;
        c.pan(Vec2::new(30.0, 10.0), 600.0);
        assert!(c.rot.angle_between(before) < 1e-6);
    }

    #[test]
    fn turntable_unrolled_matches_level_orbit() {
        let mut c = Camera::default();
        c.orbit(Vec2::new(40.0, 25.0), true);
        let (y, p, r) = c.yaw_pitch_roll();
        assert!(r.abs() < 1e-4);
        assert!((y - (45f32.to_radians() - 40.0 * 0.008)).abs() < 1e-4);
        assert!((p - (35.264f32.to_radians() + 25.0 * 0.008)).abs() < 1e-4);
    }

    #[test]
    fn turntable_rolled_drag_follows_screen() {
        // after a 90° roll, a horizontal screen drag behaves like a vertical one did before
        let mut a = Camera::default();
        a.roll(90.0);
        a.orbit(Vec2::new(20.0, 0.0), true);
        let (_, pa, _) = a.yaw_pitch_roll();
        let mut b = Camera::default();
        b.orbit(Vec2::new(0.0, -20.0), true);
        let (_, pb, _) = b.yaw_pitch_roll();
        assert!((pa - pb).abs() < 1e-4, "{pa} vs {pb}");
    }
}
