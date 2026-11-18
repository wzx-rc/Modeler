//! The app's state: the mesh being rectified (a `Pipeline`), the steps'
//! settings, the job running in the background, the view and what is picked.

use crate::camera::OrbitCamera;
use crate::view::View;
use glam::{DVec3, Vec3};
use rectify_core::edit;
use rectify_core::prim::{fit_cylinder, fit_plane, fit_sphere, Sample};
use rectify_core::segment::Segmentation;
use rectify_core::{Kind, Mesh, Params, Pipeline, Prim, Step, NONE};
use rayon::prelude::*;
use std::collections::HashSet;
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

/// What the brush does to the regions it paints over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrushMode {
    /// The regions touched become one.
    Merge,
    /// A stroke across a region cuts it in two.
    Split,
    /// The faces painted belong to no region.
    Free,
}

impl BrushMode {
    pub fn label(self) -> &'static str {
        match self {
            BrushMode::Merge => "Merge",
            BrushMode::Split => "Split",
            BrushMode::Free => "Free",
        }
    }
    fn next(self) -> BrushMode {
        match self {
            BrushMode::Merge => BrushMode::Split,
            BrushMode::Split => BrushMode::Free,
            BrushMode::Free => BrushMode::Merge,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// Click a region to inspect it; drag to orbit.
    Select,
    Brush(BrushMode),
    /// Paint over the mesh: a shape of this kind is fitted on what is
    /// painted (live), then grows over the mesh where it lies on it.
    Shape(Kind),
}

/// A brush or shape stroke being painted.
#[derive(Default)]
pub struct Stroke {
    pub faces: HashSet<u32>,
    /// Regions under the pointer, in the order they were met.
    pub regions: Vec<u32>,
    /// Points hit on the mesh.
    pub points: Vec<DVec3>,
    pub last: Option<[f32; 2]>,
    /// The shape fitted on the faces so far (Shape tool).
    pub prim: Option<Prim>,
    fitted: usize,
}

impl Stroke {
    /// The stroke's direction (from its first point to its last), once it
    /// is long enough: a cylinder's axis when the faces do not show it.
    pub fn direction(&self) -> Option<DVec3> {
        let (a, b) = (self.points.first()?, self.points.last()?);
        let d = *b - *a;
        (d.length() > 1.0).then(|| d.normalize())
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
    pub tool: Tool,
    /// Brush radius on screen (points).
    pub brush: f32,
    pub stroke: Option<Stroke>,
    /// Bumped whenever the stroke changes (what is drawn over the mesh).
    pub stroke_gen: u64,
    /// Bumped when a mesh changes (not the labels): cached normals.
    pub mesh_gen: u64,
    /// Regions before each hand edit, and the ones undone.
    pub undo: Vec<Segmentation>,
    pub redo: Vec<Segmentation>,
    /// Hand edits not carried to the result yet (done when Result is shown).
    pub edited: bool,
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
            tool: Tool::Select,
            brush: 14.0,
            stroke: None,
            stroke_gen: 0,
            mesh_gen: 1,
            undo: vec![],
            redo: vec![],
            edited: false,
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
        self.tool = Tool::Select;
        self.stroke = None;
        self.undo.clear();
        self.redo.clear();
        self.edited = false;
        self.mesh_gen += 1;
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
        if from.index() <= Step::Segment.index() && !self.undo.is_empty() {
            self.undo.clear();
            self.redo.clear();
            self.toast("Regions found again: the hand edits are dropped", true);
        }
        self.edited = false;
        self.stroke = None;
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

    /// Runs what is out of date (every step the first time).
    pub fn run_all(&mut self) {
        match self.pipe.as_ref().map(|p| p.next()) {
            Some(Some(s)) => self.run_from(s),
            Some(None) => self.toast("Everything is up to date", false),
            None => self.toast("Open an .obj first", true),
        }
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
        // hand edits reach the result when it is looked at
        if self.edited && self.job.is_none() && self.stroke.is_none() && self.shown() == ViewMode::Result {
            if let Some(Some(s)) = self.pipe.as_ref().map(|p| p.next()) {
                self.run_from(s);
            }
            self.edited = false;
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
                self.mesh_gen += 1;
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

    /// The triangle under a screen point and where the ray hits it.
    pub fn pick_hit(&self, at: [f32; 2]) -> Option<(usize, DVec3)> {
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
            .map(|(s, t)| (t, o + d * s))
    }

    /// Whether the regions can be edited by hand (found, nothing running).
    pub fn can_edit(&self) -> bool {
        self.job.is_none() && self.pipe.as_ref().map(|p| p.seg_raw.is_some()).unwrap_or(false)
    }

    pub fn set_tool(&mut self, t: Tool) {
        if t != Tool::Select && !self.can_edit() {
            self.toast(if self.busy() { "Wait for the current step" } else { "Run the steps first: the tools edit the regions" }, true);
            return;
        }
        self.tool = t;
        self.stroke = None;
        self.stroke_gen += 1;
        if t != Tool::Select {
            self.selected = None;
            self.set_mode(ViewMode::Regions);
        }
    }

    /// B: the brush, or its next mode when it is on.
    pub fn brush_key(&mut self) {
        let t = match self.tool {
            Tool::Brush(m) => Tool::Brush(m.next()),
            _ => Tool::Brush(BrushMode::Merge),
        };
        self.set_tool(t);
    }

    /// G: the shape tool, or its next kind when it is on.
    pub fn shape_key(&mut self) {
        let t = match self.tool {
            Tool::Shape(Kind::Plane) => Tool::Shape(Kind::Cylinder),
            Tool::Shape(Kind::Cylinder) => Tool::Shape(Kind::Sphere),
            Tool::Shape(Kind::Sphere) => Tool::Shape(Kind::Plane),
            _ => Tool::Shape(Kind::Plane),
        };
        self.set_tool(t);
    }

    pub fn stroke_begin(&mut self, at: [f32; 2]) {
        if !self.can_edit() || self.tool == Tool::Select {
            return;
        }
        self.stroke = Some(Stroke::default());
        self.stroke_to(at);
    }

    /// Paints from the last point to `at` (dabs about a third of the
    /// brush apart, at most 8 a frame).
    pub fn stroke_to(&mut self, at: [f32; 2]) {
        let Some(last) = self.stroke.as_ref().map(|s| s.last) else { return };
        let spacing = (self.brush * 0.35).max(2.0);
        let samples: Vec<[f32; 2]> = match last {
            None => vec![at],
            Some(l) => {
                let d = ((at[0] - l[0]).powi(2) + (at[1] - l[1]).powi(2)).sqrt();
                if d < spacing {
                    return;
                }
                let n = ((d / spacing).ceil() as usize).min(8);
                (1..=n).map(|i| {
                    let t = i as f32 / n as f32;
                    [l[0] + (at[0] - l[0]) * t, l[1] + (at[1] - l[1]) * t]
                }).collect()
            }
        };
        let view = self.view();
        let eye = view.eye.as_dvec3();
        let mut hits = vec![];
        for s in &samples {
            if let Some(h) = self.pick_hit(*s) {
                hits.push(h);
            }
        }
        let Some(p) = &self.pipe else { return };
        let m = match self.shown() {
            ViewMode::Original => &p.original,
            _ => p.smooth_out(),
        };
        let label = p.seg_raw.as_ref().map(|s| &s.label);
        let st = self.stroke.as_mut().unwrap();
        st.last = Some(at);
        for (t, x) in hits {
            let r = view.pixel_size_at((x - eye).length() as f32) as f64 * self.brush as f64;
            let toward = (eye - x).normalize_or_zero();
            edit::dab(m, &p.topo, t as u32, x, r, Some(toward), &mut st.faces);
            st.points.push(x);
            if let Some(l) = label {
                let r = l[t];
                if r != NONE && !st.regions.contains(&r) {
                    st.regions.push(r);
                }
            }
        }
        // the shape, fitted live on what is painted
        if let Tool::Shape(k) = self.tool {
            if st.faces.len() >= 6 && (st.faces.len() as f64) > st.fitted as f64 * 1.15 {
                let faces: Vec<u32> = st.faces.iter().copied().collect();
                st.prim = edit::fit_best(&edit::samples(m, &faces), Some(k), st.direction(), &self.params.seg);
                st.fitted = st.faces.len();
            }
        }
        self.stroke_gen += 1;
    }

    /// The stroke applied to the regions (undoable).
    pub fn stroke_end(&mut self) {
        let Some(st) = self.stroke.take() else { return };
        self.stroke_gen += 1;
        if st.faces.is_empty() {
            return;
        }
        let tool = self.tool;
        let segp = self.params.seg;
        let Some(p) = &mut self.pipe else { return };
        let Pipeline { original, sym_mesh, smooth_mesh, seg_raw, topo, .. } = p;
        let m: &Mesh = smooth_mesh.as_ref().or(sym_mesh.as_ref()).unwrap_or(original);
        let Some(seg) = seg_raw.as_mut() else { return };
        let before = seg.clone();
        let msg: Result<String, String> = match tool {
            Tool::Select => return,
            Tool::Brush(BrushMode::Merge) => match edit::merge(m, seg, &st.regions, &st.faces, &segp) {
                Some(r) => {
                    let k = seg.regions[r as usize].prim.kind();
                    Ok(format!("{} regions merged into one {}", st.regions.len().max(1), k.label().to_lowercase()))
                }
                None => Err("Paint over two regions or more to merge them".into()),
            },
            Tool::Brush(BrushMode::Split) => match edit::split(m, topo, seg, &st.faces) {
                0 => Err("Paint across a region, from one side to the other, to cut it".into()),
                n => Ok(format!("Cut in {} pieces", n + 1)),
            },
            Tool::Brush(BrushMode::Free) => match edit::erase(m, seg, &st.faces) {
                0 => Err("Nothing to free there".into()),
                n => Ok(format!("{n} faces freed")),
            },
            Tool::Shape(k) => {
                let faces: Vec<u32> = st.faces.iter().copied().collect();
                edit::place(m, topo, seg, &faces, Some(k), st.direction(), &segp).map(|r| {
                    let reg = &seg.regions[r as usize];
                    format!("{} placed: {} mm², {}", k.label(), reg.area.round(), reg.prim.describe())
                })
            }
        };
        match msg {
            Ok(text) => {
                self.undo.push(before);
                if self.undo.len() > 12 {
                    self.undo.remove(0);
                }
                self.redo.clear();
                self.after_edit();
                self.toast(text, false);
            }
            Err(text) => {
                *seg = before;
                self.toast(text, true);
            }
        }
    }

    /// The regions changed by hand: the later steps are out of date (run
    /// again when the result is shown).
    fn after_edit(&mut self) {
        if let Some(p) = &mut self.pipe {
            p.clear_from(Step::Regularize);
        }
        self.edited = true;
        self.gen += 1;
        if let Some(r) = self.selected {
            let gone = self.pipe.as_ref().and_then(|p| p.seg_raw.as_ref()).map(|s| s.regions.get(r as usize).map(|x| x.faces.is_empty()).unwrap_or(true)).unwrap_or(true);
            if gone {
                self.selected = None;
            }
        }
    }

    pub fn undo(&mut self) {
        if !self.can_edit() {
            return;
        }
        let Some(prev) = self.undo.pop() else {
            self.toast("Nothing to undo", false);
            return;
        };
        let seg = self.pipe.as_mut().and_then(|p| p.seg_raw.as_mut()).unwrap();
        self.redo.push(std::mem::replace(seg, prev));
        self.after_edit();
    }

    pub fn redo(&mut self) {
        if !self.can_edit() {
            return;
        }
        let Some(next) = self.redo.pop() else {
            self.toast("Nothing to redo", false);
            return;
        };
        let seg = self.pipe.as_mut().and_then(|p| p.seg_raw.as_mut()).unwrap();
        self.undo.push(std::mem::replace(seg, next));
        self.after_edit();
    }

    /// Gives the picked region another shape (fitted on its faces), or frees
    /// it (`None`).
    pub fn retype(&mut self, kind: Option<Kind>) {
        let Some(r) = self.selected else { return };
        if !self.can_edit() {
            return;
        }
        let Some(p) = &mut self.pipe else { return };
        let Pipeline { original, sym_mesh, smooth_mesh, seg_raw, .. } = p;
        let mesh: &Mesh = smooth_mesh.as_ref().or(sym_mesh.as_ref()).unwrap_or(original);
        let Some(seg) = seg_raw.as_mut() else { return };
        let before = seg.clone();
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
                let s: Vec<Sample> = edit::samples(mesh, &reg.faces);
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
        self.undo.push(before);
        self.redo.clear();
        self.after_edit();
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
