//! The app's state: the document, its history, the selection, the tools,
//! and the part the kernel built for it. Both the viewport and the panels
//! drive it; every change of the document goes to the kernel (the latest
//! one wins), and the part shown is always the kernel's.

use crate::camera::OrbitCamera;
use crate::pick::{Hit, Picker};
use crate::settings::Settings;
use crate::view::View;
use glam::{DMat4, DQuat, DVec3, Vec3};
use nurbs_kernel::client::{Kernel, Status};
use nurbs_kernel::doc::{CurveKind, CurveOp, PrimKind, ShapeBool};
use nurbs_kernel::{Doc, EdgeRef, FaceRef, FaceTag, Feature, FeatureId, Knife, Model, Op, Prim, Profile};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Create(PrimKind),
    /// Inset a flat face, or Extrude a face (a curved one along its normals).
    Push(PushKind),
    /// The Knife (K): points in edges joined by lines across flat faces.
    Knife,
    /// Point on edge (J): points only.
    Point,
    /// The Slot (S): a drag along a slot (or a raised pill) of the
    /// reference, read there on release (`scan.rs`).
    Slot,
    /// A curve of the Curves set (3ds Max's Shapes): Rectangle and Ellipse
    /// drawn corner to corner, Circle, Polygon and Star from their centre.
    Curve(CurveKind),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushKind {
    Inset,
    Extrude,
}

impl PushKind {
    pub fn label(self) -> &'static str {
        match self {
            PushKind::Inset => "Inset",
            PushKind::Extrude => "Extrude",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            PushKind::Inset => "I",
            PushKind::Extrude => "Ctrl+E",
        }
    }
}

/// What a click picks in the Select tool (the component modes, keys 1 to
/// 4 like Plasticity): the part's B-rep vertices, its edges (to fillet or
/// chamfer them), its faces (to inset or extrude them), or whole shapes.
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

/// An extrusion under way: a step in the document from the start, the
/// kernel shows the real result live. Pressed on a face and dragged
/// (`held`, the release ends it), or started on the picked face with E (the
/// pointer moves, a click ends it). (An inset is made at once, then sized
/// with its handle.)
#[derive(Clone, Debug)]
pub struct PushGesture {
    pub kind: PushKind,
    pub id: FeatureId,
    /// Where the face was grabbed (on it), its outward normal.
    pub origin: DVec3,
    pub n: DVec3,
    pub held: bool,
    /// Pointer at the phase's start, and its place along the normal then.
    pub from: [f32; 2],
    pub t0: f64,
    pub moved: bool,
    pub inset: f64,
    pub depth: f64,
    /// The other picked faces extruded with it, each along its own normal
    /// by the same depth (19 nov. 23 h 21).
    pub others: Vec<FeatureId>,
}

/// The primitives the app creates (step 1: cube and cylinder).
pub const PRIMS: [PrimKind; 2] = [PrimKind::Box, PrimKind::Cylinder];

/// A plane the base of a new shape is drawn on: the ground, a flat face of
/// the part, or the reference's (the scan's) face under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BasePlane {
    pub origin: DVec3,
    pub u: DVec3,
    pub n: DVec3,
    pub v: DVec3,
    /// The face it lies on (index in the model), None for the ground.
    pub face: Option<usize>,
    /// On the reference: Some(true) its flat face, Some(false) its tangent
    /// plane (no flat face there).
    pub scan: Option<bool>,
}

impl BasePlane {
    pub fn ground() -> BasePlane {
        BasePlane { origin: DVec3::ZERO, u: DVec3::X, n: DVec3::Y, v: DVec3::Z, face: None, scan: None }
    }
    /// Where it lies, for the pointer's chip.
    pub fn label(&self) -> &'static str {
        match (self.face, self.scan) {
            (Some(_), _) => "on the face",
            (None, Some(true)) => "on the scan's face",
            (None, Some(false)) => "tangent to the scan",
            (None, None) => "on the ground",
        }
    }
    /// On a face of the part or of the scan (a Carve goes into it).
    pub fn on_face(&self) -> bool {
        self.face.is_some() || self.scan.is_some()
    }
    /// A plane through `origin` with normal `n`, its u axis as close to the
    /// world X (else Z) as it can.
    pub fn new(origin: DVec3, n: DVec3, face: Option<usize>) -> BasePlane {
        let n = n.normalize();
        let pick = if n.x.abs() < 0.9 { DVec3::X } else { DVec3::Z };
        let u = (pick - n * pick.dot(n)).normalize();
        // (u, n, v) right-handed like (X, Y, Z)
        BasePlane { origin, u, n, v: u.cross(n), face, scan: None }
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
    /// A curve of the Curves set (`kind` is Path): no height phase.
    pub curve: Option<CurveKind>,
}

/// A closed curve or 2D boolean pulled into a solid (step 3 of the Curves
/// set, 3ds Max's Extrude): the pointer sets its height along its normal
/// (down: its frame turned half round its Z, the curve staying in place), a
/// click keeps it, Esc drops it.
#[derive(Clone, Debug)]
pub struct CurvePull {
    pub id: FeatureId,
    /// The feature as it was (flat, or already pulled).
    pub start: Feature,
    /// Where the pointer was when it began (screen).
    pub from: [f32; 2],
    /// The height it began at (an extruded one pulled again).
    pub h0: f64,
    pub h: f64,
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
    /// Each point's fillet (mm, Ctrl + drag on a point placed, from Forme
    /// Rectify's Pen), and the point whose fillet is being dragged.
    pub corner: Vec<f64>,
    pub grab: Option<usize>,
    /// The Surface tool off its plane ("On scan"): each point where it was
    /// put on the reference (None: on the plane).
    pub free: Vec<Option<DVec3>>,
    /// The Surface's Curve mode: the points where the curve breaks in a V
    /// (Shift + click on a point placed).
    pub sharp: Vec<bool>,
}

impl Sketch {
    /// Point `i` in the world: on the scan where put there, else on the plane.
    pub fn world(&self, i: usize) -> DVec3 {
        self.free.get(i).copied().flatten().unwrap_or_else(|| self.plane.at(self.pts[i]))
    }

    /// The outline in the world (the Surface): straight sides with their
    /// corners' fillets, or (`curve`) the Catmull-Rom curve through the
    /// points, broken at the sharp ones (whose fillets still apply).
    pub fn outline3(&self, extra: Option<DVec3>, closed: bool, curve: bool) -> Vec<DVec3> {
        let mut w: Vec<DVec3> = (0..self.pts.len()).map(|i| self.world(i)).collect();
        let mut c: Vec<f64> = (0..w.len()).map(|i| self.corner.get(i).copied().unwrap_or(0.0)).collect();
        let mut sharp: Vec<bool> = (0..w.len()).map(|i| self.sharp.get(i).copied().unwrap_or(false)).collect();
        if let Some(e) = extra {
            w.push(e);
            c.push(0.0);
            sharp.push(false);
        }
        if curve && w.len() >= 3 {
            catmull_rom(&w, &sharp, closed)
        } else {
            rounded(&w, &c, closed)
        }
    }
}

/// SplitIt's curve (Romain 19 nov., 22 h 09, its spec): the uniform
/// Catmull-Rom through the points, each span sampled adaptively (12 steps,
/// then halved while the middle, quarter or three-quarter point is off the
/// chord by more than 5e-4 of the points' box diagonal; at most 513 per
/// span). An open curve's ends and a sharp point take the reflected
/// control (2 P1 - P2), so the curve leaves them straight (a V at a sharp
/// point). Exact at the points.
pub fn catmull_rom(w: &[DVec3], sharp: &[bool], closed: bool) -> Vec<DVec3> {
    let n = w.len();
    if n < 3 {
        return w.to_vec();
    }
    let (lo, hi) = w.iter().fold((DVec3::splat(f64::MAX), DVec3::splat(f64::MIN)), |(a, b), q| (a.min(*q), b.max(*q)));
    let tol = (hi - lo).length() * 5e-4;
    let spans = if closed { n } else { n - 1 };
    let mut out = vec![];
    for i in 0..spans {
        let (p1, p2) = (w[i], w[(i + 1) % n]);
        let mut p0 = if closed { w[(i + n - 1) % n] } else { w[i.saturating_sub(1)] };
        let mut p3 = if closed { w[(i + 2) % n] } else { w[(i + 2).min(n - 1)] };
        if sharp.get(i).copied().unwrap_or(false) || (!closed && i == 0) {
            p0 = 2.0 * p1 - p2;
        }
        if sharp.get((i + 1) % n).copied().unwrap_or(false) || (!closed && i == n - 2) {
            p3 = 2.0 * p2 - p1;
        }
        let (c0, c1, c2, c3) = (2.0 * p1, p2 - p0, 2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3, -p0 + 3.0 * p1 - 3.0 * p2 + p3);
        let at = |u: f64| -> DVec3 {
            if u <= 0.0 {
                p1
            } else if u >= 1.0 {
                p2
            } else {
                0.5 * (c0 + c1 * u + c2 * u * u + c3 * u * u * u)
            }
        };
        // adaptive: a stack of intervals, split while off the chord
        let mut us: Vec<f64> = vec![];
        let mut stack: Vec<(f64, f64)> = (0..12).rev().map(|k| (k as f64 / 12.0, (k + 1) as f64 / 12.0)).collect();
        us.push(0.0);
        while let Some((a, b)) = stack.pop() {
            let (pa, pb) = (at(a), at(b));
            let off = [0.25, 0.5, 0.75].iter().map(|f| at(a + (b - a) * f).distance(pa.lerp(pb, *f))).fold(0.0, f64::max);
            if off > tol && b - a > 1e-8 && us.len() + stack.len() < 512 {
                let m = 0.5 * (a + b);
                stack.push((m, b));
                stack.push((a, m));
            } else {
                us.push(b);
            }
        }
        for (k, u) in us.iter().enumerate() {
            // the span's end is the next span's start
            if k + 1 == us.len() && (closed || i + 1 < spans) {
                continue;
            }
            out.push(at(*u));
        }
    }
    out
}

/// A polyline whose corners are cut by circular arcs (radius `corner[i]`,
/// at most half of each side next to it), sampled.
pub fn rounded(w: &[DVec3], corner: &[f64], closed: bool) -> Vec<DVec3> {
    let n = w.len();
    let mut out = vec![];
    for i in 0..n {
        let r = corner.get(i).copied().unwrap_or(0.0);
        let ends = !closed && (i == 0 || i + 1 == n);
        if r <= 0.0 || ends || n < 3 {
            out.push(w[i]);
            continue;
        }
        let (p, q, c) = (w[(i + n - 1) % n], w[(i + 1) % n], w[i]);
        let (lp, lq) = ((p - c).length(), (q - c).length());
        if lp < 1e-9 || lq < 1e-9 {
            out.push(c);
            continue;
        }
        let (a, b) = ((p - c) / lp, (q - c) / lq);
        let ang = a.dot(b).clamp(-1.0, 1.0).acos();
        if ang < 1e-3 || ang > std::f64::consts::PI - 1e-3 {
            out.push(c);
            continue;
        }
        let t = (r / (ang * 0.5).tan()).min(lp * 0.5).min(lq * 0.5);
        let (s0, s1) = (c + a * t, c + b * t);
        // a rational quadratic: the exact arc between both tangent points
        let wt = (ang * 0.5).sin();
        for k in 0..=10 {
            let u = k as f64 / 10.0;
            let (b0, b1, b2) = ((1.0 - u) * (1.0 - u), 2.0 * u * (1.0 - u) * wt, u * u);
            out.push((s0 * b0 + c * b1 + s1 * b2) / (b0 + b1 + b2));
        }
    }
    out
}

/// What a gizmo handle moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    /// Move along a world axis (0 X, 1 Y, 2 Z).
    Axis(usize),
    /// Move in the plane normal to an axis.
    Plane(usize),
    /// Move on all three axes (the gizmo's centre, Maya; Romain 20 nov.,
    /// 9 h 34): in the plane facing the eye through the pivot.
    Free,
    /// A size of the selected shape: 0 width (+X), 1 height (top), 2 depth (+Z), 3 radius.
    Size(usize),
    /// The selected inset's handle: the outline pulled in from the face's border.
    PushInset,
    /// The selected inset or extrusion's arrow: along the face's normal.
    PushDepth,
    /// Rotate gizmo (E): the ring round a world axis.
    Rot(usize),
    /// Scale gizmo (R): along the shape's own X, Y, Z, or all at once.
    Scale(usize),
    ScaleAll,
}

/// The directions a move handle may take: along its axis, in its plane, or
/// all three (the centre); None for the other handles.
pub fn move_mask(h: Handle) -> Option<DVec3> {
    match h {
        Handle::Axis(a) => Some(DVec3::AXES[a]),
        Handle::Plane(a) => Some(DVec3::ONE - DVec3::AXES[a]),
        Handle::Free => Some(DVec3::ONE),
        _ => None,
    }
}

/// What the gizmo of the selection does (Maya: Q select, W move, E rotate,
/// R scale; Romain 19 nov. "Réintroduire Move, Scale, Rotate, Select sur
/// QWER").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GizmoMode {
    /// No gizmo: clicks and rectangles pick.
    Select,
    #[default]
    Move,
    Rotate,
    Scale,
}

impl GizmoMode {
    pub fn label(self) -> &'static str {
        match self {
            GizmoMode::Select => "Select",
            GizmoMode::Move => "Move",
            GizmoMode::Rotate => "Rotate",
            GizmoMode::Scale => "Scale",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Drag {
    pub handle: Handle,
    /// Where the pointer grabbed (world), and the features as they were.
    pub grab: DVec3,
    pub start: Vec<Feature>,
    /// The line a push handle slides on (point, direction).
    pub line: Option<(DVec3, DVec3)>,
    /// The operations of the shapes moved, as they were, and their shape:
    /// their points go with it (19 nov.: a vertex moved, then the cube
    /// scaled, lost its vertex).
    pub ops: Vec<(Feature, FeatureId)>,
    /// The points it carries, any of which lands on a point of interest
    /// it passes near (moves along an axis or in a plane).
    pub carried: crate::snap::Carried,
    /// Where the gizmo was when it began, and whether it was a pivot put
    /// by hand (D: it goes with the move).
    pub pivot: DVec3,
    pub custom: bool,
}

/// A fillet or chamfer sized by a drag on screen (Rectify's Radius
/// gesture): on an edge (a new step), on a fillet's faces (its size), on a
/// sketch or path corner (its cut, the dot follows the pointer).
/// A direct drag on a picked component (Romain 19 nov.: "Shift + drag =
/// extrude direct, Ctrl + drag = inset direct").
#[derive(Clone, Debug)]
pub struct FaceDrag {
    pub id: FeatureId,
    pub what: FaceDragWhat,
    /// Where the press was (screen points), mm per point there.
    pub press: [f32; 2],
    pub px: f64,
    /// The size now (shown by the pointer).
    pub size: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FaceDragWhat {
    /// An inset: its size grows with the distance from the press.
    Inset,
    /// An edge pulled along its faces' mean normal (a Move edge step): the
    /// edge's point, the direction, where the press was along it.
    Edge { at: DVec3, n: DVec3, t0: f64 },
}

#[derive(Clone, Debug)]
pub struct RoundDrag {
    pub what: RoundWhat,
    /// Where the press was (screen points) and the size then.
    pub press: [f32; 2],
    pub r0: f64,
    /// mm per screen point where the press was.
    pub px: f64,
    /// A chamfer (else a fillet).
    pub chamfer: bool,
    /// The size now (shown by the pointer).
    pub size: f64,
    /// The kind it started with (Ctrl held makes it a chamfer meanwhile).
    pub chamfer0: bool,
    /// Shift held: the tension follows the pointer from here (where Shift
    /// went down, the tension then); the tension now (0: circular).
    pub tension_from: Option<([f32; 2], f64)>,
    pub tension: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RoundWhat {
    /// The edges in `edge_sel`, made one step at the first size.
    Edges,
    /// A fillet or chamfer step.
    Step(FeatureId),
    /// The picked sketch corners; the dragged one's dot: the way its cut's
    /// middle moves and how far per mm (frame of the sketch `id`), the
    /// level of its plane (local y).
    Corners { corners: Vec<(FeatureId, usize)>, id: FeatureId, bis: [f64; 2], per: f64, y: f64 },
}

/// A remesh of the part (`nurbs_kernel::remesh`).
pub struct Remeshed {
    /// The model's serial.
    pub serial: u64,
    pub target: usize,
    pub result: Result<(nurbs_kernel::Quads, nurbs_kernel::remesh::Report), String>,
    pub ms: f32,
}

/// A material ball being dragged onto the part.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatDrag {
    pub material: u32,
    pub target: Option<DropTarget>,
}

/// What a material lands on: one face (Face mode), or a whole element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DropTarget {
    Face { tag: i64, face: usize },
    Shape(FeatureId),
}

pub struct Toast {
    pub text: String,
    pub until: f64,
    pub warn: bool,
}

/// One line of the debug journal (Romain, 20 nov. 6 h 15: "un Interactive
/// Debug mode qui te permet de lire la scène actuelle et les 5 derniers
/// mouvements"): what was done, in what state, what the build said.
#[derive(Clone, Debug)]
pub struct JournalEntry {
    /// Seconds since the app started, and the time of day (UTC).
    pub at: f64,
    pub clock: String,
    /// edit, undo, redo, auto-undo, toast, tool, mode.
    pub kind: &'static str,
    pub what: String,
    pub mode: String,
    pub tool: String,
    pub selection: Vec<FeatureId>,
    /// What was picked (faces, edges, vertices), as text.
    pub picked: String,
    pub pivot: Option<[f64; 3]>,
    /// The steps this change made or changed.
    pub steps: Vec<FeatureId>,
    /// The build's new errors after it (filled when the model arrives).
    pub errors: Vec<String>,
}

/// Isolate (Maya's Isolate Select; Romain 20 nov., 9 h 38: "un isolation
/// mode comme Maya… en icône dans la top bar"): the kernel builds only
/// these elements (with their operations, the curves of their 2D booleans,
/// the Carve shapes that cut them, and the shapes made since), the
/// document unchanged; the scan hidden meanwhile.
#[derive(Clone, Debug)]
pub struct Isolate {
    pub ids: Vec<FeatureId>,
    /// The document's next id then: newer features are shown too.
    pub from: FeatureId,
    /// Whether the scan was shown (put back after).
    pub ref_shown: bool,
}

/// The selection as the undo steps keep it: Ctrl + Z goes back through
/// the clicks too, one by one (Maya; Romain 20 nov., 9 h 37: "je click une
/// face + une face + une face… il faut remonter à chaque click un par un").
#[derive(Clone, Debug, PartialEq)]
pub struct SelState {
    pub mode: SelMode,
    pub selection: Vec<FeatureId>,
    pub faces: Vec<FaceRef>,
    pub edges: Vec<EdgeRef>,
    pub verts: Vec<DVec3>,
}

/// One undo step: the document before it (None: a selection change only)
/// and the selection then.
#[derive(Clone, Debug)]
pub struct UndoStep {
    pub label: String,
    pub doc: Option<Doc>,
    pub sel: SelState,
}

pub struct AppState {
    pub doc: Doc,
    pub doc_name: String,
    /// Bumped on every change of the document.
    pub version: u64,
    pub undo: Vec<UndoStep>,
    pub redo: Vec<UndoStep>,
    /// The document before the gesture under way (drag, creation), and the
    /// selection then.
    live_before: Option<(String, Doc)>,
    live_sel: Option<SelState>,
    /// The selection the undo steps know, and the version it was seen at
    /// (`sync_selection`).
    sel_seen: SelState,
    sel_version: u64,

    pub selection: Vec<FeatureId>,
    pub sel_mode: SelMode,
    /// The faces picked in Face mode.
    pub face_sel: Vec<FaceRef>,
    /// The face under the pointer (index in the model's faces), in Face
    /// mode and with the Inset / Extrude tools.
    pub hover_face: Option<usize>,
    /// The vertices picked in Vertex mode, and the one under the pointer.
    pub vert_sel: Vec<DVec3>,
    pub hover_vertex: Option<DVec3>,
    /// The inset or extrusion under way.
    pub push: Option<PushGesture>,
    /// The edges picked in Edge mode (not rounded yet).
    pub edge_sel: Vec<EdgeRef>,
    /// The edge under the pointer (index in the model's edges).
    pub hover_edge: Option<usize>,
    /// Fillet or chamfer for the picked edges and sharp corners (the
    /// top island's option, Ctrl + B).
    pub round_chamfer: bool,
    /// The Surface tool's points: put on the reference under the pointer
    /// (true), else locked on the plane under its first point.
    pub surface_on_scan: bool,
    /// The Slot tool reads the scan (ON SCAN) instead of drawing by hand.
    pub slot_on_scan: bool,
    /// The Slot drawn by hand.
    pub slot_draw: Option<crate::slot::SlotDraw>,
    /// A curve or 2D boolean being pulled into a solid (Curves, step 3).
    pub curve_pull: Option<CurvePull>,
    /// The debug journal, newest last (at most `JOURNAL`).
    pub journal: std::collections::VecDeque<JournalEntry>,
    /// Where the journal is written as it goes, one JSON line per entry
    /// (the app's `Documents/Forme NURBS/journal.jsonl`; a crash leaves it).
    pub journal_file: Option<std::path::PathBuf>,
    journal_seen: (String, String),
    /// A change waiting for its build: (document version, the steps it
    /// made or changed, the errors before it).
    pending_check: Option<(u64, Vec<FeatureId>, Vec<String>)>,
    /// The Surface's outline: straight sides (false) or SplitIt's curve
    /// through the points.
    pub surface_curve: bool,
    /// A fillet or chamfer being sized by a drag.
    pub round_drag: Option<RoundDrag>,
    /// Ctrl + drag on a picked face (an inset) or Shift + drag on a picked
    /// edge (the edge pulled out), Maya / Plasticity style.
    pub face_drag: Option<FaceDrag>,
    /// The Knife's cut under way (its step, its last point) and the point
    /// under the pointer.
    pub knife: Option<(FeatureId, usize)>,
    pub knife_hover: Option<DVec3>,
    /// The loop cut a click would make (the Knife; Ctrl: round the part):
    /// its points and whether it closes.
    pub knife_preview: Option<(Vec<DVec3>, bool)>,
    /// The last tool armed (G calls it again).
    pub last_tool: Option<Tool>,
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
    /// Q W E R: what the gizmo does.
    pub gizmo: GizmoMode,
    /// The rotation or scale under way, as shown by the gizmo.
    pub gizmo_readout: Option<String>,
    /// The marking menu (right button), open or flashing.
    pub marking: crate::marking::Marking,
    /// A material ball dragged from the materials island, and what it
    /// would land on.
    pub mat_drag: Option<MatDrag>,
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
    /// Each request sent lately, with its shapes' frames (the drag's
    /// preview, `drag_preview`).
    sent_frames: Vec<(u64, Vec<(FeatureId, DMat4)>)>,
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
    /// The outliner's element whose operations show in its second column.
    pub outline_focus: Option<FeatureId>,
    /// Quads view: the part as the kernel's quad mesh (fillets in orange).
    pub quads: bool,
    /// Remesh view (the « Export converter », `nurbs_kernel::remesh`, kept
    /// apart from the face-by-face Quads view): the whole part remeshed in
    /// field-aligned quads, made in the background from the fine model.
    pub remesh_on: bool,
    /// QuadRemesher's "Target Quad Count".
    pub remesh_target: usize,
    /// The remesh shown: for the model `model_gen` and the target, and what
    /// it did (or why it failed).
    pub remesh: Option<Remeshed>,
    remesh_job: Option<(u64, usize, std::sync::mpsc::Receiver<(Result<(nurbs_kernel::Quads, nurbs_kernel::remesh::Report), String>, f32)>)>,
    /// The part's points of interest (`snap.rs`), for the model `snaps_gen`.
    pub snaps: Vec<crate::snap::SnapPoint>,
    pub snaps_gen: u64,
    /// What the pointer snapped to this frame (drawn by the viewport).
    pub snap_shown: Option<crate::snap::Snapped>,
    /// The gizmo's pivot put by hand (Maya's D, Romain 20 nov. 7 h 50: "via
    /// un pivot avec D"): for the selection whose key it holds, where.
    pub custom_pivot: Option<(u64, DVec3)>,
    /// D held: a click puts the pivot on a point.
    pub pivot_edit: bool,
    /// Ctrl held: no snapping to points.
    pub snap_off: bool,
    /// Shift held (the Sketch and Path lock their sides to 0°, 45°, 90°).
    pub shift_held: bool,
    /// The points of interest the pointer passed over lately (at most 4):
    /// the guides line up with them, as in Plasticity / SketchUp.
    pub acquired: Vec<crate::snap::SnapPoint>,
    /// The reference mesh (`reference.rs`; its file is `Doc.reference`),
    /// once loaded, and its load under way.
    pub reference: Option<Arc<crate::reference::Reference>>,
    pub ref_loading: Option<crate::reference::Loading>,
    /// Bumped when the reference is loaded or dropped (the renderer
    /// uploads it).
    pub ref_gen: u64,
    /// A file that could not be loaded (not tried again until imported
    /// again).
    pub ref_failed: Option<String>,
    /// Frame the view on it once loaded (imported, or a document opened).
    pub ref_frame: bool,
    /// Shown, and see-through (Ghost).
    pub ref_shown: bool,
    pub ref_ghost: bool,
    /// X-ray (Alt + X, Maya): the part drawn see-through, the scan behind it shown.
    pub xray: bool,
    /// Isolate (Maya): the elements shown alone, see `Isolate`.
    pub isolate: Option<Isolate>,
    /// The Cylinder dragged across a round part of the reference (`scan.rs`).
    pub scan_round: Option<crate::scan::ScanRound>,
    /// The Slot tool's drag along a slot of the reference.
    pub scan_slot: Option<crate::scan::ScanSlot>,
    /// The scan under the press of a Cube drawn on it: corner to corner
    /// across a block or a pocket, read on release (`scan_box_read`).
    pub scan_box_from: Option<(u32, DVec3)>,
    /// The arcs read on the scan for the fillet sized last by Fit to scan
    /// (shown while it is selected).
    pub fillet_show: Option<crate::scan::FilletShow>,
    /// The scan's flat face found last under the pointer (`scan_plane_at`):
    /// kept while the pointer stays on its plane.
    pub scan_plane_memo: std::cell::Cell<Option<crate::scan::ScanPlaneMemo>>,
    /// When the scan's flat faces were last searched.
    pub scan_plane_search: std::cell::Cell<Option<std::time::Instant>>,
    /// The part coloured by its distance to the reference (`scan.rs`):
    /// shown, its range and tolerance (mm), the last one measured and the
    /// one being measured (in a thread).
    pub dev_on: bool,
    pub dev_range: f64,
    pub dev_tol: f64,
    pub deviation: Option<crate::scan::Deviation>,
    pub dev_job: Option<std::sync::mpsc::Receiver<crate::scan::Deviation>>,
    /// The link with Claude (`remote.rs`, the MCP server's requests).
    pub link: crate::remote::Link,
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
            live_sel: None,
            sel_seen: SelState { mode: SelMode::Object, selection: vec![], faces: vec![], edges: vec![], verts: vec![] },
            sel_version: u64::MAX,
            selection: vec![],
            sel_mode: SelMode::Object,
            face_sel: vec![],
            hover_face: None,
            vert_sel: vec![],
            hover_vertex: None,
            push: None,
            edge_sel: vec![],
            hover_edge: None,
            round_chamfer: false,
            surface_on_scan: false,
            slot_on_scan: false,
            slot_draw: None,
            curve_pull: None,
            journal: Default::default(),
            journal_file: None,
            journal_seen: (String::new(), String::new()),
            pending_check: None,
            surface_curve: false,
            round_drag: None,
            face_drag: None,
            knife: None,
            knife_hover: None,
            knife_preview: None,
            last_tool: None,
            hover_feature: None,
            hover_hit: None,
            tool: Tool::Select,
            mode: Op::Add,
            material: 0,
            creation: None,
            sketch: None,
            drag: None,
            gizmo: GizmoMode::Move,
            gizmo_readout: None,
            marking: Default::default(),
            mat_drag: None,
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
            sent_frames: vec![],
            round_ms: 0.0,
            settings: Settings::default(),
            toast: None,
            now: 0.0,
            interacting: false,
            renaming: false,
            files_open: false,
            save_name: name.to_string(),
            panel_visible: true,
            outline_focus: None,
            quads: false,
            remesh_on: false,
            remesh_target: 1500,
            remesh: None,
            remesh_job: None,
            snaps: vec![],
            snaps_gen: 0,
            snap_shown: None,
            custom_pivot: None,
            pivot_edit: false,
            snap_off: false,
            shift_held: false,
            acquired: vec![],
            reference: None,
            ref_loading: None,
            ref_gen: 0,
            ref_failed: None,
            ref_frame: true,
            ref_shown: true,
            ref_ghost: false,
            xray: false,
            isolate: None,
            scan_round: None,
            scan_slot: None,
            scan_box_from: None,
            fillet_show: None,
            scan_plane_memo: std::cell::Cell::new(None),
            scan_plane_search: std::cell::Cell::new(None),
            dev_on: false,
            dev_range: 0.5,
            dev_tol: 0.1,
            deviation: None,
            dev_job: None,
            link: Default::default(),
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
        let mut b = self.model.as_ref().and_then(|m| m.bounds()).map(|(lo, hi)| (Vec3::from(lo), Vec3::from(hi))).or_else(|| self.doc.bounds().map(|(lo, hi)| (lo.as_vec3(), hi.as_vec3())));
        if let Some(r) = self.reference.as_ref().filter(|_| self.ref_shown) {
            let (lo, hi) = (r.lo.as_vec3(), r.hi.as_vec3());
            b = Some(b.map(|(a, c)| (a.min(lo), c.max(hi))).unwrap_or((lo, hi)));
        }
        match b {
            Some((lo, hi)) => self.camera.frame(lo, hi),
            None => self.camera.frame(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 20.0, 20.0)),
        }
    }

    /// The elements of what is picked: the shapes selected (an
    /// operation: its element), else those of the picked faces and edges.
    pub fn picked_elements(&self) -> Vec<FeatureId> {
        let mut out: Vec<FeatureId> = vec![];
        let add = |id: FeatureId, out: &mut Vec<FeatureId>| {
            if !out.contains(&id) {
                out.push(id);
            }
        };
        for id in &self.selection {
            add(self.element_of(*id), &mut out);
        }
        let tags = self.face_sel.iter().map(|r| r.tag).chain(self.edge_sel.iter().flat_map(|e| e.faces));
        for t in tags {
            if let Some(ft) = FaceTag::unpack(t) {
                add(self.element_of(ft.feature), &mut out);
            }
        }
        out
    }

    /// Isolate on (what is picked shown alone, framed) or off.
    pub fn toggle_isolate(&mut self) {
        if let Some(iso) = self.isolate.take() {
            self.ref_shown = iso.ref_shown;
            self.version += 1;
            self.toast("Isolate off: everything shown", false);
            return;
        }
        let ids = self.picked_elements();
        if ids.is_empty() {
            self.toast("Isolate: select what to show alone first", true);
            return;
        }
        self.isolate = Some(Isolate { ids, from: self.doc.next_id, ref_shown: self.ref_shown });
        self.ref_shown = false;
        self.version += 1;
        let keep = self.isolated_ids();
        let b = self.doc.features.iter().filter(|f| keep.contains(&f.id) && f.is_solid() && f.op != Op::Carve).map(|f| f.bounds()).reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)));
        if let Some((lo, hi)) = b {
            self.camera.frame(lo.as_vec3(), hi.as_vec3());
        }
        self.toast("Isolate: the selection alone (its icon again: everything)", false);
    }

    /// The features the kernel builds while Isolate is on.
    pub fn isolated_ids(&self) -> Vec<FeatureId> {
        let Some(iso) = &self.isolate else { return self.doc.features.iter().map(|f| f.id).collect() };
        let mut keep: Vec<FeatureId> = self.doc.with_operands(&iso.ids);
        for el in &iso.ids {
            keep.extend(self.element_ops(*el));
        }
        // the Carve shapes that cut them, with their operations
        let solid = |f: &&Feature| keep.contains(&f.id) && f.is_solid() && !f.is_path() && f.op == Op::Add;
        if let Some((lo, hi)) = self.doc.features.iter().filter(solid).map(|f| f.bounds()).reduce(|a, b| (a.0.min(b.0), a.1.max(b.1))) {
            let cuts: Vec<FeatureId> = self
                .doc
                .features
                .iter()
                .filter(|f| f.op == Op::Carve && f.is_solid() && !f.is_path() && !keep.contains(&f.id))
                .filter(|f| {
                    let (a, b) = f.bounds();
                    a.cmple(hi).all() && b.cmpge(lo).all()
                })
                .map(|f| f.id)
                .collect();
            for c in cuts {
                keep.push(c);
                keep.extend(self.element_ops(c));
            }
        }
        keep.extend(self.doc.features.iter().filter(|f| f.id >= iso.from).map(|f| f.id));
        keep
    }

    /// The document the kernel builds: the whole one, or with Isolate on
    /// only what it shows.
    fn build_doc(&self) -> Option<Doc> {
        self.isolate.as_ref()?;
        let keep = self.isolated_ids();
        let mut d = self.doc.clone();
        d.features.retain(|f| keep.contains(&f.id));
        Some(d)
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
        // the curves follow their settings and boxes
        self.doc.regen_curves();
        self.keep_markers();
        self.version += 1;
    }

    /// One change, one step of undo.
    pub fn edit(&mut self, label: &str, f: impl FnOnce(&mut Doc)) {
        let before = self.doc.clone();
        let sel0 = self.sel_state();
        f(&mut self.doc);
        if self.doc != before {
            let fresh = self.live_before.is_none();
            if fresh {
                self.push_undo(label, before.clone(), sel0);
            }
            self.touched();
            if fresh {
                self.after_change(label, &before);
            }
        }
    }

    /// A change made of many small ones (a drag, a slider): one undo step
    /// when it ends (`end_live`).
    pub fn begin_live(&mut self, label: &str) {
        if self.live_before.is_none() {
            self.live_before = Some((label.to_string(), self.doc.clone()));
            self.live_sel = Some(self.sel_state());
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
            let changed = before != self.doc;
            let sel = self.live_sel.take().unwrap_or_else(|| self.sel_seen.clone());
            if changed {
                self.push_undo(&label, before.clone(), sel);
            }
            // fine meshing now
            self.touched();
            if changed {
                self.after_change(&label, &before);
            }
        }
    }

    /// A change kept: written in the journal, and its build checked when it
    /// arrives (`check_build`).
    fn after_change(&mut self, label: &str, before: &Doc) {
        let steps: Vec<FeatureId> = self.doc.features.iter().filter(|f| before.feature(f.id) != Some(*f)).map(|f| f.id).collect();
        self.record("edit", label);
        if let Some(e) = self.journal.back_mut() {
            e.steps = steps.clone();
        }
        let prev = self.model.as_ref().map(|m| m.errors.clone()).unwrap_or_default();
        self.pending_check = Some((self.version, steps, prev));
    }

    /// Writes a line in the debug journal.
    pub fn record(&mut self, kind: &'static str, what: &str) {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) % 86400;
        let picked = format!(
            "faces {:?}; edges {:?}; vertices {:?}",
            self.face_sel.iter().map(|r| (r.tag, [r.at.x, r.at.y, r.at.z].map(|v| (v * 1000.0).round() / 1000.0))).collect::<Vec<_>>(),
            self.edge_sel.iter().map(|e| (e.faces, [e.at.x, e.at.y, e.at.z].map(|v| (v * 1000.0).round() / 1000.0))).collect::<Vec<_>>(),
            self.vert_sel.iter().map(|v| [v.x, v.y, v.z].map(|x| (x * 1000.0).round() / 1000.0)).collect::<Vec<_>>()
        );
        self.journal.push_back(JournalEntry {
            at: self.now,
            clock: format!("{:02}:{:02}:{:02} UTC", secs / 3600, secs / 60 % 60, secs % 60),
            kind,
            what: what.to_string(),
            mode: format!("{:?}", self.sel_mode),
            tool: format!("{:?}", self.tool),
            selection: self.selection.clone(),
            picked,
            pivot: self.pivot().map(|p| p.to_array()),
            steps: vec![],
            errors: vec![],
        });
        while self.journal.len() > JOURNAL {
            self.journal.pop_front();
        }
        if let Some(e) = self.journal.back() {
            self.journal_write(e);
        }
    }

    /// An entry appended to the journal file, when there is one.
    fn journal_write(&self, e: &JournalEntry) {
        let Some(path) = &self.journal_file else { return };
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            use std::io::Write;
            let _ = writeln!(f, "{}", crate::remote::entry_json(e));
        }
    }

    /// The build of a change kept has arrived: its new errors go in the
    /// journal; a move (Move face / edge / vertex, Scale) that the kernel
    /// could not make is undone and the reason said (Romain, 20 nov.
    /// 6 h 15: "c'est que le pivot qui se déplace").
    fn check_build(&mut self) {
        let Some((v, steps, prev)) = self.pending_check.clone() else { return };
        let Some(m) = self.model.clone() else { return };
        if m.serial / 4 != v || !m.fine {
            if self.version != v {
                self.pending_check = None;
            }
            return;
        }
        self.pending_check = None;
        // a change made since: its own check comes
        if self.version != v {
            return;
        }
        let new: Vec<String> = m.errors.iter().filter(|e| !prev.contains(e)).cloned().collect();
        if let Some(e) = self.journal.iter_mut().rev().find(|e| e.kind == "edit") {
            e.errors = new.clone();
        }
        if !new.is_empty() {
            self.record("errors", &new.join(" | "));
        }
        if self.is_live() {
            return;
        }
        let failed = steps.iter().filter_map(|id| self.doc.feature(*id)).filter(|f| f.tweak.as_ref().map(|t| !t.copy).unwrap_or(false)).find_map(|f| new.iter().find(|e| e.starts_with(&format!("{}:", f.name))).cloned());
        if let Some(err) = failed {
            self.undo_doc();
            self.record("auto-undo", &err);
            self.toast(&format!("{err} · undone"), true);
        }
    }

    pub fn cancel_live(&mut self) {
        if let Some((_, before)) = self.live_before.take() {
            self.live_sel = None;
            self.doc = before;
            self.touched();
        }
    }

    /// The undo steps' names, oldest first (the document's; selection
    /// changes left out).
    pub fn undo_labels(&self) -> Vec<String> {
        self.undo.iter().filter(|u| u.doc.is_some()).map(|u| u.label.clone()).collect()
    }

    /// The selection now, as an undo step keeps it.
    pub fn sel_state(&self) -> SelState {
        SelState { mode: self.sel_mode, selection: self.selection.clone(), faces: self.face_sel.clone(), edges: self.edge_sel.clone(), verts: self.vert_sel.clone() }
    }

    fn set_sel_state(&mut self, s: &SelState) {
        self.sel_mode = s.mode;
        self.selection = s.selection.clone();
        self.face_sel = s.faces.clone();
        self.edge_sel = s.edges.clone();
        self.vert_sel = s.verts.clone();
        self.clean_selection_ids();
    }

    /// A change of the document kept as an undo step.
    pub(crate) fn push_undo(&mut self, label: &str, before: Doc, sel: SelState) {
        self.undo.push(UndoStep { label: label.to_string(), doc: Some(before), sel });
        self.redo.clear();
    }

    /// Once per frame: a selection changed by itself (a click, a
    /// rectangle, a mode) becomes an undo step of its own; one changed
    /// with the document (an edit, an undo) goes with that step. Not while
    /// a gesture is under way (it ends as one step).
    fn sync_selection(&mut self) {
        let now = self.sel_state();
        if self.version != self.sel_version {
            self.sel_version = self.version;
            self.sel_seen = now;
            return;
        }
        if now == self.sel_seen {
            return;
        }
        if self.is_live() || self.drag.is_some() || self.creation.is_some() || self.marquee.is_some() || self.push.is_some() || self.face_drag.is_some() || self.round_drag.is_some() || self.curve_pull.is_some() {
            return;
        }
        let before = std::mem::replace(&mut self.sel_seen, now);
        self.undo.push(UndoStep { label: "Select".into(), doc: None, sel: before });
        self.redo.clear();
    }

    /// Redoes up to the next change of the document.
    pub fn redo_doc(&mut self) {
        while let Some(u) = self.redo.last() {
            let doc = u.doc.is_some();
            self.redo();
            if doc {
                break;
            }
        }
    }

    /// Undoes up to the last change of the document (the selection steps
    /// after it too).
    pub fn undo_doc(&mut self) {
        while let Some(u) = self.undo.last() {
            let doc = u.doc.is_some();
            self.undo();
            if doc {
                break;
            }
        }
    }

    pub fn is_live(&self) -> bool {
        self.live_before.is_some()
    }

    pub fn undo(&mut self) {
        self.abort_gestures();
        if let Some(u) = self.undo.pop() {
            let back = self.step_back(u);
            let label = if back.doc.is_some() { back.label.clone() } else { "selection".into() };
            self.redo.push(back);
            self.toast(&format!("Undo: {label}"), false);
        }
    }

    pub fn redo(&mut self) {
        self.abort_gestures();
        if let Some(u) = self.redo.pop() {
            let back = self.step_back(u);
            let label = if back.doc.is_some() { back.label.clone() } else { "selection".into() };
            self.undo.push(back);
            self.toast(&format!("Redo: {label}"), false);
        }
    }

    /// Puts back the document and selection of a step; the step that
    /// leads to what was on screen.
    fn step_back(&mut self, u: UndoStep) -> UndoStep {
        let sel = self.sel_state();
        let doc = u.doc.map(|d| {
            let now = std::mem::replace(&mut self.doc, d);
            self.touched();
            now
        });
        self.set_sel_state(&u.sel);
        self.custom_pivot = None;
        self.sel_seen = self.sel_state();
        self.sel_version = self.version;
        UndoStep { label: u.label, doc, sel }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Drops from the selection what is no longer in the document.
    pub(crate) fn clean_selection_ids(&mut self) {
        let ids: Vec<FeatureId> = self.doc.features.iter().map(|f| f.id).collect();
        self.selection.retain(|id| ids.contains(id));
    }

    fn clean_selection(&mut self) {
        let ids: Vec<FeatureId> = self.doc.features.iter().map(|f| f.id).collect();
        self.selection.retain(|id| ids.contains(id));
        self.edge_sel.clear();
        self.face_sel.clear();
        self.vert_sel.clear();
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
        // picked faces or edges: a Delete step (faces: a hole is left;
        // edges: their two faces made one)
        if self.sel_mode == SelMode::Face && !self.face_sel.is_empty() {
            let faces = std::mem::take(&mut self.face_sel);
            let n = faces.len();
            let mut id = 0;
            self.edit("Delete faces", |d| {
                id = d.add_knife(Knife { pts: vec![], segs: vec![], remove_faces: faces, remove_edges: vec![] }, "Delete faces");
            });
            self.selection = vec![id];
            self.toast(&format!("{n} face{} deleted: the part is open there", if n > 1 { "s" } else { "" }), false);
            return;
        }
        if self.sel_mode == SelMode::Edge && !self.edge_sel.is_empty() {
            let edges = std::mem::take(&mut self.edge_sel);
            let n = edges.len();
            let mut id = 0;
            self.edit("Delete edges", |d| {
                id = d.add_knife(Knife { pts: vec![], segs: vec![], remove_faces: vec![], remove_edges: edges }, "Delete edges");
            });
            self.selection = vec![id];
            self.toast(&format!("{n} edge{} deleted", if n > 1 { "s" } else { "" }), false);
            return;
        }
        if self.selection.is_empty() {
            self.edge_sel.clear();
            self.face_sel.clear();
            self.vert_sel.clear();
            return;
        }
        let mut ids = self.selection.clone();
        let n = ids.len();
        let what = match self.single() {
            Some(f) if !f.is_solid() => f.kind_label().to_string(),
            _ => format!("{n} shape{}", if n > 1 { "s" } else { "" }),
        };
        // a shape deleted takes its operations with it (20 nov.: a cube
        // deleted left its moves failing, "its edge is gone")
        for id in self.selection.clone() {
            if self.doc.feature(id).is_some_and(|f| f.is_solid()) {
                for o in self.element_ops(id) {
                    if !ids.contains(&o) {
                        ids.push(o);
                    }
                }
            }
        }
        self.edit("Delete", |d| d.remove(&ids));
        self.selection.clear();
        self.toast(&format!("{what} deleted"), false);
    }

    pub fn duplicate_selected(&mut self) {
        // picked faces, edges or vertices (Romain 19 nov. 23 h 18: "dupliquer
        // ce morceau pour le déplacer devant, toujours mergé à l'entité"):
        // the elements they belong to, copied in place with their
        // operations, then moved with the gizmo; an Add copy stays fused
        let comps = self.sel_mode != SelMode::Object;
        // picked faces (20 nov., "je n'arrive pas à faire le duplicate
        // faces"): those faces copied as a piece, fused with the part, moved
        // with the gizmo (a Duplicate faces step, a move step that copies)
        if self.sel_mode == SelMode::Face && !self.face_sel.is_empty() {
            let Some(mut t) = nurbs_kernel::Tweak::new(self.face_sel.iter().cloned().map(nurbs_kernel::Comp::Face).collect()) else { return };
            t.copy = true;
            let mut id = 0;
            self.edit("Duplicate faces", |d| id = d.add_tweak(t));
            self.face_sel.clear();
            self.select(Some(id), false);
            self.gizmo = GizmoMode::Move;
            self.toast("Faces duplicated in place: drag the arrows to move the copy", false);
            return;
        }
        let ids: Vec<FeatureId> = if comps {
            let mut tags: Vec<i64> = self.face_sel.iter().map(|r| r.tag).collect();
            tags.extend(self.edge_sel.iter().map(|e| e.faces[0]));
            if let Some(m) = self.model.as_ref() {
                for v in &self.vert_sel {
                    let at = v.as_vec3();
                    if let Some(e) = m.edges.iter().find(|e| e.pts.first().map(|p| Vec3::from_array(*p).distance(at) < 1e-3).unwrap_or(false) || e.pts.last().map(|p| Vec3::from_array(*p).distance(at) < 1e-3).unwrap_or(false)) {
                        if let Some(t) = e.faces.iter().find(|f| **f >= 0).and_then(|f| m.faces.get(*f as usize)).and_then(|f| f.tag) {
                            tags.push(t.pack());
                        }
                    }
                }
            }
            let mut out: Vec<FeatureId> = vec![];
            for t in tags {
                let Some(ft) = FaceTag::unpack(t) else { continue };
                let el = self.element_of(ft.feature);
                if self.doc.feature(el).map(|f| f.is_solid()).unwrap_or(false) && !out.contains(&el) {
                    out.push(el);
                }
            }
            out
        } else {
            self.selection.iter().map(|id| self.element_of(*id)).collect()
        };
        if ids.is_empty() {
            return;
        }
        // in place in every mode (Romain 20 nov., 9 h 24: "CTRL + D doit
        // dupliquer sur place et pas à côté"), as in Maya: the copy is
        // selected and the move gizmo takes it where it goes
        let offset = DVec3::ZERO;
        // each element with its operations, their faces' tags moved onto the copies
        let mut plan: Vec<(FeatureId, Vec<FeatureId>)> = vec![];
        for id in ids {
            if !plan.iter().any(|p| p.0 == id) {
                plan.push((id, self.element_ops(id)));
            }
        }
        let mut new = vec![];
        self.edit("Duplicate", |d| {
            let mut map: std::collections::HashMap<FeatureId, FeatureId> = Default::default();
            for (el, ops) in &plan {
                let Some(id) = d.duplicate(&[*el], offset).first().copied() else { continue };
                map.insert(*el, id);
                new.push(id);
                for o in ops {
                    let Some(mut f) = d.feature(*o).cloned() else { continue };
                    let nid = d.next_id;
                    d.next_id += 1;
                    map.insert(*o, nid);
                    f.id = nid;
                    f.name = d.fresh_name(f.kind_label());
                    let m = |p: DVec3| p + offset;
                    let mut f = carried(&f, &m);
                    retag(&mut f, &map);
                    d.features.push(f);
                }
            }
        });
        self.selection = new;
        if comps {
            self.face_sel.clear();
            self.edge_sel.clear();
            self.vert_sel.clear();
            self.sel_mode = SelMode::Object;
        }
        self.gizmo = GizmoMode::Move;
        self.toast("Duplicated in place: drag the arrows to move the copy", false);
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
        if matches!(kind, PrimKind::Profile | PrimKind::Path | PrimKind::Surface) && self.tool == Tool::Create(kind) {
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
        if matches!(kind, PrimKind::Profile | PrimKind::Path | PrimKind::Surface) {
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
                    PrimKind::Box | PrimKind::Profile | PrimKind::Path | PrimKind::Surface => Prim::Box { x: 2.0 * r, y: h + 2.0, z: 2.0 * r },
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
        // a step on the part takes the material of the face it starts from
        let mut id = id;
        for _ in 0..16 {
            let Some(f) = self.doc.feature(id) else { return 0 };
            let tag = match (&f.round, &f.push) {
                (Some(r), _) => r.edges.first().map(|e| e.faces[0]),
                (_, Some(p)) => Some(p.face.tag),
                _ => return f.material,
            };
            match tag.and_then(nurbs_kernel::FaceTag::unpack) {
                Some(t) if t.feature != id => id = t.feature,
                _ => return 0,
            }
        }
        0
    }

    /// Gives the selected shapes a material (with nothing selected: the
    /// material of new shapes).
    pub fn set_material(&mut self, m: u32) {
        let mut ids: Vec<FeatureId> = self.selection.iter().map(|id| self.element_of(*id)).filter(|id| self.doc.feature(*id).map(|f| f.is_solid()).unwrap_or(false)).collect();
        ids.dedup();
        if ids.is_empty() {
            self.material = m;
            return;
        }
        self.shapes_material(&ids, m);
    }

    /// A face's own material, if it was given one (by its packed tag).
    pub fn paint_of(&self, tag: i64) -> Option<u32> {
        self.doc.paint.iter().find(|p| p.tag == tag).map(|p| p.material)
    }

    /// The material a model face shows: its own, else its element's.
    pub fn face_material(&self, face: usize) -> u32 {
        let Some(t) = self.model.as_ref().and_then(|m| m.faces.get(face)).and_then(|f| f.tag) else { return 0 };
        self.paint_of(t.pack()).unwrap_or_else(|| self.material_of(t.feature))
    }

    /// The element (shape) a feature's faces belong to: itself for a
    /// shape, the shape an operation works on.
    pub fn element_of(&self, id: FeatureId) -> FeatureId {
        match self.doc.feature(id) {
            Some(f) if f.is_solid() || f.is_path() => id,
            _ => self.owner_of(id).unwrap_or(id),
        }
    }

    /// What a material ball let go at `p` (screen) lands on: in Face mode
    /// the face under it, else the element the face comes from.
    pub fn drop_target(&self, p: [f32; 2]) -> Option<DropTarget> {
        let h = self.pick_at(p)?;
        let t = self.model.as_ref()?.faces.get(h.face)?.tag?;
        if self.sel_mode == SelMode::Face {
            return Some(DropTarget::Face { tag: t.pack(), face: h.face });
        }
        let id = self.element_of(t.feature);
        self.doc.feature(id).filter(|f| f.is_solid()).map(|_| DropTarget::Shape(id))
    }

    /// Whether model face `face` is (part of) a drop target.
    pub fn face_in_target(&self, face: usize, t: &DropTarget) -> bool {
        let Some(tag) = self.model.as_ref().and_then(|m| m.faces.get(face)).and_then(|f| f.tag) else { return false };
        match t {
            DropTarget::Face { tag: x, .. } => tag.pack() == *x,
            DropTarget::Shape(id) => self.element_of(tag.feature) == *id && self.paint_of(tag.pack()).is_none(),
        }
    }

    /// A material ball clicked: the faces picked in Face mode, else the
    /// selected shapes (an operation's: its shape's), else new shapes.
    pub fn apply_material(&mut self, m: u32) {
        let name = crate::materials::get(m).name;
        if self.sel_mode == SelMode::Face && !self.face_sel.is_empty() {
            let mut tags: Vec<i64> = self.face_sel.iter().map(|r| r.tag).collect();
            tags.dedup();
            let n = tags.len();
            self.paint_faces(&tags, m);
            self.toast(&format!("{name} on {n} face{}", if n == 1 { "" } else { "s" }), false);
            return;
        }
        let mut ids: Vec<FeatureId> = self.selection.iter().map(|id| self.element_of(*id)).filter(|id| self.doc.feature(*id).map(|f| f.is_solid()).unwrap_or(false)).collect();
        ids.dedup();
        if ids.is_empty() {
            self.material = m;
            self.toast(&format!("New shapes: {name}"), false);
            return;
        }
        self.shapes_material(&ids, m);
        let what = if ids.len() == 1 { self.doc.feature(ids[0]).map(|f| f.name.clone()).unwrap_or_default() } else { format!("{} shapes", ids.len()) };
        self.toast(&format!("{name} on {what}"), false);
    }

    /// A material ball let go on the part.
    pub fn drop_material(&mut self, t: DropTarget, m: u32) {
        let name = crate::materials::get(m).name;
        match t {
            DropTarget::Face { tag, .. } => {
                self.paint_faces(&[tag], m);
                self.toast(&format!("{name} on a face"), false);
            }
            DropTarget::Shape(id) => {
                self.shapes_material(&[id], m);
                let n = self.doc.feature(id).map(|f| f.name.clone()).unwrap_or_default();
                self.toast(&format!("{name} on {n}"), false);
            }
        }
    }

    /// Gives faces their own material (one undo step).
    fn paint_faces(&mut self, tags: &[i64], m: u32) {
        let tags = tags.to_vec();
        self.edit("Material", |d| {
            for t in tags {
                d.paint.retain(|p| p.tag != t);
                d.paint.push(nurbs_kernel::Paint { tag: t, material: m });
            }
        });
    }

    /// Gives whole shapes a material: their faces' own materials go (as
    /// in Maya, a material given to the object replaces its faces').
    fn shapes_material(&mut self, ids: &[FeatureId], m: u32) {
        let owned: Vec<i64> = self.doc.paint.iter().map(|p| p.tag).filter(|t| FaceTag::unpack(*t).map(|x| ids.contains(&self.element_of(x.feature))).unwrap_or(false)).collect();
        let ids = ids.to_vec();
        self.edit("Material", |d| {
            for id in &ids {
                if let Some(f) = d.feature_mut(*id) {
                    f.material = m;
                }
            }
            d.paint.retain(|p| !owned.contains(&p.tag));
        });
    }

    pub fn select_tool(&mut self) {
        self.abort_gestures();
        self.tool = Tool::Select;
    }

    /// Maya's Q W E R: back to Select with that gizmo. Rotate and Scale
    /// work on whole shapes, so they go back to Object mode.
    pub fn set_gizmo(&mut self, g: GizmoMode) {
        if self.tool != Tool::Select || self.sketch.is_some() || self.creation.is_some() || self.drag.is_some() {
            self.select_tool();
        }
        // rotate works on whole shapes; scale on components too
        if g == GizmoMode::Rotate && self.sel_mode != SelMode::Object {
            self.set_sel_mode(SelMode::Object);
        }
        self.gizmo = g;
    }

    /// What the Select tool picks: vertices, edges, faces or shapes.
    pub fn set_sel_mode(&mut self, m: SelMode) {
        self.abort_gestures();
        self.tool = Tool::Select;
        if m != self.sel_mode {
            self.sel_mode = m;
            self.edge_sel.clear();
            self.face_sel.clear();
            self.vert_sel.clear();
            self.hover_edge = None;
            self.hover_face = None;
            self.hover_vertex = None;
            // the steps of the mode stay selected: a fillet in Edge mode,
            // an inset or extrusion in Face mode
            let keep = |f: &Feature| match m {
                SelMode::Object => true,
                SelMode::Edge => f.round.is_some(),
                SelMode::Face => f.push.is_some(),
                SelMode::Vertex => false,
            };
            let ids: Vec<FeatureId> = self.selection.iter().copied().filter(|id| self.doc.feature(*id).map(keep).unwrap_or(false)).collect();
            self.selection = ids;
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
    /// those joining the same two faces). Measured to the edge's segments:
    /// a straight edge has only its two ends, and the middle of one cut in
    /// two pieces (a Loop cut across a face) is as far from both pieces'
    /// ends, so the other piece was drawn picked (Romain 20 nov.: "le 3e
    /// n'arrive pas à sélectionner").
    pub fn model_edge_of(&self, r: &EdgeRef) -> Option<usize> {
        let m = self.model.as_ref()?;
        let want = r.key();
        m.edges
            .iter()
            .enumerate()
            .filter(|(i, _)| self.edge_ref(*i).map(|x| x.key() == want).unwrap_or(false))
            .map(|(i, e)| (i, polyline_distance(&e.pts, r.at)))
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
            if let Some(id) = self.single().filter(|f| f.round.is_some()).map(|f| f.id) {
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
        self.single().filter(|f| f.round.is_some())
    }

    /// Sets the size (and kind) of the selected fillet, or makes one of the
    /// picked edges. `start`: a new gesture (one undo step until `end_live`).
    pub fn set_round(&mut self, size: f64, chamfer: bool, start: bool) {
        let label = if chamfer { "Chamfer" } else { "Fillet" };
        if let Some(id) = self.selected_round().map(|f| f.id) {
            if start {
                self.begin_live(label);
            }
            // it stops at the largest size OpenCascade could make
            let size = match self.round_limit(id) {
                Some(l) => size.min(l),
                None => size,
            };
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

    /// Blend corners on or off for the selected fillet.
    pub fn set_blend(&mut self, on: bool) {
        let Some(id) = self.selected_round().map(|f| f.id) else { return };
        self.edit(if on { "Blend corners" } else { "Open corners" }, |d| {
            if let Some(r) = d.feature_mut(id).and_then(|f| f.round.as_mut()) {
                r.blend = on;
            }
        });
    }

    /// The largest size the kernel could give this fillet / chamfer, when
    /// the size asked was too big for it (it is then built at that size).
    pub fn round_limit(&self, id: u32) -> Option<f64> {
        self.model.as_ref()?.limits.iter().find(|(i, _)| *i == id).map(|(_, l)| *l)
    }

    /// The largest size the fillet slider offers: half the part's thinnest
    /// extent (OpenCascade says when it cannot make it).
    pub fn round_max(&self) -> f64 {
        let ext = self.model.as_ref().and_then(|m| m.bounds()).map(|(lo, hi)| (0..3).map(|k| (hi[k] - lo[k]) as f64).fold(f64::MAX, f64::min)).unwrap_or(20.0);
        crate::widgets::nice_ceil((ext * 0.5).clamp(1.0, 100.0) as f32) as f64
    }

    /// Cancels what is under way (creation, sketch, drag, inset).
    pub fn abort_gestures(&mut self) {
        if self.creation.take().is_some() || self.drag.take().is_some() || self.push.take().is_some() || self.round_drag.take().is_some() || self.face_drag.take().is_some() || self.curve_pull.take().is_some() {
            self.cancel_live();
            self.clean_selection_ids();
        }
        self.gizmo_readout = None;
        self.scan_round = None;
        self.scan_slot = None;
        self.slot_draw = None;
        self.scan_box_from = None;
        self.sketch = None;
        self.knife = None;
        self.knife_hover = None;
        self.knife_preview = None;
        self.marquee = None;
        self.acquired.clear();
        self.snap_shown = None;
    }

    /// The base plane under the pointer: the flat face hit, else the scan's
    /// plane there (when the part is not in front of it), else the ground.
    /// `hover`: the pointer only passes (`scan_plane_at`).
    pub fn base_plane_at(&self, p: [f32; 2], hover: bool) -> Option<(BasePlane, DVec3)> {
        if let Some(r) = self.scan_plane_at(p, hover) {
            return Some(r);
        }
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

    /// The element (a shape or a path) an operation works on: the shape
    /// that the faces or edges it names come from (through other
    /// operations' faces). None for a shape, a path, or an operation whose
    /// faces cannot be told.
    pub fn owner_of(&self, id: FeatureId) -> Option<FeatureId> {
        let mut id = id;
        for _ in 0..64 {
            let f = self.doc.feature(id)?;
            let tag = if let Some(r) = &f.round {
                r.edges.first().map(|e| e.faces[0])
            } else if let Some(p) = &f.push {
                Some(p.face.tag)
            } else if let Some(t) = &f.tweak {
                match &t.comp {
                    nurbs_kernel::Comp::Face(r) => Some(r.tag),
                    nurbs_kernel::Comp::Edge(e) => Some(e.faces[0]),
                    nurbs_kernel::Comp::Vertex(v) => {
                        // the solid made before it whose box holds the point
                        let pos = self.doc.features.iter().position(|g| g.id == id)?;
                        return self.doc.features[..pos].iter().filter(|g| g.is_solid() && !g.is_path()).find(|g| {
                            let (lo, hi) = g.bounds();
                            v.cmpge(lo - 1e-3).all() && v.cmple(hi + 1e-3).all()
                        }).map(|g| g.id);
                    }
                }
            } else if let Some(k) = &f.cut {
                k.pts.first().map(|p| p.edge.faces[0]).or(k.remove_faces.first().map(|r| r.tag)).or(k.remove_edges.first().map(|e| e.faces[0]))
            } else {
                return None;
            };
            let owner = FaceTag::unpack(tag?)?.feature;
            let g = self.doc.feature(owner)?;
            if g.is_solid() || g.is_path() {
                return Some(owner);
            }
            if owner == id {
                return None;
            }
            id = owner;
        }
        None
    }

    /// The element the outliner's second column shows: the selected shape
    /// or path, the owner of the selected operation, else the last clicked.
    pub fn outline_element(&self) -> Option<FeatureId> {
        if let Some(f) = self.single() {
            if f.is_solid() || f.is_path() {
                return Some(f.id);
            }
            if let Some(o) = self.owner_of(f.id) {
                return Some(o);
            }
        }
        self.outline_focus.filter(|id| self.doc.feature(*id).is_some())
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
        if matches!(kind, PrimKind::Profile | PrimKind::Path | PrimKind::Surface) {
            return;
        }
        let Some((plane, a, snapped)) = self.start_plane(p) else { return };
        self.snap_shown = snapped;
        // a Cube drawn on the scan reads it when let go
        self.scan_box_from = if kind == PrimKind::Box && plane.scan.is_some() { self.scan_under(p) } else { None };
        self.begin_live(&format!("New {}", kind.label()));
        let op = self.mode;
        let origin = plane.at(a);
        let rot = basis_rotation(plane.u, plane.n);
        // a tiny shape at first: it grows with the drag
        let prim = match kind {
            PrimKind::Box | PrimKind::Profile | PrimKind::Path | PrimKind::Surface => Prim::Box { x: 0.01, y: 0.01, z: 0.01 },
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
        self.creation = Some(Creation { kind, id, plane, a, b: a, height: 0.0, phase: Phase::Base, height_from: p, profile: None, curve: None });
        self.selection = vec![id];
    }

    /// The creation follows the pointer.
    pub fn update_creation(&mut self, p: [f32; 2], shift: bool) {
        let Some(mut c) = self.creation.clone() else { return };
        let (ro, rd) = self.ray(p);
        match c.phase {
            Phase::Base => {
                if let Some((b, snapped)) = self.snap_on_plane(&c.plane, p, &[c.id]) {
                    c.b = b;
                    self.snap_shown = snapped;
                    if shift && (c.kind == PrimKind::Box || matches!(c.curve, Some(CurveKind::Rectangle | CurveKind::Ellipse))) {
                        // a square
                        let d = (c.b[0] - c.a[0]).abs().max((c.b[1] - c.a[1]).abs());
                        c.b = [c.a[0] + d * (c.b[0] - c.a[0]).signum(), c.a[1] + d * (c.b[1] - c.a[1]).signum()];
                    }
                }
            }
            Phase::Height => {
                // the height along the normal through the base centre, closest to the ray
                let centre = self.creation_base_centre(&c);
                if let Some((t, snapped)) = self.snap_height(centre, c.plane.n, p, &[c.id]).filter(|(t, _)| t.abs() > 1e-3) {
                    c.height = t;
                    self.snap_shown = Some(snapped);
                } else if c.plane.n.dot(rd).abs() > HEAD_ON {
                    // the face seen nearly head-on (often on the scan): the
                    // line along its normal runs almost along the ray, so
                    // the height follows the pointer's travel up the screen
                    let t = (c.height_from[1] - p[1]) as f64 * self.mm_per_point(centre);
                    c.height = self.snap(t);
                    self.snap_shown = None;
                } else {
                    // from where the pointer was when the height phase
                    // began: the shape starts flat there, not at the height
                    // the corner's ray passes the line at
                    let (fo, fd) = self.ray(c.height_from);
                    let t0 = closest_on_line(centre, c.plane.n, fo, fd).unwrap_or(0.0);
                    let t = closest_on_line(centre, c.plane.n, ro, rd).map(|t| t - t0).unwrap_or(0.0);
                    c.height = self.snap(t);
                    self.snap_shown = None;
                }
            }
        }
        self.creation = Some(c.clone());
        self.apply_creation(&c);
    }

    fn creation_base_centre(&self, c: &Creation) -> DVec3 {
        if c.curve.map(|k| k.from_centre()).unwrap_or(false) {
            return c.plane.at(c.a);
        }
        match c.kind {
            PrimKind::Box | PrimKind::Profile | PrimKind::Path | PrimKind::Surface => c.plane.at([(c.a[0] + c.b[0]) * 0.5, (c.a[1] + c.b[1]) * 0.5]),
            PrimKind::Cylinder => c.plane.at(c.a),
        }
    }

    /// Writes the creation into its feature.
    fn apply_creation(&mut self, c: &Creation) {
        if let Some(kind) = c.curve {
            self.apply_curve(c, kind);
            return;
        }
        let centre = self.creation_base_centre(c);
        use crate::snap::tidy;
        let (w, dpt) = (tidy((c.b[0] - c.a[0]).abs()), tidy((c.b[1] - c.a[1]).abs()));
        let r = tidy(((c.b[0] - c.a[0]).powi(2) + (c.b[1] - c.a[1]).powi(2)).sqrt());
        let base_ok = match c.kind {
            PrimKind::Box | PrimKind::Profile | PrimKind::Path | PrimKind::Surface => w > 1e-6 && dpt > 1e-6,
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
                    PrimKind::Path | PrimKind::Surface => Prim::Path { x: w.max(0.01), z: dpt.max(0.01) },
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
            PrimKind::Box | PrimKind::Profile | PrimKind::Path | PrimKind::Surface => (c.b[0] - c.a[0]).abs() < 0.5 || (c.b[1] - c.a[1]).abs() < 0.5,
            PrimKind::Cylinder => ((c.b[0] - c.a[0]).powi(2) + (c.b[1] - c.a[1]).powi(2)).sqrt() < 0.5,
        };
        if tiny {
            // direct: a default shape, centred on the click
            let s = 10.0;
            match c.kind {
                PrimKind::Box | PrimKind::Profile | PrimKind::Path | PrimKind::Surface => {
                    c.b = [c.a[0] + s, c.a[1] + s];
                    c.a = [c.a[0] - s, c.a[1] - s];
                }
                PrimKind::Cylinder => c.b = [c.a[0] + s, c.a[1]],
            }
            c.phase = Phase::Height;
            c.height = if self.mode == Op::Carve && c.plane.on_face() { -20.0 } else { 20.0 };
            self.creation = Some(c.clone());
            self.apply_creation(&c);
            self.finish_creation();
            return;
        }
        // drawn on the scan corner to corner: the block or pocket there,
        // made at once when one is read (else on to the height as drawn)
        if let Some(from) = self.scan_box_from.take() {
            if c.kind == PrimKind::Box && self.scan_box_read(&c, from, p) {
                return;
            }
        }
        c.phase = Phase::Height;
        c.height_from = p;
        c.height = 0.0;
        self.creation = Some(c.clone());
        self.apply_creation(&c);
    }

    /// Click in the height phase: done.
    pub fn finish_creation(&mut self) {
        self.snap_shown = None;
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
        // one shape per tool, then Select (Romain, 19 nov. 23 h 18); G
        // calls the tool again
        self.tool = Tool::Select;
    }

    /// G: the last tool used again (Maya).
    pub fn repeat_tool(&mut self, at: Option<[f32; 2]>) {
        match self.last_tool {
            Some(Tool::Curve(k)) => self.arm_curve(k),
            Some(Tool::Create(k)) => {
                self.abort_gestures();
                self.tool = Tool::Create(k);
            }
            Some(Tool::Push(k)) => self.arm_push(k, at),
            Some(t @ (Tool::Knife | Tool::Point)) => self.arm_knife(t),
            Some(Tool::Slot) => self.arm_slot(),
            _ => self.toast("No tool used yet", false),
        }
    }

    // -------------------------------------------------------------- curves

    /// Arms a tool of the Curves set.
    pub fn arm_curve(&mut self, kind: CurveKind) {
        self.abort_gestures();
        self.tool = Tool::Curve(kind);
    }

    /// Starts a curve at `p` (screen): its first corner, or its centre.
    pub fn begin_curve(&mut self, kind: CurveKind, p: [f32; 2]) {
        let Some((plane, a, snapped)) = self.start_plane(p) else { return };
        self.snap_shown = snapped;
        self.begin_live(&format!("New {}", kind.label()));
        let origin = plane.at(a);
        let rot = basis_rotation(plane.u, plane.n);
        let mut id = 0;
        self.live(|d| {
            let name = d.fresh_name(kind.label());
            id = d.add(Op::Add, Prim::Path { x: 0.01, z: 0.01 }, origin, rot);
            if let Some(f) = d.feature_mut(id) {
                f.name = name;
                f.curve = Some(kind.new_curve());
                // hidden until it has a size
                f.visible = false;
            }
        });
        self.creation = Some(Creation { kind: PrimKind::Path, id, plane, a, b: a, height: 0.0, phase: Phase::Base, height_from: p, profile: None, curve: Some(kind) });
        self.selection = vec![id];
    }

    /// The curve being drawn written into its feature: corner to corner
    /// (a Rectangle, an Ellipse), else from its centre to the pointer (a
    /// Circle's radius, a Polygon's or Star's outer radius).
    fn apply_curve(&mut self, c: &Creation, kind: CurveKind) {
        use crate::snap::tidy;
        let id = c.id;
        let Some(curve) = self.doc.feature(id).and_then(|f| f.curve) else { return };
        let (w, d, centre) = if kind.from_centre() {
            let r = tidy(((c.b[0] - c.a[0]).powi(2) + (c.b[1] - c.a[1]).powi(2)).sqrt());
            let (b, off) = curve.unit_box();
            // the box's centre: off the curve's own for an odd polygon
            (r * b[0], r * b[1], c.plane.at([c.a[0] - off[0] * r, c.a[1] - off[1] * r]))
        } else {
            (tidy((c.b[0] - c.a[0]).abs()), tidy((c.b[1] - c.a[1]).abs()), c.plane.at([(c.a[0] + c.b[0]) * 0.5, (c.a[1] + c.b[1]) * 0.5]))
        };
        let ok = w > 1e-6 && d > 1e-6;
        let rot = basis_rotation(c.plane.u, c.plane.n);
        self.live(|doc| {
            if let Some(f) = doc.feature_mut(id) {
                f.origin = centre;
                f.rot = rot;
                f.prim = Prim::Path { x: w.max(0.01), z: d.max(0.01) };
                f.visible = ok;
            }
        });
    }

    /// The curve's drag let go: made (a click without a drag: 20 mm across,
    /// centred there).
    pub fn curve_released(&mut self) {
        let Some(mut c) = self.creation.clone() else { return };
        let Some(kind) = c.curve else { return };
        let tiny = if kind.from_centre() { ((c.b[0] - c.a[0]).powi(2) + (c.b[1] - c.a[1]).powi(2)).sqrt() < 0.5 } else { (c.b[0] - c.a[0]).abs() < 0.5 || (c.b[1] - c.a[1]).abs() < 0.5 };
        if tiny {
            let s = 10.0;
            if kind.from_centre() {
                c.b = [c.a[0] + s, c.a[1]];
            } else {
                c.b = [c.a[0] + s, c.a[1] + s];
                c.a = [c.a[0] - s, c.a[1] - s];
            }
            self.apply_curve(&c, kind);
        }
        self.snap_shown = None;
        self.creation = None;
        let id = c.id;
        self.live(|d| {
            if let Some(f) = d.feature_mut(id) {
                f.visible = true;
            }
        });
        self.end_live();
        self.selection = vec![id];
        self.sel_mode = SelMode::Object;
        // one shape per tool, then Select; G calls it again
        self.tool = Tool::Select;
    }

    /// The selected closed curves (Object mode) made one 2D boolean, kept
    /// live (3ds Max's Shape Boolean): the first picked is the base,
    /// Subtract takes the others from it. They must lie on one plane.
    pub fn make_curve_bool(&mut self, op: CurveOp) {
        let ids: Vec<FeatureId> = self.selection.iter().copied().filter(|id| self.doc.is_closed_curve(*id)).collect();
        if ids.len() < 2 {
            self.toast("Pick two closed curves or more", true);
            return;
        }
        let Some(first) = self.doc.feature(ids[0]).cloned() else { return };
        let n0 = first.axis();
        if let Some(g) = ids[1..].iter().filter_map(|id| self.doc.feature(*id)).find(|g| g.axis().cross(n0).length() > 1e-6 || (g.origin - first.origin).dot(n0).abs() > 1e-4) {
            let msg = format!("{} is not on the plane of {}", g.name, first.name);
            self.toast(&msg, true);
            return;
        }
        let mut id = 0;
        self.edit(op.label(), |d| {
            let name = d.fresh_name(op.label());
            id = d.add(Op::Add, first.prim, first.origin, first.rot);
            if let Some(f) = d.feature_mut(id) {
                f.name = name;
                f.shape_bool = Some(ShapeBool { op, operands: ids.clone() });
            }
        });
        self.selection = vec![id];
        self.sel_mode = SelMode::Object;
    }

    /// A 2D boolean's operation changed.
    /// A 2D boolean's operation changed (its name too while it is the
    /// operation's own, "Subtract 2" → "Union 2").
    pub fn set_curve_bool_op(&mut self, id: FeatureId, op: CurveOp) {
        self.edit(op.label(), |d| {
            let Some(old) = d.feature(id).and_then(|f| f.shape_bool.as_ref()).map(|b| b.op) else { return };
            let named = d.feature(id).map(|f| f.name == old.label() || f.name.strip_prefix(old.label()).map(|r| r.starts_with(' ') && r[1..].parse::<u32>().is_ok()).unwrap_or(false)).unwrap_or(false);
            let name = if named && old != op { Some(d.fresh_name(op.label())) } else { None };
            if let Some(f) = d.feature_mut(id) {
                if let Some(b) = f.shape_bool.as_mut() {
                    b.op = op;
                }
                if let Some(n) = name {
                    f.name = n;
                }
            }
        });
    }

    /// One curve taken out of a 2D boolean (it stays, on its own); the
    /// last one takes the boolean away.
    pub fn take_out_operand(&mut self, id: FeatureId, operand: FeatureId) {
        let Some(n) = self.doc.feature(id).and_then(|f| f.shape_bool.as_ref()).map(|b| b.operands.len()) else { return };
        self.edit("Take out", |d| {
            if n <= 1 {
                d.features.retain(|f| f.id != id);
            } else if let Some(b) = d.feature_mut(id).and_then(|f| f.shape_bool.as_mut()) {
                b.operands.retain(|o| *o != operand);
            }
        });
        self.selection = vec![operand];
    }

    /// A curve of a 2D boolean made its base (first).
    pub fn make_base_operand(&mut self, id: FeatureId, operand: FeatureId) {
        self.edit("Base curve", |d| {
            if let Some(b) = d.feature_mut(id).and_then(|f| f.shape_bool.as_mut()) {
                if let Some(k) = b.operands.iter().position(|o| *o == operand) {
                    let o = b.operands.remove(k);
                    b.operands.insert(0, o);
                }
            }
        });
    }

    /// The curve (or 2D boolean) that Ctrl + E or Extrude pulls: the one
    /// selected, closed, flat or already pulled.
    pub fn pullable(&self) -> Option<FeatureId> {
        let f = self.single().filter(|_| self.sel_mode == SelMode::Object)?;
        let id = self.top_boolean(f.id);
        let f = self.doc.feature(id)?;
        (self.doc.is_closed_curve(f.id) || f.is_extruded_curve()).then_some(f.id)
    }

    /// The 2D boolean a curve is in, up to the outermost (itself when in
    /// none): an operand picked, its boolean is what Extrude pulls.
    pub fn top_boolean(&self, mut id: FeatureId) -> FeatureId {
        for _ in 0..64 {
            match self.curve_inside(id) {
                Some(b) => id = b.id,
                None => break,
            }
        }
        id
    }

    /// Starts pulling the curve into a solid from the pointer at `p`.
    pub fn begin_curve_pull(&mut self, id: FeatureId, p: [f32; 2]) {
        let Some(f) = self.doc.feature(id).cloned() else { return };
        if !(self.doc.is_closed_curve(id) || f.is_extruded_curve()) {
            self.toast("Only a closed curve can be extruded", true);
            return;
        }
        self.abort_gestures();
        self.begin_live("Extrude");
        let h0 = if f.is_extruded_curve() { f.prim.height() * if self.pull_down(&f) { -1.0 } else { 1.0 } } else { 0.0 };
        self.curve_pull = Some(CurvePull { id, start: f, from: p, h0, h: h0 });
        self.selection = vec![id];
    }

    /// Whether an extruded curve goes down from its curve: its frame is
    /// turned over from its curve's (a 2D boolean: from its first curve's;
    /// a curve of its own: from the plane it was drawn on is not known, so
    /// it is the frame's own way, up).
    fn pull_down(&self, f: &Feature) -> bool {
        let first = f.shape_bool.as_ref().and_then(|b| b.operands.first()).and_then(|o| self.doc.feature(*o));
        first.map(|g| g.axis().dot(f.axis()) < 0.0).unwrap_or(false)
    }

    /// The height follows the pointer, along the curve's normal.
    pub fn update_curve_pull(&mut self, p: [f32; 2]) {
        let Some(mut cp) = self.curve_pull.clone() else { return };
        let st = &cp.start;
        // the curve's plane and way up (an extruded one: from its base)
        let down0 = cp.h0 < 0.0;
        let up = if down0 { -st.axis() } else { st.axis() };
        let base = st.origin;
        let (ro, rd) = self.ray(p);
        let mut skip = vec![cp.id];
        skip.extend(self.doc.with_operands(&[cp.id]));
        let h = if let Some((t, snapped)) = self.snap_height(base, up, p, &skip).filter(|(t, _)| t.abs() > 1e-3) {
            self.snap_shown = Some(snapped);
            t
        } else if up.dot(rd).abs() > HEAD_ON {
            self.snap_shown = None;
            self.snap(cp.h0 + (cp.from[1] - p[1]) as f64 * self.mm_per_point(base))
        } else {
            self.snap_shown = None;
            let (fo, fd) = self.ray(cp.from);
            let t0 = closest_on_line(base, up, fo, fd).unwrap_or(0.0);
            self.snap(cp.h0 + closest_on_line(base, up, ro, rd).map(|t| t - t0).unwrap_or(0.0))
        };
        cp.h = h;
        let mode = self.mode;
        let start = cp.start.clone();
        let flip_of = |f: &Feature, down: bool| {
            // the frame the curve was drawn in, then turned over for down
            let drawn = if down0 { f.rot * DQuat::from_rotation_z(std::f64::consts::PI) } else { f.rot };
            if down { drawn * DQuat::from_rotation_z(std::f64::consts::PI) } else { drawn }
        };
        self.live(|d| {
            let Some(f) = d.feature_mut(cp.id) else { return };
            *f = start.clone();
            let down = h < 0.0;
            // a curve with no settings of its own: its outline mirrored so
            // it stays in place when the frame turns over
            if down != down0 && f.curve.is_none() && f.shape_bool.is_none() {
                f.profile = f.profile.as_ref().map(|p| p.mirrored());
            }
            f.rot = flip_of(&start, down);
            let (x, z) = match f.prim {
                Prim::Path { x, z } | Prim::Profile { x, z, .. } => (x, z),
                _ => return,
            };
            if h.abs() < 1e-3 {
                f.prim = Prim::Path { x, z };
            } else {
                f.prim = Prim::Profile { x, y: h.abs(), z };
                if !start.is_extruded_curve() {
                    f.op = mode;
                }
            }
        });
        self.curve_pull = Some(cp);
    }

    /// The pull kept (none: dropped).
    pub fn finish_curve_pull(&mut self) {
        self.snap_shown = None;
        let Some(cp) = self.curve_pull.take() else { return };
        if (cp.h - cp.h0).abs() < 1e-6 {
            self.cancel_live();
            return;
        }
        self.end_live();
        self.selection = vec![cp.id];
        self.tool = Tool::Select;
    }

    /// An extruded curve made flat again (a curve, its settings kept).
    pub fn flatten_curve(&mut self, id: FeatureId) {
        self.edit("Flat", |d| {
            if let Some(f) = d.feature_mut(id) {
                if let Prim::Profile { x, z, .. } = f.prim {
                    f.prim = Prim::Path { x, z };
                }
            }
        });
    }

    /// The kernel's error about a feature ("name: why"), if any.
    pub fn feature_error(&self, f: &Feature) -> Option<String> {
        let head = format!("{}: ", f.name);
        self.model.as_ref()?.errors.iter().find(|e| e.starts_with(&head)).map(|e| e[head.len()..].to_string())
    }

    pub fn cancel_creation(&mut self) {
        self.snap_shown = None;
        if self.creation.take().is_some() {
            self.cancel_live();
            self.clean_selection();
        }
    }

    // -------------------------------------------------------------- sketch

    /// Where the pointer is on the sketch's plane (snapped).
    pub fn sketch_point(&self, p: [f32; 2]) -> Option<[f64; 2]> {
        self.sketch_snap(p).map(|s| s.0)
    }

    /// The same, with what it snapped to.
    pub fn sketch_snap(&self, p: [f32; 2]) -> Option<([f64; 2], Option<crate::snap::Snapped>)> {
        let k = self.sketch.as_ref()?;
        let (q, snapped) = self.snap_on_plane(&k.plane, p, &[])?;
        // Shift: the side from the last point locked level, upright or at
        // 45° on the plane, its length on the grid step
        let n = k.pts.len() - k.held.map(|_| 1).unwrap_or(0);
        if self.shift_held && n >= 1 {
            let last = k.pts[n - 1];
            let w = [q[0] - last[0], q[1] - last[1]];
            if w[0].hypot(w[1]) > 1e-9 {
                let step = std::f64::consts::FRAC_PI_4;
                let a = (w[1].atan2(w[0]) / step).round() * step;
                let u = [a.cos(), a.sin()];
                let len = self.snap((w[0] * u[0] + w[1] * u[1]).max(0.0));
                return Some(([last[0] + u[0] * len, last[1] + u[1] * len], None));
            }
        }
        Some((q, snapped))
    }

    /// The point placed under the pointer (within 10 points): Ctrl + drag
    /// from it sets its fillet.
    pub fn sketch_point_at(&self, p: [f32; 2]) -> Option<usize> {
        let k = self.sketch.as_ref()?;
        let v = self.view();
        let mut best: Option<(f32, usize)> = None;
        for (i, q) in k.pts.iter().enumerate() {
            let _ = q;
            let Some(s) = v.project(k.world(i).as_vec3()) else { continue };
            let d = (s[0] - p[0]).hypot(s[1] - p[1]);
            if d <= 10.0 && !k.smooth[i] && best.map(|b| d < b.0).unwrap_or(true) {
                best = Some((d, i));
            }
        }
        best.map(|b| b.1)
    }

    /// Ctrl + press on a point placed: it is grabbed (its fillet follows
    /// the drag). False when no point is there.
    pub fn sketch_grab(&mut self, p: [f32; 2]) -> bool {
        let i = self.sketch_point_at(p);
        if let (Some(k), Some(i)) = (self.sketch.as_mut(), i) {
            k.grab = Some(i);
            k.held = None;
            return true;
        }
        false
    }

    /// The grabbed point's fillet: the pointer's distance from it on the
    /// plane, on the grid step.
    pub fn sketch_fillet_drag(&mut self, p: [f32; 2]) {
        let (ro, rd) = self.ray(p);
        let step = self.settings.step;
        let snap = self.settings.snap;
        let Some(k) = self.sketch.as_mut() else { return };
        let Some(i) = k.grab else { return };
        // on the plane through the point (a point on the scan has its own)
        let c = k.world(i);
        let den = rd.dot(k.plane.n);
        if den.abs() < 1e-9 {
            return;
        }
        let d = (ro + rd * ((c - ro).dot(k.plane.n) / den)).distance(c);
        let r = if snap && step > 0.0 { (d / step).round() * step } else { d };
        if k.corner.len() < k.pts.len() {
            k.corner.resize(k.pts.len(), 0.0);
        }
        k.corner[i] = r;
    }

    pub fn sketch_grab_end(&mut self) {
        if let Some(k) = self.sketch.as_mut() {
            k.grab = None;
        }
    }

    /// Whether the pointer is on the sketch's first point (it closes it).
    pub fn sketch_on_first(&self, p: [f32; 2]) -> bool {
        let Some(k) = &self.sketch else { return false };
        if k.pts.len() < 3 {
            return false;
        }
        let Some(s) = self.view().project(k.world(0).as_vec3()) else { return false };
        (s[0] - p[0]).hypot(s[1] - p[1]) < 10.0
    }

    /// A press with the Sketch tool: the first point (it picks the face
    /// under it, else the ground), the next ones, or the first point again
    /// to close the outline.
    pub fn sketch_press(&mut self, p: [f32; 2]) {
        if self.sketch.is_none() {
            let Some((plane, a, _)) = self.start_plane(p) else { return };
            let free = self.surface_free(p);
            self.sketch = Some(Sketch { plane, pts: vec![a], smooth: vec![false], held: Some((0, p)), corner: vec![0.0], grab: None, free: vec![free], sharp: vec![false] });
            self.selection.clear();
            return;
        }
        if self.sketch_on_first(p) {
            self.sketch_close(p);
            return;
        }
        let Some(q) = self.sketch_point(p) else { return };
        let free = self.surface_free(p);
        let k = self.sketch.as_mut().unwrap();
        // the same point twice: nothing
        if k.pts.last().map(|l| (l[0] - q[0]).abs() + (l[1] - q[1]).abs() < 1e-9).unwrap_or(false) {
            k.held = None;
            return;
        }
        k.pts.push(q);
        k.smooth.push(false);
        k.corner.push(0.0);
        k.free.push(free);
        k.sharp.push(false);
        k.held = Some((k.pts.len() - 1, p));
    }

    /// The Surface's outline as drawn and made: in Curve mode on the scan,
    /// each sample put back on the scan (its nearest point) where the span
    /// joins two points put there.
    pub fn surface_line(&self, k: &Sketch, extra: Option<DVec3>, closed: bool) -> Vec<DVec3> {
        let line = k.outline3(extra, closed, self.surface_curve);
        let on_scan = k.free.iter().any(|f| f.is_some()) || (extra.is_some() && self.surface_on_scan);
        match self.reference.as_ref() {
            Some(r) if self.surface_curve && on_scan => {
                // near only: a farther surface is not this one's
                let reach = (r.edge.max(0.05) * 12.0).max(1.0);
                line.iter().map(|q| r.bvh.closest(&r.mesh, *q, reach).map(|c| c.1).unwrap_or(*q)).collect()
            }
            _ => line,
        }
    }

    /// Shift + click on a point placed (Surface, Curve mode): it breaks the
    /// curve in a V, or no longer. False when no point is there.
    pub fn sketch_toggle_sharp(&mut self, p: [f32; 2]) -> bool {
        if self.tool != Tool::Create(PrimKind::Surface) || !self.surface_curve {
            return false;
        }
        let i = self.sketch_point_at(p);
        if let (Some(k), Some(i)) = (self.sketch.as_mut(), i) {
            if k.sharp.len() < k.pts.len() {
                k.sharp.resize(k.pts.len(), false);
            }
            k.sharp[i] = !k.sharp[i];
            return true;
        }
        false
    }

    /// The Surface tool "On scan": the reference under the pointer.
    pub fn surface_free(&self, p: [f32; 2]) -> Option<DVec3> {
        (self.tool == Tool::Create(PrimKind::Surface) && self.surface_on_scan).then(|| self.reference_at(p).map(|h| h.0)).flatten()
    }

    /// Dragging the press: the point follows, still a corner (Romain, 19
    /// nov., 21 h 28: a drag no longer bends it into a curve point).
    pub fn sketch_drag(&mut self, p: [f32; 2]) {
        let q = self.sketch_point(p);
        let free = self.surface_free(p);
        let Some(k) = self.sketch.as_mut() else { return };
        let (Some((i, from)), Some(q)) = (k.held, q) else { return };
        if (p[0] - from[0]).hypot(p[1] - from[1]) > 4.0 {
            k.pts[i] = q;
            if let Some(f) = k.free.get_mut(i) {
                *f = free;
            }
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
            k.corner.truncate(k.pts.len());
            k.free.truncate(k.pts.len());
            k.sharp.truncate(k.pts.len());
            k.held = None;
            if k.pts.is_empty() {
                self.sketch = None;
            }
        }
    }

    /// Closes the outline (Enter, or a click on its first point): it becomes
    /// a shape whose height follows the pointer, like the other tools.
    pub fn sketch_close(&mut self, p: [f32; 2]) {
        if self.tool == Tool::Create(PrimKind::Path) {
            self.path_finish(true);
            return;
        }
        if self.tool == Tool::Create(PrimKind::Surface) {
            self.surface_finish();
            return;
        }
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
        let profile = Profile { points: k.pts.iter().map(|q| [(q[0] - c[0]) / w, (q[1] - c[1]) / dp]).collect(), smooth: k.smooth.clone(), corner: k.corner.clone(), tension: vec![], open: false };
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
        let cr = Creation { kind: PrimKind::Profile, id, plane: k.plane, a: lo, b: hi, height: 0.0, phase: Phase::Height, height_from: p, profile: Some(profile), curve: None };
        self.creation = Some(cr.clone());
        self.selection = vec![id];
        self.apply_creation(&cr);
    }

    /// Ends the Surface tool's outline: filled with a surface with no
    /// thickness, through its points (on the plane, or where they were put
    /// on the scan), smooth inside; "On scan", passing near the scan inside
    /// the outline too.
    pub fn surface_finish(&mut self) {
        let Some(k) = self.sketch.clone() else { return };
        if k.pts.len() < 3 {
            self.toast("A surface needs 3 points at least", true);
            return;
        }
        let w: Vec<DVec3> = self.surface_line(&k, None, true);
        let c = w.iter().copied().sum::<DVec3>() / w.len() as f64;
        // its frame: the plane's axes (its normal from the outline when
        // the points are on the scan)
        let mut n = DVec3::ZERO;
        for i in 0..w.len() {
            n += (w[i] - c).cross(w[(i + 1) % w.len()] - c);
        }
        let n = if k.free.iter().any(|f| f.is_some()) && n.length() > 1e-9 { n.normalize() } else { k.plane.n };
        let n = if n.dot(k.plane.n) < 0.0 { -n } else { n };
        let u = k.plane.u - n * k.plane.u.dot(n);
        let u = if u.length() > 1e-6 { u.normalize() } else { n.any_orthonormal_vector() };
        let rot = basis_rotation(u, n);
        let inv = rot.inverse();
        let local: Vec<DVec3> = w.iter().map(|q| inv * (*q - c)).collect();
        let (lo, hi) = local.iter().fold((DVec3::splat(f64::MAX), DVec3::splat(f64::MIN)), |(a, b), q| (a.min(*q), b.max(*q)));
        let size = (hi - lo).max_element();
        if size < 0.5 {
            self.toast("The outline is too small: draw it wider", true);
            return;
        }
        // on the scan: the scan's points inside the outline (a grid of
        // about 8 × 8 over its box, each found along the normal)
        // on the scan: its heights on a grid over the outline's box (a
        // little past it), each found along the normal from above the
        // outline; the kernel makes the surface through them and trims it
        let mut inner = vec![];
        let mut grid = None;
        // on its plane: a flat grid (one surface trimmed by the outline,
        // as fast with a curve's hundreds of sides as with four)
        if !(self.surface_on_scan || k.free.iter().any(|f| f.is_some())) {
            for i in 0..4 {
                for j in 0..4 {
                    let x = lo.x - 1.0 + (hi.x - lo.x + 2.0) * i as f64 / 3.0;
                    let z = lo.z - 1.0 + (hi.z - lo.z + 2.0) * j as f64 / 3.0;
                    inner.push(DVec3::new(x, 0.0, z));
                }
            }
            grid = Some([4, 4]);
        }
        // a point put on the scan (the mode may have changed while drawing):
        // the surface follows the scan
        if self.surface_on_scan || k.free.iter().any(|f| f.is_some()) {
            if let Some(r) = self.reference.as_ref() {
                let cells = |len: f64| ((len / size * 24.0).round() as usize).clamp(6, 40);
                let (nx, nz) = (cells(hi.x - lo.x), cells(hi.z - lo.z));
                let pad = size * 0.04;
                let (x0, x1, z0, z1) = (lo.x - pad, hi.x + pad, lo.z - pad, hi.z + pad);
                let top = hi.y + size * 0.25;
                let reach = size * 0.6;
                let mut hs: Vec<Option<f64>> = vec![None; nx * nz];
                for i in 0..nx {
                    for k in 0..nz {
                        let x = x0 + (x1 - x0) * i as f64 / (nx - 1) as f64;
                        let z = z0 + (z1 - z0) * k as f64 / (nz - 1) as f64;
                        let o = c + rot * DVec3::new(x, top, z);
                        if let Some((h, _)) = r.hit(o, -n) {
                            let l = inv * (h - c);
                            if l.y.abs() < reach {
                                hs[i * nz + k] = Some(l.y);
                            }
                        }
                    }
                }
                // the cells the scan missed: their neighbours' mean, spread
                if hs.iter().any(|h| h.is_some()) {
                    for _ in 0..(nx + nz) {
                        if hs.iter().all(|h| h.is_some()) {
                            break;
                        }
                        let was = hs.clone();
                        for i in 0..nx {
                            for k in 0..nz {
                                if was[i * nz + k].is_some() {
                                    continue;
                                }
                                let mut sum = 0.0;
                                let mut cnt = 0.0;
                                for (di, dk) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                                    let (a, b) = (i as i64 + di, k as i64 + dk);
                                    if a >= 0 && b >= 0 && (a as usize) < nx && (b as usize) < nz {
                                        if let Some(v) = was[a as usize * nz + b as usize] {
                                            sum += v;
                                            cnt += 1.0;
                                        }
                                    }
                                }
                                if cnt > 0.0 {
                                    hs[i * nz + k] = Some(sum / cnt);
                                }
                            }
                        }
                    }
                    // smoothed a little: the scan's noise and small steps
                    // are not the surface's
                    let mut hv: Vec<f64> = hs.iter().map(|h| h.unwrap_or(0.0)).collect();
                    for _ in 0..2 {
                        let was = hv.clone();
                        for i in 0..nx {
                            for k in 0..nz {
                                let (mut sum, mut cnt) = (0.0, 0.0);
                                for di in -1i64..=1 {
                                    for dk in -1i64..=1 {
                                        let (a, b) = (i as i64 + di, k as i64 + dk);
                                        if a >= 0 && b >= 0 && (a as usize) < nx && (b as usize) < nz {
                                            let w = if di == 0 && dk == 0 { 4.0 } else if di == 0 || dk == 0 { 2.0 } else { 1.0 };
                                            sum += was[a as usize * nz + b as usize] * w;
                                            cnt += w;
                                        }
                                    }
                                }
                                hv[i * nz + k] = sum / cnt;
                            }
                        }
                    }
                    for i in 0..nx {
                        for k in 0..nz {
                            let x = x0 + (x1 - x0) * i as f64 / (nx - 1) as f64;
                            let z = z0 + (z1 - z0) * k as f64 / (nz - 1) as f64;
                            inner.push(DVec3::new(x, hv[i * nz + k], z));
                        }
                    }
                    grid = Some([nx, nz]);
                }
            }
        }
        self.sketch = None;
        let mut id = 0;
        let mat = self.material;
        self.edit("New Surface", |d| {
            id = d.add(Op::Add, Prim::Path { x: (hi.x - lo.x).max(0.01), z: (hi.z - lo.z).max(0.01) }, c, rot);
            if let Some(f) = d.feature_mut(id) {
                f.name = f.name.replace("Path", "Surface");
                f.material = mat;
                f.sheet = Some(nurbs_kernel::Sheet { pts: local, inner, grid });
            }
        });
        self.selection = vec![id];
        // one shape per tool (G calls it again)
        self.tool = Tool::Select;
    }

    /// Ends the Path tool's curve: open (Enter) or closed (a click on its
    /// first point). It is kept as drawn, with no thickness, and selected.
    pub fn path_finish(&mut self, closed: bool) {
        let Some(k) = self.sketch.clone() else { return };
        let need = if closed { 3 } else { 2 };
        if k.pts.len() < need {
            self.toast(if closed { "A closed path needs 3 points at least" } else { "A path needs 2 points at least" }, true);
            return;
        }
        let lo = k.pts.iter().fold([f64::MAX; 2], |a, q| [a[0].min(q[0]), a[1].min(q[1])]);
        let hi = k.pts.iter().fold([f64::MIN; 2], |a, q| [a[0].max(q[0]), a[1].max(q[1])]);
        let (w, dp) = (hi[0] - lo[0], hi[1] - lo[1]);
        if w.max(dp) < 0.5 {
            self.toast("The path is too short: draw it longer", true);
            return;
        }
        // a straight path along X or Z: its box is 0.01 mm across
        let (w, dp) = (w.max(0.01), dp.max(0.01));
        let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
        let profile = Profile { points: k.pts.iter().map(|q| [(q[0] - c[0]) / w, (q[1] - c[1]) / dp]).collect(), smooth: k.smooth.clone(), corner: k.corner.clone(), tension: vec![], open: !closed };
        self.sketch = None;
        let (origin, rot) = (k.plane.at(c), basis_rotation(k.plane.u, k.plane.n));
        let mut id = 0;
        self.edit("New Path", |d| {
            id = d.add(Op::Add, Prim::Path { x: w, z: dp }, origin, rot);
            if let Some(f) = d.feature_mut(id) {
                f.profile = Some(profile);
            }
        });
        self.selection = vec![id];
        self.tool = Tool::Select;
        self.sel_mode = SelMode::Object;
    }

    /// A curve's lines (world): a 2D boolean's as the kernel made them,
    /// else its own outline.
    pub fn curve_lines(&self, f: &Feature) -> Vec<Vec<DVec3>> {
        if f.shape_bool.is_some() {
            let Some(c) = self.model.as_ref().and_then(|m| m.curves.iter().find(|c| c.0 == f.id)) else { return vec![] };
            return c.1.iter().map(|l| l.iter().map(|p| DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64)).collect()).collect();
        }
        f.outline()
    }

    /// A solid's cage (world); a 2D boolean pulled into a solid: its
    /// region's lines at its base and its top.
    pub fn shape_outline(&self, f: &Feature) -> Vec<Vec<DVec3>> {
        if f.shape_bool.is_some() && f.is_extruded_curve() {
            let base = self.curve_lines(f);
            let v = f.axis() * f.prim.height();
            let top: Vec<Vec<DVec3>> = base.iter().map(|l| l.iter().map(|p| *p + v).collect()).collect();
            return base.into_iter().chain(top).collect();
        }
        f.outline()
    }

    /// The 2D boolean (in use) that takes this curve in.
    pub fn curve_inside(&self, id: FeatureId) -> Option<&Feature> {
        self.doc.boolean_of(id).and_then(|b| self.doc.feature(b)).filter(|b| !b.suspended)
    }

    /// Whether a 2D boolean is open: it or one of its operands selected,
    /// so a click picks its operands.
    pub fn boolean_open(&self, b: &Feature) -> bool {
        self.selection.contains(&b.id) || b.shape_bool.as_ref().map(|s| s.operands.iter().any(|o| self.selection.contains(o))).unwrap_or(false)
    }

    /// How far from a curve's line the pointer still catches it, in points.
    const PATH_CATCH: f32 = 12.0;

    /// The path drawn under the pointer (within a few points), in Object
    /// mode: paths have no faces for the picker. A 2D boolean's operands
    /// only once it is open (it selected, a click again: the operand under
    /// the pointer, as 3ds Max's operands level); they win over it there.
    pub fn pick_path(&self, p: [f32; 2]) -> Option<FeatureId> {
        let v = self.view();
        let mut best: Option<(FeatureId, f32)> = None;
        for f in self.doc.features.iter().filter(|f| f.visible && f.is_path()) {
            let mut bias = 0.0;
            if let Some(b) = self.curve_inside(f.id) {
                if !b.visible || !self.boolean_open(b) {
                    continue;
                }
                bias = 1.0;
            }
            for line in self.curve_lines(f) {
                let s: Vec<Option<[f32; 2]>> = line.iter().map(|q| v.project(q.as_vec3())).collect();
                for w in s.windows(2) {
                    let (Some(a), Some(b)) = (w[0], w[1]) else { continue };
                    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                    let l = dx * dx + dy * dy;
                    let t = if l > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l).clamp(0.0, 1.0) } else { 0.0 };
                    let d = (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy);
                    // a few points either side (Maya's pre-selection)
                    if d < Self::PATH_CATCH && best.map(|b| d - bias < b.1).unwrap_or(true) {
                        best = Some((f.id, d - bias));
                    }
                }
            }
        }
        best.map(|b| b.0)
    }

    // --------------------------------------------------------------- moves

    /// Where the gizmo sits: the pivot put by hand (D) for this selection,
    /// else the selection's own.
    pub fn pivot(&self) -> Option<DVec3> {
        let p = self.own_pivot()?;
        Some(self.custom_pivot.filter(|c| c.0 == self.pivot_key()).map(|c| c.1).unwrap_or(p))
    }

    /// Whether the gizmo sits on a pivot put by hand.
    pub fn custom_pivot_on(&self) -> bool {
        self.own_pivot().is_some() && self.custom_pivot.map(|c| c.0 == self.pivot_key()).unwrap_or(false)
    }

    /// What the pivot put by hand belongs to: the mode, the selection and
    /// what is picked (another selection drops it).
    pub fn pivot_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let bits = |v: DVec3| v.to_array().map(f64::to_bits);
        (self.sel_mode as u8).hash(&mut h);
        self.selection.hash(&mut h);
        for r in &self.face_sel {
            (r.tag, bits(r.at)).hash(&mut h);
        }
        for r in &self.edge_sel {
            (r.faces, bits(r.at)).hash(&mut h);
        }
        for v in &self.vert_sel {
            bits(*v).hash(&mut h);
        }
        h.finish()
    }

    /// D held, a click or a drag (Maya): the pivot jumps onto the nearest
    /// vertex or point of an edge (its ends, its middle, a circle's
    /// centre) on screen, the piece's own too, whatever the Snap setting
    /// (Romain, 7 h 56: "ça saute / snap sur des points ou points repère
    /// des edges"); none near: back to the selection's own.
    pub fn put_pivot(&mut self, p: [f32; 2]) {
        if self.own_pivot().is_none() {
            return;
        }
        let at = self.pivot_target(p);
        self.custom_pivot = at.map(|s| (self.pivot_key(), s.pos));
    }

    /// The point D would put the pivot on (drawn while D is held): the
    /// nearest on screen within 40 points, corners and centres first.
    pub fn pivot_target(&self, p: [f32; 2]) -> Option<crate::snap::Snapped> {
        use crate::snap::SnapKind;
        const JUMP: f32 = 40.0;
        let v = self.view();
        let mut cands: Vec<(f32, &crate::snap::SnapPoint)> = self
            .snaps
            .iter()
            .filter(|s| matches!(s.kind, SnapKind::End | SnapKind::Mid | SnapKind::Center | SnapKind::Origin))
            .filter_map(|s| {
                let q = v.project(s.pos.as_vec3())?;
                let d = (q[0] - p[0]).hypot(q[1] - p[1]);
                (d < JUMP).then_some((d + s.kind.rank(), s))
            })
            .collect();
        cands.sort_by(|a, b| a.0.total_cmp(&b.0));
        let s = cands.into_iter().take(12).map(|c| c.1).find(|s| s.kind == SnapKind::Origin || !self.hidden_but(s.pos.as_vec3(), &[]))?;
        Some(crate::snap::Snapped { pos: s.pos, kind: s.kind, source: None, guides: vec![] })
    }

    /// The selection's own pivot (centre of the selected shapes, or of the
    /// picked components).
    fn own_pivot(&self) -> Option<DVec3> {
        if self.sel_mode != SelMode::Object {
            return self.comp_pivot();
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
            Handle::Free => {
                let n = (self.view().eye.as_dvec3() - pivot).try_normalize()?;
                BasePlane::new(pivot, n, None).hit(ro, rd)
            }
            Handle::PushInset | Handle::PushDepth => None,
            Handle::Rot(a) => BasePlane::new(pivot, DVec3::AXES[a], None).hit(ro, rd),
            Handle::Scale(a) => {
                let axis = self.scale_frame()? * DVec3::AXES[a];
                closest_on_line(pivot, axis, ro, rd).map(|t| pivot + axis * t)
            }
            // all at once: by the distance on screen from the pivot
            Handle::ScaleAll => Some(DVec3::new(p[0] as f64, p[1] as f64, 0.0)),
            Handle::Size(_) => {
                let f = self.single()?;
                let axis = size_axis(f, h)?;
                closest_on_line(pivot, axis, ro, rd).map(|t| pivot + axis * t)
            }
        }
    }

    pub fn begin_drag(&mut self, h: Handle, p: [f32; 2]) {
        if matches!(h, Handle::PushInset | Handle::PushDepth) {
            self.begin_push_drag(h, p);
            return;
        }
        if self.sel_mode != SelMode::Object {
            self.begin_tweak_drag(h, p);
            return;
        }
        let Some(pivot) = self.handle_pivot(h) else { return };
        let Some(grab) = self.handle_param(h, pivot, p) else { return };
        // a 2D boolean's operands go with it
        let ids = self.doc.with_operands(&self.selection);
        let start: Vec<Feature> = self.doc.features.iter().filter(|f| ids.contains(&f.id) && f.is_solid()).cloned().collect();
        let custom = !matches!(h, Handle::Size(_)) && self.custom_pivot_on();
        // the elements' points (their operations' faces too) go with them;
        // a pivot put by hand alone
        let carried = match h {
            Handle::Axis(_) | Handle::Plane(_) | Handle::Free => {
                let feats: Vec<FeatureId> = start.iter().flat_map(|s| std::iter::once(s.id).chain(self.element_ops(s.id))).collect();
                let mut c = self.carried(&feats, &[], &[]);
                if custom {
                    c.pts = vec![(pivot, crate::snap::SnapKind::End)];
                }
                c
            }
            _ => Default::default(),
        };
        self.begin_live(match h {
            Handle::Size(_) => "Resize",
            Handle::Rot(_) => "Rotate",
            Handle::Scale(_) | Handle::ScaleAll => "Scale",
            _ => "Move",
        });
        let ops: Vec<(Feature, FeatureId)> = start
            .iter()
            .flat_map(|s| self.element_ops(s.id).into_iter().filter_map(|o| self.doc.feature(o).cloned()).map(move |o| (o, s.id)).collect::<Vec<_>>())
            .collect();
        self.drag = Some(Drag { handle: h, grab, start, line: None, ops, carried, pivot, custom });
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
        self.update_drag_shapes(p);
        if let Some(d) = self.drag.clone() {
            if !d.ops.is_empty() {
                self.carry_ops(&d);
            }
        }
    }

    /// The operations of the shapes being moved, turned or sized follow
    /// them: their points (edges', faces', vertices', knife points) mapped
    /// from the shape's frame and sizes before to those now.
    fn carry_ops(&mut self, d: &Drag) {
        let mut out: Vec<Feature> = vec![];
        for (op, owner) in &d.ops {
            let (Some(old), Some(new)) = (d.start.iter().find(|s| s.id == *owner), self.doc.feature(*owner)) else { continue };
            let m = |p: DVec3| from_norm(new, to_norm(old, p));
            out.push(carried(op, &m));
        }
        self.live(|doc| {
            for o in &out {
                if let Some(f) = doc.feature_mut(o.id) {
                    *f = o.clone();
                }
            }
        });
    }

    fn update_drag_shapes(&mut self, p: [f32; 2]) {
        let Some(d) = self.drag.clone() else { return };
        if d.line.is_some() {
            self.update_push_drag(&d, p);
            return;
        }
        if d.start.len() == 1 && d.start[0].tweak.is_some() {
            self.update_tweak_drag(&d, p);
            return;
        }
        let pivot0 = match d.handle {
            Handle::Size(_) => d.start.first().and_then(|f| size_handle_pos(f, d.handle)),
            _ => Some(d.pivot),
        };
        let Some(pivot0) = pivot0 else { return };
        let Some(now) = self.handle_param(d.handle, pivot0, p) else { return };
        let mut delta = now - d.grab;
        // the shapes moved and their operations: not snapped to
        let skip: Vec<FeatureId> = d.start.iter().map(|f| f.id).chain(d.ops.iter().map(|o| o.0.id)).collect();
        self.snap_shown = None;
        match d.handle {
            Handle::Axis(_) | Handle::Plane(_) | Handle::Free => {
                let raw = delta;
                delta = DVec3::new(self.snap(delta.x), self.snap(delta.y), self.snap(delta.z));
                // a point of the shape passing near a point of interest
                // lands on it; the pointer on one: the shape's centre, base
                // or top lines up with it (whichever is the closest to where
                // the pointer took it), along the axis or in the plane
                let keys: Vec<DVec3> = match d.start.as_slice() {
                    [f] if !d.custom => vec![f.centre(), f.origin, f.origin + f.axis() * f.prim.height()],
                    _ => vec![pivot0],
                };
                let Some(mask) = move_mask(d.handle) else { return };
                if let Some((best, shown)) = self.align_move(p, raw, mask, &keys, &d.carried, &skip) {
                    delta = best;
                    self.snap_shown = Some(shown);
                }
                if d.custom {
                    self.custom_pivot = Some((self.pivot_key(), pivot0 + delta));
                }
                let start = d.start.clone();
                self.live(|doc| {
                    for s in &start {
                        if let Some(f) = doc.feature_mut(s.id) {
                            f.origin = s.origin + delta;
                        }
                    }
                });
            }
            Handle::PushInset | Handle::PushDepth => {}
            Handle::Rot(a) => {
                let axis = DVec3::AXES[a];
                let flat = |v: DVec3| v - axis * v.dot(axis);
                let (v0, v1) = (flat(d.grab - pivot0), flat(now - pivot0));
                if v0.length() < 1e-9 || v1.length() < 1e-9 {
                    return;
                }
                let mut ang = v0.cross(v1).dot(axis).atan2(v0.dot(v1));
                if self.settings.snap {
                    let step = 15f64.to_radians();
                    ang = (ang / step).round() * step;
                }
                self.gizmo_readout = Some(format!("{} {:.0}°", ["X", "Y", "Z"][a], ang.to_degrees()));
                let dq = DQuat::from_axis_angle(axis, ang);
                let start = d.start.clone();
                self.live(|doc| {
                    for s in &start {
                        if let Some(f) = doc.feature_mut(s.id) {
                            f.rot = (dq * s.rot).normalize();
                            // turned round the pivot: its centre too
                            let c = pivot0 + dq * (s.centre() - pivot0);
                            f.origin = c - f.rot * DVec3::Y * s.prim.height() * 0.5;
                        }
                    }
                });
            }
            Handle::Scale(_) | Handle::ScaleAll => {
                let Some(frame) = self.scale_frame_of(&d.start) else { return };
                let k = match d.handle {
                    Handle::Scale(a) => {
                        let axis = frame * DVec3::AXES[a];
                        let (t0, t) = ((d.grab - pivot0).dot(axis), (now - pivot0).dot(axis));
                        if t0.abs() < 1e-9 {
                            return;
                        }
                        let mut v = DVec3::ONE;
                        v[a] = t / t0;
                        v
                    }
                    // all at once (Maya's centre): to the right bigger, to
                    // the left smaller, twice per 120 points
                    _ => DVec3::splat(2f64.powf((now.x - d.grab.x) / 120.0)),
                };
                // two decimals (snap on), never flat
                let snap = self.settings.snap;
                let k = k.map(|x| if snap { (x * 100.0).round() / 100.0 } else { x }.clamp(0.02, 100.0));
                self.gizmo_readout = Some(match d.handle {
                    Handle::Scale(a) => format!("{} × {:.2}", ["X", "Y", "Z"][a], k[a]),
                    _ => format!("× {:.2}", k.x),
                });
                let start = d.start.clone();
                self.live(|doc| {
                    for s in &start {
                        if let Some(f) = doc.feature_mut(s.id) {
                            *f = scaled(s, pivot0, frame, k);
                        }
                    }
                });
            }
            Handle::Size(k) => {
                let Some(s) = d.start.first().cloned() else { return };
                let Some(axis) = size_axis(&s, d.handle) else { return };
                let mut amount = self.snap(delta.dot(axis));
                // the handle reaches the point of interest under the pointer
                if let Some(sp) = self.pick_snap(p, &skip).cloned() {
                    amount = (sp.pos - pivot0).dot(axis);
                    let pos = pivot0 + axis * amount;
                    self.snap_shown = Some(crate::snap::Snapped { pos, kind: sp.kind, source: (pos.distance(sp.pos) > 1e-3).then_some(sp.pos), guides: vec![] });
                }
                let mut f = s.clone();
                match (&mut f.prim, k) {
                    (Prim::Box { x, .. } | Prim::Profile { x, .. } | Prim::Path { x, .. }, 0) => {
                        let nx = (*x + amount).max(0.1);
                        f.origin = s.origin + axis * (nx - *x) * 0.5;
                        *x = nx;
                    }
                    (Prim::Box { z, .. } | Prim::Profile { z, .. } | Prim::Path { z, .. }, 2) => {
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

    /// The frame the scale gizmo works in: the active shape's own axes.
    pub fn scale_frame(&self) -> Option<DQuat> {
        if self.sel_mode != SelMode::Object {
            return self.comp_pivot().map(|_| DQuat::IDENTITY);
        }
        let fs: Vec<Feature> = self.doc.features.iter().filter(|f| self.selection.contains(&f.id) && f.is_solid()).cloned().collect();
        self.scale_frame_of(&fs)
    }

    fn scale_frame_of(&self, fs: &[Feature]) -> Option<DQuat> {
        let last = self.selection.last().copied();
        fs.iter().find(|f| Some(f.id) == last).or(fs.first()).map(|f| f.rot)
    }

    /// The whole shapes an Object-mode drag moves, turns or scales.
    pub fn dragged_shapes(&self) -> Option<Vec<FeatureId>> {
        let d = self.drag.as_ref()?;
        if self.sel_mode != SelMode::Object || d.start.is_empty() || !d.start.iter().all(|f| f.is_solid() && !f.is_path()) {
            return None;
        }
        Some(d.start.iter().map(|f| f.id).collect())
    }

    /// Object mode is a transform (Romain, 20 nov. 5 h 52: "déplace le
    /// transform qui contient l'élément, du coup ce doit être fluide et
    /// léger"): while whole shapes are dragged, each added one's faces (its
    /// operations' too) shown at once where the pointer takes them, from
    /// where the model on screen has them, until the kernel's model of the
    /// document as it is now arrives (it rebuilds live, the latest request
    /// winning). Each moved element with its matrix (model → now); None
    /// when the model on screen is the document's.
    pub fn drag_preview(&self) -> Option<std::collections::HashMap<FeatureId, DMat4>> {
        let m = self.model.as_ref()?;
        if m.serial / 4 == self.version {
            return None;
        }
        // the shapes' frames when the model on screen was asked for
        let (_, then) = self.sent_frames.iter().find(|(s, _)| *s == m.serial)?;
        let mut out = std::collections::HashMap::new();
        for (id, t) in then {
            // a shape carved out is the part's hole: the kernel's model
            // shows it, its cage where the pointer takes it
            let Some(now) = self.doc.feature(*id).filter(|f| f.op != Op::Carve) else { continue };
            let x = shape_frame(now) * t.inverse();
            if !x.abs_diff_eq(DMat4::IDENTITY, 1e-9) && x.is_finite() {
                out.insert(*id, x);
            }
        }
        Some(out)
    }

    pub fn end_drag(&mut self) {
        self.snap_shown = None;
        self.gizmo_readout = None;
        if let Some(d) = self.drag.take() {
            self.end_live();
            // the pivot put by hand stays with what was moved
            if let (true, Some(c)) = (d.custom, self.custom_pivot) {
                self.custom_pivot = Some((self.pivot_key(), c.1));
            }
        }
    }

    // ----------------------------------------------------- kernel and model

    /// Once per frame: takes the kernel's answers, sends the document when
    /// it changed (coarse meshing while a gesture is live, fine after).
    pub fn tick(&mut self, now: f64) {
        self.now = now;
        self.sync_selection();
        if self.toast.as_ref().map(|t| now > t.until).unwrap_or(false) {
            self.toast = None;
        }
        while let Some(b) = self.kernel.take() {
            if self.model.as_ref().map(|m| b.model.serial >= m.serial).unwrap_or(true) {
                self.picker = Picker::new(&b.model);
                self.round_ms = b.round_ms;
                self.model = Some(Arc::new(b.model));
                self.model_gen += 1;
                self.clamp_rounds();
                self.check_build();
            }
        }
        // the journal: tool and mode changes
        let seen = (format!("{:?}", self.tool), format!("{:?}", self.sel_mode));
        if seen != self.journal_seen {
            let what = format!("{} · {}", seen.0, seen.1);
            self.journal_seen = seen;
            self.record("tool", &what);
        }
        self.refresh_snaps();
        self.remesh_tick();
        self.reference_tick();
        self.deviation_tick();
        let fine = !self.is_live();
        // quads only for a finished state (not while a gesture is live)
        let quads = self.quads && fine;
        let want = self.version * 4 + fine as u64 * 2 + quads as u64;
        // whole shapes dragged in Object mode: the kernel rebuilds live
        // (Romain, 20 nov. 6 h 45: "remets l'aperçu au déplacement… sans
        // couper"), and until each of its models arrives the shapes added
        // are drawn moved from the model on screen (`drag_preview`)
        if want != self.submitted && self.kernel.status() != Status::Missing {
            let iso = self.build_doc();
            self.kernel.submit(want, iso.as_ref().unwrap_or(&self.doc), fine, quads);
            self.submitted = want;
            let frames = self.doc.features.iter().filter(|f| f.is_solid() && !f.is_path()).map(|f| (f.id, shape_frame(f))).collect();
            self.sent_frames.push((want, frames));
            if self.sent_frames.len() > 64 {
                self.sent_frames.remove(0);
            }
        }
    }

    /// A fillet asked bigger than OpenCascade can make stops at its largest
    /// size: once the model of the current document says so, and no gesture
    /// is live, the document takes that size (no undo step of its own).
    fn clamp_rounds(&mut self) {
        let Some(m) = self.model.clone() else { return };
        if self.is_live() || m.serial / 4 != self.version || (m.limits.is_empty() && m.tension_limits.is_empty()) {
            return;
        }
        let mut said = None;
        for (id, l) in &m.limits {
            if let Some(f) = self.doc.feature_mut(*id) {
                let name = f.name.clone();
                if let Some(r) = f.round.as_mut() {
                    if r.size > l + 1e-6 {
                        r.size = *l;
                        said = Some(format!("{name}: {} mm is its largest size", crate::widgets::fmt_mm(*l as f32)));
                    }
                }
                if let Some(p) = f.push.as_mut() {
                    if p.inset > l + 1e-6 {
                        p.inset = *l;
                        said = Some(format!("{name}: {} mm is its largest inset", crate::widgets::fmt_mm(*l as f32)));
                    }
                }
            }
        }
        // a tension too full or too flat for a fillet running round it
        for (id, t) in &m.tension_limits {
            if let Some(f) = self.doc.feature_mut(*id) {
                let name = f.name.clone();
                if let Some(r) = f.round.as_mut() {
                    if (r.tension - t).abs() > 1e-6 {
                        let fuller = *t < r.tension;
                        r.tension = *t;
                        said = Some(format!("{name}: tension {t:.2} is its {} here (a fillet runs round it)", if fuller { "fullest" } else { "flattest" }));
                    }
                }
            }
        }
        if let Some(text) = said {
            self.touched();
            self.toast(&text, false);
        }
    }

    // ------------------------------------------------------ the reference

    /// Shows a mesh (.obj) as the reference to model on (one undo step; it
    /// replaces the one there was).
    pub fn import_reference(&mut self, path: &str) {
        self.ref_failed = None;
        self.ref_frame = true;
        self.ref_shown = true;
        let r = nurbs_kernel::RefMesh { path: path.to_string() };
        self.edit("Import reference", |d| d.reference = Some(r));
    }

    pub fn remove_reference(&mut self) {
        self.edit("Remove reference", |d| d.reference = None);
    }

    /// The reference is being read (the frames go on until it is there).
    pub fn ref_busy(&self) -> bool {
        self.ref_loading.is_some()
    }

    /// The reference under a point of the screen, when shown: the point
    /// and its distance from the eye.
    pub fn reference_at(&self, p: [f32; 2]) -> Option<(DVec3, f64)> {
        let r = self.reference.as_ref().filter(|_| self.ref_shown)?;
        let (o, d) = self.ray(p);
        r.hit(o, d.normalize())
    }

    /// Keeps the loaded reference in step with the document's (imported,
    /// removed, undone, another document opened).
    fn reference_tick(&mut self) {
        if let Some(l) = &self.ref_loading {
            match l.rx.try_recv() {
                Ok(res) => {
                    let path = l.path.clone();
                    self.ref_loading = None;
                    if self.doc.reference.as_ref().map(|r| r.path == path).unwrap_or(false) {
                        match res {
                            Ok(r) => {
                                let text = format!("{}: {} triangles ({:.1} s)", r.name, crate::reference::short_count(r.triangles()), r.secs);
                                self.reference = Some(Arc::new(r));
                                self.ref_gen += 1;
                                if std::mem::take(&mut self.ref_frame) {
                                    self.frame_all();
                                }
                                self.toast(&text, false);
                            }
                            Err(e) => {
                                self.ref_failed = Some(path);
                                self.toast(&format!("Reference not loaded: {e}"), true);
                            }
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(_) => self.ref_loading = None,
            }
        }
        let want = self.doc.reference.as_ref().map(|r| r.path.clone());
        if self.reference.as_ref().map(|r| Some(&r.path) != want.as_ref()).unwrap_or(false) {
            self.reference = None;
            self.ref_gen += 1;
        }
        if let Some(p) = want {
            if self.reference.is_none() && self.ref_failed.as_ref() != Some(&p) {
                self.ref_loading = Some(crate::reference::Reference::start(&p));
            }
        }
    }

    /// The model shown is behind the document (the kernel is working).
    pub fn building(&self) -> bool {
        self.kernel.busy() || self.model.as_ref().map(|m| m.serial / 4 != self.version).unwrap_or(!self.doc.features.is_empty())
    }

    /// The quad mesh of the part, when the Quads view (face by face) or the
    /// Remesh view has it.
    pub fn quad_mesh(&self) -> Option<&nurbs_kernel::Quads> {
        if self.remesh_on {
            return self.remesh.as_ref().and_then(|r| r.result.as_ref().ok()).map(|r| &r.0);
        }
        self.model.as_ref().and_then(|m| m.quads.as_ref()).filter(|_| self.quads)
    }

    /// The Quads view (face by face) on or off; it turns the Remesh view
    /// off (two separate views).
    pub fn set_quads_view(&mut self, on: bool) {
        self.quads = on;
        if on {
            self.remesh_on = false;
        }
        self.model_gen += 1;
    }

    /// The Remesh view on or off; it turns the Quads view off.
    pub fn set_remesh_view(&mut self, on: bool) {
        self.remesh_on = on;
        if on {
            self.quads = false;
        }
        self.model_gen += 1;
    }

    /// The Remesh view's target quad count.
    pub fn set_remesh_target(&mut self, n: usize) {
        self.remesh_target = n.clamp(100, 50_000);
    }

    /// The remesh under way (once per fine model of the current document
    /// and target, in a thread: it takes seconds).
    pub fn remeshing(&self) -> bool {
        let serial = self.model.as_ref().map(|m| m.serial).unwrap_or(0);
        self.remesh_on && (self.remesh_job.is_some() || self.remesh.as_ref().map(|r| r.serial != serial || r.target != self.remesh_target).unwrap_or(true))
    }

    fn remesh_tick(&mut self) {
        // its answer
        if let Some((g, t, rx)) = &self.remesh_job {
            match rx.try_recv() {
                Ok((res, ms)) => {
                    let (g, t) = (*g, *t);
                    self.remesh_job = None;
                    if self.model.as_ref().map(|m| m.serial) == Some(g) && t == self.remesh_target {
                        self.remesh = Some(Remeshed { serial: g, target: t, result: res, ms });
                        // the renderer takes the new quads
                        self.model_gen += 1;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(_) => self.remesh_job = None,
            }
        }
        if !self.remesh_on || self.remesh_job.is_some() {
            return;
        }
        let Some(m) = self.model.clone() else { return };
        // only a finished model of the current document
        if !m.fine || self.is_live() || m.serial / 4 != self.version || m.triangles.is_empty() {
            return;
        }
        if self.remesh.as_ref().map(|r| r.serial == m.serial && r.target == self.remesh_target).unwrap_or(false) {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let target = self.remesh_target;
        let serial = m.serial;
        std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            let s = nurbs_kernel::remesh::Settings { target_quads: target, ..Default::default() };
            let res = std::panic::catch_unwind(|| nurbs_kernel::remesh::remesh(&m, s)).unwrap_or_else(|_| Err("the remesh crashed".into()));
            let _ = tx.send((res, t0.elapsed().as_secs_f32() * 1000.0));
        });
        self.remesh_job = Some((serial, target, rx));
    }

    /// Writes the quads as Wavefront OBJ next to the documents (one group
    /// per face, named after its shape; faces the converter left out are
    /// not in it).
    pub fn export_obj(&mut self) {
        let Some(m) = self.model.clone() else { return };
        if self.remesh_on {
            let Some(q) = self.remesh.as_ref().and_then(|r| r.result.as_ref().ok()).map(|r| r.0.clone()) else {
                self.toast("No remesh yet: it is being made", true);
                return;
            };
            let s = nurbs_kernel::remesh::obj(&q, |f| {
                let tag = m.faces.get(f as usize).and_then(|x| x.tag);
                let who = tag.and_then(|t| self.doc.feature(t.feature)).map(|x| crate::files::sanitize(&x.name).replace(' ', "_")).unwrap_or_else(|| "part".into());
                format!("{who}_face{f}")
            });
            let path = crate::files::documents_dir().join(format!("{}-remesh.obj", crate::files::sanitize(&self.doc_name)));
            match std::fs::write(&path, s) {
                Ok(()) => {
                    let text = format!("Saved {} quads to {}", q.quads.len(), path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                    self.toast(&text, false);
                }
                Err(e) => self.toast(&format!("Export failed: {e}"), true),
            }
            return;
        }
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
        self.toast = Some(Toast { text: text.to_string(), until: self.now + if warn { 4.0 } else { 2.5 }, warn });
        self.record("toast", text);
    }

    // ---------------------------------------------------------- documents

    pub fn new_doc(&mut self) {
        self.abort_gestures();
        let before = std::mem::take(&mut self.doc);
        let sel = self.sel_state();
        self.push_undo("New", before, sel);
        self.selection.clear();
        self.doc_name = "Untitled".into();
        self.save_name = self.doc_name.clone();
        self.touched();
    }

    /// The gun scan used for the retopo tests: `FORME_GUN`, else where it
    /// lies on Romain's PC, else in the project files.
    pub fn gun_scan() -> Option<String> {
        let list = [
            std::env::var("FORME_GUN").unwrap_or_default(),
            r"C:\Users\wzx\Dropbox\WZX\WORKS\TOOLS RD\RESHAPEIT\GUN_A_ORIGIN.obj".to_string(),
            "/mnt/project-files/GUN_A_ORIGIN.obj".to_string(),
        ];
        list.into_iter().find(|p| !p.is_empty() && std::path::Path::new(p).is_file())
    }

    /// A blank document with the gun scan alone as its reference (one undo
    /// step).
    pub fn new_gun_scene(&mut self) {
        let Some(path) = Self::gun_scan() else {
            self.toast("Gun scan not found (set FORME_GUN to its .obj)", true);
            return;
        };
        self.abort_gestures();
        let before = std::mem::take(&mut self.doc);
        let sel = self.sel_state();
        self.push_undo("New gun scene", before, sel);
        self.selection.clear();
        self.doc.reference = Some(nurbs_kernel::RefMesh { path });
        self.ref_failed = None;
        self.ref_frame = true;
        self.ref_shown = true;
        self.doc_name = "Gun scan".into();
        self.save_name = self.doc_name.clone();
        self.touched();
    }

    pub fn open(&mut self, name: &str, doc: Doc) {
        self.abort_gestures();
        let before = std::mem::replace(&mut self.doc, doc);
        let sel = self.sel_state();
        self.push_undo("Open", before, sel);
        self.selection.clear();
        self.doc_name = name.to_string();
        self.save_name = name.to_string();
        self.touched();
        self.ref_frame = true;
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

/// How far a point is from a polyline (its segments).
pub fn polyline_distance(pts: &[[f32; 3]], at: DVec3) -> f64 {
    let p: Vec<DVec3> = pts.iter().map(|q| DVec3::new(q[0] as f64, q[1] as f64, q[2] as f64)).collect();
    if p.len() == 1 {
        return p[0].distance(at);
    }
    p.windows(2)
        .map(|w| {
            let d = w[1] - w[0];
            let t = if d.length_squared() > 0.0 { ((at - w[0]).dot(d) / d.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
            (w[0] + d * t).distance(at)
        })
        .fold(f64::MAX, f64::min)
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
/// A creation's height phase on a plane whose normal is within 20° of the
/// view's ray takes the pointer's travel on screen (cos 20°).
const HEAD_ON: f64 = 0.94;

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

/// A shape scaled round `pivot` by `k` along the axes of `frame`: its
/// centre moves, its sizes grow along its own axes (a cylinder's radius by
/// its X and Z).
/// An operation's face tags moved onto copied features (`map`: old id →
/// new id).
fn retag(f: &mut Feature, map: &std::collections::HashMap<FeatureId, FeatureId>) {
    let t = |tag: &mut i64| {
        if let Some(ft) = FaceTag::unpack(*tag) {
            if let Some(n) = map.get(&ft.feature) {
                *tag = FaceTag { feature: *n, sub: ft.sub }.pack();
            }
        }
    };
    if let Some(r) = f.round.as_mut() {
        for e in &mut r.edges {
            e.faces.iter_mut().for_each(t);
        }
    }
    if let Some(p) = f.push.as_mut() {
        t(&mut p.face.tag);
    }
    if let Some(tw) = f.tweak.as_mut() {
        let c = |c: &mut nurbs_kernel::Comp| match c {
            nurbs_kernel::Comp::Face(r) => t(&mut r.tag),
            nurbs_kernel::Comp::Edge(e) => e.faces.iter_mut().for_each(t),
            nurbs_kernel::Comp::Vertex(_) => {}
        };
        c(&mut tw.comp);
        tw.more.iter_mut().for_each(c);
    }
    if let Some(k) = f.cut.as_mut() {
        for p in &mut k.pts {
            p.edge.faces.iter_mut().for_each(t);
        }
        k.remove_faces.iter_mut().for_each(|r| t(&mut r.tag));
        for e in &mut k.remove_edges {
            e.faces.iter_mut().for_each(t);
        }
    }
}

/// A shape's sizes along its own X, Y, Z.
fn size3(f: &Feature) -> DVec3 {
    match f.prim {
        Prim::Box { x, y, z } | Prim::Profile { x, y, z } => DVec3::new(x, y, z),
        Prim::Cylinder { radius, height } => DVec3::new(radius * 2.0, height, radius * 2.0),
        Prim::Path { x, z } => DVec3::new(x, 1.0, z),
    }
    .max(DVec3::splat(1e-6))
}

/// A point in a shape's own frame, divided by its sizes.
fn to_norm(f: &Feature, p: DVec3) -> DVec3 {
    (f.rot.inverse() * (p - f.origin)) / size3(f)
}

fn from_norm(f: &Feature, q: DVec3) -> DVec3 {
    f.origin + f.rot * (q * size3(f))
}

/// An operation with its points mapped by `m` (an affine map): its edges',
/// face's, components' and knife points; a move's delta as a vector.
pub fn carried(op: &Feature, m: &dyn Fn(DVec3) -> DVec3) -> Feature {
    let mut f = op.clone();
    let lin = |v: DVec3| m(v) - m(DVec3::ZERO);
    if let Some(r) = f.round.as_mut() {
        for e in &mut r.edges {
            e.at = m(e.at);
        }
    }
    if let Some(p) = f.push.as_mut() {
        p.face.at = m(p.face.at);
    }
    if let Some(t) = f.tweak.as_mut() {
        let c = |c: &mut nurbs_kernel::Comp| match c {
            nurbs_kernel::Comp::Face(r) => r.at = m(r.at),
            nurbs_kernel::Comp::Edge(e) => e.at = m(e.at),
            nurbs_kernel::Comp::Vertex(v) => *v = m(*v),
        };
        c(&mut t.comp);
        t.more.iter_mut().for_each(c);
        t.delta = lin(t.delta);
    }
    if let Some(k) = f.cut.as_mut() {
        for p in &mut k.pts {
            p.at = m(p.at);
            p.edge.at = m(p.edge.at);
        }
    }
    f
}

pub fn scaled(s: &Feature, pivot: DVec3, frame: DQuat, k: DVec3) -> Feature {
    let mut f = s.clone();
    let c = s.centre();
    let local = frame.inverse() * (c - pivot);
    let c2 = pivot + frame * (local * k);
    match &mut f.prim {
        Prim::Box { x, y, z } | Prim::Profile { x, y, z } => {
            *x = (*x * k.x).max(0.1);
            *y = (*y * k.y).max(0.1);
            *z = (*z * k.z).max(0.1);
        }
        Prim::Cylinder { radius, height } => {
            let r = if (k.x - 1.0).abs() > 1e-9 { k.x } else { k.z };
            *radius = (*radius * r).max(0.05);
            *height = (*height * k.y).max(0.1);
        }
        Prim::Path { x, z } => {
            *x = (*x * k.x).max(0.1);
            *z = (*z * k.z).max(0.1);
        }
    }
    f.origin = c2 - f.axis() * f.prim.height() * 0.5;
    f
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
        (Handle::Size(0), Prim::Box { .. } | Prim::Profile { .. } | Prim::Path { .. }) => DVec3::new(hx, hh * 0.5, 0.0),
        (Handle::Size(2), Prim::Box { .. } | Prim::Profile { .. } | Prim::Path { .. }) => DVec3::new(0.0, hh * 0.5, hz),
        (Handle::Size(1), _) => DVec3::new(0.0, hh, 0.0),
        (Handle::Size(3), Prim::Cylinder { .. }) => DVec3::new(hx, hh * 0.5, 0.0),
        _ => return None,
    };
    Some(m.transform_point3(local))
}

/// The size handles of a shape.
pub fn size_handles(f: &Feature) -> Vec<Handle> {
    // a 2D boolean's box is its operands' (Scale, R, sizes them)
    if !f.is_solid() || f.is_sheet() || f.shape_bool.is_some() {
        return vec![];
    }
    match f.prim {
        Prim::Box { .. } | Prim::Profile { .. } => vec![Handle::Size(0), Handle::Size(1), Handle::Size(2)],
        Prim::Cylinder { .. } => vec![Handle::Size(3), Handle::Size(1)],
        Prim::Path { .. } => vec![Handle::Size(0), Handle::Size(2)],
    }
}

/// A shape's frame with its sizes: its unit shape to the world (what a move,
/// turn or scale changes).
pub fn shape_frame(f: &Feature) -> DMat4 {
    let k = match f.prim {
        Prim::Box { x, y, z } | Prim::Profile { x, y, z } => DVec3::new(x, y, z),
        Prim::Cylinder { radius, height } => DVec3::new(radius, height, radius),
        Prim::Path { x, z } => DVec3::new(x, 1.0, z),
    };
    f.matrix() * DMat4::from_scale(k.max(DVec3::splat(1e-6)))
}

/// How many lines the debug journal keeps.
pub const JOURNAL: usize = 40;
