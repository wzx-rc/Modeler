//! A camera view: eye, target, vertical field of view, image size and the
//! projection centre (the part sits in the free area between the panels).

use glam::Vec3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub eye: Vec3,
    pub target: Vec3,
    pub fov_y: f32,
    pub size: [f32; 2],
    /// Projection centre (same units as `size`).
    pub principal: [f32; 2],
}

impl View {
    pub fn basis(&self) -> (Vec3, Vec3, Vec3) {
        let fwd = (self.target - self.eye).normalize_or(Vec3::NEG_Z);
        let mut right = fwd.cross(Vec3::Y);
        if right.length_squared() < 1e-8 {
            right = Vec3::X;
        }
        let right = right.normalize();
        let up = right.cross(fwd).normalize();
        (fwd, right, up)
    }

    /// Ray through a point of the image (origin top-left).
    pub fn ray(&self, px: f32, py: f32) -> (Vec3, Vec3) {
        let (fwd, right, up) = self.basis();
        let half_h = self.size[1] * 0.5;
        let sx = (px - self.principal[0]) / half_h;
        let sy = -(py - self.principal[1]) / half_h;
        let t = (self.fov_y * 0.5).tan();
        (self.eye, (fwd + right * sx * t + up * sy * t).normalize())
    }

    /// A world point in the image. None behind the camera.
    pub fn project(&self, p: Vec3) -> Option<[f32; 2]> {
        let (fwd, right, up) = self.basis();
        let d = p - self.eye;
        let z = d.dot(fwd);
        if z <= 1e-3 {
            return None;
        }
        let t = (self.fov_y * 0.5).tan();
        let half_h = self.size[1] * 0.5;
        let sx = d.dot(right) / (z * t);
        let sy = d.dot(up) / (z * t);
        Some([self.principal[0] + sx * half_h, self.principal[1] - sy * half_h])
    }

    /// World size of one image unit at distance `dist` from the eye.
    pub fn pixel_size_at(&self, dist: f32) -> f32 {
        (self.fov_y * 0.5).tan() * dist / (self.size[1] * 0.5)
    }

    /// The same view in pixels (`k` pixels per point).
    pub fn scaled(&self, k: f32) -> View {
        View { size: [self.size[0] * k, self.size[1] * k], principal: [self.principal[0] * k, self.principal[1] * k], ..*self }
    }
}
