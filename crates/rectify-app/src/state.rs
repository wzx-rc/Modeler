//! The app's state: the scan (it never changes), the tool in hand, the view.

use crate::camera::OrbitCamera;
use crate::view::View;
use glam::{DVec3, Vec3};
use rayon::prelude::*;
use rectify_core::{Mesh, Topo};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub light: bool,
    pub grid: bool,
    /// Zebra stripes: shows how true the surfaces are.
    pub zebra: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { light: false, grid: true, zebra: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// No tool: the left button orbits.
    Navigate,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Navigate => "Navigate",
        }
    }
}

/// The scan open, and what does not change.
pub struct Doc {
    pub mesh: Mesh,
    pub topo: Topo,
    /// Median edge length (mm).
    pub edge: f64,
}

pub enum Msg {
    Loaded(Result<Box<Doc>, String>, PathBuf),
}

pub struct Job {
    pub rx: Receiver<Msg>,
    pub what: String,
}

pub struct Toast {
    pub text: String,
    pub warn: bool,
    pub until: f64,
}

pub struct AppState {
    pub camera: OrbitCamera,
    /// Window size (points) and the free area between the panels.
    pub screen: [f32; 2],
    pub view_rect: [f32; 4],
    pub settings: Settings,
    pub doc: Option<Doc>,
    pub file: Option<PathBuf>,
    pub name: String,
    pub job: Option<Job>,
    /// Bumped when the scan must be uploaded again (opened).
    pub gen: u64,
    /// Bumped when what is drawn over the scan changes.
    pub overlay_gen: u64,
    pub toast: Option<Toast>,
    pub time: f64,
    pub interacting: bool,
    pub files_open: bool,
    pub panel_visible: bool,
    /// No interface, the view only (captures).
    pub bare: bool,
    pub tool: Tool,
}

impl Default for AppState {
    fn default() -> Self {
        AppState::new()
    }
}

impl AppState {
    pub fn new() -> AppState {
        AppState {
            camera: OrbitCamera::default(),
            screen: [1600.0, 900.0],
            view_rect: [0.0, 0.0, 1600.0, 900.0],
            settings: Settings::default(),
            doc: None,
            file: None,
            name: String::new(),
            job: None,
            gen: 1,
            overlay_gen: 1,
            toast: None,
            time: 0.0,
            interacting: false,
            files_open: false,
            panel_visible: true,
            bare: false,
            tool: Tool::Navigate,
        }
    }

    pub fn principal(&self) -> [f32; 2] {
        [(self.view_rect[0] + self.view_rect[2]) * 0.5, (self.view_rect[1] + self.view_rect[3]) * 0.5]
    }

    pub fn view(&self) -> View {
        self.camera.view(self.screen, self.principal())
    }

    pub fn toast(&mut self, text: impl Into<String>, warn: bool) {
        self.toast = Some(Toast { text: text.into(), warn, until: self.time + 3.5 });
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    // ------------------------------------------------------------ files

    fn make_doc(m: Mesh) -> Doc {
        let topo = Topo::new(&m);
        let edge = m.edge_median();
        Doc { mesh: m, topo, edge }
    }

    /// Reads an .obj in the background.
    pub fn open(&mut self, path: PathBuf) {
        if self.busy() {
            self.toast("Busy: wait a moment", true);
            return;
        }
        let (tx, rx) = channel();
        let p = path.clone();
        std::thread::spawn(move || {
            let r = Mesh::load_obj(&p).map(|m| Box::new(Self::make_doc(m)));
            let _ = tx.send(Msg::Loaded(r, p));
        });
        let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        self.job = Some(Job { rx, what: format!("Reading {name}") });
    }

    /// Opens a mesh already in memory (the capture harness).
    pub fn set_mesh(&mut self, m: Mesh, name: &str) {
        self.doc = Some(Self::make_doc(m));
        self.name = name.to_string();
        self.after_load();
    }

    fn after_load(&mut self) {
        self.gen += 1;
        self.overlay_gen += 1;
        self.frame_all();
    }

    /// Waits for the background job (capture harness, tests).
    pub fn wait(&mut self) {
        while self.job.is_some() {
            self.tick(self.time);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Reads what the background job sent.
    pub fn tick(&mut self, now: f64) {
        self.time = now;
        if self.toast.as_ref().map(|t| now > t.until).unwrap_or(false) {
            self.toast = None;
        }
        let Some(job) = &mut self.job else { return };
        let Ok(m) = job.rx.try_recv() else { return };
        self.job = None;
        match m {
            Msg::Loaded(Ok(d), path) => {
                self.doc = Some(*d);
                self.name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                self.file = Some(path);
                self.after_load();
            }
            Msg::Loaded(Err(e), _) => self.toast(format!("Could not read the file: {e}"), true),
        }
    }

    // ------------------------------------------------------------ view

    pub fn frame_all(&mut self) {
        match self.doc.as_ref().map(|d| d.mesh.bounds()) {
            Some((lo, hi)) => self.camera.frame(lo.as_vec3(), hi.as_vec3()),
            None => self.camera.frame(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 20.0, 20.0)),
        }
    }

    /// The triangle under a screen point and where the ray hits it.
    pub fn pick_hit(&self, at: [f32; 2]) -> Option<(usize, DVec3)> {
        let m = &self.doc.as_ref()?.mesh;
        let (o, d) = self.view().ray(at[0], at[1]);
        let (o, d) = (o.as_dvec3(), d.as_dvec3());
        (0..m.tris.len())
            .into_par_iter()
            .filter_map(|t| {
                let [a, b, c] = m.tri_pts(t);
                ray_tri(o, d, a, b, c).map(|s| (s, t))
            })
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
            .map(|(s, t)| (t, o + d * s))
    }

    pub fn set_tool(&mut self, t: Tool) {
        if t != Tool::Navigate && self.doc.is_none() {
            self.toast("Open a mesh first", true);
            return;
        }
        self.tool = t;
        self.overlay_gen += 1;
    }
}

/// A number with a decimal comma.
pub fn fmt(v: f64, digits: usize) -> String {
    format!("{:.*}", digits, v).replace('.', ",")
}

/// Möller–Trumbore: distance along the ray, if it hits.
fn ray_tri(o: DVec3, d: DVec3, a: DVec3, b: DVec3, c: DVec3) -> Option<f64> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-14 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0).then_some(t)
}
