//! The app's state: the scan (it never changes), the NURBS objects laid on
//! it (undo / redo), the tool in hand, the view.

use crate::camera::OrbitCamera;
use crate::view::View;
use glam::{DVec3, Vec3};
use rayon::prelude::*;
use rectify_core::circle::{Circle, ProfileKind, Round, RoundParams};
use rectify_core::nurbs::Surface;
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
    /// Drag across a round part: a NURBS surface on its wall, its rings
    /// exact circles.
    Circle,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Navigate => "Navigate",
            Tool::Circle => "Circle",
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

/// A NURBS object laid on the scan.
pub struct Object {
    pub tool: Tool,
    /// What it is, in a few words (the list on the left).
    pub what: String,
    pub surface: Surface,
}

/// A Circle drag: from the press on the rim to the pointer, on the plane
/// under the press.
#[derive(Clone)]
pub struct CircleDrag {
    pub a: DVec3,
    pub b: DVec3,
    pub seeds: Vec<u32>,
    /// The plane's normal at the press.
    pub n: DVec3,
}

/// The round part found by the Circle tool, the surface laid on it and its
/// settings (changed live; kept with Enter).
pub struct CircleEdit {
    pub circle: Circle,
    pub params: RoundParams,
    pub round: Round,
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
    /// The NURBS objects made, in order, and the ones undone.
    pub objects: Vec<Object>,
    pub undone: Vec<Object>,
    /// Bumped when the objects change (made, kept, undone, being set).
    pub objects_gen: u64,
    /// The scan not drawn: the objects alone (H).
    pub hide_scan: bool,
    pub circle_drag: Option<CircleDrag>,
    pub circle: Option<CircleEdit>,
    /// The Circle's profile, kept from one to the next.
    pub circle_profile: ProfileKind,
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
            objects: vec![],
            undone: vec![],
            objects_gen: 1,
            hide_scan: false,
            circle_drag: None,
            circle: None,
            circle_profile: ProfileKind::Cylinder,
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
        self.objects.clear();
        self.undone.clear();
        self.circle = None;
        self.circle_drag = None;
        self.hide_scan = false;
        self.objects_gen += 1;
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
        // what is placed is kept
        self.circle_apply();
        self.circle_drag = None;
        self.tool = t;
        self.overlay_gen += 1;
    }

    /// The vertices of a triangle.
    pub fn tri_verts(&self, t: usize) -> Vec<u32> {
        self.doc.as_ref().map(|d| d.mesh.tris[t].to_vec()).unwrap_or_default()
    }

    // ------------------------------------------------------------ objects

    pub fn add_object(&mut self, o: Object) {
        self.objects.push(o);
        self.undone.clear();
        self.objects_gen += 1;
    }

    pub fn undo(&mut self) {
        // what is placed and not kept goes first
        if self.circle.is_some() {
            self.circle_cancel();
            self.toast("Circle removed", false);
            return;
        }
        let Some(o) = self.objects.pop() else {
            self.toast("Nothing to undo", false);
            return;
        };
        self.toast(format!("Undone: {} {}", o.tool.label(), o.what), false);
        self.undone.push(o);
        self.objects_gen += 1;
    }

    pub fn redo(&mut self) {
        let Some(o) = self.undone.pop() else {
            self.toast("Nothing to redo", false);
            return;
        };
        self.toast(format!("Redone: {} {}", o.tool.label(), o.what), false);
        self.objects.push(o);
        self.objects_gen += 1;
    }

    // ------------------------------------------------------------ Circle

    pub fn circle_press(&mut self, at: [f32; 2]) {
        let Some((t, x)) = self.pick_hit(at) else { return };
        let seeds = self.tri_verts(t);
        // a new circle keeps the one placed before
        self.circle_apply();
        let Some(d) = &self.doc else { return };
        let n = seeds.iter().flat_map(|&v| d.topo.tris_of(v as usize).iter()).map(|&t| d.mesh.tri_cross(t as usize).normalize_or_zero()).sum::<DVec3>().normalize_or_zero();
        self.circle_drag = Some(CircleDrag { a: x, b: x, seeds, n });
        self.overlay_gen += 1;
    }

    pub fn circle_move(&mut self, at: [f32; 2]) {
        if self.circle_drag.is_none() {
            return;
        }
        // on the plane under the press (else the scan under the pointer)
        let (o, dir) = self.view().ray(at[0], at[1]);
        let (o, dir) = (o.as_dvec3(), dir.as_dvec3());
        let hit = self.pick_hit(at).map(|h| h.1);
        let Some(cd) = &mut self.circle_drag else { return };
        let dn = dir.dot(cd.n);
        let on_plane = (dn.abs() > 0.05).then(|| o + dir * ((cd.a - o).dot(cd.n) / dn)).filter(|x| (*x - o).dot(dir) > 0.0);
        if let Some(x) = on_plane.or(hit) {
            cd.b = x;
        }
        self.overlay_gen += 1;
    }

    pub fn circle_release(&mut self) {
        let Some(CircleDrag { a, b, seeds, .. }) = self.circle_drag.take() else { return };
        self.overlay_gen += 1;
        let Some(d) = &self.doc else { return };
        match Circle::detect(&d.mesh, &d.topo, &seeds, a, b, d.edge) {
            Ok(c) => {
                // rings about a third of the radius apart
                let gap = (c.r / 3.0).max(d.edge * 4.0);
                let params = RoundParams { height: c.height, profile: self.circle_profile, levels: ((c.height / gap).round() as usize).clamp(1, 24) };
                let round = c.nurbs(&d.mesh, &d.topo, params);
                self.circle = Some(CircleEdit { circle: c, params, round });
                self.objects_gen += 1;
            }
            Err(e) => self.toast(e, true),
        }
    }

    /// The surface again after the height, the profile or the levels changed.
    pub fn circle_refit(&mut self) {
        let (Some(d), Some(ce)) = (&self.doc, &mut self.circle) else { return };
        ce.params.levels = ce.params.levels.clamp(1, 64);
        ce.round = ce.circle.nurbs(&d.mesh, &d.topo, ce.params);
        self.circle_profile = ce.params.profile;
        self.objects_gen += 1;
        self.overlay_gen += 1;
    }

    /// The surface placed is kept.
    pub fn circle_apply(&mut self) {
        let Some(ce) = self.circle.take() else { return };
        let p = ce.params;
        let what = format!("Ø {} mm · {} mm · {} · {} rings", fmt(ce.circle.r * 2.0, 2), fmt(p.height, 1), profile_label(p.profile), p.levels + 1);
        self.toast(format!("Kept: {what}"), false);
        self.add_object(Object { tool: Tool::Circle, what, surface: ce.round.surface });
        self.overlay_gen += 1;
    }

    /// The surface placed taken away.
    pub fn circle_cancel(&mut self) {
        if self.circle.take().is_some() {
            self.objects_gen += 1;
        }
        self.overlay_gen += 1;
    }
}

pub fn profile_label(p: ProfileKind) -> &'static str {
    match p {
        ProfileKind::Cylinder => "cylinder",
        ProfileKind::Cone => "cone",
        ProfileKind::Curve => "curve",
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
