//! The app's state: the scan (it never changes), the shapes laid on it (the
//! Pen's and the Circle's bodies, built by OpenCascade; the Slot's and the
//! Box's NURBS surfaces), undo / redo by copies, the tool in hand, the
//! selection by mode (Vertex, Edge, Face, Object), the view.

use crate::body::{self, Base, Body, EdgeRef, Frame, Step};
use crate::camera::OrbitCamera;
use crate::view::View;
use glam::{DVec2, DVec3, Vec3};
use rayon::prelude::*;
use rectify_core::circle::ProfileKind;
use rectify_core::nurbs::Surface;
use rectify_core::planes;
use rectify_core::slot::{Kind, Laid, Slot, SlotParams};
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
    /// Pick by mode; Shift + drag pulls what is picked (a thickness, an
    /// open edge, a fillet); a plain drag turns around.
    Select,
    /// Click points: a straight outline on the plane found under the
    /// first one; closed on its first point, it is filled.
    Pen,
    /// Drag across a round part, freely: released, it is found on the scan
    /// and made a cylinder (else a flat disc as drawn).
    Circle,
    /// Drag along a slot with round ends: a NURBS surface on its wall.
    Slot,
    /// Drag across a block with rounded corners: the same.
    Box,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Pen => "Pen",
            Tool::Circle => "Circle",
            Tool::Slot => "Slot",
            Tool::Box => "Box",
        }
    }
}

/// What a click picks (Plasticity's component modes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelMode {
    Vertex,
    Edge,
    Face,
    Object,
}

impl SelMode {
    pub const ALL: [SelMode; 4] = [SelMode::Vertex, SelMode::Edge, SelMode::Face, SelMode::Object];

    pub fn label(self) -> &'static str {
        match self {
            SelMode::Vertex => "Vertex",
            SelMode::Edge => "Edge",
            SelMode::Face => "Face",
            SelMode::Object => "Object",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            SelMode::Vertex => "1",
            SelMode::Edge => "2",
            SelMode::Face => "3",
            SelMode::Object => "4",
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

/// What a shape is.
#[derive(Clone)]
pub enum Shape {
    /// The Slot's and the Box's surface.
    Surface(Surface),
    /// The Pen's and the Circle's: an outline and its steps, by OpenCascade.
    Body(Body),
}

/// A shape laid on the scan.
#[derive(Clone)]
pub struct Object {
    pub tool: Tool,
    /// What it is, in a few words (the list on the left).
    pub what: String,
    pub shape: Shape,
}

impl Object {
    pub fn body(&self) -> Option<&Body> {
        match &self.shape {
            Shape::Body(b) => Some(b),
            _ => None,
        }
    }
}

/// Something picked: an object, one of its faces, edges or vertices (a
/// corner of its outline), or a step (a fillet's faces).
#[derive(Clone, Debug, PartialEq)]
pub enum Comp {
    Object(usize),
    Face { obj: usize, tag: i64, at: DVec3 },
    Edge { obj: usize, edge: EdgeRef },
    Corner { obj: usize, i: usize },
    Vertex { obj: usize, at: DVec3 },
    Step { obj: usize, k: usize },
}

impl Comp {
    pub fn obj(&self) -> usize {
        match self {
            Comp::Object(o) => *o,
            Comp::Face { obj, .. } | Comp::Edge { obj, .. } | Comp::Corner { obj, .. } | Comp::Vertex { obj, .. } | Comp::Step { obj, .. } => *obj,
        }
    }

    fn fits(&self, m: SelMode) -> bool {
        match self {
            Comp::Object(_) => m == SelMode::Object,
            Comp::Face { .. } | Comp::Step { .. } => m == SelMode::Face,
            Comp::Edge { .. } => m == SelMode::Edge,
            Comp::Corner { .. } | Comp::Vertex { .. } => m == SelMode::Vertex,
        }
    }
}

/// The plane the Pen would draw on: found under the pointer (a flat face of
/// the scan, else its tangent plane, or a flat face of a shape).
#[derive(Clone, Copy, Debug)]
pub struct PlaneHint {
    pub frame: Frame,
    /// A flat face (of the scan or a shape), not the tangent plane.
    pub flat: bool,
}

/// The outline being drawn: its plane and corners (in the plane's x, y).
#[derive(Clone, Debug)]
pub struct PenEdit {
    pub frame: Frame,
    pub flat: bool,
    pub pts: Vec<DVec2>,
    /// Each point's fillet (a press and a drag on it), 0: sharp.
    pub radii: Vec<f64>,
    /// The point whose fillet a drag is setting, and whether its release
    /// closes the outline (the drag began on the first point).
    pub drag: Option<(usize, bool)>,
}

/// What caught the Pen's point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PenSnap {
    Free,
    /// The first point: closes the outline.
    Close,
    /// A corner of a shape, or a point already placed.
    Point,
    /// Shift: the side locked horizontal, vertical or on a 45° diagonal of
    /// the plane (its angle from the horizontal, 0 to 180°).
    Angle(f64),
}

impl PenSnap {
    pub fn label(self) -> String {
        match self {
            PenSnap::Free => String::new(),
            PenSnap::Close => "Close".into(),
            PenSnap::Point => "Point".into(),
            PenSnap::Angle(a) if a.round() == 0.0 || a.round() == 180.0 => "Horizontal".into(),
            PenSnap::Angle(a) if a.round() == 90.0 => "Vertical".into(),
            PenSnap::Angle(a) => format!("{a:.0}°"),
        }
    }
}

/// Where the Pen's next point would go.
#[derive(Clone, Copy, Debug)]
pub struct PenHover {
    pub q: DVec2,
    pub p: DVec3,
    pub snap: PenSnap,
}

/// A Circle being drawn: rim to rim, on the plane under the press.
#[derive(Clone, Debug)]
pub struct CircleDraw {
    /// Through the press, square to the way most of the scan faces there.
    pub frame: Frame,
    pub tri: u32,
    /// The scan under the press and under the pointer: the diameter drawn.
    pub a: DVec3,
    pub b: DVec3,
}

impl CircleDraw {
    /// The drawn ring's axis: the plane's normal turned square to the
    /// diameter, so the ring goes through both ends.
    pub fn ring_normal(&self) -> DVec3 {
        let u = (self.b - self.a).normalize_or_zero();
        let n = self.frame.n;
        let m = (n - u * n.dot(u)).normalize_or_zero();
        if m == DVec3::ZERO { n } else { m }
    }
}

/// A Shift + drag going on.
#[derive(Clone, Debug)]
pub enum Gesture {
    /// The thickness (step `k`), along `n` from the point pressed `o`.
    Thick { obj: usize, k: usize, o: DVec3, n: DVec3, t0: f64 },
    /// An open edge pulled (step `k`) from the point pressed `a`; `along`:
    /// a straight edge's direction (the pull stays across it).
    Extrude { obj: usize, k: usize, a: DVec3, along: Option<DVec3>, n: DVec3 },
    /// A radius: a fillet (step `k`) or the outline's corners, growing as
    /// the pointer goes away from where it was pressed (`px`: mm a point).
    Radius { obj: usize, k: Option<usize>, corners: Vec<usize>, at: [f32; 2], r0: f64, px: f64, handle: Option<(DVec3, DVec3, f64)> },
}

/// The left button down in the Select tool: where, with Shift, what was
/// under it.
#[derive(Clone, Debug)]
pub struct Press {
    pub at: [f32; 2],
    pub shift: bool,
    pub moved: bool,
    pub comp: Option<Comp>,
    /// On a corner's handle: a plain drag sizes it.
    pub handle: bool,
}

/// A drag on the scan (Slot, Box): from the press to the pointer.
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
    pub mode: SelMode,
    /// The shapes made, in order; the copies before each change (undo) and
    /// after (redo), each with what the change was.
    pub objects: Vec<Object>,
    pub history: Vec<(String, Vec<Object>)>,
    pub future: Vec<(String, Vec<Object>)>,
    /// A change going on (a slider, a drag): one undo step for all of it.
    live: bool,
    /// Bodies built coarse during the live change.
    coarse: Vec<usize>,
    /// Bumped when the objects or what is picked change.
    pub objects_gen: u64,
    /// The scan not drawn: the shapes alone (H).
    pub hide_scan: bool,
    pub sel: Vec<Comp>,
    pub hover: Option<Comp>,
    hover_key: Option<([f32; 2], [f32; 4], u32)>,
    pub press: Option<Press>,
    pub gesture: Option<Gesture>,
    pub pen: Option<PenEdit>,
    /// The plane under the pointer before the Pen's first point.
    pub pen_plane: Option<PlaneHint>,
    pub pen_hover: Option<PenHover>,
    pub circle: Option<CircleDraw>,
    /// The shape just made by a tool: Enter keeps it, Esc removes it.
    pub fresh: Option<usize>,
    /// The Slot drag (from one end to the other), the surface placed, its
    /// profile kept from one to the next.
    pub slot_drag: Option<Drag>,
    pub slot: Option<SlotEdit>,
    pub slot_profile: ProfileKind,
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
            tool: Tool::Select,
            mode: SelMode::Object,
            objects: vec![],
            history: vec![],
            future: vec![],
            live: false,
            coarse: vec![],
            objects_gen: 1,
            hide_scan: false,
            sel: vec![],
            hover: None,
            hover_key: None,
            press: None,
            gesture: None,
            pen: None,
            pen_plane: None,
            pen_hover: None,
            circle: None,
            fresh: None,
            slot_drag: None,
            slot: None,
            slot_profile: ProfileKind::Cylinder,
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
        self.history.clear();
        self.future.clear();
        self.live = false;
        self.sel.clear();
        self.hover = None;
        self.press = None;
        self.gesture = None;
        self.pen = None;
        self.pen_plane = None;
        self.pen_hover = None;
        self.circle = None;
        self.fresh = None;
        self.slot = None;
        self.slot_drag = None;
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

    pub fn ray(&self, at: [f32; 2]) -> (DVec3, DVec3) {
        let (o, d) = self.view().ray(at[0], at[1]);
        (o.as_dvec3(), d.as_dvec3())
    }

    pub fn project(&self, x: DVec3) -> Option<[f32; 2]> {
        self.view().project(x.as_vec3())
    }

    /// The size of a screen point at `x` (mm).
    pub fn px_at(&self, x: DVec3) -> f64 {
        let v = self.view();
        let (fwd, _, _) = v.basis();
        v.pixel_size_at((x.as_vec3() - v.eye).dot(fwd).max(1e-3)) as f64
    }

    /// The scan's triangle under a screen point and where the ray hits it.
    pub fn pick_hit(&self, at: [f32; 2]) -> Option<(usize, DVec3)> {
        let m = &self.doc.as_ref()?.mesh;
        let (o, d) = self.ray(at);
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
        if t != Tool::Select && self.doc.is_none() {
            self.toast("Open a mesh first", true);
            return;
        }
        if t != Tool::Pen {
            // an outline of two points or more becomes a line
            self.pen_finish();
        }
        self.slot_apply();
        self.slot_drag = None;
        self.circle = None;
        self.end_live();
        self.fresh = None;
        self.pen_plane = None;
        self.pen_hover = None;
        self.hover = None;
        self.hover_key = None;
        self.press = None;
        self.tool = t;
        self.overlay_gen += 1;
        self.objects_gen += 1;
    }

    /// Another component mode (1 to 4): the Select tool, what is picked
    /// kept when it fits.
    pub fn set_mode(&mut self, m: SelMode) {
        self.mode = m;
        if self.tool != Tool::Select {
            self.set_tool(Tool::Select);
        }
        self.sel.retain(|c| c.fits(m));
        self.hover = None;
        self.hover_key = None;
        self.objects_gen += 1;
        self.overlay_gen += 1;
    }

    // ------------------------------------------------------------ undo

    fn snapshot(&mut self, label: &str) {
        self.history.push((label.to_string(), self.objects.clone()));
        if self.history.len() > 200 {
            self.history.remove(0);
        }
        self.future.clear();
    }

    /// A change that goes on (a slider, a drag): one undo step for all.
    pub fn begin_live(&mut self, label: &str) {
        if !self.live {
            self.snapshot(label);
            self.live = true;
        }
    }

    pub fn end_live(&mut self) {
        self.live = false;
        self.fine_again();
    }

    /// The bodies built coarse during a live change built fine again.
    pub fn fine_again(&mut self) {
        if self.coarse.is_empty() {
            return;
        }
        for obj in std::mem::take(&mut self.coarse) {
            if let Some(Shape::Body(b)) = self.objects.get_mut(obj).map(|o| &mut o.shape) {
                b.rebuild();
            }
        }
        self.objects_gen += 1;
        self.overlay_gen += 1;
    }

    pub fn is_live(&self) -> bool {
        self.live
    }

    /// The change going on taken back.
    pub fn cancel_live(&mut self) {
        if !self.live {
            return;
        }
        self.live = false;
        self.coarse.clear();
        if let Some((_, o)) = self.history.pop() {
            self.objects = o;
        }
        self.after_change();
    }

    fn after_change(&mut self) {
        let n = self.objects.len();
        self.sel.retain(|c| c.obj() < n);
        if self.fresh.map(|f| f >= n).unwrap_or(false) {
            self.fresh = None;
        }
        self.hover = None;
        self.hover_key = None;
        self.objects_gen += 1;
        self.overlay_gen += 1;
    }

    pub fn undo(&mut self) {
        if self.gesture.is_some() {
            self.gesture = None;
            self.press = None;
            self.cancel_live();
            return;
        }
        if self.pen.is_some() {
            self.pen_back();
            return;
        }
        if self.slot.is_some() {
            self.slot_cancel();
            self.toast("Removed", false);
            return;
        }
        self.end_live();
        let Some((label, objs)) = self.history.pop() else {
            self.toast("Nothing to undo", false);
            return;
        };
        let now = std::mem::replace(&mut self.objects, objs);
        self.future.push((label.clone(), now));
        self.fresh = None;
        self.sel.clear();
        self.after_change();
        self.toast(format!("Undone: {label}"), false);
    }

    pub fn redo(&mut self) {
        self.end_live();
        let Some((label, objs)) = self.future.pop() else {
            self.toast("Nothing to redo", false);
            return;
        };
        let now = std::mem::replace(&mut self.objects, objs);
        self.history.push((label.clone(), now));
        self.fresh = None;
        self.sel.clear();
        self.after_change();
        self.toast(format!("Redone: {label}"), false);
    }

    /// A new shape (one undo step); its place in the list.
    fn add_object(&mut self, label: &str, o: Object) -> usize {
        self.live = false;
        self.snapshot(label);
        self.objects.push(o);
        self.objects_gen += 1;
        self.overlay_gen += 1;
        self.objects.len() - 1
    }

    pub fn body(&self, obj: usize) -> Option<&Body> {
        self.objects.get(obj).and_then(|o| o.body())
    }

    /// A body changed by `f` and built again (`live`: with the changes
    /// before it, one undo step).
    pub fn edit_body(&mut self, obj: usize, label: &str, live: bool, f: impl FnOnce(&mut Body)) -> bool {
        if self.body(obj).is_none() {
            return false;
        }
        if live {
            self.begin_live(label);
        } else {
            self.live = false;
            self.snapshot(label);
        }
        let o = &mut self.objects[obj];
        if let Shape::Body(b) = &mut o.shape {
            let before = (b.frame, b.base.clone(), b.corners.clone(), b.bevels.clone(), b.steps.clone());
            f(b);
            // the same values (the pointer held still): not built again
            if before == (b.frame, b.base.clone(), b.corners.clone(), b.bevels.clone(), b.steps.clone()) {
                return true;
            }
            if live {
                // coarse while it goes on, fine once it ends
                b.rebuild_coarse();
                if !self.coarse.contains(&obj) {
                    self.coarse.push(obj);
                }
            } else {
                b.rebuild();
            }
            o.what = b.describe();
        }
        self.hover_key = None;
        self.objects_gen += 1;
        self.overlay_gen += 1;
        true
    }

    /// What is picked removed: the objects, a step, a corner's rounding.
    pub fn delete_selected(&mut self) {
        let mut objs: Vec<usize> = self.sel.iter().filter_map(|c| if let Comp::Object(i) = c { Some(*i) } else { None }).collect();
        if !objs.is_empty() {
            objs.sort_unstable();
            objs.dedup();
            self.live = false;
            self.snapshot(if objs.len() == 1 { "Delete" } else { "Delete objects" });
            for &i in objs.iter().rev() {
                if i < self.objects.len() {
                    self.objects.remove(i);
                }
            }
            self.sel.clear();
            self.fresh = None;
            self.after_change();
            self.toast(if objs.len() == 1 { "Deleted".to_string() } else { format!("{} objects deleted", objs.len()) }, false);
            return;
        }
        if let Some(Comp::Step { obj, k }) = self.sel.first().cloned() {
            self.remove_step(obj, k);
            return;
        }
        let corners: Vec<(usize, usize)> = self.sel.iter().filter_map(|c| if let Comp::Corner { obj, i } = c { Some((*obj, *i)) } else { None }).collect();
        if let Some(&(obj, _)) = corners.first() {
            self.edit_body(obj, "Corner sharp", false, |b| {
                for &(o, i) in &corners {
                    if o == obj && i < b.corners.len() {
                        b.corners[i] = 0.0;
                    }
                }
            });
        }
    }

    /// A body's step taken away.
    pub fn remove_step(&mut self, obj: usize, k: usize) {
        let label = self.body(obj).and_then(|b| b.steps.get(k)).map(|s| format!("Remove {}", s.label().to_lowercase())).unwrap_or_default();
        self.edit_body(obj, &label, false, |b| {
            if k < b.steps.len() {
                b.steps.remove(k);
            }
        });
        self.sel = vec![Comp::Object(obj)];
        if self.mode != SelMode::Object {
            self.mode = SelMode::Object;
        }
    }
}

impl AppState {
    // ------------------------------------------------------------ picking

    /// The nearest triangle of a body under the pointer: its object, the
    /// triangle, how far along the ray, and where.
    pub fn pick_body(&self, at: [f32; 2]) -> Option<(usize, usize, f64, DVec3)> {
        let (o, d) = self.ray(at);
        let mut best: Option<(usize, usize, f64, DVec3)> = None;
        for (i, ob) in self.objects.iter().enumerate() {
            let Some(b) = ob.body() else { continue };
            let t = &b.built.topo;
            let p = |j: u32| DVec3::from_array(t.positions[j as usize].map(|x| x as f64));
            for (k, tri) in t.triangles.iter().enumerate() {
                if let Some(s) = ray_tri(o, d, p(tri[0]), p(tri[1]), p(tri[2])) {
                    if best.map(|b| s < b.2).unwrap_or(true) {
                        best = Some((i, k, s, o + d * s));
                    }
                }
            }
        }
        best
    }

    /// The object under the pointer (a body, or a surface).
    pub fn pick_object(&self, at: [f32; 2]) -> Option<usize> {
        let (o, d) = self.ray(at);
        let mut best = self.pick_body(at).map(|b| (b.2, b.0));
        for (i, ob) in self.objects.iter().enumerate() {
            let Shape::Surface(s) = &ob.shape else { continue };
            let (su, sv) = (32, 12);
            let (pts, _) = s.tessellate(su, sv);
            let at = |i: usize, j: usize| pts[j * (su + 1) + i];
            for j in 0..sv {
                for k in 0..su {
                    for (a, b, c) in [(at(k, j), at(k + 1, j), at(k + 1, j + 1)), (at(k, j), at(k + 1, j + 1), at(k, j + 1))] {
                        if let Some(t) = ray_tri(o, d, a, b, c) {
                            if best.map(|b| t < b.0).unwrap_or(true) {
                                best = Some((t, i));
                            }
                        }
                    }
                }
            }
        }
        best.map(|b| b.1)
    }

    /// No body hides `p` from the eye.
    pub fn seen(&self, p: DVec3) -> bool {
        let eye = self.view().eye.as_dvec3();
        let v = p - eye;
        let dist = v.length();
        if dist < 1e-9 {
            return true;
        }
        let dir = v / dist;
        let tol = self.px_at(p) * 3.0 + dist * 1e-4;
        for ob in &self.objects {
            let Some(b) = ob.body() else { continue };
            let t = &b.built.topo;
            let q = |j: u32| DVec3::from_array(t.positions[j as usize].map(|x| x as f64));
            for tri in &t.triangles {
                if let Some(s) = ray_tri(eye, dir, q(tri[0]), q(tri[1]), q(tri[2])) {
                    if s < dist - tol {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// The nearest edge of a body within 9 points on screen, seen: its
    /// object, its index, the point of it nearest the pointer.
    pub fn pick_edge(&self, at: [f32; 2]) -> Option<(usize, usize, DVec3)> {
        let mut found: Vec<(f32, usize, usize, DVec3)> = vec![];
        for (i, ob) in self.objects.iter().enumerate() {
            let Some(b) = ob.body() else { continue };
            for (e, ed) in b.built.topo.edges.iter().enumerate() {
                if ed.seam {
                    continue;
                }
                let pts: Vec<DVec3> = ed.pts.iter().map(|p| DVec3::from_array(*p)).collect();
                let mut best: Option<(f32, DVec3)> = None;
                for w in pts.windows(2) {
                    let (Some(a), Some(c)) = (self.project(w[0]), self.project(w[1])) else { continue };
                    let (dx, dy) = (c[0] - a[0], c[1] - a[1]);
                    let l2 = (dx * dx + dy * dy).max(1e-6);
                    let t = (((at[0] - a[0]) * dx + (at[1] - a[1]) * dy) / l2).clamp(0.0, 1.0);
                    let d = (a[0] + dx * t - at[0]).hypot(a[1] + dy * t - at[1]);
                    if d <= 9.0 && best.map(|b| d < b.0).unwrap_or(true) {
                        best = Some((d, w[0].lerp(w[1], t as f64)));
                    }
                }
                if let Some((d, p)) = best {
                    found.push((d, i, e, p));
                }
            }
        }
        found.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        found.into_iter().find(|f| self.seen(f.3)).map(|f| (f.1, f.2, f.3))
    }

    /// The nearest vertex of a body within 10 points on screen, seen.
    pub fn pick_vertex(&self, at: [f32; 2]) -> Option<(usize, DVec3)> {
        let mut found: Vec<(f32, usize, DVec3)> = vec![];
        for (i, ob) in self.objects.iter().enumerate() {
            let Some(b) = ob.body() else { continue };
            for v in &b.built.verts {
                if let Some(s) = self.project(*v) {
                    let d = (s[0] - at[0]).hypot(s[1] - at[1]);
                    if d <= 10.0 {
                        found.push((d, i, *v));
                    }
                }
            }
        }
        found.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        found.into_iter().find(|f| self.seen(f.2)).map(|f| (f.1, f.2))
    }

    /// What a click there picks in the mode.
    pub fn pick_at(&self, at: [f32; 2]) -> Option<Comp> {
        match self.mode {
            SelMode::Object => self.pick_object(at).map(Comp::Object),
            SelMode::Face => {
                let (obj, tri, _, p) = self.pick_body(at)?;
                let b = self.body(obj)?;
                let f = *b.built.topo.tri_face.get(tri)? as usize;
                let tag = b.built.topo.faces.get(f)?.tag;
                match body::step_of(tag).filter(|&k| matches!(b.steps.get(k), Some(Step::Fillet { .. } | Step::Bevel { .. }))) {
                    Some(k) => Some(Comp::Step { obj, k }),
                    None => Some(Comp::Face { obj, tag, at: p }),
                }
            }
            SelMode::Edge => {
                let (obj, e, _) = self.pick_edge(at)?;
                Some(Comp::Edge { obj, edge: self.body(obj)?.built.edge_ref(e) })
            }
            SelMode::Vertex => {
                let (obj, p) = self.pick_vertex(at)?;
                match self.body(obj)?.corner_at(p) {
                    Some(i) => Some(Comp::Corner { obj, i }),
                    None => Some(Comp::Vertex { obj, at: p }),
                }
            }
        }
    }

    /// The pointer moved in the Select tool (no button down).
    pub fn hover_at(&mut self, at: [f32; 2]) {
        let key = (at, self.cam_key(), self.mode as u32);
        if self.hover_key == Some(key) {
            return;
        }
        self.hover_key = Some(key);
        let h = self.pick_at(at);
        if h != self.hover {
            self.hover = h;
            self.objects_gen += 1;
            self.overlay_gen += 1;
        }
    }

    pub fn hover_off(&mut self) {
        self.hover_key = None;
        if self.hover.is_some() || self.pen_hover.is_some() || self.pen_plane.is_some() {
            self.hover = None;
            self.pen_hover = None;
            self.pen_plane = None;
            self.objects_gen += 1;
            self.overlay_gen += 1;
        }
    }

    fn cam_key(&self) -> [f32; 4] {
        let c = &self.camera;
        [c.yaw, c.pitch, c.dist, c.target.x + c.target.y * 3.1 + c.target.z * 7.3]
    }

    /// A click picks what is under the pointer (`add`: Shift, added to what
    /// is picked or taken out of it); on nothing, nothing is picked.
    pub fn click(&mut self, comp: Option<Comp>, add: bool) {
        match comp {
            None if !add => self.sel.clear(),
            None => {}
            Some(c) if add => {
                if let Some(k) = self.sel.iter().position(|s| *s == c) {
                    self.sel.remove(k);
                } else {
                    self.sel.push(c);
                }
            }
            Some(c) => self.sel = vec![c],
        }
        self.objects_gen += 1;
        self.overlay_gen += 1;
    }

    /// One object picked (the list on the left).
    pub fn select_object(&mut self, i: usize) {
        if self.tool != Tool::Select {
            self.set_tool(Tool::Select);
        }
        self.mode = SelMode::Object;
        self.sel = vec![Comp::Object(i)];
        self.objects_gen += 1;
        self.overlay_gen += 1;
    }

    /// The faces a pick stands for (object, face index), to show it.
    pub fn comp_faces(&self, c: &Comp) -> Vec<(usize, usize)> {
        let Some(b) = self.body(c.obj()) else { return vec![] };
        let t = &b.built.topo;
        match c {
            Comp::Object(o) => (0..t.faces.len()).map(|f| (*o, f)).collect(),
            Comp::Face { obj, tag, at } => b.built.find_face(*tag, *at).map(|f| vec![(*obj, f)]).unwrap_or_default(),
            Comp::Step { obj, k } => (0..t.faces.len()).filter(|&f| body::step_of(t.faces[f].tag) == Some(*k)).map(|f| (*obj, f)).collect(),
            _ => vec![],
        }
    }

    /// The edges a pick stands for (object, edge index).
    pub fn comp_edges(&self, c: &Comp) -> Vec<(usize, usize)> {
        let Some(b) = self.body(c.obj()) else { return vec![] };
        match c {
            Comp::Edge { obj, edge } => b.built.find_edge(edge).map(|e| vec![(*obj, e)]).unwrap_or_default(),
            _ => vec![],
        }
    }

    /// The points a pick stands for (vertices, corners).
    pub fn comp_points(&self, c: &Comp) -> Vec<DVec3> {
        let Some(b) = self.body(c.obj()) else { return vec![] };
        match c {
            Comp::Corner { i, .. } => b.corner_show(*i).into_iter().collect(),
            Comp::Vertex { at, .. } => vec![*at],
            _ => vec![],
        }
    }

    /// The corners picked (of one object: the first one's).
    pub fn picked_corners(&self) -> Option<(usize, Vec<usize>)> {
        let obj = self.sel.iter().find_map(|c| if let Comp::Corner { obj, .. } = c { Some(*obj) } else { None })?;
        let v: Vec<usize> = self.sel.iter().filter_map(|c| if let Comp::Corner { obj: o, i } = c { (*o == obj).then_some(*i) } else { None }).collect();
        Some((obj, v))
    }

    /// The edges picked (of one object: the first one's), and whether each
    /// has a face on both sides (a fillet can round it).
    pub fn picked_edges(&self) -> Option<(usize, Vec<(EdgeRef, bool)>)> {
        let obj = self.sel.iter().find_map(|c| if let Comp::Edge { obj, .. } = c { Some(*obj) } else { None })?;
        let b = self.body(obj)?;
        let v = self
            .sel
            .iter()
            .filter_map(|c| match c {
                Comp::Edge { obj: o, edge } if *o == obj => Some((*edge, b.built.find_edge(edge).map(|e| !b.built.edge_open(e)).unwrap_or(false))),
                _ => None,
            })
            .collect();
        Some((obj, v))
    }

    // ------------------------------------------------------------ fillets

    /// The corners picked rounded by `r` (each at most what fits).
    pub fn set_corners(&mut self, r: f64, live: bool) {
        let Some((obj, cs)) = self.picked_corners() else { return };
        self.edit_body(obj, "Corner fillet", live, |b| {
            for &i in &cs {
                if i < b.corners.len() {
                    b.corners[i] = r.min(b.corner_max(i)).max(0.0);
                }
            }
        });
    }

    /// The edges picked (with a face on both sides) rounded by `r`: a new
    /// Fillet step, then picked.
    pub fn make_fillet(&mut self, r: f64) {
        let Some((obj, edges)) = self.picked_edges() else { return };
        let edges: Vec<EdgeRef> = edges.into_iter().filter(|e| e.1).map(|e| e.0).collect();
        if edges.is_empty() {
            self.toast("These edges have a face on one side only: Shift + drag pulls them out", true);
            return;
        }
        let mut k = 0;
        self.edit_body(obj, "Fillet", true, |b| {
            k = b.steps.len();
            b.steps.push(Step::Fillet { edges, r });
        });
        self.mode = SelMode::Face;
        self.sel = vec![Comp::Step { obj, k }];
        self.report(obj);
    }

    /// The corners picked cut straight (`bevel`) or rounded; a corner not
    /// cut yet gets 1 mm (at most what fits).
    pub fn set_corner_kind(&mut self, bevel: bool) {
        let Some((obj, cs)) = self.picked_corners() else { return };
        self.end_live();
        self.edit_body(obj, if bevel { "Corner bevel" } else { "Corner fillet" }, false, |b| {
            for &i in &cs {
                if i < b.bevels.len() {
                    b.bevels[i] = bevel;
                }
                if i < b.corners.len() {
                    let r = if b.corners[i] > 0.0 { b.corners[i] } else { 1.0 };
                    b.corners[i] = r.min(b.corner_max(i)).max(0.0);
                }
            }
        });
    }

    /// The edges picked (with a face on both sides) cut straight by `d`: a
    /// new Bevel step, then picked.
    pub fn make_bevel(&mut self, d: f64) {
        let Some((obj, edges)) = self.picked_edges() else { return };
        let edges: Vec<EdgeRef> = edges.into_iter().filter(|e| e.1).map(|e| e.0).collect();
        if edges.is_empty() {
            self.toast("These edges have a face on one side only", true);
            return;
        }
        let mut k = 0;
        self.edit_body(obj, "Bevel", false, |b| {
            k = b.steps.len();
            b.steps.push(Step::Bevel { edges, d });
        });
        self.mode = SelMode::Face;
        self.sel = vec![Comp::Step { obj, k }];
        self.report(obj);
    }

    /// A fillet step made a bevel, or the other way (same edges, same size).
    pub fn set_step_kind(&mut self, obj: usize, k: usize, bevel: bool) {
        self.end_live();
        self.edit_body(obj, if bevel { "Bevel" } else { "Fillet" }, false, |b| {
            let st = match b.steps.get(k) {
                Some(Step::Fillet { edges, r } | Step::Bevel { edges, d: r }) => {
                    if bevel {
                        Step::Bevel { edges: edges.clone(), d: *r }
                    } else {
                        Step::Fillet { edges: edges.clone(), r: *r }
                    }
                }
                _ => return,
            };
            b.steps[k] = st;
        });
        self.report(obj);
    }

    /// Bevel (Ctrl+B) or Fillet on what is picked: corners, edges, or a
    /// fillet / bevel step.
    pub fn cut_selected(&mut self, bevel: bool) {
        match self.sel.first().cloned() {
            Some(Comp::Corner { .. }) => self.set_corner_kind(bevel),
            Some(Comp::Edge { .. }) => {
                self.end_live();
                if bevel {
                    self.make_bevel(1.0);
                } else {
                    self.make_fillet(1.0);
                    self.end_live();
                }
            }
            Some(Comp::Step { obj, k }) if matches!(self.body(obj).and_then(|b| b.steps.get(k)), Some(Step::Fillet { .. } | Step::Bevel { .. })) => self.set_step_kind(obj, k, bevel),
            _ => self.toast(if bevel { "Pick corners (1) or edges (2) to bevel" } else { "Pick corners (1) or edges (2) to round" }, true),
        }
    }

    /// A step's value: a fillet's radius, a pull's length, a thickness.
    pub fn set_step(&mut self, obj: usize, k: usize, v: f64, live: bool) {
        let label = self.body(obj).and_then(|b| b.steps.get(k)).map(|s| s.label()).unwrap_or("Step");
        self.edit_body(obj, label, live, |b| match b.steps.get_mut(k) {
            Some(Step::Fillet { r, .. } | Step::Bevel { d: r, .. }) => *r = v.max(0.0),
            Some(Step::Thick { t }) => *t = v,
            Some(Step::Extrude { v: e, .. }) => {
                let l = e.length();
                if l > 1e-9 {
                    *e *= v.max(0.01) / l;
                }
            }
            None => {}
        });
    }

    /// A body's thickness (a Thick step added when it has none).
    pub fn set_thickness(&mut self, obj: usize, t: f64, live: bool) {
        self.edit_body(obj, "Thickness", live, |b| match b.thick() {
            Some((k, _)) => b.steps[k] = Step::Thick { t },
            None => b.steps.push(Step::Thick { t }),
        });
    }

    /// A Circle's radius.
    pub fn set_radius(&mut self, obj: usize, r: f64, live: bool) {
        self.edit_body(obj, "Radius", live, |b| {
            if let Base::Circle { r: rr } = &mut b.base {
                *rr = r.max(0.01);
            }
        });
    }

    /// Says what went wrong in the last build of `obj`.
    fn report(&mut self, obj: usize) {
        let Some(b) = self.body(obj) else { return };
        let msg = match (&b.built.failed, b.built.limits.first()) {
            (Some((k, e)), _) if *k == usize::MAX => Some((format!("Not made: {e}"), true)),
            (Some((k, e)), _) => Some((format!("{} not made: {e}", b.steps.get(*k).map(|s| s.label()).unwrap_or("Step")), true)),
            (None, Some((_, r))) => Some((format!("Too big: stopped at the largest that works, {} mm", fmt(*r, 2)), false)),
            _ => None,
        };
        if let Some((m, w)) = msg {
            self.toast(m, w);
        }
    }
}

impl AppState {
    // ------------------------------------------------------------ Select

    /// The left button down in the Select tool (and in the Pen with Shift,
    /// before a point).
    pub fn select_press(&mut self, at: [f32; 2], shift: bool) {
        self.end_live();
        if let Some((obj, i)) = self.pick_handle(at) {
            self.press = Some(Press { at, shift, moved: false, comp: Some(Comp::Corner { obj, i }), handle: true });
            return;
        }
        let comp = self.pick_at(at);
        self.press = Some(Press { at, shift, moved: false, comp, handle: false });
    }

    /// A corner's handle under the pointer (within 9 points, seen): a
    /// rounded or bevelled corner in Vertex mode, or a corner picked.
    pub fn pick_handle(&self, at: [f32; 2]) -> Option<(usize, usize)> {
        let mut best: Option<(f32, usize, usize)> = None;
        for (obj, ob) in self.objects.iter().enumerate() {
            let Some(b) = ob.body() else { continue };
            let Base::Poly { pts, .. } = &b.base else { continue };
            for i in 0..pts.len() {
                let picked = self.sel.contains(&Comp::Corner { obj, i });
                let shown = self.mode == SelMode::Vertex && b.corners.get(i).copied().unwrap_or(0.0) > 0.0;
                if !picked && !shown {
                    continue;
                }
                let Some(x) = b.corner_show(i) else { continue };
                let Some(s) = self.project(x) else { continue };
                let d = (s[0] - at[0]).hypot(s[1] - at[1]);
                if d <= 9.0 && best.map(|b| d < b.0).unwrap_or(true) && self.seen(x) {
                    best = Some((d, obj, i));
                }
            }
        }
        best.map(|b| (b.1, b.2))
    }

    /// The pointer moving with the button down: true when a gesture takes
    /// it (else the view turns).
    pub fn select_drag(&mut self, at: [f32; 2], free: bool) -> bool {
        let Some(pr) = &mut self.press else { return false };
        if !pr.moved {
            if (at[0] - pr.at[0]).hypot(at[1] - pr.at[1]) < 3.0 {
                return (pr.shift || pr.handle) && pr.comp.is_some();
            }
            pr.moved = true;
            if self.tool == Tool::Pen && self.pen.is_some() {
                let (p0, sh) = (pr.at, pr.shift);
                self.pen_drag_start(p0, sh, free);
            } else if pr.shift || pr.handle {
                if let Some(c) = pr.comp.clone() {
                    let p0 = pr.at;
                    self.gesture_start(p0, c);
                }
            }
        }
        if self.pen.as_ref().map(|p| p.drag.is_some()).unwrap_or(false) {
            self.pen_drag_move(at, free);
            return true;
        }
        if self.gesture.is_some() {
            self.gesture_move(at, free);
            return true;
        }
        false
    }

    /// The button up: a click picks; a gesture ends.
    pub fn select_release(&mut self) {
        let Some(pr) = self.press.take() else { return };
        if self.gesture.is_some() {
            self.gesture_end();
            return;
        }
        if !pr.moved {
            self.click(pr.comp, pr.shift);
        }
    }

    fn gesture_start(&mut self, at: [f32; 2], comp: Comp) {
        let obj = comp.obj();
        let Some(b) = self.body(obj).cloned() else { return };
        match comp {
            Comp::Object(_) | Comp::Face { .. } => {
                let Some(p) = self.pick_body(at).filter(|h| h.0 == obj).map(|h| h.3) else { return };
                let (k, t0) = match b.thick() {
                    Some(s) => s,
                    None => (b.steps.len(), 0.0),
                };
                self.edit_body(obj, "Thickness", true, |b| {
                    if b.thick().is_none() {
                        b.steps.push(Step::Thick { t: 0.0 });
                    }
                });
                self.gesture = Some(Gesture::Thick { obj, k, o: p, n: b.frame.n, t0 });
            }
            Comp::Edge { edge, .. } => {
                let Some(e) = b.built.find_edge(&edge) else { return };
                let k = b.steps.len();
                if b.built.edge_open(e) {
                    let a = self.pick_edge(at).filter(|h| h.0 == obj && h.1 == e).map(|h| h.2).unwrap_or_else(|| b.built.edge_mid(e).0);
                    let along = (b.built.topo.edges[e].kind == forme_occt::EdgeKind::Line).then(|| b.built.edge_mid(e).1);
                    self.edit_body(obj, "Extrude edge", true, |b| b.steps.push(Step::Extrude { edge, v: DVec3::ZERO }));
                    self.gesture = Some(Gesture::Extrude { obj, k, a, along, n: b.frame.n });
                } else {
                    let px = self.px_at(b.built.edge_mid(e).0);
                    self.edit_body(obj, "Fillet", true, |b| b.steps.push(Step::Fillet { edges: vec![edge], r: 0.0 }));
                    self.gesture = Some(Gesture::Radius { obj, k: Some(k), corners: vec![], at, r0: 0.0, px, handle: None });
                }
            }
            Comp::Corner { i, .. } => {
                let mut corners = self.picked_corners().filter(|c| c.0 == obj).map(|c| c.1).unwrap_or_default();
                if !corners.contains(&i) {
                    corners = vec![i];
                }
                let px = self.px_at(b.corner_show(i).unwrap_or(b.frame.o));
                let handle = b.corner_handle(i);
                self.begin_live(if b.bevel_at(i) { "Corner bevel" } else { "Corner fillet" });
                self.gesture = Some(Gesture::Radius { obj, k: None, corners, at, r0: b.corners.get(i).copied().unwrap_or(0.0), px, handle });
            }
            Comp::Step { k, .. } => {
                if let Some(Step::Fillet { r, .. } | Step::Bevel { d: r, .. }) = b.steps.get(k) {
                    let px = self.pick_body(at).map(|h| self.px_at(h.3)).unwrap_or(0.05);
                    self.begin_live(b.steps[k].label());
                    self.gesture = Some(Gesture::Radius { obj, k: Some(k), corners: vec![], at, r0: *r, px, handle: None });
                }
            }
            Comp::Vertex { .. } => self.toast("Only the outline's corners round: this vertex comes from a step", true),
        }
    }

    fn gesture_move(&mut self, at: [f32; 2], free: bool) {
        let Some(g) = self.gesture.clone() else { return };
        let (ro, rd) = self.ray(at);
        match g {
            Gesture::Thick { obj, k, o, n, t0 } => {
                // the point of the normal's line nearest the pointer's ray
                let w = o - ro;
                let (b, d, e) = (n.dot(rd), n.dot(w), rd.dot(w));
                let den = 1.0 - b * b;
                let s = if den > 1e-4 {
                    (b * e - d) / den
                } else {
                    let p0 = self.press.as_ref().map(|p| p.at).unwrap_or(at);
                    -((at[1] - p0[1]) as f64) * self.px_at(o)
                };
                let t = t0 + s;
                let t = if free { t } else { nice_round(t, self.px_at(o)) };
                self.edit_body(obj, "Thickness", true, |b| {
                    if let Some(Step::Thick { t: tt }) = b.steps.get_mut(k) {
                        *tt = t;
                    }
                });
            }
            Gesture::Extrude { obj, k, a, along, n } => {
                // on the scan under the pointer (Ctrl: on the plane through
                // the edge facing the view)
                let on_scan = if free { None } else { self.pick_hit(at).map(|h| h.1) };
                let x = on_scan.or_else(|| {
                    let mut m = rd;
                    if let Some(e) = along {
                        m -= e * e.dot(rd);
                    }
                    let m = m.normalize_or_zero();
                    let dn = rd.dot(m);
                    (dn.abs() > 1e-6).then(|| ro + rd * ((a - ro).dot(m) / dn))
                });
                let Some(x) = x else { return };
                let mut v = x - a;
                if let Some(e) = along {
                    v -= e * e.dot(v);
                }
                // straight along the plane's normal when close to it (8°)
                let l = v.length();
                if l > 1e-9 && v.dot(n).abs() / l > 8f64.to_radians().cos() && !free {
                    v = n * v.dot(n);
                }
                let l = v.length();
                if l > 1e-9 && !free {
                    v *= nice_round(l, self.px_at(a)) / l;
                }
                self.edit_body(obj, "Extrude edge", true, |b| {
                    if let Some(Step::Extrude { v: vv, .. }) = b.steps.get_mut(k) {
                        *vv = v;
                    }
                });
            }
            Gesture::Radius { obj, k, corners, at: p0, r0, px, handle } => {
                let dist = (at[0] - p0[0]).hypot(at[1] - p0[1]) as f64;
                // a corner's handle follows the pointer along its bisector;
                // else the size grows as the pointer goes away
                let on_handle = handle.and_then(|(p, bis, per)| {
                    let n = self.body(obj)?.frame.n;
                    let dn = rd.dot(n);
                    let x = if dn.abs() > 1e-3 { ro + rd * ((p - ro).dot(n) / dn) } else { return None };
                    Some(((x - p).dot(bis) / per).max(0.0))
                });
                let r = on_handle.unwrap_or(r0 + dist * px);
                let mut r = if free { r } else { nice_round(r, px) };
                match k {
                    Some(k) => {
                        // past the largest that works: it stops there (no
                        // search by halves at each move)
                        if let Some(&(_, lim)) = self.body(obj).and_then(|b| b.built.limits.iter().find(|l| l.0 == k)) {
                            r = r.min(lim);
                        }
                        let label = if matches!(self.body(obj).and_then(|b| b.steps.get(k)), Some(Step::Bevel { .. })) { "Bevel" } else { "Fillet" };
                        self.edit_body(obj, label, true, |b| {
                            if let Some(Step::Fillet { r: rr, .. } | Step::Bevel { d: rr, .. }) = b.steps.get_mut(k) {
                                *rr = r;
                            }
                        });
                    }
                    None => {
                        let r = r.max(0.0);
                        self.edit_body(obj, "Corner fillet", true, |b| {
                            for &i in &corners {
                                if i < b.corners.len() {
                                    b.corners[i] = r.min(b.corner_max(i));
                                }
                            }
                        });
                    }
                }
            }
        }
    }

    fn gesture_end(&mut self) {
        let Some(g) = self.gesture.take() else { return };
        let obj = match &g {
            Gesture::Thick { obj, .. } | Gesture::Extrude { obj, .. } | Gesture::Radius { obj, .. } => *obj,
        };
        let Some(b) = self.body(obj) else {
            self.end_live();
            return;
        };
        // nothing done: taken back
        let nothing = match &g {
            Gesture::Thick { k, t0, .. } => matches!(b.steps.get(*k), Some(Step::Thick { t }) if (t - t0).abs() < 1e-9),
            Gesture::Extrude { k, .. } => matches!(b.steps.get(*k), Some(Step::Extrude { v, .. }) if v.length() < 1e-9),
            Gesture::Radius { k: Some(k), r0, .. } => matches!(b.steps.get(*k), Some(Step::Fillet { r, .. } | Step::Bevel { d: r, .. }) if (r - r0).abs() < 1e-9),
            Gesture::Radius { corners, r0, .. } => corners.iter().all(|&i| (b.corners.get(i).copied().unwrap_or(0.0) - r0).abs() < 1e-9),
        };
        if nothing {
            self.cancel_live();
            return;
        }
        self.end_live();
        match g {
            Gesture::Thick { .. } => {
                if self.mode == SelMode::Object {
                    self.sel = vec![Comp::Object(obj)];
                }
            }
            Gesture::Extrude { k, .. } | Gesture::Radius { k: Some(k), .. } => {
                self.mode = SelMode::Face;
                self.sel = vec![Comp::Step { obj, k }];
            }
            Gesture::Radius { .. } => {}
        }
        self.report(obj);
        self.objects_gen += 1;
        self.overlay_gen += 1;
    }
}

impl AppState {
    // ------------------------------------------------------------ Pen

    /// The plane under the pointer: a flat face of a shape when one is
    /// there in front, else the scan's flat face, else its tangent plane.
    pub fn plane_at(&self, at: [f32; 2]) -> Option<(PlaneHint, DVec3, Option<(u32, DVec3)>)> {
        let scan = self.pick_hit(at);
        let shape = self.pick_body(at);
        let eye = self.view().eye.as_dvec3();
        if let Some((obj, tri, s, p)) = shape {
            let nearer = scan.map(|(_, x)| s <= (x - eye).length() + self.px_at(p) * 4.0).unwrap_or(true);
            let b = self.body(obj)?;
            let f = b.built.topo.faces.get(*b.built.topo.tri_face.get(tri)? as usize)?;
            if nearer && f.info.kind == forme_occt::SurfKind::Plane {
                let mut n = DVec3::from_array(f.info.normal).normalize_or_zero();
                if n.dot(eye - p) < 0.0 {
                    n = -n;
                }
                let o = p - n * n.dot(p - DVec3::from_array(f.info.point));
                return Some((PlaneHint { frame: Frame::level(o, n), flat: true }, o, None));
            }
        }
        let (t, x) = scan?;
        let d = self.doc.as_ref()?;
        let reach = (self.px_at(x) * 90.0).clamp(1.5, 25.0);
        let (pl, flat) = planes::plane_under(&d.mesh, &d.topo, t as u32, x, reach, d.edge);
        let o = pl.onto(x);
        Some((PlaneHint { frame: Frame::level(o, pl.n), flat }, o, Some((t as u32, x))))
    }

    /// The plane under the pointer, sticky: a flat face found before stays
    /// while the pointer is on it or near its border (where the scan rounds
    /// off into the next face: a click at a face's corner keeps its plane).
    fn plane_sticky(&self, at: [f32; 2]) -> Option<(PlaneHint, DVec3, Option<(u32, DVec3)>)> {
        let (h, o, hit) = self.plane_at(at)?;
        let (Some(prev), Some((t, x))) = (self.pen_plane, hit) else { return Some((h, o, hit)) };
        if !prev.flat {
            return Some((h, o, hit));
        }
        let tol = 0.6 + self.px_at(x) * 4.0;
        let same = h.flat && h.frame.n.dot(prev.frame.n) > 4f64.to_radians().cos() && h.frame.n.dot(h.frame.o - prev.frame.o).abs() < tol;
        let d = prev.frame.n.dot(x - prev.frame.o);
        if same || d.abs() > tol {
            return Some((h, o, hit));
        }
        Some((prev, x - prev.frame.n * d, Some((t, x))))
    }

    /// Where the Pen's next point goes for the pointer at `at` (`shift`:
    /// the side locked horizontal, vertical or at 45°; `free`: no point
    /// catches).
    pub fn pen_point(&self, at: [f32; 2], shift: bool, free: bool) -> Option<PenHover> {
        let pe = self.pen.as_ref()?;
        let f = pe.frame;
        let h = self.pen_point_open(at, shift, free)?;
        // the first point closes it: the pointer near it, or the point
        // caught (a point, a locked side) landing on it
        if pe.pts.len() >= 3 {
            let first = self.project(f.to3(pe.pts[0]));
            let near = |s: Option<[f32; 2]>, r: f32| first.zip(s).map(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1]) <= r).unwrap_or(false);
            if near(Some(at), 12.0) || (h.snap != PenSnap::Free && near(self.project(h.p), 4.0)) {
                return Some(PenHover { q: pe.pts[0], p: f.to3(pe.pts[0]), snap: PenSnap::Close });
            }
        }
        Some(h)
    }

    /// `pen_point` without the closing.
    fn pen_point_open(&self, at: [f32; 2], shift: bool, free: bool) -> Option<PenHover> {
        let pe = self.pen.as_ref()?;
        let f = pe.frame;
        let (o, d) = self.ray(at);
        let p = f.hit(o, d)?;
        let mut q = f.to2(p);
        let dist = |x: DVec3| self.project(x).map(|s| (s[0] - at[0]).hypot(s[1] - at[1])).unwrap_or(f32::MAX);
        if !free && !shift {
            // a corner of a shape near the plane, a point already placed
            let tol = self.px_at(p) * 20.0;
            let mut cands: Vec<DVec3> = self.objects.iter().filter_map(|o| o.body()).flat_map(|b| b.built.verts.iter().copied()).filter(|v| f.n.dot(*v - f.o).abs() <= tol.max(1.0)).collect();
            cands.extend(pe.pts.iter().take(pe.pts.len().saturating_sub(1)).map(|q| f.to3(*q)));
            if let Some(v) = cands.into_iter().map(|v| (dist(v), v)).filter(|(d, _)| *d <= 10.0).min_by(|a, b| a.0.partial_cmp(&b.0).unwrap()).map(|x| x.1) {
                let q = f.to2(v);
                return Some(PenHover { q, p: f.to3(q), snap: PenSnap::Point });
            }
        }
        let last = *pe.pts.last()?;
        if shift {
            // locked: horizontal, vertical or a 45° diagonal of the plane
            // (its x is level)
            let w = q - last;
            if w.length() > 1e-9 {
                let step = 45f64.to_radians();
                let a = (w.y.atan2(w.x) / step).round() * step;
                let u = DVec2::new(a.cos(), a.sin());
                q = last + u * w.dot(u).max(0.0);
                let deg = a.to_degrees().rem_euclid(180.0);
                return Some(PenHover { q, p: f.to3(q), snap: PenSnap::Angle(deg) });
            }
        }
        Some(PenHover { q, p: f.to3(q), snap: PenSnap::Free })
    }

    /// The pointer moved in the Pen tool: the plane it would draw on (before
    /// the first point), where the next point would go.
    pub fn pen_hover_at(&mut self, at: [f32; 2], shift: bool, free: bool) {
        let n = self.pen.as_ref().map(|p| p.pts.len()).unwrap_or(0) as u32;
        let key = (at, self.cam_key(), n * 4 + shift as u32 * 2 + free as u32);
        if self.hover_key == Some(key) {
            return;
        }
        self.hover_key = Some(key);
        if self.pen.is_some() {
            self.pen_plane = None;
            self.pen_hover = self.pen_point(at, shift, free);
        } else {
            match self.plane_sticky(at) {
                Some((h, o, _)) => {
                    let q = h.frame.to2(o);
                    self.pen_plane = Some(h);
                    self.pen_hover = Some(PenHover { q, p: h.frame.to3(q), snap: PenSnap::Free });
                }
                None => {
                    self.pen_plane = None;
                    self.pen_hover = None;
                }
            }
        }
        self.overlay_gen += 1;
    }

    /// The left button down with the Pen: a click places a point (on the
    /// release); with Shift before the first point, on a shape, a drag
    /// thickens it.
    pub fn pen_press(&mut self, at: [f32; 2], shift: bool) {
        let comp = if shift && self.pen.is_none() { self.pick_object(at).filter(|&i| self.body(i).is_some()).map(Comp::Object) } else { None };
        if comp.is_some() {
            self.end_live();
        }
        self.press = Some(Press { at, shift, moved: false, comp, handle: false });
    }

    /// A click with the Pen: the first one finds the plane and starts the
    /// outline (the shape before is kept); on the first point (three or
    /// more placed) it closes and fills.
    pub fn pen_click(&mut self, at: [f32; 2], shift: bool, free: bool) {
        self.hover_key = None;
        if self.pen.is_none() {
            let Some((h, o, _)) = self.plane_sticky(at) else { return };
            let p = o;
            self.end_live();
            self.fresh = None;
            self.pen = Some(PenEdit { frame: Frame { o: p, ..h.frame }, flat: h.flat, pts: vec![DVec2::ZERO], radii: vec![0.0], drag: None });
            self.pen_plane = None;
            self.pen_hover = None;
            self.overlay_gen += 1;
            return;
        }
        let Some(h) = self.pen_point(at, shift, free) else { return };
        if h.snap == PenSnap::Close {
            self.pen_make(true);
            return;
        }
        let pe = self.pen.as_mut().unwrap();
        if pe.pts.last().map(|l| (*l - h.q).length() < 1e-6).unwrap_or(false) {
            return;
        }
        pe.pts.push(h.q);
        pe.radii.push(0.0);
        self.pen_hover = None;
        self.overlay_gen += 1;
    }

    /// A press with the Pen dragged (an outline begun): the point placed
    /// there (or the first one, to close on it) and its fillet set by the
    /// drag.
    pub fn pen_drag_start(&mut self, at: [f32; 2], shift: bool, free: bool) -> bool {
        if self.pen.is_none() {
            return false;
        }
        let Some(h) = self.pen_point(at, shift, free) else { return false };
        let pe = self.pen.as_mut().unwrap();
        let drag = if h.snap == PenSnap::Close {
            (0, true)
        } else {
            if pe.pts.last().map(|l| (*l - h.q).length() >= 1e-6).unwrap_or(true) {
                pe.pts.push(h.q);
                pe.radii.push(0.0);
            }
            (pe.pts.len() - 1, false)
        };
        pe.drag = Some(drag);
        self.pen_hover = None;
        self.overlay_gen += 1;
        true
    }

    /// The fillet follows the pointer: its distance from the point, on the
    /// plane.
    pub fn pen_drag_move(&mut self, at: [f32; 2], free: bool) {
        let (o, d) = self.ray(at);
        let Some(pe) = &self.pen else { return };
        let Some((i, _)) = pe.drag else { return };
        let Some(x) = pe.frame.hit(o, d) else { return };
        let q = pe.pts[i];
        let r = (pe.frame.to2(x) - q).length();
        let r = if free { r } else { nice_round(r, self.px_at(pe.frame.to3(q))) };
        let pe = self.pen.as_mut().unwrap();
        pe.radii[i] = r;
        self.overlay_gen += 1;
    }

    /// Released: on the first point, the outline closes; else it goes on.
    pub fn pen_drag_end(&mut self) {
        let Some(pe) = &mut self.pen else { return };
        let Some((_, close)) = pe.drag.take() else { return };
        self.hover_key = None;
        self.overlay_gen += 1;
        if close {
            self.pen_make(true);
        }
    }

    /// Enter: the outline drawn made an open line (two points or more).
    pub fn pen_finish(&mut self) {
        match self.pen.as_ref().map(|p| p.pts.len()) {
            Some(n) if n >= 2 => self.pen_make(false),
            Some(_) => self.pen_cancel(),
            None => {}
        }
    }

    /// The outline made a shape: closed, a filled face; open, a line.
    fn pen_make(&mut self, closed: bool) {
        let Some(pe) = self.pen.clone() else { return };
        let mut b = Body::new(pe.frame, Base::Poly { pts: pe.pts.clone(), closed });
        if pe.radii.iter().any(|r| *r > 0.0) {
            for i in 0..b.corners.len() {
                b.corners[i] = pe.radii.get(i).copied().unwrap_or(0.0).min(b.corner_max(i)).max(0.0);
            }
            b.rebuild();
        }
        if let Some((_, e)) = b.built.failed.clone() {
            self.toast(format!("Not made ({e}): move on or take the last point back (Backspace)"), true);
            return;
        }
        self.pen = None;
        self.pen_hover = None;
        self.hover_key = None;
        let n = pe.pts.len();
        let what = b.describe();
        let i = self.add_object(if closed { "Pen outline" } else { "Pen line" }, Object { tool: Tool::Pen, what, shape: Shape::Body(b) });
        self.fresh = Some(i);
        self.mode = SelMode::Object;
        self.sel = vec![Comp::Object(i)];
        self.toast(
            if closed { format!("Filled: {n} corners · Shift + drag on it thickens it · Esc removes it") } else { format!("A line of {n} points · Shift + drag on an edge (Select, 2) pulls it · Esc removes it") },
            false,
        );
    }

    /// The last point taken back (Backspace, Ctrl+Z).
    pub fn pen_back(&mut self) {
        let Some(pe) = &mut self.pen else { return };
        pe.pts.pop();
        pe.radii.truncate(pe.pts.len());
        if pe.pts.is_empty() {
            self.pen = None;
        }
        self.hover_key = None;
        self.overlay_gen += 1;
    }

    pub fn pen_cancel(&mut self) {
        if self.pen.take().is_some() {
            self.hover_key = None;
            self.overlay_gen += 1;
        }
    }

    /// Esc after a tool made a shape: it goes (one undo step).
    pub fn remove_fresh(&mut self) -> bool {
        let Some(i) = self.fresh.take() else { return false };
        if i >= self.objects.len() {
            return false;
        }
        self.live = false;
        self.snapshot("Remove");
        self.objects.remove(i);
        self.sel.clear();
        self.after_change();
        self.toast("Removed", false);
        true
    }

    /// Enter after a tool made a shape: kept.
    pub fn keep_fresh(&mut self) {
        if self.fresh.take().is_some() {
            self.end_live();
            self.toast("Kept", false);
            self.objects_gen += 1;
        }
    }
}

impl AppState {
    // ------------------------------------------------------------ Circle

    /// The Circle pressed on the scan: the plane under it.
    pub fn circle_press(&mut self, at: [f32; 2]) {
        let Some((t, x)) = self.pick_hit(at) else { return };
        let Some(d) = &self.doc else { return };
        // the face round the rim, not the fillet or the wall the press is
        // on: drawn on a plane through the press square to it
        let reach = (self.px_at(x) * 60.0).clamp(1.0, 20.0);
        let mut n = planes::dominant_normal(&d.mesh, &d.topo, t as u32, x, reach);
        if n.dot(self.ray(at).1) > 0.0 {
            n = -n;
        }
        let a = x;
        if std::env::var_os("RECTIFY_DEBUG").is_some() {
            eprintln!("circle press at {:.2},{:.2},{:.2}: plane n {:.3},{:.3},{:.3} (reach {reach:.2})", x.x, x.y, x.z, n.x, n.y, n.z);
        }
        self.end_live();
        self.fresh = None;
        self.circle = Some(CircleDraw { frame: Frame::level(a, n), tri: t as u32, a, b: a });
        self.overlay_gen += 1;
    }

    /// The pointer moving: the circle drawn follows it, on the plane.
    pub fn circle_move(&mut self, at: [f32; 2]) {
        let Some(cd) = &self.circle else { return };
        let (o, d) = self.ray(at);
        // on the scan under the pointer (what the release reads), else on
        // the plane
        let b = self.pick_hit(at).map(|h| h.1).or_else(|| cd.frame.hit(o, d));
        if let (Some(b), Some(cd)) = (b, &mut self.circle) {
            cd.b = b;
        }
        self.overlay_gen += 1;
    }

    /// Released: a round part found there (its wall going round the circle
    /// drawn, about its size) makes a cylinder its size and height; else a
    /// flat disc as drawn.
    pub fn circle_release(&mut self) {
        let Some(cd) = self.circle.take() else { return };
        self.overlay_gen += 1;
        let r = (cd.b - cd.a).length() * 0.5;
        if r * 2.0 < self.px_at(cd.a) * 4.0 {
            return;
        }
        let c = (cd.a + cd.b) * 0.5;
        let n = cd.ring_normal();
        let found = {
            let Some(d) = &self.doc else { return };
            Slot::detect_round(&d.mesh, &d.topo, cd.tri, cd.a, cd.b, d.edge)
        };
        if std::env::var_os("RECTIFY_DEBUG").is_some() {
            match &found {
                Ok(s) => eprintln!("circle: drawn Ø {:.2} at {:.2},{:.2},{:.2} n {:.2},{:.2},{:.2}; found Ø {:.2} at {:.2},{:.2},{:.2} axis {:.2},{:.2},{:.2} depth {:.2} inward {}", r * 2.0, c.x, c.y, c.z, cd.frame.n.x, cd.frame.n.y, cd.frame.n.z, s.r * 2.0, s.a.x, s.a.y, s.a.z, s.axis.x, s.axis.y, s.axis.z, s.depth, s.inward),
                Err(e) => eprintln!("circle: drawn Ø {:.2} from {:.2},{:.2},{:.2} to {:.2},{:.2},{:.2}: {e}", r * 2.0, cd.a.x, cd.a.y, cd.a.z, cd.b.x, cd.b.y, cd.b.z),
            }
        }
        let found = found.ok().filter(|s| {
            let off = s.a - c;
            let off = off - s.axis * s.axis.dot(off);
            (s.r - r).abs() <= r * 0.5 && off.length() <= r
        });
        let (b, msg) = match found {
            Some(s) => {
                let mut b = Body::new(Frame::level(s.a, s.axis), Base::Circle { r: s.r });
                // a depth the scan cannot hold (a wall read past its end):
                // a flat disc
                let sane = s.depth.is_finite() && s.depth < (s.r * 20.0).max(30.0);
                let mut s = s;
                if !sane {
                    s.depth = 0.0;
                }
                if s.depth > 0.02 {
                    b.steps.push(Step::Thick { t: s.depth });
                    b.rebuild();
                }
                let way = if s.inward { "deep (a hole)" } else { "high" };
                (b, format!("Found: Ø {} mm, {} mm {way} · Esc removes it", fmt(s.r * 2.0, 2), fmt(s.depth, 2)))
            }
            None => (Body::new(Frame::level(c, n), Base::Circle { r }), format!("Nothing round there: a disc Ø {} mm as drawn · Esc removes it", fmt(r * 2.0, 2))),
        };
        if let Some((_, e)) = &b.built.failed {
            self.toast(format!("Not made: {e}"), true);
            return;
        }
        let what = b.describe();
        let i = self.add_object("Circle", Object { tool: Tool::Circle, what, shape: Shape::Body(b) });
        self.fresh = Some(i);
        self.mode = SelMode::Object;
        self.sel = vec![Comp::Object(i)];
        self.toast(msg, false);
    }
}

impl AppState {
    // ------------------------------------------------------------ Slot

    /// A drag starting on the scan (Slot, Box): where, and the plane under it.
    fn drag_start(&self, at: [f32; 2]) -> Option<Drag> {
        let (t, x) = self.pick_hit(at)?;
        let d = self.doc.as_ref()?;
        let n = d.mesh.tris[t].iter().flat_map(|&v| d.topo.tris_of(v as usize).iter()).map(|&t| d.mesh.tri_cross(t as usize).normalize_or_zero()).sum::<DVec3>().normalize_or_zero();
        Some(Drag { a: x, b: x, tri: t as u32, n })
    }

    /// Where a drag goes: on the plane under the press (else the scan
    /// under the pointer).
    fn drag_to(&self, cd: &Drag, at: [f32; 2]) -> Option<DVec3> {
        let (o, dir) = self.ray(at);
        let dn = dir.dot(cd.n);
        let on_plane = (dn.abs() > 0.05).then(|| o + dir * ((cd.a - o).dot(cd.n) / dn)).filter(|x| (*x - o).dot(dir) > 0.0);
        on_plane.or_else(|| self.pick_hit(at).map(|h| h.1))
    }

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
        let tool = if sl.kind == Kind::Box { Tool::Box } else { Tool::Slot };
        self.add_object(tool.label(), Object { tool, what, shape: Shape::Surface(se.laid.surface) });
    }

    /// The surface placed taken away.
    pub fn slot_cancel(&mut self) {
        if self.slot.take().is_some() {
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

/// A number with a decimal comma.
pub fn fmt(v: f64, digits: usize) -> String {
    format!("{:.*}", digits, v).replace('.', ",")
}

/// `v` rounded to a round step a little larger than `px` (the size of a
/// screen point there): 0.01, 0.02, 0.05, 0.1 … mm.
pub fn nice_round(v: f64, px: f64) -> f64 {
    let want = (px * 2.0).max(1e-4);
    let e = 10f64.powf(want.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0].iter().map(|m| m * e).find(|s| *s >= want).unwrap_or(want);
    (v / step).round() * step
}

/// Möller–Trumbore: distance along the ray, if it hits.
pub fn ray_tri(o: DVec3, d: DVec3, a: DVec3, b: DVec3, c: DVec3) -> Option<f64> {
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
