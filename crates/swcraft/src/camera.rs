//! Turntable orbit camera (Y-up, like SolidWorks).

use glam::{Mat4, Vec2, Vec3};

#[derive(Clone, Debug)]
pub struct Camera {
    pub target: Vec3,
    pub distance: f32,
    /// Rotation around +Y (radians). 0 = looking from +Z (front).
    pub yaw: f32,
    /// Elevation (radians), clamped to [-90°, 90°].
    pub pitch: f32,
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
            yaw: 0.0,
            pitch: 0.0,
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
        self.yaw = 45f32.to_radians();
        self.pitch = 35.264f32.to_radians();
    }
    pub fn set_view(&mut self, yaw_deg: f32, pitch_deg: f32) {
        self.yaw = yaw_deg.to_radians();
        self.pitch = pitch_deg.to_radians();
    }

    pub fn forward_dir(&self) -> Vec3 {
        // direction from target to eye
        Vec3::new(self.pitch.cos() * self.yaw.sin(), self.pitch.sin(), self.pitch.cos() * self.yaw.cos())
    }
    pub fn up(&self) -> Vec3 {
        Vec3::new(-self.pitch.sin() * self.yaw.sin(), self.pitch.cos(), -self.pitch.sin() * self.yaw.cos())
    }
    pub fn right(&self) -> Vec3 {
        self.up().cross(self.forward_dir()).normalize()
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
            Mat4::orthographic_rh(-h * aspect, h * aspect, -h, h, near.min(self.distance * 0.01), far)
        } else {
            Mat4::perspective_rh(self.fov_y, aspect, near, far)
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

    pub fn orbit(&mut self, delta: Vec2) {
        self.yaw -= delta.x * 0.008;
        self.pitch = (self.pitch + delta.y * 0.008).clamp(-89.9f32.to_radians(), 89.9f32.to_radians());
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

    /// Smoothly move towards another orientation (used by the view cube).
    pub fn lerp_towards(&mut self, yaw: f32, pitch: f32, k: f32) {
        let mut dy = (yaw - self.yaw) % std::f32::consts::TAU;
        if dy > std::f32::consts::PI {
            dy -= std::f32::consts::TAU;
        } else if dy < -std::f32::consts::PI {
            dy += std::f32::consts::TAU;
        }
        self.yaw += dy * k;
        self.pitch += (pitch - self.pitch) * k;
    }

    /// Distance multiplier (dolly) without moving the target.
    pub fn dolly(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(self.radius * 1e-4, self.radius * 1e4);
    }
}
