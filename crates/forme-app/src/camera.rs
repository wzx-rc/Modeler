//! Orbit camera driven by touch (one finger orbit, two fingers pan/zoom).

use forme_render::View;
use glam::Vec3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewPreset {
    Perspective,
    Front,
    Right,
    Top,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub fov_y: f32,
    pub ortho: bool,
}

const ORTHO_FOV: f32 = 0.035;

impl Default for OrbitCamera {
    fn default() -> Self {
        OrbitCamera { target: Vec3::new(10.0, 78.0, 0.0), yaw: 0.42, pitch: 0.16, dist: 330.0, fov_y: 0.52, ortho: false }
    }
}

impl OrbitCamera {
    pub fn fov(&self) -> f32 {
        if self.ortho {
            ORTHO_FOV
        } else {
            self.fov_y
        }
    }

    /// Distance actually used for the eye (orthographic = far away, narrow fov).
    pub fn eye_dist(&self) -> f32 {
        if self.ortho {
            self.dist * (self.fov_y * 0.5).tan() / (ORTHO_FOV * 0.5).tan()
        } else {
            self.dist
        }
    }

    pub fn dir(&self) -> Vec3 {
        Vec3::new(self.yaw.sin() * self.pitch.cos(), self.pitch.sin(), self.yaw.cos() * self.pitch.cos())
    }

    pub fn eye(&self) -> Vec3 {
        self.target + self.dir() * self.eye_dist()
    }

    /// View in logical points (the caller scales to pixels for rendering).
    pub fn view(&self, size: [f32; 2], principal: [f32; 2]) -> View {
        View { eye: self.eye(), target: self.target, fov_y: self.fov(), size, principal }
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.0085;
        self.pitch = (self.pitch + dy * 0.0085).clamp(-1.52, 1.52);
        self.ortho = false;
    }

    pub fn pan(&mut self, dx: f32, dy: f32, viewport_h: f32) {
        let v = self.view([viewport_h, viewport_h], [0.0, 0.0]);
        let (_, right, up) = v.basis();
        let k = (self.fov_y * 0.5).tan() * self.dist / (viewport_h * 0.5);
        self.target += (-right * dx + up * dy) * k;
    }

    pub fn zoom(&mut self, factor: f32) {
        self.dist = (self.dist / factor.max(0.05)).clamp(15.0, 5000.0);
    }

    pub fn preset(&mut self, p: ViewPreset) {
        match p {
            ViewPreset::Perspective => {
                self.yaw = 0.42;
                self.pitch = 0.16;
                self.ortho = false;
            }
            ViewPreset::Front => {
                self.yaw = 0.0;
                self.pitch = 0.0;
                self.ortho = true;
            }
            ViewPreset::Right => {
                self.yaw = std::f32::consts::FRAC_PI_2;
                self.pitch = 0.0;
                self.ortho = true;
            }
            ViewPreset::Top => {
                self.yaw = 0.0;
                self.pitch = 1.52;
                self.ortho = true;
            }
        }
    }

    pub fn preset_of(&self) -> ViewPreset {
        if !self.ortho {
            return ViewPreset::Perspective;
        }
        if self.pitch > 1.4 {
            ViewPreset::Top
        } else if (self.yaw - std::f32::consts::FRAC_PI_2).abs() < 0.01 {
            ViewPreset::Right
        } else {
            ViewPreset::Front
        }
    }

    /// Frames a bounding box.
    pub fn frame(&mut self, lo: Vec3, hi: Vec3) {
        self.target = (lo + hi) * 0.5;
        let r = (hi - lo).length() * 0.5;
        self.dist = (r / (self.fov_y * 0.5).sin() * 0.98).clamp(30.0, 4000.0);
    }
}
