//! The app's state: the mesh being rectified (a `Pipeline`), the steps'
//! settings, the job running in the background, the view and what is picked.

use crate::camera::OrbitCamera;
use crate::view::View;
use glam::{DVec3, Vec3};
use rectify_core::prim::{fit_cylinder, fit_plane, fit_sphere, Sample};
use rectify_core::{Kind, Mesh, Params, Pipeline, Step, NONE};
use rayon::prelude::*;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    /// The mesh as it came.
    Original,
    /// The regions found, one colour each (planes cool, curved warm).
    Regions,
    /// The rectified mesh.
    Result,
}

impl ViewMode {
    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Original => "Original",
            ViewMode::Regions => "Regions",
            ViewMode::Result => "Result",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub light: bool,
    pub grid: bool,
    /// The lines where regions meet.
    pub edges: bool,
    /// Zebra stripes: shows how true the surfaces are.
    pub zebra: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { light: false, grid: true, edges: true, zebra: false }
    }
}

pub enum Msg {
    Progress(Step, f32),
    Loaded(Result<Box<Pipeline>, String>, PathBuf),
    Done(Box<Pipeline>),
    Exported(Result<PathBuf, String>),
}

pub struct Job {
    pub rx: Receiver<Msg>,
    pub what: String,
    pub step: Option<Step>,
    pub progress: f32,
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
    pub params: Params,
    pub pipe: Option<Pipeline>,
    pub file: Option<PathBuf>,
    pub name: String,
    pub job: Option<Job>,
    pub mode: ViewMode,
    /// Space held: the original shown whatever the mode.
    pub compare: bool,
    /// Bumped whenever what is drawn changes.
    pub gen: u64,
    /// The step whose settings the inspector shows.
    pub step: Step,
    /// Picked region (same index in every segmentation of the pipeline).
    pub selected: Option<u32>,
    pub toast: Option<Toast>,
    pub time: f64,
    pub interacting: bool,
    pub files_open: bool,
    pub panel_visible: bool,
    /// No interface, the view only (captures).
    pub bare: bool,
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
            params: Params::default(),
            pipe: None,
            file: None,
            name: String::new(),
            job: None,
            mode: ViewMode::Original,
            compare: false,
            gen: 1,
            step: Step::Symmetry,
            selected: None,
            toast: None,
            time: 0.0,
            interacting: false,
            files_open: false,
            panel_visible: true,
            bare: false,
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

    /// The mode actually drawn (Space shows the original).
    pub fn shown(&self) -> ViewMode {
        if self.compare {
            ViewMode::Original
        } else {
            self.mode
        }
    }

    pub fn set_mode(&mut self, m: ViewMode) {
        if self.mode != m {
            self.mode = m;
            self.gen += 1;
        }
    }

    // ------------------------------------------------------------ files

    /// Reads an .obj in the background.
    pub fn open(&mut self, path: PathBuf) {
        if self.busy() {
            self.toast("Busy: wait for the current step", true);
            return;
        }
        let (tx, rx) = channel();
        let p = path.clone();
        std::thread::spawn(move || {
            let r = Mesh::load_obj(&p).map(|m| Box::new(Pipeline::new(m)));
            let _ = tx.send(Msg::Loaded(r, p));
        });
        let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        self.job = Some(Job { rx, what: format!("Reading {name}"), step: None, progress: 0.0 });
    }

    /// Opens a mesh already in memory (the capture harness).
    pub fn set_mesh(&mut self, m: Mesh, name: &str) {
        self.pipe = Some(Pipeline::new(m));
        self.name = name.to_string();
        self.after_load();
    }

    fn after_load(&mut self) {
        self.selected = None;
        self.mode = ViewMode::Original;
        self.step = Step::Symmetry;
        self.gen += 1;
        self.frame_all();
    }

    pub fn export(&mut self) {
        let Some(p) = &self.pipe else { return };
        let Some(m) = &p.result else {
            self.toast("Nothing to export: run the steps first", true);
            return;
        };
        let m = m.clone();
        let path = crate::files::documents_dir().join(format!("{} rectified.obj", crate::files::sanitize(&self.name)));
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r = std::fs::write(&path, m.obj(None)).map(|_| path.clone()).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Exported(r));
        });
        self.job = Some(Job { rx, what: "Exporting".into(), step: None, progress: 0.0 });
    }

    // ------------------------------------------------------------ steps

    /// Runs `from` and every step after it in the background.
    pub fn run_from(&mut self, from: Step) {
        if self.busy() {
            return;
        }
        let Some(mut pipe) = self.pipe.take() else {
            self.toast("Open an .obj first", true);
            return;
        };
        // the steps before it that were not run yet run too
        let from = match pipe.next() {
            Some(n) if n.index() < from.index() => n,
            _ => from,
        };
        let params = self.params;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            pipe.run_from(from, &params, &move |s, f| {
                let _ = tx2.send(Msg::Progress(s, f));
            });
            let _ = tx.send(Msg::Done(Box::new(pipe)));
        });
        self.job = Some(Job { rx, what: from.label().to_string(), step: Some(from), progress: 0.0 });
    }

    pub fn run_all(&mut self) {
        self.run_from(Step::Symmetry);
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
        let mut finished = None;
        while let Ok(m) = job.rx.try_recv() {
            match m {
                Msg::Progress(s, f) => {
                    job.step = Some(s);
                    job.what = s.label().to_string();
                    job.progress = f;
                }
                other => finished = Some(other),
            }
        }
        let Some(m) = finished else { return };
        self.job = None;
        match m {
            Msg::Loaded(Ok(p), path) => {
                self.pipe = Some(*p);
                self.name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                self.file = Some(path);
                self.after_load();
            }
            Msg::Loaded(Err(e), _) => self.toast(format!("Could not read the file: {e}"), true),
            Msg::Done(p) => {
                self.pipe = Some(*p);
                self.gen += 1;
                if self.mode == ViewMode::Original {
                    self.mode = ViewMode::Result;
                }
            }
            Msg::Exported(Ok(path)) => self.toast(format!("Exported to {}", path.display()), false),
            Msg::Exported(Err(e)) => self.toast(format!("Export failed: {e}"), true),
            Msg::Progress(..) => {}
        }
    }

    // ------------------------------------------------------------ view

    /// The mesh drawn in the current mode.
    pub fn shown_mesh(&self) -> Option<&Mesh> {
        let p = self.pipe.as_ref()?;
        Some(match self.shown() {
            ViewMode::Original => &p.original,
            ViewMode::Regions => p.smooth_out(),
            ViewMode::Result => p.result.as_ref().unwrap_or_else(|| p.smooth_out()),
        })
    }

    /// The latest segmentation (after the snap step's border clean-up if run).
    pub fn shown_seg(&self) -> Option<&rectify_core::segment::Segmentation> {
        let p = self.pipe.as_ref()?;
        if self.shown() == ViewMode::Original {
            return None;
        }
        p.seg_final.as_ref().or(p.seg.as_ref()).or(p.seg_raw.as_ref())
    }

    pub fn frame_all(&mut self) {
        match self.pipe.as_ref().map(|p| p.original.bounds()) {
            Some((lo, hi)) => self.camera.frame(lo.as_vec3(), hi.as_vec3()),
            None => self.camera.frame(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 20.0, 20.0)),
        }
    }

    /// The triangle under a screen point (ray against the shown mesh).
    pub fn pick(&self, at: [f32; 2]) -> Option<usize> {
        let m = self.shown_mesh()?;
        let (o, d) = self.view().ray(at[0], at[1]);
        let (o, d) = (o.as_dvec3(), d.as_dvec3());
        (0..m.tris.len())
            .into_par_iter()
            .filter_map(|t| {
                let [a, b, c] = m.tri_pts(t);
                ray_tri(o, d, a, b, c).map(|s| (s, t))
            })
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
            .map(|x| x.1)
    }

    pub fn click(&mut self, at: [f32; 2]) {
        if self.busy() {
            return;
        }
        let region = self.pick(at).and_then(|t| self.shown_seg().map(|s| s.label[t])).filter(|r| *r != NONE);
        self.selected = region;
    }

    // ------------------------------------------------------------ edits

    /// Gives the picked region another shape (fitted on its faces), or frees
    /// it (`None`); then regularizes and snaps again.
    pub fn retype(&mut self, kind: Option<Kind>) {
        let Some(r) = self.selected else { return };
        let Some(p) = &mut self.pipe else { return };
        let mesh = p.smooth_out().clone();
        let Some(seg) = &mut p.seg_raw else { return };
        let reg = &mut seg.regions[r as usize];
        match kind {
            None => {
                for &f in &reg.faces {
                    seg.label[f as usize] = NONE;
                }
                reg.faces.clear();
                reg.area = 0.0;
                self.selected = None;
            }
            Some(k) => {
                let s: Vec<Sample> = reg.faces.iter().map(|&f| {
                    let c = mesh.tri_cross(f as usize);
                    let l = c.length();
                    Sample { p: mesh.tri_centre(f as usize), n: if l > 0.0 { c / l } else { DVec3::Y }, w: l * 0.5 }
                }).collect();
                let q = match k {
                    Kind::Plane => fit_plane(&s),
                    Kind::Cylinder => fit_cylinder(&s, None, None),
                    Kind::Sphere => fit_sphere(&s, None),
                };
                match q {
                    Some(q) => {
                        reg.rms = q.rms(&s);
                        reg.prim = q;
                    }
                    None => {
                        self.toast(format!("This region is not a {}", k.label().to_lowercase()), true);
                        return;
                    }
                }
            }
        }
        self.run_from(Step::Regularize);
    }
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
