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
use nurbs_kernel::{Doc, EdgeRef, Feature, FeatureId, Model, Op, Prim, Profile};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Create(PrimKind),
}

/// What a click picks in the Select tool: whole shapes, or the part's B-rep
/// edges (to fillet or chamfer them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelMode {
    Object,
    Edge,
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
    /// A sketch's outline (unit square, as it grows upwards).
    pub profile: Option<Profile>,
}

/// The Sketch tool's outline being drawn, on the flat face under its first
/// point (else the ground): plane coordinates, each point a corner or a
/// curve point.
#[derive(Clone, Debug)]
pub struct Sketch {
    pub plane: BasePlane,
    pub pts: Vec<[f64; 2]>,
    pub smooth: Vec<bool>,
    /// The point the press under way placed, and where it began (screen).
    pub held: Option<(usize, [f32; 2])>,
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
    pub sel_mode: SelMode,
    /// The edges picked in Edge mode (not rounded yet).
    pub edge_sel: Vec<EdgeRef>,
    /// The edge under the pointer (index in the model's edges).
    pub hover_edge: Option<usize>,
    /// Fillet or chamfer for the picked edges.
    pub round_chamfer: bool,
    pub hover_feature: Option<FeatureId>,
    pub hover_hit: Option<Hit>,
    pub tool: Tool,
    /// Add or Carve for new shapes.
    pub mode: Op,
    /// Material of new shapes (index in `materials::MATERIALS`).
    pub material: u32,
    pub creation: Option<Creation>,
    /// The Sketch tool's outline being drawn.
    pub sketch: Option<Sketch>,
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
    /// Quads view: the part as the kernel's quad mesh (fillets in orange).
    pub quads: bool,
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
            sel_mode: SelMode::Object,
            edge_sel: vec![],
            hover_edge: None,
            round_chamfer: false,
            hover_feature: None,
            hover_hit: None,
            tool: Tool::Select,
            mode: Op::Add,
            material: 0,
            creation: None,
            sketch: None,
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
            quads: false,
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
        self.edge_sel.clear();
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
            if !self.edge_sel.is_empty() {
                self.edge_sel.clear();
            }
            return;
        }
        let ids = self.selection.clone();
        let n = ids.len();
        let what = match self.single() {
            Some(f) if !f.is_solid() => f.kind_label().to_string(),
            _ => format!("{n} shape{}", if n > 1 { "s" } else { "" }),
        };
        self.edit("Delete", |d| d.remove(&ids));
        self.selection.clear();
        self.toast(&format!("{what} deleted"), false);
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

    /// Arms a creation tool; the armed tool again places a shape at once
    /// (`quick_add`).
    pub fn arm(&mut self, kind: PrimKind) {
        if kind == PrimKind::Profile && self.tool == Tool::Create(kind) {
            return;
        }
        self.abort_gestures();
        if self.tool == Tool::Create(kind) {
            self.quick_add(kind);
        } else {
            self.tool = Tool::Create(kind);
        }
    }

    /// Quick creation (the armed tool again, or Shift + its key): a shape
    /// placed at once and selected. Add: 20 mm, stacked on the selected
    /// shape's top along its axis (at the world origin with nothing
    /// selected). Carve: drilled through the selected shape, from its top.
    pub fn quick_add(&mut self, kind: PrimKind) {
        if kind == PrimKind::Profile {
            self.arm(kind);
            return;
        }
        self.abort_gestures();
        let base = self.selection.iter().rev().filter_map(|id| self.doc.feature(*id)).find(|f| f.is_solid()).cloned();
        let op = self.mode;
        let (prim, origin, rot, how) = match (&base, op) {
            (Some(b), Op::Add) => (kind.default_prim(), b.origin + b.axis() * b.prim.height(), b.rot, "on top of"),
            (Some(b), Op::Carve) => {
                let h = b.prim.height();
                let (hx, hz) = b.prim.half_xz();
                // a quarter of the base's smaller side, at most 5 mm
                let r = (hx.min(hz) * 0.5).clamp(0.2, 5.0);
                let prim = match kind {
                    PrimKind::Box | PrimKind::Profile => Prim::Box { x: 2.0 * r, y: h + 2.0, z: 2.0 * r },
                    PrimKind::Cylinder => Prim::Cylinder { radius: r, height: h + 2.0 },
                };
                // from 1 mm above the top, down through the base
                (prim, b.origin + b.axis() * (h + 1.0), b.rot * DQuat::from_rotation_x(std::f64::consts::PI), "through")
            }
            (None, _) => (kind.default_prim(), DVec3::ZERO, DQuat::IDENTITY, "at the origin"),
        };
        let mat = self.material;
        let mut id = 0;
        self.edit(&format!("New {}", kind.label()), |d| {
            id = d.add(op, prim, origin, rot);
            if let Some(f) = d.feature_mut(id) {
                f.material = mat;
            }
        });
        self.tool = Tool::Select;
        self.sel_mode = SelMode::Object;
        self.edge_sel.clear();
        self.selection = vec![id];
        let target = base.map(|b| format!(" {how} {}", b.name)).unwrap_or_else(|| format!(" {how}"));
        self.toast(&format!("{} {}{target}", kind.label(), op.label()), false);
    }

    /// The material a feature's faces show: its own, or for a fillet the
    /// one of the shape its first edge belongs to.
    pub fn material_of(&self, id: FeatureId) -> u32 {
        let Some(f) = self.doc.feature(id) else { return 0 };
        match &f.round {
            None => f.material,
            Some(r) => r
                .edges
                .first()
                .and_then(|e| nurbs_kernel::FaceTag::unpack(e.faces[0]))
                .and_then(|t| self.doc.feature(t.feature))
                .filter(|g| g.is_solid())
                .map(|g| g.material)
                .unwrap_or(0),
        }
    }

    /// Gives the selected shapes a material (with nothing selected: the
    /// material of new shapes).
    pub fn set_material(&mut self, m: u32) {
        let ids: Vec<FeatureId> = self.selection.iter().copied().filter(|id| self.doc.feature(*id).map(|f| f.is_solid()).unwrap_or(false)).collect();
        if ids.is_empty() {
            self.material = m;
            return;
        }
        self.edit("Material", |d| {
            for id in &ids {
                if let Some(f) = d.feature_mut(*id) {
                    f.material = m;
                }
            }
        });
    }

    pub fn select_tool(&mut self) {
        self.abort_gestures();
        self.tool = Tool::Select;
    }

    /// Object or Edge picking (with the Select tool).
    pub fn set_sel_mode(&mut self, m: SelMode) {
        self.abort_gestures();
        self.tool = Tool::Select;
        if m != self.sel_mode {
            self.sel_mode = m;
            self.edge_sel.clear();
            self.hover_edge = None;
            // edges: only a fillet stays selected
            if m == SelMode::Edge {
                let keep: Vec<FeatureId> = self.selection.iter().copied().filter(|id| self.doc.feature(*id).map(|f| !f.is_solid()).unwrap_or(false)).collect();
                self.selection = keep;
            }
        }
    }

    // --------------------------------------------------------------- edges

    /// The reference of a model edge: the tags of its two faces and its
    /// middle point. None for a seam or an edge without two tagged faces.
    pub fn edge_ref(&self, i: usize) -> Option<EdgeRef> {
        let m = self.model.as_ref()?;
        let e = m.edges.get(i)?;
        if e.seam || e.faces[0] < 0 || e.faces[1] < 0 {
            return None;
        }
        let a = m.faces.get(e.faces[0] as usize)?.tag?.pack();
        let b = m.faces.get(e.faces[1] as usize)?.tag?.pack();
        Some(EdgeRef { faces: [a, b], at: polyline_middle(&e.pts)? })
    }

    /// The model edge a reference names (the one nearest its point among
    /// those joining the same two faces).
    pub fn model_edge_of(&self, r: &EdgeRef) -> Option<usize> {
        let m = self.model.as_ref()?;
        let want = r.key();
        let at = r.at.as_vec3();
        m.edges
            .iter()
            .enumerate()
            .filter(|(i, _)| self.edge_ref(*i).map(|x| x.key() == want).unwrap_or(false))
            .map(|(i, e)| (i, e.pts.iter().map(|p| Vec3::from(*p).distance(at)).fold(f32::MAX, f32::min)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// The edge under the pointer: the nearest on screen (within a few
    /// points) that the part does not hide.
    pub fn pick_edge(&self, p: [f32; 2]) -> Option<usize> {
        let m = self.model.as_ref()?;
        let v = self.view();
        let mut cands: Vec<(f32, usize, Vec3)> = vec![];
        for (i, e) in m.edges.iter().enumerate() {
            if e.seam || e.faces[1] < 0 {
                continue;
            }
            let mut best: Option<(f32, Vec3)> = None;
            for w in e.pts.windows(2) {
                let (a, b) = (Vec3::from(w[0]), Vec3::from(w[1]));
                let (Some(sa), Some(sb)) = (v.project(a), v.project(b)) else { continue };
                let ab = [sb[0] - sa[0], sb[1] - sa[1]];
                let len2 = (ab[0] * ab[0] + ab[1] * ab[1]).max(1e-6);
                let t = (((p[0] - sa[0]) * ab[0] + (p[1] - sa[1]) * ab[1]) / len2).clamp(0.0, 1.0);
                let d = ((sa[0] + ab[0] * t - p[0]).powi(2) + (sa[1] + ab[1] * t - p[1]).powi(2)).sqrt();
                if best.map(|b| d < b.0).unwrap_or(true) {
                    best = Some((d, a.lerp(b, t)));
                }
            }
            if let Some((d, q)) = best {
                if d < 7.0 {
                    cands.push((d, i, q));
                }
            }
        }
        cands.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, i, q) in cands {
            // hidden when the part is hit before the edge, seen from the eye
            let to = q - v.eye;
            let dist = to.length();
            let hit = self.picker.pick(m, v.eye, to / dist.max(1e-6));
            let tol = 0.3 + dist * 0.004;
            if hit.map(|h| h.t >= dist - tol).unwrap_or(true) {
                return Some(i);
            }
        }
        None
    }

    /// A click in Edge mode on an edge (or on nothing): picks it, adds it
    /// (Shift / Ctrl), or adds it to the selected fillet.
    pub fn click_edge(&mut self, edge: Option<usize>, add: bool) {
        let Some(r) = edge.and_then(|i| self.edge_ref(i)) else {
            if !add {
                self.edge_sel.clear();
                self.selection.clear();
            }
            return;
        };
        // a fillet selected and Shift / Ctrl: the edge joins it
        if add {
            if let Some(id) = self.single().filter(|f| !f.is_solid()).map(|f| f.id) {
                let label = self.single().map(|f| f.kind_label()).unwrap_or("Fillet");
                self.edit(&format!("{label}: one more edge"), |d| {
                    if let Some(rd) = d.feature_mut(id).and_then(|f| f.round.as_mut()) {
                        rd.edges.push(r);
                    }
                });
                return;
            }
        }
        self.selection.clear();
        let key = r.key();
        let same = |x: &EdgeRef| x.key() == key && x.at.distance(r.at) < 1e-3;
        if add {
            if let Some(i) = self.edge_sel.iter().position(same) {
                self.edge_sel.remove(i);
            } else {
                self.edge_sel.push(r);
            }
        } else {
            self.edge_sel = vec![r];
        }
    }

    /// The selected fillet or chamfer, if one is.
    pub fn selected_round(&self) -> Option<&Feature> {
        self.single().filter(|f| !f.is_solid())
    }

    /// Sets the size (and kind) of the selected fillet, or makes one of the
    /// picked edges. `start`: a new gesture (one undo step until `end_live`).
    pub fn set_round(&mut self, size: f64, chamfer: bool, start: bool) {
        let label = if chamfer { "Chamfer" } else { "Fillet" };
        if let Some(id) = self.selected_round().map(|f| f.id) {
            if start {
                self.begin_live(label);
            }
            self.live(|d| {
                // "Fillet 2" becomes "Chamfer 2"… when its kind changes (a
                // name the user gave stays)
                let rename = d.feature(id).and_then(|f| {
                    let old = f.round.as_ref()?.label();
                    let new = if chamfer { "Chamfer" } else { "Fillet" };
                    (old != new && f.name.starts_with(old)).then(|| d.fresh_name(new))
                });
                if let Some(f) = d.feature_mut(id) {
                    if let Some(name) = rename {
                        f.name = name;
                    }
                    if let Some(r) = f.round.as_mut() {
                        r.size = size.max(0.0);
                        r.chamfer = chamfer;
                    }
                }
            });
        } else if !self.edge_sel.is_empty() && size > 1e-6 {
            self.begin_live(label);
            let edges = std::mem::take(&mut self.edge_sel);
            let mut id = 0;
            self.live(|d| id = d.add_round(size, chamfer, edges));
            self.selection = vec![id];
        }
        self.round_chamfer = chamfer;
    }

    /// The largest size the fillet slider offers: half the part's thinnest
    /// extent (OpenCascade says when it cannot make it).
    pub fn round_max(&self) -> f64 {
        let ext = self.model.as_ref().and_then(|m| m.bounds()).map(|(lo, hi)| (0..3).map(|k| (hi[k] - lo[k]) as f64).fold(f64::MAX, f64::min)).unwrap_or(20.0);
        crate::widgets::nice_ceil((ext * 0.5).clamp(1.0, 100.0) as f32) as f64
    }

    /// Cancels what is under way (creation, sketch, drag).
    pub fn abort_gestures(&mut self) {
        if self.creation.take().is_some() || self.drag.take().is_some() {
            self.cancel_live();
        }
        self.sketch = None;
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
        if kind == PrimKind::Profile {
            return;
        }
        let Some((plane, q)) = self.base_plane_at(p) else { return };
        let mut a = plane.to_plane(q);
        a = [self.snap(a[0]), self.snap(a[1])];
        self.begin_live(&format!("New {}", kind.label()));
        let op = self.mode;
        let origin = plane.at(a);
        let rot = basis_rotation(plane.u, plane.n);
        // a tiny shape at first: it grows with the drag
        let prim = match kind {
            PrimKind::Box | PrimKind::Profile => Prim::Box { x: 0.01, y: 0.01, z: 0.01 },
            PrimKind::Cylinder => Prim::Cylinder { radius: 0.01, height: 0.01 },
        };
        let mut id = 0;
        let mat = self.material;
        self.live(|d| id = d.add(op, prim, origin, rot));
        // hidden until it has a size
        if let Some(f) = self.doc.feature_mut(id) {
            f.visible = false;
            f.material = mat;
        }
        self.creation = Some(Creation { kind, id, plane, a, b: a, height: 0.0, phase: Phase::Base, height_from: p, profile: None });
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
            PrimKind::Box | PrimKind::Profile => c.plane.at([(c.a[0] + c.b[0]) * 0.5, (c.a[1] + c.b[1]) * 0.5]),
            PrimKind::Cylinder => c.plane.at(c.a),
        }
    }

    /// Writes the creation into its feature.
    fn apply_creation(&mut self, c: &Creation) {
        let centre = self.creation_base_centre(c);
        let (w, dpt) = ((c.b[0] - c.a[0]).abs(), (c.b[1] - c.a[1]).abs());
        let r = ((c.b[0] - c.a[0]).powi(2) + (c.b[1] - c.a[1]).powi(2)).sqrt();
        let base_ok = match c.kind {
            PrimKind::Box | PrimKind::Profile => w > 1e-6 && dpt > 1e-6,
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
                    PrimKind::Profile => Prim::Profile { x: w.max(0.01), y: shown_h, z: dpt.max(0.01) },
                };
                // turned over to grow down: the outline mirrored stays in place
                if let Some(pr) = &c.profile {
                    f.profile = Some(if down { pr.flipped() } else { pr.clone() });
                }
                f.visible = base_ok;
            }
        });
    }

    /// Release of the base drag: on to the height (a click without a drag
    /// places a 20 mm shape).
    pub fn base_released(&mut self, p: [f32; 2]) {
        let Some(mut c) = self.creation.clone() else { return };
        let tiny = match c.kind {
            PrimKind::Box | PrimKind::Profile => (c.b[0] - c.a[0]).abs() < 0.5 || (c.b[1] - c.a[1]).abs() < 0.5,
            PrimKind::Cylinder => ((c.b[0] - c.a[0]).powi(2) + (c.b[1] - c.a[1]).powi(2)).sqrt() < 0.5,
        };
        if tiny {
            // direct: a default shape, centred on the click
            let s = 10.0;
            match c.kind {
                PrimKind::Box | PrimKind::Profile => {
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

    // -------------------------------------------------------------- sketch

    /// Where the pointer is on the sketch's plane (snapped).
    pub fn sketch_point(&self, p: [f32; 2]) -> Option<[f64; 2]> {
        let k = self.sketch.as_ref()?;
        let (ro, rd) = self.ray(p);
        let q = k.plane.to_plane(k.plane.hit(ro, rd)?);
        Some([self.snap(q[0]), self.snap(q[1])])
    }

    /// Whether the pointer is on the sketch's first point (it closes it).
    pub fn sketch_on_first(&self, p: [f32; 2]) -> bool {
        let Some(k) = &self.sketch else { return false };
        if k.pts.len() < 3 {
            return false;
        }
        let Some(s) = self.view().project(k.plane.at(k.pts[0]).as_vec3()) else { return false };
        (s[0] - p[0]).hypot(s[1] - p[1]) < 10.0
    }

    /// A press with the Sketch tool: the first point (it picks the face
    /// under it, else the ground), the next ones, or the first point again
    /// to close the outline.
    pub fn sketch_press(&mut self, p: [f32; 2]) {
        if self.sketch.is_none() {
            let Some((plane, q)) = self.base_plane_at(p) else { return };
            let a = plane.to_plane(q);
            self.sketch = Some(Sketch { plane, pts: vec![[self.snap(a[0]), self.snap(a[1])]], smooth: vec![false], held: Some((0, p)) });
            self.selection.clear();
            return;
        }
        if self.sketch_on_first(p) {
            self.sketch_close(p);
            return;
        }
        let Some(q) = self.sketch_point(p) else { return };
        let k = self.sketch.as_mut().unwrap();
        // the same point twice: nothing
        if k.pts.last().map(|l| (l[0] - q[0]).abs() + (l[1] - q[1]).abs() < 1e-9).unwrap_or(false) {
            k.held = None;
            return;
        }
        k.pts.push(q);
        k.smooth.push(false);
        k.held = Some((k.pts.len() - 1, p));
    }

    /// Dragging the press: the point follows and becomes a curve point.
    pub fn sketch_drag(&mut self, p: [f32; 2]) {
        let q = self.sketch_point(p);
        let Some(k) = self.sketch.as_mut() else { return };
        let (Some((i, from)), Some(q)) = (k.held, q) else { return };
        if (p[0] - from[0]).hypot(p[1] - from[1]) > 4.0 {
            k.pts[i] = q;
            k.smooth[i] = true;
        }
    }

    pub fn sketch_release(&mut self) {
        if let Some(k) = self.sketch.as_mut() {
            k.held = None;
        }
    }

    /// Backspace: the last point goes (the sketch with it if none is left).
    pub fn sketch_back(&mut self) {
        if let Some(k) = self.sketch.as_mut() {
            k.pts.pop();
            k.smooth.pop();
            k.held = None;
            if k.pts.is_empty() {
                self.sketch = None;
            }
        }
    }

    /// Closes the outline (Enter, or a click on its first point): it becomes
    /// a shape whose height follows the pointer, like the other tools.
    pub fn sketch_close(&mut self, p: [f32; 2]) {
        let Some(k) = self.sketch.clone() else { return };
        if k.pts.len() < 3 {
            self.toast("A sketch needs 3 points at least", true);
            return;
        }
        let lo = k.pts.iter().fold([f64::MAX; 2], |a, q| [a[0].min(q[0]), a[1].min(q[1])]);
        let hi = k.pts.iter().fold([f64::MIN; 2], |a, q| [a[0].max(q[0]), a[1].max(q[1])]);
        let (w, dp) = (hi[0] - lo[0], hi[1] - lo[1]);
        if w < 0.5 || dp < 0.5 {
            self.toast("The outline is flat: draw it wider", true);
            return;
        }
        let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
        let profile = Profile { points: k.pts.iter().map(|q| [(q[0] - c[0]) / w, (q[1] - c[1]) / dp]).collect(), smooth: k.smooth.clone() };
        self.sketch = None;
        self.begin_live("New Sketch");
        let op = self.mode;
        let mat = self.material;
        let (origin, rot) = (k.plane.at(c), basis_rotation(k.plane.u, k.plane.n));
        let mut id = 0;
        let pr = profile.clone();
        self.live(|d| {
            id = d.add(op, Prim::Profile { x: w, y: 0.2, z: dp }, origin, rot);
            if let Some(f) = d.feature_mut(id) {
                f.material = mat;
                f.profile = Some(pr);
            }
        });
        let cr = Creation { kind: PrimKind::Profile, id, plane: k.plane, a: lo, b: hi, height: 0.0, phase: Phase::Height, height_from: p, profile: Some(profile) };
        self.creation = Some(cr.clone());
        self.selection = vec![id];
        self.apply_creation(&cr);
    }

    // --------------------------------------------------------------- moves

    /// Pivot of the selection (centre of the selected shapes).
    pub fn pivot(&self) -> Option<DVec3> {
        if self.sel_mode != SelMode::Object {
            return None;
        }
        let fs: Vec<&Feature> = self.doc.features.iter().filter(|f| self.selection.contains(&f.id) && f.is_solid()).collect();
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
        let start: Vec<Feature> = self.doc.features.iter().filter(|f| self.selection.contains(&f.id) && f.is_solid()).cloned().collect();
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
                    (Prim::Box { x, .. } | Prim::Profile { x, .. }, 0) => {
                        let nx = (*x + amount).max(0.1);
                        f.origin = s.origin + axis * (nx - *x) * 0.5;
                        *x = nx;
                    }
                    (Prim::Box { z, .. } | Prim::Profile { z, .. }, 2) => {
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
        // quads only for a finished state (not while a gesture is live)
        let quads = self.quads && fine;
        let want = self.version * 4 + fine as u64 * 2 + quads as u64;
        if want != self.submitted && self.kernel.status() != Status::Missing {
            self.kernel.submit(want, &self.doc, fine, quads);
            self.submitted = want;
        }
    }

    /// The model shown is behind the document (the kernel is working).
    pub fn building(&self) -> bool {
        self.kernel.busy() || self.model.as_ref().map(|m| m.serial / 4 != self.version).unwrap_or(!self.doc.features.is_empty())
    }

    /// The quad mesh of the part, when the Quads view has it.
    pub fn quad_mesh(&self) -> Option<&nurbs_kernel::Quads> {
        self.model.as_ref().and_then(|m| m.quads.as_ref()).filter(|_| self.quads)
    }

    /// Writes the quads as Wavefront OBJ next to the documents (one group
    /// per face, named after its shape; faces the converter left out are
    /// not in it).
    pub fn export_obj(&mut self) {
        let Some(m) = self.model.clone() else { return };
        let Some(q) = m.quads.as_ref() else {
            self.toast("No quads yet: turn on the Quads view", true);
            return;
        };
        let mut s = String::from("# Forme NURBS: quads from the B-rep\n");
        for p in &q.positions {
            s.push_str(&format!("v {:.5} {:.5} {:.5}\n", p[0], p[1], p[2]));
        }
        let mut last = u32::MAX;
        for (k, f) in q.quads.iter().zip(&q.quad_face) {
            if *f != last {
                last = *f;
                let tag = m.faces.get(*f as usize).and_then(|x| x.tag);
                let who = tag.and_then(|t| self.doc.feature(t.feature)).map(|x| crate::files::sanitize(&x.name).replace(' ', "_")).unwrap_or_else(|| "part".into());
                s.push_str(&format!("g {who}_face{f}\n"));
            }
            s.push_str(&format!("f {} {} {} {}\n", k[0] + 1, k[1] + 1, k[2] + 1, k[3] + 1));
        }
        let path = crate::files::documents_dir().join(format!("{}.obj", crate::files::sanitize(&self.doc_name)));
        match std::fs::write(&path, s) {
            Ok(()) => {
                let text = format!("Saved {} quads to {}", q.quads.len(), path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                self.toast(&text, false);
            }
            Err(e) => self.toast(&format!("Export failed: {e}"), true),
        }
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

/// The point halfway along a polyline (a straight edge has only its two
/// ends: its middle, not a corner shared with other edges).
pub fn polyline_middle(pts: &[[f32; 3]]) -> Option<DVec3> {
    let p: Vec<DVec3> = pts.iter().map(|q| DVec3::new(q[0] as f64, q[1] as f64, q[2] as f64)).collect();
    let total: f64 = p.windows(2).map(|w| w[0].distance(w[1])).sum();
    let mut left = total * 0.5;
    for w in p.windows(2) {
        let l = w[0].distance(w[1]);
        if l >= left && l > 0.0 {
            return Some(w[0].lerp(w[1], left / l));
        }
        left -= l;
    }
    p.first().copied()
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
    if !f.is_solid() {
        return None;
    }
    let m = f.matrix();
    let hh = f.prim.height();
    let (hx, hz) = f.prim.half_xz();
    let local = match (h, f.prim) {
        (Handle::Size(0), Prim::Box { .. } | Prim::Profile { .. }) => DVec3::new(hx, hh * 0.5, 0.0),
        (Handle::Size(2), Prim::Box { .. } | Prim::Profile { .. }) => DVec3::new(0.0, hh * 0.5, hz),
        (Handle::Size(1), _) => DVec3::new(0.0, hh, 0.0),
        (Handle::Size(3), Prim::Cylinder { .. }) => DVec3::new(hx, hh * 0.5, 0.0),
        _ => return None,
    };
    Some(m.transform_point3(local))
}

/// The size handles of a shape.
pub fn size_handles(f: &Feature) -> Vec<Handle> {
    if !f.is_solid() {
        return vec![];
    }
    match f.prim {
        Prim::Box { .. } | Prim::Profile { .. } => vec![Handle::Size(0), Handle::Size(1), Handle::Size(2)],
        Prim::Cylinder { .. } => vec![Handle::Size(3), Handle::Size(1)],
    }
}
