//! 相机：3D 轨道球相机 + 2D 正交投影（俯视）相机。
//! 移植自 gpui-wgpu 参考实现（javagg/gpui-demo）。

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

    /// 屏幕坐标系下的相机右/上方向（用于平移）
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

/// 俯视图正交相机：沿 -Y 向下看，屏幕上方为世界 -Z。
pub struct OrthoCamera {
    /// 注视的世界 XZ 点
    pub center: Vec3,
    /// 视口半高（世界单位）
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
