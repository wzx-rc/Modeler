//! The app's state: the scan (it never changes), the NURBS objects laid on
//! it (undo / redo), the tool in hand, the view.

use crate::camera::OrbitCamera;
use crate::view::View;
use glam::{DVec2, DVec3, Vec3};
use rayon::prelude::*;
use rectify_core::circle::ProfileKind;
use rectify_core::mesh::Surface as Closest;
use rectify_core::nurbs::{Curve, Surface};
use rectify_core::patch::{self, PatchParams, Patched};
use rectify_core::path::{self, Built, PathParams, Snap, SnapKind};
use rectify_core::slot::{Kind, Laid, Slot, SlotParams};
use rectify_core::{Mesh, Topo};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::OnceLock;

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
    /// Click points on the scan: one NURBS curve through them, caught by
    /// the flat faces, their edges and corners.
    Path,
    /// Drag along a slot with round ends: a NURBS surface on its wall,
    /// every row an exact slot outline.
    Slot,
    /// Drag across a block with rounded corners (raised or cut in), corner
    /// to corner: the same, every row an exact rounded rectangle.
    Box,
    /// Draw on the scan with a pencil: the stroke becomes a path (straight
    /// stretches, curves, caught by the faces, edges and corners), its ends
    /// joining the paths already there.
    Line,
    /// Click the paths around a part of the scan: one NURBS surface fills
    /// the loop they close, laid on the scan.
    Patch,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Navigate => "Navigate",
            Tool::Circle => "Circle",
            Tool::Path => "Path",
            Tool::Slot => "Slot",
            Tool::Box => "Box",
            Tool::Line => "Line",
            Tool::Patch => "Patch",
        }
    }
}

/// The scan open, and what does not change.
pub struct Doc {
    pub mesh: Mesh,
    pub topo: Topo,
    /// Median edge length (mm).
    pub edge: f64,
    /// The closest point on the scan, made the first time it is needed.
    closest: OnceLock<Closest>,
}

impl Doc {
    pub fn closest(&self) -> &Closest {
        self.closest.get_or_init(|| Closest::new(&self.mesh))
    }
}

/// What a NURBS object is.
pub enum Shape {
    Surface(Surface),
    Curve(Curve),
}

/// A NURBS object laid on the scan.
pub struct Object {
    pub tool: Tool,
    /// What it is, in a few words (the list on the left).
    pub what: String,
    pub shape: Shape,
}

/// The path being drawn: its points (where they went, what caught them),
/// its settings (live) and the curve made.
pub struct PathEdit {
    pub pts: Vec<Snap>,
    pub params: PathParams,
    pub built: Option<Built>,
    /// The curve with the point under the pointer added (the next segment).
    pub preview: Option<Built>,
}

/// A press in the Path tool: on one of the path's points (to move it), or
/// not (a click adds a point).
pub struct PathPress {
    pub at: [f32; 2],
    pub point: Option<usize>,
    pub moved: bool,
}

/// A pencil stroke being drawn (the Line tool): the pointer's positions on
/// screen and the scan under them (triangle, point).
pub struct Stroke {
    pub scr: Vec<[f32; 2]>,
    pub hits: Vec<(u32, DVec3)>,
}

/// The Patch being made: the paths picked (their places in `objects`), its
/// settings (live) and the surface made, or why not. A click inside a cell
/// of the paths gives its loop (`lines`, runs of one path each); else the
/// loop is found from the paths picked.
pub struct PatchEdit {
    pub picked: Vec<usize>,
    pub lines: Option<Vec<Vec<DVec3>>>,
    pub params: PatchParams,
    pub made: Result<Patched, String>,
}

/// A drag on the scan (Circle, Slot, Box): from the press to the pointer.
#[derive(Clone)]
pub struct Drag {
    pub a: DVec3,
    pub b: DVec3,
    /// The triangle pressed.
    pub tri: u32,
    /// The plane's normal at the press.
    pub n: DVec3,
}

/// The slot found by the Slot tool, the surface laid on it and its
/// settings (changed live; kept with Enter).
pub struct SlotEdit {
    pub slot: Slot,
    pub params: SlotParams,
    pub laid: Laid,
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
    pub path: Option<PathEdit>,
    /// What the pointer would place (the Path tool), and where it was.
    pub path_hover: Option<Snap>,
    hover_key: Option<([f32; 2], [f32; 4], bool)>,
    pub path_press: Option<PathPress>,
    /// The Path's settings, kept from one to the next.
    pub path_params: PathParams,
    /// The Slot drag (from one end to the other), the surface placed, its
    /// profile kept from one to the next.
    pub slot_drag: Option<Drag>,
    pub slot: Option<SlotEdit>,
    pub slot_profile: ProfileKind,
    /// The Line's stroke being drawn.
    pub stroke: Option<Stroke>,
    /// The Patch being made, its settings kept from one to the next, the
    /// path under the pointer.
    pub patch: Option<PatchEdit>,
    pub patch_params: PatchParams,
    pub patch_hover: Option<usize>,
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
            path: None,
            path_hover: None,
            hover_key: None,
            path_press: None,
            path_params: PathParams::default(),
            slot_drag: None,
            slot: None,
            slot_profile: ProfileKind::Cylinder,
            stroke: None,
            patch: None,
            patch_params: PatchParams::default(),
            patch_hover: None,
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
        Doc { mesh: m, topo, edge, closest: OnceLock::new() }
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
        self.path = None;
        self.path_hover = None;
        self.path_press = None;
        self.slot = None;
        self.slot_drag = None;
        self.stroke = None;
        self.patch = None;
        self.patch_hover = None;
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
        self.path_apply();
        self.path_hover = None;
        self.path_press = None;
        self.slot_apply();
        self.slot_drag = None;
        self.stroke = None;
        self.patch_apply();
        self.patch_hover = None;
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
        if self.path.is_some() {
            self.path_back();
            return;
        }
        if self.slot.is_some() {
            self.slot_cancel();
            self.toast("Removed", false);
            return;
        }
        if self.patch.is_some() {
            self.patch_cancel();
            self.toast("Removed", false);
            return;
        }
        let Some(o) = self.objects.pop() else {
            self.toast("Nothing to undo", false);
            return;
        };
        self.toast(format!("Undone: {} {}", o.tool.label(), o.what), false);
        self.undone.push(o);
        self.patch_hover = None;
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

    // ------------------------------------------------------------ drags

    /// A drag starting on the scan (Circle, Slot, Box): where, and the plane
    /// under it.
    fn drag_start(&self, at: [f32; 2]) -> Option<Drag> {
        let (t, x) = self.pick_hit(at)?;
        let seeds = self.tri_verts(t);
        let d = self.doc.as_ref()?;
        let n = seeds.iter().flat_map(|&v| d.topo.tris_of(v as usize).iter()).map(|&t| d.mesh.tri_cross(t as usize).normalize_or_zero()).sum::<DVec3>().normalize_or_zero();
        Some(Drag { a: x, b: x, tri: t as u32, n })
    }

    /// Where a drag goes: on the plane under the press (else the scan
    /// under the pointer).
    fn drag_to(&self, cd: &Drag, at: [f32; 2]) -> Option<DVec3> {
        let (o, dir) = self.view().ray(at[0], at[1]);
        let (o, dir) = (o.as_dvec3(), dir.as_dvec3());
        let dn = dir.dot(cd.n);
        let on_plane = (dn.abs() > 0.05).then(|| o + dir * ((cd.a - o).dot(cd.n) / dn)).filter(|x| (*x - o).dot(dir) > 0.0);
        on_plane.or_else(|| self.pick_hit(at).map(|h| h.1))
    }
}

impl AppState {
    // ------------------------------------------------------------ Path

    /// How far the pointer catches at `x` (mm): 12 points on screen.
    fn catch_at(&self, x: DVec3) -> f64 {
        let v = self.view();
        let (fwd, _, _) = v.basis();
        let depth = (x.as_vec3() - v.eye).dot(fwd).max(1e-3);
        (v.pixel_size_at(depth) * 12.0) as f64
    }

    /// The point the pointer would place: on the scan under it, caught by
    /// a corner, an edge or a face near it (`free`: the scan only).
    pub fn snap_at(&self, at: [f32; 2], free: bool) -> Option<Snap> {
        let (t, x) = self.pick_hit(at)?;
        let d = self.doc.as_ref()?;
        if free || !self.path_params.planes {
            return Some(Snap::scan(x, t as u32));
        }
        Some(path::snap(&d.mesh, &d.topo, t as u32, x, self.catch_at(x), d.edge))
    }

    /// The pointer moved over the scan (no button down).
    pub fn path_hover_at(&mut self, at: [f32; 2], free: bool) {
        let cam = &self.camera;
        let key = (at, [cam.yaw, cam.pitch, cam.dist, cam.target.x + cam.target.y * 3.1 + cam.target.z * 7.3], free);
        if self.hover_key == Some(key) {
            return;
        }
        self.hover_key = Some(key);
        self.path_hover = self.snap_at(at, free);
        self.path_preview();
        self.overlay_gen += 1;
    }

    /// The pointer left the view.
    pub fn path_hover_off(&mut self) {
        if self.path_hover.is_some() || self.hover_key.is_some() {
            self.path_hover = None;
            self.hover_key = None;
            if let Some(pe) = &mut self.path {
                pe.preview = None;
            }
            self.overlay_gen += 1;
        }
    }

    fn path_preview(&mut self) {
        let Some(d) = &self.doc else { return };
        let Some(pe) = &mut self.path else { return };
        pe.preview = match (&self.path_hover, pe.params.closed) {
            (Some(h), false) => {
                let mut pts = pe.pts.clone();
                pts.push(h.clone());
                path::build(&d.mesh, &d.topo, d.closest(), &pts, &pe.params, d.edge)
            }
            _ => None,
        };
    }

    /// The path's points on screen.
    pub fn path_screen(&self) -> Vec<[f32; 2]> {
        let v = self.view();
        let Some(pe) = &self.path else { return vec![] };
        pe.pts.iter().map(|s| v.project(if pe.params.planes { s.p } else { s.hit }.as_vec3()).unwrap_or([-1e6, -1e6])).collect()
    }

    /// The nearest of the path's points within 9 points of `at`.
    fn path_point_at(&self, at: [f32; 2]) -> Option<usize> {
        let scr = self.path_screen();
        let d2 = |p: [f32; 2]| (p[0] - at[0]).powi(2) + (p[1] - at[1]).powi(2);
        (0..scr.len()).filter(|&i| d2(scr[i]) <= 81.0).min_by(|&a, &b| d2(scr[a]).partial_cmp(&d2(scr[b])).unwrap())
    }

    pub fn path_press_at(&mut self, at: [f32; 2]) {
        let point = self.path_point_at(at);
        self.path_press = Some(PathPress { at, point, moved: false });
    }

    /// The button down, the pointer moving: a point of the path follows.
    pub fn path_drag(&mut self, at: [f32; 2], free: bool) {
        let Some(pr) = &mut self.path_press else { return };
        if !pr.moved && (at[0] - pr.at[0]).hypot(at[1] - pr.at[1]) < 3.0 {
            return;
        }
        pr.moved = true;
        let Some(i) = pr.point else { return };
        let Some(s) = self.snap_at(at, free) else { return };
        if let Some(pe) = &mut self.path {
            if i < pe.pts.len() {
                pe.pts[i] = s;
            }
        }
        self.path_hover = None;
        self.path_rebuild();
    }

    /// The button up: a click adds a point (on the first one: closes the
    /// path); a drag ends.
    pub fn path_release(&mut self, at: [f32; 2], free: bool) {
        let Some(pr) = self.path_press.take() else { return };
        if pr.moved {
            self.hover_key = None;
            return;
        }
        let n = self.path.as_ref().map(|p| p.pts.len()).unwrap_or(0);
        let closed = self.path.as_ref().map(|p| p.params.closed).unwrap_or(false);
        match pr.point {
            Some(0) if n >= 3 && !closed => {
                if let Some(pe) = &mut self.path {
                    pe.params.closed = true;
                }
                self.path_rebuild();
                self.toast("Path closed: Enter keeps it", false);
            }
            Some(_) => {}
            None => self.path_add(at, free),
        }
    }

    fn path_add(&mut self, at: [f32; 2], free: bool) {
        let Some(s) = self.snap_at(at, free) else { return };
        // a closed path is done: kept, and a new one starts
        if self.path.as_ref().map(|p| p.params.closed).unwrap_or(false) {
            self.path_apply();
        }
        let params = PathParams { closed: false, ..self.path_params };
        let pe = self.path.get_or_insert(PathEdit { pts: vec![], params, built: None, preview: None });
        pe.pts.push(s);
        self.path_rebuild();
    }

    /// The curve again (points or settings changed).
    pub fn path_rebuild(&mut self) {
        let Some(d) = &self.doc else { return };
        let Some(pe) = &mut self.path else { return };
        if pe.pts.len() < 3 {
            pe.params.closed = false;
        }
        pe.built = path::build(&d.mesh, &d.topo, d.closest(), &pe.pts, &pe.params, d.edge);
        self.path_params = PathParams { closed: false, ..pe.params };
        self.path_preview();
        self.objects_gen += 1;
        self.overlay_gen += 1;
    }

    /// Settings changed in the panel; the points caught again when the
    /// faces are switched on or off.
    pub fn path_set(&mut self, p: PathParams) {
        let Some(pe) = &mut self.path else {
            self.path_params = PathParams { closed: false, ..p };
            return;
        };
        let recatch = p.planes && !pe.params.planes;
        pe.params = p;
        if recatch {
            let catches: Vec<f64> = self.path.as_ref().unwrap().pts.iter().map(|s| self.catch_at(s.hit)).collect();
            if let (Some(d), Some(pe)) = (&self.doc, &mut self.path) {
                for (s, c) in pe.pts.iter_mut().zip(catches) {
                    *s = path::snap(&d.mesh, &d.topo, s.tri, s.hit, c, d.edge);
                }
            }
        }
        self.path_rebuild();
    }

    /// The last point taken away (Backspace, Ctrl+Z).
    pub fn path_back(&mut self) {
        let Some(pe) = &mut self.path else { return };
        if pe.params.closed {
            pe.params.closed = false;
        } else {
            pe.pts.pop();
        }
        if pe.pts.is_empty() {
            self.path = None;
            self.objects_gen += 1;
            self.overlay_gen += 1;
            return;
        }
        self.path_rebuild();
    }

    /// The path drawn is kept (two points at least).
    pub fn path_apply(&mut self) {
        let Some(pe) = self.path.take() else { return };
        self.objects_gen += 1;
        self.overlay_gen += 1;
        let Some(b) = pe.built else { return };
        let corners = pe.pts.iter().filter(|s| pe.params.planes && s.kind != SnapKind::Scan).count();
        let what = format!("{} points{} · {} mm{}", pe.pts.len(), if corners > 0 { format!(" ({corners} caught)") } else { String::new() }, fmt(b.length, 1), if pe.params.closed { " · closed" } else { "" });
        self.toast(format!("Kept: path, {what}"), false);
        let tool = if self.tool == Tool::Line { Tool::Line } else { Tool::Path };
        self.add_object(Object { tool, what, shape: Shape::Curve(b.curve) });
    }

    /// The path drawn taken away.
    pub fn path_cancel(&mut self) {
        if self.path.take().is_some() {
            self.objects_gen += 1;
        }
        self.path_press = None;
        self.overlay_gen += 1;
    }
}

impl AppState {
    // ------------------------------------------------------------ Line

    /// The pencil down on the scan: the path placed before is kept, a
    /// stroke starts.
    pub fn line_press(&mut self, at: [f32; 2]) {
        let Some((t, x)) = self.pick_hit(at) else { return };
        self.path_apply();
        self.stroke = Some(Stroke { scr: vec![at], hits: vec![(t as u32, x)] });
        self.overlay_gen += 1;
    }

    /// The pencil moving: the scan under it, every 2 points or more.
    pub fn line_move(&mut self, at: [f32; 2]) {
        let Some(st) = &self.stroke else { return };
        let last = *st.scr.last().unwrap();
        if (at[0] - last[0]).hypot(at[1] - last[1]) < 2.0 {
            return;
        }
        let Some((t, x)) = self.pick_hit(at) else { return };
        let st = self.stroke.as_mut().unwrap();
        st.scr.push(at);
        st.hits.push((t as u32, x));
        self.overlay_gen += 1;
    }

    /// The pencil up: the stroke becomes a path, set live like a Path
    /// (Enter keeps it; the next stroke keeps it too). Its ends join the
    /// paths already there; ending where it began closes it. `free`: the
    /// scan only, nothing caught.
    pub fn line_release(&mut self, free: bool) {
        let Some(st) = self.stroke.take() else { return };
        self.overlay_gen += 1;
        let scr: Vec<DVec2> = st.scr.iter().map(|p| DVec2::new(p[0] as f64, p[1] as f64)).collect();
        let len: f64 = scr.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        if len < 8.0 {
            return;
        }
        let mut keys = path::stroke_keys(&scr, 45.0, 12.0, 40.0);
        let closed = keys.len() >= 3 && len > 80.0 && (scr[0] - scr[scr.len() - 1]).length() <= 14.0;
        if closed {
            keys.pop();
        }
        let free = free || !self.path_params.planes;
        let catches: Vec<f64> = keys.iter().map(|&(i, _)| self.catch_at(st.hits[i].1)).collect();
        let Some(d) = &self.doc else { return };
        let mut pts: Vec<Snap> = keys
            .iter()
            .zip(&catches)
            .map(|(&(i, through), &c)| {
                let (t, x) = st.hits[i];
                let mut s = if free {
                    Snap::scan(x, t)
                } else if through {
                    path::snap_face(&d.mesh, &d.topo, t, x, c, d.edge)
                } else {
                    path::snap(&d.mesh, &d.topo, t, x, c, d.edge)
                };
                s.through = through;
                s
            })
            .collect();
        // the ends join the paths already there (their ends first)
        if !closed {
            let n = pts.len();
            for k in [0, n - 1] {
                let c = catches[if k == 0 { 0 } else { catches.len() - 1 }];
                if let Some(j) = self.join_at(pts[k].hit, c) {
                    let tol = (d.edge * 1.5).max(0.08) * 3.0;
                    let s = &mut pts[k];
                    s.planes.retain(|pl| pl.dist(j).abs() <= tol + pl.rms * 3.0);
                    s.p = j;
                    s.hit = j;
                    s.kind = SnapKind::Path;
                }
            }
        }
        let params = PathParams { closed, ..self.path_params };
        self.path = Some(PathEdit { pts, params, built: None, preview: None });
        self.path_rebuild();
        if self.path.as_ref().map(|pe| pe.built.is_none()).unwrap_or(false) {
            self.path = None;
            self.toast("No path there: draw on the scan", true);
        }
    }

    /// The nearest point of a path already kept within `catch` of `x`: one
    /// of its ends when one is that close (a little further), else on it.
    fn join_at(&self, x: DVec3, catch: f64) -> Option<DVec3> {
        let mut best: Option<(f64, DVec3)> = None;
        for o in &self.objects {
            let Shape::Curve(c) = &o.shape else { continue };
            let (a, b) = c.domain();
            for e in [c.eval(a), c.eval(b)] {
                let d = (e - x).length();
                if d <= catch * 1.5 && best.map(|(bd, _)| d - catch < bd).unwrap_or(true) {
                    // an end wins over a point on a curve by a catch
                    best = Some((d - catch, e));
                }
            }
            let (_, p) = c.nearest(x);
            let d = (p - x).length();
            if d <= catch && best.map(|(bd, _)| d < bd).unwrap_or(true) {
                best = Some((d, p));
            }
        }
        best.map(|b| b.1)
    }
}

impl AppState {
    // ------------------------------------------------------------ Slot

    pub fn slot_press(&mut self, at: [f32; 2]) {
        let Some(cd) = self.drag_start(at) else { return };
        // a new slot keeps the one placed before
        self.slot_apply();
        self.slot_drag = Some(cd);
        self.overlay_gen += 1;
    }

    pub fn slot_move(&mut self, at: [f32; 2]) {
        let Some(cd) = &self.slot_drag else { return };
        // on the scan under the pointer (the press may be on a slope: its
        // plane would send the end far away), else on the plane
        if let Some(x) = self.pick_hit(at).map(|h| h.1).or_else(|| self.drag_to(cd, at)) {
            self.slot_drag.as_mut().unwrap().b = x;
        }
        self.overlay_gen += 1;
    }

    pub fn slot_release(&mut self) {
        let Some(Drag { a, b, tri, .. }) = self.slot_drag.take() else { return };
        self.overlay_gen += 1;
        let Some(d) = &self.doc else { return };
        let found = match self.tool {
            Tool::Box => Slot::detect_box(&d.mesh, &d.topo, tri, a, b, d.edge),
            Tool::Circle => Slot::detect_round(&d.mesh, &d.topo, tri, a, b, d.edge),
            _ => Slot::detect(&d.mesh, &d.topo, tri, a, b, d.edge),
        };
        match found {
            Ok(s) => {
                // the simplest profile that follows this slot's wall
                let (profile, levels) = s.best_profile(s.depth);
                let params = SlotParams { depth: s.depth, profile, levels };
                let laid = s.nurbs(params);
                self.slot = Some(SlotEdit { slot: s, params, laid });
                self.objects_gen += 1;
            }
            Err(e) => self.toast(e, true),
        }
    }

    /// The surface again after the depth, the profile or the levels changed.
    pub fn slot_refit(&mut self) {
        let Some(se) = &mut self.slot else { return };
        se.params.levels = se.params.levels.clamp(1, 64);
        se.laid = se.slot.nurbs(se.params);
        self.slot_profile = se.params.profile;
        self.objects_gen += 1;
        self.overlay_gen += 1;
    }

    /// Another profile: the fewest levels for it again.
    pub fn slot_set_profile(&mut self, k: ProfileKind) {
        let Some(se) = &mut self.slot else { return };
        se.params.profile = k;
        se.params.levels = se.slot.fewest_levels(se.params.depth, k);
        self.slot_refit();
    }

    /// The surface placed is kept.
    pub fn slot_apply(&mut self) {
        let Some(se) = self.slot.take() else { return };
        let p = se.params;
        let sl = &se.slot;
        let size = if sl.kind == Kind::Round { format!("Ø {}", fmt(sl.width(), 2)) } else { format!("{} × {}", fmt(sl.length(), 2), fmt(sl.width(), 2)) };
        let what = format!("{size} mm · {} mm {} · {} · {} rows", fmt(p.depth, 1), if sl.inward { "deep" } else { "high" }, slot_profile_label(p.profile), p.levels + 1);
        self.toast(format!("Kept: {what}"), false);
        let tool = match sl.kind {
            Kind::Box => Tool::Box,
            Kind::Round => Tool::Circle,
            Kind::Slot => Tool::Slot,
        };
        self.add_object(Object { tool, what, shape: Shape::Surface(se.laid.surface) });
        self.overlay_gen += 1;
    }

    /// The surface placed taken away.
    pub fn slot_cancel(&mut self) {
        if self.slot.take().is_some() {
            self.objects_gen += 1;
        }
        self.overlay_gen += 1;
    }
}

impl AppState {
    // ------------------------------------------------------------ Patch

    /// The kept path within 9 points of `at` on screen (the nearest).
    pub fn curve_at(&self, at: [f32; 2]) -> Option<usize> {
        let v = self.view();
        let mut best: Option<(f32, usize)> = None;
        for (i, o) in self.objects.iter().enumerate() {
            let Shape::Curve(c) = &o.shape else { continue };
            let scr: Vec<[f32; 2]> = patch::polyline(c).iter().filter_map(|x| v.project(x.as_vec3())).collect();
            for w in scr.windows(2) {
                let (a, b) = (w[0], w[1]);
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let l2 = (dx * dx + dy * dy).max(1e-6);
                let t = (((at[0] - a[0]) * dx + (at[1] - a[1]) * dy) / l2).clamp(0.0, 1.0);
                let d = (a[0] + dx * t - at[0]).hypot(a[1] + dy * t - at[1]);
                if d <= 9.0 && best.map(|b| d < b.0).unwrap_or(true) {
                    best = Some((d, i));
                }
            }
        }
        best.map(|b| b.1)
    }

    /// The pointer moved (no button down): the path under it.
    pub fn patch_hover_at(&mut self, at: [f32; 2]) {
        let h = self.curve_at(at);
        if h != self.patch_hover {
            self.patch_hover = h;
            self.overlay_gen += 1;
        }
    }

    /// A click: on a path, the path picked (or no longer); inside a cell of
    /// the paths, its loop filled (the patch placed before is kept).
    pub fn patch_click(&mut self, at: [f32; 2]) {
        let Some(i) = self.curve_at(at) else {
            self.patch_inside(at);
            return;
        };
        let params = self.patch_params;
        let pe = self.patch.get_or_insert(PatchEdit { picked: vec![], lines: None, params, made: Err(String::new()) });
        pe.lines = None;
        if let Some(k) = pe.picked.iter().position(|&j| j == i) {
            pe.picked.remove(k);
        } else {
            pe.picked.push(i);
        }
        if pe.picked.is_empty() {
            self.patch = None;
            self.objects_gen += 1;
            self.overlay_gen += 1;
            return;
        }
        self.patch_rebuild();
    }

    /// The smallest loop of paths around the click, filled.
    fn patch_inside(&mut self, at: [f32; 2]) {
        let Some((_, near)) = self.pick_hit(at) else { return };
        let Some(d) = &self.doc else { return };
        let curves: Vec<(usize, Vec<DVec3>)> = self
            .objects
            .iter()
            .enumerate()
            .filter_map(|(i, o)| match &o.shape {
                Shape::Curve(c) => Some((i, patch::polyline(c))),
                _ => None,
            })
            .collect();
        if curves.is_empty() {
            self.toast("Draw the paths around the patch first (Line), then click inside them", true);
            return;
        }
        let v = self.view();
        let scr = |x: DVec3| v.project(x.as_vec3()).map(|p| DVec2::new(p[0] as f64, p[1] as f64));
        let lines: Vec<Vec<DVec3>> = curves.iter().map(|c| c.1.clone()).collect();
        let tol = (d.edge * 4.0).max(0.3);
        match patch::loop_around(&lines, tol, scr, DVec2::new(at[0] as f64, at[1] as f64), near) {
            Ok((runs, used)) => {
                self.patch_apply();
                let picked = used.iter().map(|&k| curves[k].0).collect();
                self.patch = Some(PatchEdit { picked, lines: Some(runs), params: self.patch_params, made: Err(String::new()) });
                self.patch_rebuild();
            }
            Err(e) => self.toast(e, true),
        }
    }

    /// The surface again (paths picked or settings changed).
    pub fn patch_rebuild(&mut self) {
        let Some(d) = &self.doc else { return };
        let Some(pe) = &mut self.patch else { return };
        let lines: Vec<Vec<DVec3>> = match &pe.lines {
            Some(l) => l.clone(),
            None => pe
                .picked
                .iter()
                .filter_map(|&i| match self.objects.get(i).map(|o| &o.shape) {
                    Some(Shape::Curve(c)) => Some(patch::polyline(c)),
                    _ => None,
                })
                .collect(),
        };
        let tol = (d.edge * 4.0).max(0.3);
        pe.made = patch::loop_sides(&lines, tol).and_then(|sides| patch::patch(&d.mesh, d.closest(), &sides, pe.params, d.edge));
        self.patch_params = pe.params;
        self.objects_gen += 1;
        self.overlay_gen += 1;
    }

    pub fn patch_set(&mut self, p: PatchParams) {
        self.patch_params = p;
        if let Some(pe) = &mut self.patch {
            pe.params = p;
            self.patch_rebuild();
        }
    }

    /// The surface made is kept.
    pub fn patch_apply(&mut self) {
        let Some(pe) = self.patch.take() else { return };
        self.objects_gen += 1;
        self.overlay_gen += 1;
        let Ok(made) = pe.made else { return };
        let what = format!("{} paths · {} × {} spans · {} mm from the scan", pe.picked.len(), made.spans[0], made.spans[1], fmt(made.off, 3));
        self.toast(format!("Kept: patch, {what}"), false);
        self.add_object(Object { tool: Tool::Patch, what, shape: Shape::Surface(made.surface) });
    }

    /// The patch taken away (the paths stay).
    pub fn patch_cancel(&mut self) {
        if self.patch.take().is_some() {
            self.objects_gen += 1;
        }
        self.overlay_gen += 1;
    }
}

/// The Slot's profiles, as its walls go down.
pub fn slot_profile_label(p: ProfileKind) -> &'static str {
    match p {
        ProfileKind::Cylinder => "straight",
        ProfileKind::Cone => "tapered",
        ProfileKind::Curve => "curve",
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
