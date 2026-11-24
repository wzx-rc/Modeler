//! Pattern / Array (Romain's choice, 20 nov., 16 h 44, among the tools
//! proposed: "une forme et ses opérations répétées en ligne ou en cercle
//! (évents, rivets), modifiables après"): a step that makes its shapes and
//! their operations again in a line, in rows or round an axis. The kernel
//! makes the copies (`build::arrayed_doc`) with the original's tags: a
//! click on a copy names the original, and an operation made on a copy is
//! moved before the step and made on all of them (`Doc::keep_arrays`).

use crate::state::{closest_on_line, AppState, BasePlane, Drag, Handle, SelMode, Tool};
use glam::{DQuat, DVec3};
use nurbs_kernel::{Array, ArrayKind, Feature, FeatureId};

/// The most copies a grip gives along one way (the fields go further).
const GRIP_MAX: u32 = 100;

/// A grip of the selected Array on screen: a square (`from` None) or an
/// arrow from `from` to `at`.
#[derive(Clone, Copy, Debug)]
pub struct ArrayGrip {
    pub handle: Handle,
    pub at: DVec3,
    pub from: Option<DVec3>,
}

/// A step on the grid of 1 mm, at least 1 mm (0 stays 0).
fn nice(v: f64) -> f64 {
    if v.abs() < 1e-9 {
        0.0
    } else {
        v.signum() * v.abs().ceil().max(1.0)
    }
}

impl AppState {
    /// The selected Array step.
    pub fn selected_array(&self) -> Option<&Feature> {
        self.single().filter(|f| f.array.is_some())
    }

    /// The shapes an Array of what is picked would repeat (the elements of
    /// the shapes selected, of the faces and edges picked).
    fn array_sources(&self) -> Vec<FeatureId> {
        let els = match self.selected_array() {
            Some(f) => f.array.as_ref().map(|a| a.sources.clone()).unwrap_or_default(),
            None => self.picked_elements(),
        };
        els.into_iter().filter(|id| self.doc.feature(*id).is_some_and(|f| f.is_solid() && !f.is_path() && f.shape_bool.is_none() && f.loft.is_none())).collect()
    }

    /// The box round some shapes (their own boxes).
    fn boxes_of(&self, ids: &[FeatureId]) -> Option<(DVec3, DVec3)> {
        ids.iter().filter_map(|id| self.doc.feature(*id)).map(|f| f.bounds()).reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
    }

    /// Adds an Array of what is picked at the end (Ctrl + Shift + D, as
    /// Maya's Duplicate Special; the icon by Mirror; the marking menu): 4
    /// in a line along X, a step half again their width, on whole mm;
    /// selected, its window open.
    pub fn add_array(&mut self) -> Option<FeatureId> {
        let src = self.array_sources();
        if src.is_empty() {
            self.toast("Pick a shape to repeat first", true);
            return None;
        }
        self.abort_gestures();
        let (lo, hi) = self.boxes_of(&src)?;
        let size = hi - lo;
        let a = Array {
            sources: src.clone(),
            kind: ArrayKind::Line,
            count: 4,
            step: DVec3::new(nice(size.x.max(1.0) * 1.5), 0.0, 0.0),
            count2: 2,
            step2: DVec3::new(0.0, 0.0, nice(size.z.max(1.0) * 1.5)),
            axis: 1,
            centre: self.circle_centre(&src),
            angle: 360.0,
        };
        let mut id = None;
        self.edit("Array", |d| id = Some(d.add_array(a)));
        if let Some(id) = id {
            if self.sel_mode != SelMode::Object {
                self.set_sel_mode(SelMode::Object);
            }
            self.face_sel.clear();
            self.edge_sel.clear();
            self.vert_sel.clear();
            self.selection = vec![id];
            let n = src.len();
            self.toast(&format!("Array of {}: 4 in a line · its window: Line, Grid or Circle, how many, how far", if n == 1 { self.doc.feature(src[0]).map(|f| f.name.clone()).unwrap_or_default() } else { format!("{n} shapes") }), false);
        }
        id
    }

    /// Where a Circle Array turns round by default: the middle of the part
    /// under the shapes (the box of the solids made before them, else the
    /// world's origin), on the ground.
    pub fn circle_centre(&self, src: &[FeatureId]) -> DVec3 {
        let first = self.doc.features.iter().position(|f| src.contains(&f.id)).unwrap_or(0);
        let under = self.doc.features[..first].iter().filter(|f| f.visible && f.is_solid() && !f.is_path() && f.op == nurbs_kernel::Op::Add).map(|f| f.bounds()).reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)));
        match under {
            Some((lo, hi)) => {
                let c = (lo + hi) * 0.5;
                DVec3::new((c.x * 10.0).round() / 10.0, 0.0, (c.z * 10.0).round() / 10.0)
            }
            None => DVec3::ZERO,
        }
    }

    /// Sets the selected Array (live while a field is dragged: `start`
    /// begins a new undo step).
    pub fn set_array(&mut self, a: Array, start: bool) {
        let Some(id) = self.selected_array().map(|f| f.id) else { return };
        if start {
            if self.is_live() {
                self.end_live();
            }
            self.begin_live("Array");
        } else if !self.is_live() {
            self.begin_live("Array");
        }
        self.live(|d| {
            if let Some(f) = d.feature_mut(id) {
                f.array = Some(a);
            }
        });
    }

    /// The selected Array's grips (with the Select tool): Line and Grid, a
    /// square on the first copy (the step; dragged along it, the copies
    /// follow) and an arrow past the last copy (pulled out: more copies,
    /// the same step), a Grid's rows likewise; Circle, a square on the first
    /// copy (round the axis: the angle to the next, so the count) and an
    /// arrow at the end of the arc (the angle, the count kept).
    pub fn array_grips(&self) -> Vec<ArrayGrip> {
        if self.tool != Tool::Select || self.creation.is_some() || self.push.is_some() {
            return vec![];
        }
        let Some((a, pts)) = self.array_marks() else { return vec![] };
        let o = pts[0];
        let v = self.view();
        // so many mm for so many points on screen there
        let px = |at: DVec3| v.pixel_size_at((at.as_vec3() - v.eye).length()) as f64;
        let mut out = vec![];
        match a.kind {
            ArrayKind::Line | ArrayKind::Grid => {
                let ways = if a.kind == ArrayKind::Grid { 2 } else { 1 };
                for w in 0..ways {
                    let (s, n) = if w == 0 { (a.step, a.count) } else { (a.step2, a.count2) };
                    let dir = s.try_normalize().unwrap_or(if w == 0 { DVec3::X } else { DVec3::Z });
                    out.push(ArrayGrip { handle: Handle::ArrayStep(w as u8), at: o + s, from: None });
                    let last = o + s * (n.max(1) - 1) as f64;
                    let tip = last + dir * px(last) * 46.0;
                    out.push(ArrayGrip { handle: Handle::ArrayCount(w as u8), at: tip, from: Some(last) });
                }
            }
            ArrayKind::Circle => {
                let (c, r0, n) = circle_frame(&a, o);
                if r0.length() < 1e-6 {
                    return vec![];
                }
                let step = if a.count >= 2 { a.step_angle() } else { 30.0 };
                out.push(ArrayGrip { handle: Handle::ArrayCount(0), at: c + DQuat::from_axis_angle(n, step.to_radians()) * r0, from: None });
                // the arrow: out of the arc by 26 points, along it
                let end = if a.angle.abs() >= 359.999 { 360.0 } else { a.angle };
                let q = DQuat::from_axis_angle(n, end.to_radians());
                let on = c + q * r0;
                let k = 1.0 + px(on) * 26.0 / r0.length();
                let foot = c + q * r0 * k;
                let along = n.cross(q * r0).normalize_or_zero();
                out.push(ArrayGrip { handle: Handle::ArrayAngle, at: foot + along * px(foot) * 34.0, from: Some(foot) });
            }
        }
        out
    }

    /// A grip pressed: the Array as it is, where the pointer is along what
    /// the grip moves (`Drag.grab`: x where it began, y and z an angle's
    /// last value and its turn so far).
    pub(crate) fn begin_array_drag(&mut self, h: Handle, p: [f32; 2]) {
        let Some(f) = self.selected_array().cloned() else { return };
        let Some((a, pts)) = self.array_marks() else { return };
        let o = pts[0];
        let grab = match h {
            Handle::ArrayStep(w) | Handle::ArrayCount(w) if a.kind != ArrayKind::Circle => {
                let s = if w == 0 { a.step } else { a.step2 };
                let dir = s.try_normalize().unwrap_or(if w == 0 { DVec3::X } else { DVec3::Z });
                let (ro, rd) = self.ray(p);
                let Some(t) = closest_on_line(o, dir, ro, rd) else { return };
                DVec3::new(t, 0.0, 0.0)
            }
            _ => {
                let Some(t) = self.circle_angle(&a, o, p) else { return };
                DVec3::new(t, t, 0.0)
            }
        };
        self.begin_live("Array");
        self.drag = Some(Drag { handle: h, grab, start: vec![f], line: None, ops: vec![], carried: Default::default(), pivot: o, custom: false, keep: vec![], cage: None, pts: vec![], groups: vec![], objects: vec![] });
    }

    pub(crate) fn update_array_drag(&mut self, d: &Drag, p: [f32; 2]) {
        let Some(f0) = d.start.first() else { return };
        let Some(a0) = f0.array.clone() else { return };
        let id = f0.id;
        let o = d.pivot;
        let mut a = a0.clone();
        let readout;
        match (d.handle, a0.kind) {
            (Handle::ArrayStep(w) | Handle::ArrayCount(w), ArrayKind::Line | ArrayKind::Grid) => {
                let s = if w == 0 { a0.step } else { a0.step2 };
                let len = s.length();
                let dir = s.try_normalize().unwrap_or(if w == 0 { DVec3::X } else { DVec3::Z });
                let (ro, rd) = self.ray(p);
                let Some(t) = closest_on_line(o, dir, ro, rd) else { return };
                let moved = t - d.grab.x;
                if matches!(d.handle, Handle::ArrayStep(_)) {
                    // the first copy follows the pointer (the grid's step, 0.1 mm at least)
                    let raw = len + moved;
                    let mut l = self.snap(raw);
                    if l.abs() < 0.1 {
                        l = 0.1f64.copysign(raw);
                    }
                    if w == 0 {
                        a.step = dir * l;
                    } else {
                        a.step2 = dir * l;
                    }
                    readout = format!("{} {} mm", if w == 0 { "Step" } else { "Row step" }, crate::widgets::fmt3(l));
                } else {
                    if len < 1e-6 {
                        return;
                    }
                    // one more copy per step pulled out, at most 200 in all
                    let (n0, other) = if w == 0 { (a0.count, if a0.kind == ArrayKind::Grid { a0.count2.max(1) } else { 1 }) } else { (a0.count2, a0.count.max(1)) };
                    let cap = (nurbs_kernel::doc::ARRAY_MAX as u32 / other).clamp(1, GRIP_MAX);
                    let n = ((n0 as f64 + (moved / len).round()).max(1.0) as u32).min(cap);
                    if w == 0 {
                        a.count = n;
                    } else {
                        a.count2 = n;
                    }
                    readout = format!("{} {n}", if w == 0 { "Count" } else { "Rows" });
                }
            }
            (Handle::ArrayCount(_), ArrayKind::Circle) => {
                // the first copy round the axis: the angle to it sets how many
                let Some(t) = self.circle_angle(&a0, o, p) else { return };
                let th = t.rem_euclid(360.0).max(1.0);
                let n = if a0.angle.abs() >= 359.999 { (360.0 / th).round() } else { (a0.angle.abs() / th).round() + 1.0 };
                a.count = (n as u32).clamp(2, GRIP_MAX);
                readout = format!("Count {}", a.count);
            }
            (Handle::ArrayAngle, ArrayKind::Circle) => {
                let Some(t) = self.circle_angle(&a0, o, p) else { return };
                // the turn so far, unwrapped past a full turn
                let dt = (t - d.grab.y + 180.0).rem_euclid(360.0) - 180.0;
                let turn = d.grab.z + dt;
                if let Some(dd) = self.drag.as_mut() {
                    dd.grab.y = t;
                    dd.grab.z = turn;
                }
                let end0 = if a0.angle.abs() >= 359.999 { 360.0 } else { a0.angle };
                let mut ang = end0 + turn;
                if self.settings.snap {
                    ang = (ang / 15.0).round() * 15.0;
                }
                let ang = if ang >= 359.5 { 360.0 } else { ang.clamp(1.0, 360.0) };
                a.angle = ang;
                readout = format!("Angle {}°", ang.round() as i64);
            }
            _ => return,
        }
        self.gizmo_readout = Some(readout);
        if self.doc.feature(id).and_then(|f| f.array.as_ref()) == Some(&a) {
            return;
        }
        self.live(|doc| {
            if let Some(f) = doc.feature_mut(id) {
                f.array = Some(a);
            }
        });
    }

    /// Where the pointer is round a Circle Array's axis, in degrees from the
    /// original (on the plane through it square to the axis).
    fn circle_angle(&self, a: &Array, o: DVec3, p: [f32; 2]) -> Option<f64> {
        let (c, r0, n) = circle_frame(a, o);
        let u = r0.try_normalize()?;
        let v = n.cross(u);
        let (ro, rd) = self.ray(p);
        let q = BasePlane::new(c, n, None).hit(ro, rd)? - c;
        Some(q.dot(v).atan2(q.dot(u)).to_degrees())
    }

    /// The selected Array's places to draw: the original's middle first,
    /// then each copy's, in order.
    pub fn array_marks(&self) -> Option<(Array, Vec<DVec3>)> {
        let a = self.selected_array()?.array.clone()?;
        let (lo, hi) = self.boxes_of(&a.sources)?;
        let c = (lo + hi) * 0.5;
        let mut pts = vec![c];
        pts.extend(a.copies().iter().map(|(_, m)| m.transform_point3(c)));
        Some((a, pts))
    }
}

/// A Circle Array's frame through a point: the centre on the plane through
/// it square to the axis, the point from there, the axis.
fn circle_frame(a: &Array, o: DVec3) -> (DVec3, DVec3, DVec3) {
    let n = a.axis_dir();
    let c = a.centre + n * (o - a.centre).dot(n);
    (c, o - c, n)
}
