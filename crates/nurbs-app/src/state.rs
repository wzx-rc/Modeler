//! The app's state: the document, its history, the selection, the tools,
//! and the part the kernel built for it. Both the viewport and the panels
//! drive it; every change of the document goes to the kernel (the latest
//! one wins), and the part shown is always the kernel's.

use crate::camera::OrbitCamera;
use crate::pick::{Hit, Picker};
use crate::settings::Settings;
use crate::view::View;
use glam::{DQuat, DVec3, Vec3};
use nurbs_kernel::client::{Kernel, Status};
use nurbs_kernel::doc::PrimKind;
use nurbs_kernel::{Doc, Feature, FeatureId, Model, Op, Prim};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Create(PrimKind),
}

/// The primitives the app creates (step 1: cube and cylinder).
pub const PRIMS: [PrimKind; 2] = [PrimKind::Box, PrimKind::Cylinder];

/// A plane the base of a new shape is drawn on: the ground, or a flat face
/// of the part.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BasePlane {
    pub origin: DVec3,
    pub u: DVec3,
    pub n: DVec3,
    pub v: DVec3,
    /// The face it lies on (index in the model), None for the ground.
    pub face: Option<usize>,
}

impl BasePlane {
    pub fn ground() -> BasePlane {
        BasePlane { origin: DVec3::ZERO, u: DVec3::X, n: DVec3::Y, v: DVec3::Z, face: None }
    }
    /// A plane through `origin` with normal `n`, its u axis as close to the
    /// world X (else Z) as it can.
    pub fn new(origin: DVec3, n: DVec3, face: Option<usize>) -> BasePlane {
        let n = n.normalize();
        let pick = if n.x.abs() < 0.9 { DVec3::X } else { DVec3::Z };
        let u = (pick - n * pick.dot(n)).normalize();
        // (u, n, v) right-handed like (X, Y, Z)
        BasePlane { origin, u, n, v: u.cross(n), face }
    }
    pub fn to_plane(&self, p: DVec3) -> [f64; 2] {
        let d = p - self.origin;
        [d.dot(self.u), d.dot(self.v)]
    }
    pub fn at(&self, a: [f64; 2]) -> DVec3 {
        self.origin + self.u * a[0] + self.v * a[1]
    }
    pub fn hit(&self, ro: DVec3, rd: DVec3) -> Option<DVec3> {
        let den = rd.dot(self.n);
        if den.abs() < 1e-9 {
            return None;
        }
        let t = (self.origin - ro).dot(self.n) / den;
        (t > 0.0).then(|| ro + rd * t)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The base is being dragged out.
    Base,
    /// The base is down: the pointer sets the height, a click ends it.
    Height,
}

/// A shape being created, 3ds Max style: press on the ground or a face,
/// drag the base, release, move for the height, click. It is in the
/// document all along: the kernel shows the real result live.
#[derive(Clone, Debug)]
pub struct Creation {
    pub kind: PrimKind,
    pub id: FeatureId,
    pub plane: BasePlane,
    /// First corner (box) or centre (cylinder), plane coordinates.
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub height: f64,
    pub phase: Phase,
    /// Pointer position (screen) when the height phase began.
    pub height_from: [f32; 2],
}

/// What a gizmo handle moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    /// Move along a world axis (0 X, 1 Y, 2 Z).
    Axis(usize),
    /// Move in the plane normal to an axis.
    Plane(usize),
    /// A size of the selected shape: 0 width (+X), 1 height (top), 2 depth (+Z), 3 radius.
    Size(usize),
}

#[derive(Clone, Debug)]
pub struct Drag {
    pub handle: Handle,
    /// Where the pointer grabbed (world), and the features as they were.
    pub grab: DVec3,
    pub start: Vec<Feature>,
}

pub struct Toast {
    pub text: String,
    pub until: f64,
    pub warn: bool,
}

pub struct AppState {
    pub doc: Doc,
    pub doc_name: String,
    /// Bumped on every change of the document.
    pub version: u64,
    undo: Vec<(String, Doc)>,
    redo: Vec<(String, Doc)>,
    /// The document before the gesture under way (drag, creation).
    live_before: Option<(String, Doc)>,

    pub selection: Vec<FeatureId>,
    pub hover_feature: Option<FeatureId>,
    pub hover_hit: Option<Hit>,
    pub tool: Tool,
    /// Add or Carve for new shapes.
    pub mode: Op,
    pub creation: Option<Creation>,
    pub drag: Option<Drag>,
    pub hot_handle: Option<Handle>,
    /// Selection rectangle (screen).
    pub marquee: Option<([f32; 2], [f32; 2])>,

    pub camera: OrbitCamera,
    /// Screen size and the free 3D area (min x, min y, max x, max y), points.
    pub screen: [f32; 2],
    pub view_rect: [f32; 4],

    pub kernel: Kernel,
    pub model: Option<Arc<Model>>,
    /// Bumped when a new model arrives (the renderer uploads it).
    pub model_gen: u64,
    pub picker: Picker,
    submitted: u64,
    /// Kernel round trip of the last model (ms).
    pub round_ms: f32,

    pub settings: Settings,
    pub toast: Option<Toast>,
    pub now: f64,
    /// Something moves (the image changes from frame to frame).
    pub interacting: bool,
    pub renaming: bool,
    pub files_open: bool,
    pub save_name: String,
    pub panel_visible: bool,
}

impl AppState {
    pub fn new(doc: Doc, name: &str, kernel: Kernel) -> AppState {
        AppState {
            doc,
            doc_name: name.to_string(),
            version: 1,
            undo: vec![],
            redo: vec![],
            live_before: None,
            selection: vec![],
            hover_feature: None,
            hover_hit: None,
            tool: Tool::Select,
            mode: Op::Add,
            creation: None,
            drag: None,
            hot_handle: None,
            marquee: None,
            camera: OrbitCamera { target: Vec3::new(0.0, 10.0, 0.0), yaw: 0.62, pitch: 0.42, dist: 160.0, ..Default::default() },
            screen: [1600.0, 900.0],
            view_rect: [0.0, 0.0, 1600.0, 900.0],
            kernel,
            model: None,
            model_gen: 0,
            picker: Picker::default(),
            submitted: 0,
            round_ms: 0.0,
            settings: Settings::default(),
            toast: None,
            now: 0.0,
            interacting: false,
            renaming: false,
            files_open: false,
            save_name: name.to_string(),
            panel_visible: true,
        }
    }

    // ------------------------------------------------------------ the view

    /// Projection centre: the middle of the free area.
    pub fn principal(&self) -> [f32; 2] {
        [(self.view_rect[0] + self.view_rect[2]) * 0.5, (self.view_rect[1] + self.view_rect[3]) * 0.5]
    }

    /// The camera view in points.
    pub fn view(&self) -> View {
        self.camera.view(self.screen, self.principal())
    }

    pub fn ray(&self, p: [f32; 2]) -> (DVec3, DVec3) {
        let (o, d) = self.view().ray(p[0], p[1]);
        (o.as_dvec3(), d.as_dvec3())
    }

    pub fn frame_all(&mut self) {
        let b = self.model.as_ref().and_then(|m| m.bounds()).map(|(lo, hi)| (Vec3::from(lo), Vec3::from(hi))).or_else(|| self.doc.bounds().map(|(lo, hi)| (lo.as_vec3(), hi.as_vec3())));
        match b {
            Some((lo, hi)) => self.camera.frame(lo, hi),
            None => self.camera.frame(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 20.0, 20.0)),
        }
    }

    pub fn frame_selection(&mut self) {
        let b = self.doc.features.iter().filter(|f| self.selection.contains(&f.id)).map(|f| f.bounds()).reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)));
        match b {
            Some((lo, hi)) => self.camera.frame(lo.as_vec3(), hi.as_vec3()),
            None => self.frame_all(),
        }
    }

    // ------------------------------------------------------------- history

    fn touched(&mut self) {
        self.version += 1;
    }

    /// One change, one step of undo.
    pub fn edit(&mut self, label: &str, f: impl FnOnce(&mut Doc)) {
        let before = self.doc.clone();
        f(&mut self.doc);
        if self.doc != before {
            if self.live_before.is_none() {
                self.undo.push((label.to_string(), before));
                self.redo.clear();
            }
            self.touched();
        }
    }

    /// A change made of many small ones (a drag, a slider): one undo step
    /// when it ends (`end_live`).
    pub fn begin_live(&mut self, label: &str) {
        if self.live_before.is_none() {
            self.live_before = Some((label.to_string(), self.doc.clone()));
        }
    }

    pub fn live(&mut self, f: impl FnOnce(&mut Doc)) {
        let old = self.doc.clone();
        f(&mut self.doc);
        if self.doc != old {
            self.touched();
        }
    }

    pub fn end_live(&mut self) {
        if let Some((label, before)) = self.live_before.take() {
            if before != self.doc {
                self.undo.push((label, before));
                self.redo.clear();
            }
            // fine meshing now
            self.touched();
        }
    }

    pub fn cancel_live(&mut self) {
        if let Some((_, before)) = self.live_before.take() {
            self.doc = before;
            self.touched();
        }
    }

    pub fn is_live(&self) -> bool {
        self.live_before.is_some()
    }

    pub fn undo(&mut self) {
        self.abort_gestures();
        if let Some((label, d)) = self.undo.pop() {
            let now = std::mem::replace(&mut self.doc, d);
            self.redo.push((label.clone(), now));
            self.touched();
            self.clean_selection();
            self.toast(&format!("Undo: {label}"), false);
        }
    }

    pub fn redo(&mut self) {
        self.abort_gestures();
        if let Some((label, d)) = self.redo.pop() {
            let now = std::mem::replace(&mut self.doc, d);
            self.undo.push((label.clone(), now));
            self.touched();
            self.clean_selection();
            self.toast(&format!("Redo: {label}"), false);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn clean_selection(&mut self) {
        let ids: Vec<FeatureId> = self.doc.features.iter().map(|f| f.id).collect();
        self.selection.retain(|id| ids.contains(id));
    }

    // ----------------------------------------------------------- selection

    pub fn select(&mut self, id: Option<FeatureId>, add: bool) {
        match (id, add) {
            (Some(id), true) => {
                if let Some(i) = self.selection.iter().position(|s| *s == id) {
                    self.selection.remove(i);
                } else {
                    self.selection.push(id);
                }
            }
            (Some(id), false) => self.selection = vec![id],
            (None, true) => {}
            (None, false) => self.selection.clear(),
        }
        self.renaming = false;
    }

    pub fn is_selected(&self, id: FeatureId) -> bool {
        self.selection.contains(&id)
    }

    /// The one selected feature (None for none or several).
    pub fn single(&self) -> Option<&Feature> {
        match self.selection.as_slice() {
            [id] => self.doc.feature(*id),
            _ => None,
        }
    }

    pub fn delete_selected(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let ids = self.selection.clone();
        let n = ids.len();
        self.edit("Delete", |d| d.remove(&ids));
        self.selection.clear();
        self.toast(&format!("{n} shape{} deleted", if n > 1 { "s" } else { "" }), false);
    }

    pub fn duplicate_selected(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let ids = self.selection.clone();
        let mut new = vec![];
        self.edit("Duplicate", |d| new = d.duplicate(&ids, DVec3::new(10.0, 0.0, 10.0)));
        self.selection = new;
    }

    pub fn toggle_visible(&mut self, id: FeatureId) {
        self.edit("Hide", |d| {
            if let Some(f) = d.feature_mut(id) {
                f.visible = !f.visible;
            }
        });
    }

    pub fn set_op(&mut self, id: FeatureId, op: Op) {
        self.edit(op.label(), |d| {
            if let Some(f) = d.feature_mut(id) {
                f.op = op;
            }
        });
    }

    /// Moves a feature one step earlier (-1) or later (+1) in the history.
    pub fn reorder(&mut self, id: FeatureId, delta: i32) {
        self.edit("Reorder", |d| {
            if let Some(i) = d.index_of(id) {
                let j = i as i32 + delta;
                if j >= 0 && (j as usize) < d.features.len() {
                    d.features.swap(i, j as usize);
                }
            }
        });
    }

    // -------------------------------------------------------------- tools

    pub fn arm(&mut self, kind: PrimKind) {
        self.abort_gestures();
        self.tool = if self.tool == Tool::Create(kind) { Tool::Select } else { Tool::Create(kind) };
    }

    pub fn select_tool(&mut self) {
        self.abort_gestures();
        self.tool = Tool::Select;
    }

    /// Cancels what is under way (creation, drag).
    pub fn abort_gestures(&mut self) {
        if self.creation.take().is_some() || self.drag.take().is_some() {
            self.cancel_live();
        }
        self.marquee = None;
    }

    /// The base plane under the pointer: the flat face hit, else the ground.
    pub fn base_plane_at(&self, p: [f32; 2]) -> Option<(BasePlane, DVec3)> {
        let (ro, rd) = self.ray(p);
        if let (Some(m), Some(h)) = (&self.model, self.pick_at(p)) {
            let f = &m.faces[h.face];
            if f.kind == nurbs_kernel::model::SurfKind::Plane {
                let n = DVec3::from_array(f.normal);
                let plane = BasePlane::new(DVec3::from_array(f.point), n, Some(h.face));
                // the plane through the face, origin at the face's middle point
                return plane.hit(ro, rd).map(|q| (plane, q));
            }
        }
        let g = BasePlane::ground();
        g.hit(ro, rd).map(|q| (g, q))
    }

    pub fn pick_at(&self, p: [f32; 2]) -> Option<Hit> {
        let m = self.model.as_ref()?;
        let (ro, rd) = self.view().ray(p[0], p[1]);
        self.picker.pick(m, ro, rd)
    }

    /// The feature a face comes from.
    pub fn feature_of_face(&self, face: usize) -> Option<FeatureId> {
        self.model.as_ref()?.faces.get(face)?.tag.map(|t| t.feature)
    }

    pub fn snap(&self, v: f64) -> f64 {
        if self.settings.snap && self.settings.step > 0.0 {
            (v / self.settings.step).round() * self.settings.step
        } else {
            v
        }
    }

    /// Starts a creation at `p` (screen).
    pub fn begin_creation(&mut self, kind: PrimKind, p: [f32; 2]) {
        let Some((plane, q)) = self.base_plane_at(p) else { return };
        let mut a = plane.to_plane(q);
        a = [self.snap(a[0]), self.snap(a[1])];
        self.begin_live(&format!("New {}", kind.label()));
        let op = self.mode;
        let origin = plane.at(a);
        let rot = basis_rotation(plane.u, plane.n);
        // a tiny shape at first: it grows with the drag
        let prim = match kind {
            PrimKind::Box => Prim::Box { x: 0.01, y: 0.01, z: 0.01 },
            PrimKind::Cylinder => Prim::Cylinder { radius: 0.01, height: 0.01 },
        };
        let mut id = 0;
        self.live(|d| id = d.add(op, prim, origin, rot));
        // hidden until it has a size
        if let Some(f) = self.doc.feature_mut(id) {
            f.visible = false;
        }
        self.creation = Some(Creation { kind, id, plane, a, b: a, height: 0.0, phase: Phase::Base, height_from: p });
        self.selection = vec![id];
    }

    /// The creation follows the pointer.
    pub fn update_creation(&mut self, p: [f32; 2], shift: bool) {
        let Some(mut c) = self.creation.clone() else { return };
        let (ro, rd) = self.ray(p);
        match c.phase {
            Phase::Base => {
                if let Some(q) = c.plane.hit(ro, rd) {
                    let b = c.plane.to_plane(q);
                    c.b = [self.snap(b[0]), self.snap(b[1])];
                    if shift && c.kind == PrimKind::Box {
                        // a square
                        let d = (c.b[0] - c.a[0]).abs().max((c.b[1] - c.a[1]).abs());
                        c.b = [c.a[0] + d * (c.b[0] - c.a[0]).signum(), c.a[1] + d * (c.b[1] - c.a[1]).signum()];
                    }
                }
            }
            Phase::Height => {
                // the height along the normal through the base centre, closest to the ray
                let centre = self.creation_base_centre(&c);
                let t = closest_on_line(centre, c.plane.n, ro, rd).unwrap_or(0.0);
                c.height = self.snap(t);
            }
        }
        self.creation = Some(c.clone());
        self.apply_creation(&c);
    }

    fn creation_base_centre(&self, c: &Creation) -> DVec3 {
        match c.kind {
            PrimKind::Box => c.plane.at([(c.a[0] + c.b[0]) * 0.5, (c.a[1] + c.b[1]) * 0.5]),
            PrimKind::Cylinder => c.plane.at(c.a),
        }
    }

    /// Writes the creation into its feature.
    fn apply_creation(&mut self, c: &Creation) {
        let centre = self.creation_base_centre(c);
        let (w, dpt) = ((c.b[0] - c.a[0]).abs(), (c.b[1] - c.a[1]).abs());
        let r = ((c.b[0] - c.a[0]).powi(2) + (c.b[1] - c.a[1]).powi(2)).sqrt();
        let base_ok = match c.kind {
            PrimKind::Box => w > 1e-6 && dpt > 1e-6,
            PrimKind::Cylinder => r > 1e-6,
        };
        let h = match c.phase {
            Phase::Base => 0.0,
            Phase::Height => c.height,
        };
        // a thin slab while the base is drawn
        let shown_h = if h.abs() < 1e-6 { 0.2 } else { h.abs() };
        let down = h < 0.0;
        let rot = basis_rotation(c.plane.u, c.plane.n);
        let rot = if down { rot * DQuat::from_rotation_x(std::f64::consts::PI) } else { rot };
        let id = c.id;
        self.live(|d| {
            if let Some(f) = d.feature_mut(id) {
                f.origin = centre;
                f.rot = rot;
                f.prim = match c.kind {
                    PrimKind::Box => Prim::Box { x: w.max(0.01), y: shown_h, z: dpt.max(0.01) },
                    PrimKind::Cylinder => Prim::Cylinder { radius: r.max(0.01), height: shown_h },
                };
                f.visible = base_ok;
            }
        });
    }

    /// Release of the base drag: on to the height (a click without a drag
    /// places a 20 mm shape).
    pub fn base_released(&mut self, p: [f32; 2]) {
        let Some(mut c) = self.creation.clone() else { return };
        let tiny = match c.kind {
            PrimKind::Box => (c.b[0] - c.a[0]).abs() < 0.5 || (c.b[1] - c.a[1]).abs() < 0.5,
            PrimKind::Cylinder => ((c.b[0] - c.a[0]).powi(2) + (c.b[1] - c.a[1]).powi(2)).sqrt() < 0.5,
        };
        if tiny {
            // direct: a default shape, centred on the click
            let s = 10.0;
            match c.kind {
                PrimKind::Box => {
                    c.b = [c.a[0] + s, c.a[1] + s];
                    c.a = [c.a[0] - s, c.a[1] - s];
                }
                PrimKind::Cylinder => c.b = [c.a[0] + s, c.a[1]],
            }
            c.phase = Phase::Height;
            c.height = if self.mode == Op::Carve && c.plane.face.is_some() { -20.0 } else { 20.0 };
            self.creation = Some(c.clone());
            self.apply_creation(&c);
            self.finish_creation();
            return;
        }
        c.phase = Phase::Height;
        c.height_from = p;
        c.height = 0.0;
        self.creation = Some(c.clone());
        self.apply_creation(&c);
    }

    /// Click in the height phase: done.
    pub fn finish_creation(&mut self) {
        let Some(c) = self.creation.take() else { return };
        if c.height.abs() < 1e-6 {
            // no height: nothing to make
            self.creation = Some(c);
            self.cancel_creation();
            return;
        }
        let id = c.id;
        self.live(|d| {
            if let Some(f) = d.feature_mut(id) {
                f.visible = true;
            }
        });
        self.end_live();
        self.selection = vec![id];
    }

    pub fn cancel_creation(&mut self) {
        if self.creation.take().is_some() {
            self.cancel_live();
            self.clean_selection();
        }
    }

    // --------------------------------------------------------------- moves

    /// Pivot of the selection (centre of the selected shapes).
    pub fn pivot(&self) -> Option<DVec3> {
        let fs: Vec<&Feature> = self.doc.features.iter().filter(|f| self.selection.contains(&f.id)).collect();
        if fs.is_empty() {
            return None;
        }
        Some(fs.iter().map(|f| f.centre()).sum::<DVec3>() / fs.len() as f64)
    }

    /// Where the pointer ray meets what a handle moves along.
    pub fn handle_param(&self, h: Handle, pivot: DVec3, p: [f32; 2]) -> Option<DVec3> {
        let (ro, rd) = self.ray(p);
        match h {
            Handle::Axis(a) => {
                let axis = DVec3::AXES[a];
                closest_on_line(pivot, axis, ro, rd).map(|t| pivot + axis * t)
            }
            Handle::Plane(a) => BasePlane::new(pivot, DVec3::AXES[a], None).hit(ro, rd),
            Handle::Size(_) => {
                let f = self.single()?;
                let axis = size_axis(f, h)?;
                closest_on_line(pivot, axis, ro, rd).map(|t| pivot + axis * t)
            }
        }
    }

    pub fn begin_drag(&mut self, h: Handle, p: [f32; 2]) {
        let Some(pivot) = self.handle_pivot(h) else { return };
        let Some(grab) = self.handle_param(h, pivot, p) else { return };
        let start: Vec<Feature> = self.doc.features.iter().filter(|f| self.selection.contains(&f.id)).cloned().collect();
        self.begin_live(if matches!(h, Handle::Size(_)) { "Resize" } else { "Move" });
        self.drag = Some(Drag { handle: h, grab, start });
    }

    /// The point a handle sits on.
    pub fn handle_pivot(&self, h: Handle) -> Option<DVec3> {
        match h {
            Handle::Size(_) => {
                let f = self.single()?;
                size_handle_pos(f, h)
            }
            _ => self.pivot(),
        }
    }

    pub fn update_drag(&mut self, p: [f32; 2]) {
        let Some(d) = self.drag.clone() else { return };
        let pivot0 = match d.handle {
            Handle::Size(_) => d.start.first().and_then(|f| size_handle_pos(f, d.handle)),
            _ => Some(d.start.iter().map(|f| f.centre()).sum::<DVec3>() / d.start.len().max(1) as f64),
        };
        let Some(pivot0) = pivot0 else { return };
        let Some(now) = self.handle_param(d.handle, pivot0, p) else { return };
        let mut delta = now - d.grab;
        match d.handle {
            Handle::Axis(_) | Handle::Plane(_) => {
                delta = DVec3::new(self.snap(delta.x), self.snap(delta.y), self.snap(delta.z));
                let start = d.start.clone();
                self.live(|doc| {
                    for s in &start {
                        if let Some(f) = doc.feature_mut(s.id) {
                            f.origin = s.origin + delta;
                        }
                    }
                });
            }
            Handle::Size(k) => {
                let Some(s) = d.start.first().cloned() else { return };
                let Some(axis) = size_axis(&s, d.handle) else { return };
                let amount = self.snap(delta.dot(axis));
                let mut f = s.clone();
                match (&mut f.prim, k) {
                    (Prim::Box { x, .. }, 0) => {
                        let nx = (*x + amount).max(0.1);
                        f.origin = s.origin + axis * (nx - *x) * 0.5;
                        *x = nx;
                    }
                    (Prim::Box { z, .. }, 2) => {
                        let nz = (*z + amount).max(0.1);
                        f.origin = s.origin + axis * (nz - *z) * 0.5;
                        *z = nz;
                    }
                    (p, 1) => {
                        let h = (p.height() + amount).max(0.1);
                        p.set_height(h);
                    }
                    (Prim::Cylinder { radius, .. }, 3) => *radius = (*radius + amount).max(0.05),
                    _ => {}
                }
                self.live(|doc| {
                    if let Some(g) = doc.feature_mut(f.id) {
                        *g = f;
                    }
                });
            }
        }
    }

    pub fn end_drag(&mut self) {
        if self.drag.take().is_some() {
            self.end_live();
        }
    }

    // ----------------------------------------------------- kernel and model

    /// Once per frame: takes the kernel's answers, sends the document when
    /// it changed (coarse meshing while a gesture is live, fine after).
    pub fn tick(&mut self, now: f64) {
        self.now = now;
        if self.toast.as_ref().map(|t| now > t.until).unwrap_or(false) {
            self.toast = None;
        }
        while let Some(b) = self.kernel.take() {
            if self.model.as_ref().map(|m| b.model.serial >= m.serial).unwrap_or(true) {
                self.picker = Picker::new(&b.model);
                self.round_ms = b.round_ms;
                self.model = Some(Arc::new(b.model));
                self.model_gen += 1;
            }
        }
        let fine = !self.is_live();
        let want = self.version * 2 + fine as u64;
        if want != self.submitted && self.kernel.status() != Status::Missing {
            self.kernel.submit(want, &self.doc, fine);
            self.submitted = want;
        }
    }

    /// The model shown is behind the document (the kernel is working).
    pub fn building(&self) -> bool {
        self.kernel.busy() || self.model.as_ref().map(|m| m.serial / 2 != self.version).unwrap_or(!self.doc.features.is_empty())
    }

    pub fn toast(&mut self, text: &str, warn: bool) {
        self.toast = Some(Toast { text: text.to_string(), until: self.now + 2.5, warn });
    }

    // ---------------------------------------------------------- documents

    pub fn new_doc(&mut self) {
        self.abort_gestures();
        let before = std::mem::take(&mut self.doc);
        self.undo.push(("New".into(), before));
        self.redo.clear();
        self.selection.clear();
        self.doc_name = "Untitled".into();
        self.save_name = self.doc_name.clone();
        self.touched();
    }

    pub fn open(&mut self, name: &str, doc: Doc) {
        self.abort_gestures();
        let before = std::mem::replace(&mut self.doc, doc);
        self.undo.push(("Open".into(), before));
        self.redo.clear();
        self.selection.clear();
        self.doc_name = name.to_string();
        self.save_name = name.to_string();
        self.touched();
        self.frame_all();
    }

    pub fn save_settings(&self) {
        self.settings.save();
    }
}

/// The rotation turning local X, Y into `u`, `n` (Z = u × n reversed so
/// the frame stays right-handed).
pub fn basis_rotation(u: DVec3, n: DVec3) -> DQuat {
    let x = u.normalize();
    let y = n.normalize();
    let z = x.cross(y);
    DQuat::from_mat3(&glam::DMat3::from_cols(x, y, z))
}

/// Parameter t of the point of the line (o + t d) closest to the ray.
pub fn closest_on_line(o: DVec3, d: DVec3, ro: DVec3, rd: DVec3) -> Option<f64> {
    let w = o - ro;
    let a = d.dot(d);
    let b = d.dot(rd);
    let c = rd.dot(rd);
    let dd = d.dot(w);
    let e = rd.dot(w);
    let den = a * c - b * b;
    if den.abs() < 1e-9 {
        return None;
    }
    Some((b * e - c * dd) / den)
}

/// The world direction a size handle pulls along.
pub fn size_axis(f: &Feature, h: Handle) -> Option<DVec3> {
    match h {
        Handle::Size(0) | Handle::Size(3) => Some(f.rot * DVec3::X),
        Handle::Size(1) => Some(f.axis()),
        Handle::Size(2) => Some(f.rot * DVec3::Z),
        _ => None,
    }
}

/// Where a size handle sits on the shape.
pub fn size_handle_pos(f: &Feature, h: Handle) -> Option<DVec3> {
    let m = f.matrix();
    let hh = f.prim.height();
    let (hx, hz) = f.prim.half_xz();
    let local = match (h, f.prim) {
        (Handle::Size(0), Prim::Box { .. }) => DVec3::new(hx, hh * 0.5, 0.0),
        (Handle::Size(2), Prim::Box { .. }) => DVec3::new(0.0, hh * 0.5, hz),
        (Handle::Size(1), _) => DVec3::new(0.0, hh, 0.0),
        (Handle::Size(3), Prim::Cylinder { .. }) => DVec3::new(hx, hh * 0.5, 0.0),
        _ => return None,
    };
    Some(m.transform_point3(local))
}

/// The size handles of a shape.
pub fn size_handles(f: &Feature) -> Vec<Handle> {
    match f.prim {
        Prim::Box { .. } => vec![Handle::Size(0), Handle::Size(1), Handle::Size(2)],
        Prim::Cylinder { .. } => vec![Handle::Size(3), Handle::Size(1)],
    }
}
