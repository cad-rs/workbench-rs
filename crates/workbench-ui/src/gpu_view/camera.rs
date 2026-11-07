//! Cameras: a 3D orbit camera + a 2D orthographic (top-down) camera.
//! Ported from the gpui-wgpu reference implementation (javagg/gpui-demo).

use glam::{Mat4, Vec3};

pub struct OrbitCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub target: Vec3,
}

impl OrbitCamera {
    pub fn new() -> Self {
        Self {
            yaw: 45f32.to_radians(),
            pitch: 30f32.to_radians(),
            dist: 420.0,
            target: Vec3::new(0.0, 40.0, 0.0),
        }
    }

    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + self.dist * Vec3::new(cp * cy, sp, cp * sy)
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let view = Mat4::look_at_rh(self.eye(), self.target, Vec3::Y);
        let proj = Mat4::perspective_rh(50f32.to_radians(), aspect, 5.0, 8000.0);
        proj * view
    }

    /// The camera's right/up directions in screen space (used for panning)
    fn basis(&self) -> (Vec3, Vec3) {
        let (sy, cy) = self.yaw.sin_cos();
        let right = Vec3::new(sy, 0.0, -cy);
        let (sp, cp) = self.pitch.sin_cos();
        let up = Vec3::new(-sp * cy, cp, -sp * sy);
        (right, up)
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.008;
        self.pitch = (self.pitch + dy * 0.008).clamp(-85f32.to_radians(), 85f32.to_radians());
    }

    pub fn pan(&mut self, dx: f32, dy: f32) {
        let scale = self.dist * 0.0016;
        let (right, up) = self.basis();
        self.target += right * (-dx * scale) + up * (dy * scale);
    }

    pub fn zoom(&mut self, factor: f32) {
        self.dist = (self.dist * factor).clamp(60.0, 4000.0);
    }
}

/// Top-view orthographic camera: looks down along -Y, with world -Z at the top of the screen.
pub struct OrthoCamera {
    /// The world XZ point being looked at
    pub center: Vec3,
    /// Viewport half-height (world units)
    pub half_h: f32,
}

impl OrthoCamera {
    pub fn new() -> Self {
        Self {
            center: Vec3::new(0.0, 0.0, 0.0),
            half_h: 110.0,
        }
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let eye = Vec3::new(self.center.x, 1000.0, self.center.z);
        let view = Mat4::look_at_rh(eye, Vec3::new(self.center.x, 0.0, self.center.z), -Vec3::Z);
        let hh = self.half_h;
        let hw = hh * aspect;
        let proj = Mat4::orthographic_rh(-hw, hw, -hh, hh, 1.0, 5000.0);
        proj * view
    }

    pub fn pan(&mut self, dx: f32, dy: f32) {
        let scale = self.half_h * 2.0 * 0.0016;
        self.center.x -= dx * scale;
        self.center.z += dy * scale;
    }

    pub fn zoom(&mut self, factor: f32) {
        self.half_h = (self.half_h * factor).clamp(12.0, 800.0);
    }
}
