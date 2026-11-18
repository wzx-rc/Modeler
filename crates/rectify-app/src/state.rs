//! The app's state: the mesh being rectified (a `Pipeline`), the steps'
//! settings, the job running in the background, the view and what is picked.

use crate::camera::OrbitCamera;
use crate::view::View;
use glam::{DVec3, Vec3};
use rectify_core::edit;
use rectify_core::script::{self, Cmd, Entry, Ref, Script};
use rectify_core::segment::Segmentation;
use rectify_core::{Kind, Mesh, Params, Pipeline, Prim, Stage, Step, NONE};
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

/// How the shown stage is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    /// The mesh in clay.
    Surface,
    /// The regions it was made with, one colour each (planes cool, curved
    /// warm, free faces dark).
    Regions,
    /// The diff map against the AI mesh: grey untouched, orange moved, red
    /// rougher than the AI mesh.
    Diff,
}

impl Style {
    pub const ALL: [Style; 3] = [Style::Surface, Style::Regions, Style::Diff];
    pub fn label(self) -> &'static str {
        match self {
            Style::Surface => "Surface",
            Style::Regions => "Regions",
            Style::Diff => "Diff",
        }
    }
    pub fn next(self) -> Style {
        match self {
            Style::Surface => Style::Regions,
            Style::Regions => Style::Diff,
            Style::Diff => Style::Surface,
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
    /// The stage shown (0 scan, 1 algorithm, 2 cleaned, 3 AI) and how.
    pub stage: Stage,
    pub style: Style,
    /// Space held: the scan shown whatever the stage.
    pub compare: bool,
    /// Bumped whenever what is drawn changes.
    pub gen: u64,
    /// The step whose settings the inspector shows (or stage 3's edits).
    pub step: Step,
    pub edits_open: bool,
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
    /// The edited regions and edits before each hand edit, and the ones
    /// undone.
    pub undo: Vec<(Segmentation, Vec<Entry>)>,
    pub redo: Vec<(Segmentation, Vec<Entry>)>,
    /// Edits not carried to stage 3's mesh yet (done when it is shown).
    pub edited: bool,
    /// Requests from the MCP bridge (Claude Desktop) waiting for an answer.
    pub remote: crate::remote::Pending,
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
            stage: Stage::Scan,
            style: Style::Surface,
            compare: false,
            gen: 1,
            step: Step::Symmetry,
            edits_open: false,
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
            remote: Default::default(),
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

    /// The stage actually drawn (Space shows the scan).
    pub fn shown(&self) -> Stage {
        if self.compare {
            Stage::Scan
        } else {
            self.stage
        }
    }

    /// Shows a stage; computes it first when it is not (then it is only a
    /// switch).
    pub fn set_stage(&mut self, s: Stage) {
        if self.stage != s {
            self.stage = s;
            self.gen += 1;
        }
        let need = self.pipe.as_ref().map(|p| !p.ready(s)).unwrap_or(false);
        if need && !self.busy() && !(s == Stage::Guided && self.style == Style::Regions) {
            self.run_stage(s);
        }
    }

    pub fn set_style(&mut self, s: Style) {
        if self.style != s {
            self.style = s;
            self.gen += 1;
        }
        // regions are drawn from the edits, the rest needs the mesh
        let st = self.stage;
        if s != Style::Regions {
            self.set_stage(st);
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
            let r = Mesh::load_obj(&p).map(|m| {
                let mut pipe = Pipeline::new(m);
                // the edits saved next to the mesh (AI pass, hand tools)
                if let Ok(s) = Script::load(&Script::path_for(&p)) {
                    pipe.script = s;
                }
                Box::new(pipe)
            });
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
        self.stage = Stage::Scan;
        self.style = Style::Surface;
        self.step = Step::Symmetry;
        self.gen += 1;
        self.frame_all();
        let n = self.pipe.as_ref().map(|p| p.script.edits.len()).unwrap_or(0);
        if n > 0 {
            let ai = self.pipe.as_ref().map(|p| p.script.count("ai")).unwrap_or(0);
            self.toast(format!("{n} edits loaded for stage 3 ({ai} by the AI pass)"), false);
        }
    }

    /// The edits' file next to the mesh (None: no file, the harness).
    pub fn script_path(&self) -> Option<PathBuf> {
        self.file.as_ref().map(|f| Script::path_for(f))
    }

    /// Saves the edits next to the mesh.
    pub fn save_script(&mut self) {
        let (Some(path), Some(p)) = (self.script_path(), &self.pipe) else { return };
        let mut sc = p.script.clone();
        sc.mesh = self.file.as_ref().and_then(|f| f.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if let Err(e) = sc.save(&path) {
            self.toast(format!("Could not save the edits: {e}"), true);
        }
    }

    pub fn export(&mut self) {
        let Some(p) = &self.pipe else { return };
        let stage = if self.stage == Stage::Scan { Stage::Cleaned } else { self.stage };
        let Some(m) = p.stage_mesh(stage) else {
            self.toast("Nothing to export: run the steps first", true);
            return;
        };
        let m = m.clone();
        let path = crate::files::documents_dir().join(format!("{} stage {}.obj", crate::files::sanitize(&self.name), stage.number()));
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r = std::fs::write(&path, m.obj(None)).map(|_| path.clone()).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Exported(r));
        });
        self.job = Some(Job { rx, what: "Exporting".into(), step: None, progress: 0.0 });
    }

    // ------------------------------------------------------------ steps

    /// Runs a job on the pipeline in the background.
    fn spawn(&mut self, what: String, first: Option<Step>, f: impl FnOnce(&mut Pipeline, &Params, &(dyn Fn(Step, f32) + Sync)) + Send + 'static) {
        let Some(mut pipe) = self.pipe.take() else {
            self.toast("Open an .obj first", true);
            return;
        };
        self.stroke = None;
        let params = self.params;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let tx2 = std::sync::Mutex::new(tx.clone());
            f(&mut pipe, &params, &move |s, x| {
                let _ = tx2.lock().unwrap().send(Msg::Progress(s, x));
            });
            let _ = tx.send(Msg::Done(Box::new(pipe)));
        });
        self.job = Some(Job { rx, what, step: first, progress: 0.0 });
    }

    /// Runs `from` and every step after it in the background (stage 3's
    /// lane too when there are edits).
    pub fn run_from(&mut self, from: Step) {
        if self.busy() {
            return;
        }
        let Some(pipe) = &self.pipe else {
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
        }
        self.edited = false;
        self.spawn(from.label().to_string(), Some(from), move |p, params, prog| p.run_from(from, params, prog));
    }

    /// Computes what a stage needs (nothing when it is ready).
    pub fn run_stage(&mut self, stage: Stage) {
        if self.busy() {
            return;
        }
        let Some(first) = self.pipe.as_ref().and_then(|p| p.next_for(stage)) else { return };
        if stage == Stage::Guided {
            self.edited = false;
        }
        self.spawn(format!("Stage {}", stage.number()), Some(first), move |p, params, prog| p.run_stage(stage, params, prog));
    }

    /// Runs what is out of date (every step the first time), up to the
    /// stage shown (at least stage 2).
    pub fn run_all(&mut self) {
        let target = if self.stage == Stage::Guided { Stage::Guided } else { Stage::Cleaned };
        match self.pipe.as_ref().map(|p| p.next_for(target)) {
            Some(Some(_)) => {
                if self.stage == Stage::Scan {
                    self.stage = Stage::Cleaned;
                    self.gen += 1;
                }
                self.run_stage(target)
            }
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
        // edits reach stage 3's mesh when it is looked at
        if self.edited && self.job.is_none() && self.stroke.is_none() && self.shown() == Stage::Guided && self.style != Style::Regions {
            self.edited = false;
            self.run_stage(Stage::Guided);
        }
        crate::remote::poll(self);
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
                if self.stage == Stage::Scan {
                    self.stage = Stage::Cleaned;
                }
                self.check_log();
            }
            Msg::Exported(Ok(path)) => self.toast(format!("Exported to {}", path.display()), false),
            Msg::Exported(Err(e)) => self.toast(format!("Export failed: {e}"), true),
            Msg::Progress(..) => {}
        }
        crate::remote::poll(self);
    }

    /// Edits that failed when replayed: said once.
    fn check_log(&mut self) {
        let failed = self.pipe.as_ref().map(|p| p.edit_log.iter().filter(|r| r.is_err()).count()).unwrap_or(0);
        if failed > 0 {
            self.toast(format!("{failed} edits could not be applied (see Edits)"), true);
        }
    }

    // ------------------------------------------------------------ view

    /// The mesh drawn: the shown stage's, or (not computed yet) the latest
    /// one before it.
    pub fn shown_mesh(&self) -> Option<&Mesh> {
        let p = self.pipe.as_ref()?;
        let order = match self.shown() {
            Stage::Scan => vec![Stage::Scan],
            Stage::Algorithm => vec![Stage::Algorithm, Stage::Scan],
            Stage::Cleaned => vec![Stage::Cleaned, Stage::Algorithm, Stage::Scan],
            Stage::Guided => vec![Stage::Guided, Stage::Cleaned, Stage::Algorithm, Stage::Scan],
        };
        if self.style == Style::Regions && self.shown() == Stage::Scan {
            return Some(p.smooth_out());
        }
        order.into_iter().find_map(|s| p.stage_mesh(s))
    }

    /// The regions drawn (Regions style) and picked: the shown stage's;
    /// stage 3's are the edited ones.
    pub fn shown_seg(&self) -> Option<&Segmentation> {
        let p = self.pipe.as_ref()?;
        match self.shown() {
            Stage::Scan => {
                if self.style == Style::Regions {
                    p.seg_raw.as_ref()
                } else {
                    None
                }
            }
            Stage::Guided => p.seg_edit.as_ref().or(p.stage_seg(Stage::Guided)),
            s => p.stage_seg(s),
        }
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
            // the tools edit stage 3's regions
            self.selected = None;
            self.ensure_edit();
            self.stage = Stage::Guided;
            self.style = Style::Regions;
            self.gen += 1;
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
        let m = p.smooth_out();
        let label = p.seg_edit.as_ref().or(p.seg_raw.as_ref()).map(|s| &s.label);
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

    /// The edited regions (stage 3), built from the edits when missing.
    fn ensure_edit(&mut self) -> bool {
        let params = self.params;
        let Some(p) = &mut self.pipe else { return false };
        if p.seg_raw.is_none() {
            return false;
        }
        if p.seg_edit.is_none() {
            p.build_edits(&params);
        }
        true
    }

    /// Adds an edit to stage 3 and applies it to the edited regions at once
    /// (region edits) or when stage 3 is computed (the others). Undoable.
    pub fn add_edit(&mut self, e: Entry) -> Result<String, String> {
        if !self.ensure_edit() {
            return Err("Run the steps first: the edits change the regions".into());
        }
        let segp = self.params.seg;
        let p = self.pipe.as_mut().unwrap();
        let before = (p.seg_edit.clone().unwrap(), p.script.edits.clone());
        let Pipeline { original, sym_mesh, smooth_mesh, seg_edit, topo, .. } = p;
        let m: &Mesh = smooth_mesh.as_ref().or(sym_mesh.as_ref()).unwrap_or(original);
        let seg = seg_edit.as_mut().unwrap();
        let res = script::apply_region_edit(m, topo, seg, &e.cmd, &segp);
        match res {
            Err(t) => {
                *seg = before.0;
                Err(t)
            }
            Ok(msg) => {
                p.script.edits.push(e.clone());
                p.edit_log.push(Ok(msg.clone().unwrap_or_default()));
                p.clear_lane(1, Step::Regularize);
                self.undo.push(before);
                if self.undo.len() > 30 {
                    self.undo.remove(0);
                }
                self.redo.clear();
                self.after_edit();
                Ok(msg.unwrap_or_else(|| match &e.cmd {
                    Cmd::SameRadius { .. } | Cmd::Coaxial { .. } | Cmd::Coplanar { .. } => "shape rule added (applied after Regularize)".into(),
                    Cmd::Revolve { .. } | Cmd::NoRevolve { .. } => "round piece rule added (applied in Round)".into(),
                    Cmd::Keep { .. } => "zone kept as the AI mesh (applied in Clean)".into(),
                    _ => "done".into(),
                }))
            }
        }
    }

    /// Removes the edit at `i` (the edits are replayed without it).
    pub fn remove_edit(&mut self, i: usize) {
        let Some(p) = &mut self.pipe else { return };
        if i >= p.script.edits.len() {
            return;
        }
        let before = (p.seg_edit.clone().unwrap_or_else(|| p.seg_raw.clone().unwrap_or(Segmentation { label: vec![], regions: vec![] })), p.script.edits.clone());
        p.script.edits.remove(i);
        p.edits_changed();
        self.undo.push(before);
        self.redo.clear();
        self.ensure_edit();
        self.after_edit();
    }

    /// The stroke applied to stage 3's regions (undoable).
    pub fn stroke_end(&mut self) {
        let Some(st) = self.stroke.take() else { return };
        self.stroke_gen += 1;
        if st.faces.is_empty() {
            return;
        }
        let mut faces: Vec<u32> = st.faces.iter().copied().collect();
        faces.sort_unstable();
        let hint = st.direction().map(|d| d.to_array());
        let cmd = match self.tool {
            Tool::Select => return,
            Tool::Brush(BrushMode::Merge) => {
                if st.regions.is_empty() {
                    self.toast("Paint over two regions or more to merge them", true);
                    return;
                }
                Cmd::Paint { mode: "merge".into(), faces, regions: st.regions.clone() }
            }
            Tool::Brush(BrushMode::Split) => Cmd::Paint { mode: "split".into(), faces, regions: vec![] },
            Tool::Brush(BrushMode::Free) => Cmd::Paint { mode: "free".into(), faces, regions: vec![] },
            Tool::Shape(k) => Cmd::Place { kind: k.label().to_lowercase(), faces, hint },
        };
        let tool = self.tool;
        match self.add_edit(Entry { by: "hand".into(), note: String::new(), cmd }) {
            Ok(t) => {
                let t = match tool {
                    Tool::Brush(BrushMode::Merge) => {
                        let k = self.selected_kind_after_merge().unwrap_or_default();
                        format!("{} regions merged into one {k}", st.regions.len().max(1))
                    }
                    Tool::Brush(BrushMode::Split) => t.replace("cut", "Cut"),
                    _ => capitalize(&t),
                };
                self.toast(t, false);
            }
            Err(t) => {
                let t = match tool {
                    Tool::Brush(BrushMode::Merge) => "Paint over two regions or more to merge them".into(),
                    Tool::Brush(BrushMode::Split) => "Paint across a region, from one side to the other, to cut it".into(),
                    Tool::Brush(BrushMode::Free) => "Nothing to free there".into(),
                    _ => capitalize(&t),
                };
                self.toast(t, true);
            }
        }
    }

    /// The kind of the region the last merge made (for its message).
    fn selected_kind_after_merge(&self) -> Option<String> {
        let p = self.pipe.as_ref()?;
        let msg = p.edit_log.last()?.as_ref().ok()?;
        let r: u32 = msg.rsplit(' ').next()?.parse().ok()?;
        let reg = p.seg_edit.as_ref()?.regions.get(r as usize)?;
        Some(reg.prim.kind().label().to_lowercase())
    }

    /// The edits changed: stage 3 is out of date (computed again when its
    /// mesh is shown).
    fn after_edit(&mut self) {
        self.edited = true;
        self.gen += 1;
        self.save_script();
        if let Some(r) = self.selected {
            let gone = self.shown_seg().map(|s| s.regions.get(r as usize).map(|x| x.faces.is_empty()).unwrap_or(true)).unwrap_or(true);
            if gone {
                self.selected = None;
            }
        }
    }

    pub fn undo(&mut self) {
        if !self.can_edit() {
            return;
        }
        let Some((seg, edits)) = self.undo.pop() else {
            self.toast("Nothing to undo", false);
            return;
        };
        let p = self.pipe.as_mut().unwrap();
        let cur = (p.seg_edit.replace(seg).unwrap_or_else(|| p.seg_raw.clone().unwrap()), std::mem::replace(&mut p.script.edits, edits));
        p.clear_lane(1, Step::Regularize);
        p.edit_log.truncate(p.script.edits.len());
        self.redo.push(cur);
        self.after_edit();
    }

    pub fn redo(&mut self) {
        if !self.can_edit() {
            return;
        }
        let Some((seg, edits)) = self.redo.pop() else {
            self.toast("Nothing to redo", false);
            return;
        };
        let p = self.pipe.as_mut().unwrap();
        let cur = (p.seg_edit.replace(seg).unwrap_or_else(|| p.seg_raw.clone().unwrap()), std::mem::replace(&mut p.script.edits, edits));
        p.clear_lane(1, Step::Regularize);
        self.undo.push(cur);
        self.after_edit();
    }

    /// A reference to region `r` of the shown regions, for an edit.
    pub fn region_ref(&self, r: u32) -> Ref {
        let at = self.pipe.as_ref().and_then(|p| {
            let m = p.smooth_out();
            self.shown_seg().and_then(|s| script::point_of(m, s, r))
        });
        Ref { id: Some(r), at }
    }

    /// Gives the picked region another shape (fitted on its faces), or frees
    /// it (`None`): an edit of stage 3.
    pub fn retype(&mut self, kind: Option<Kind>) {
        let Some(r) = self.selected else { return };
        if !self.can_edit() {
            return;
        }
        // the region as stage 3 sees it
        if self.stage != Stage::Guided {
            self.stage = Stage::Guided;
            self.style = Style::Regions;
        }
        let region = self.region_ref(r);
        let kind_s = kind.map(|k| k.label().to_lowercase()).unwrap_or_else(|| "free".into());
        match self.add_edit(Entry { by: "hand".into(), note: String::new(), cmd: Cmd::Retype { region, kind: kind_s } }) {
            Ok(t) => self.toast(capitalize(&t), false),
            Err(t) => self.toast(capitalize(&t), true),
        }
        if kind.is_none() {
            self.selected = None;
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
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
