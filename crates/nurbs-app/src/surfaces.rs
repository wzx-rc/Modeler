//! Curves made into surfaces and solids (Romain, 20 nov. 13 h 05: "créer
//! proprement une courbe… fermée, et que ça puisse faire une surface…
//! le truc des resolve et des lofts… pour bien avoir tout le kit nurbs"):
//! a curve drawn as an exact spline or with straight sides, its points
//! moved afterwards in Vertex mode, filled into a flat face (Fill), turned
//! round an axis in its plane (Revolve), or several curves joined by a
//! loft (Loft). The kernel builds them exactly (`build::sheet_tool`).

use crate::state::{basis_rotation, AppState, Drag, Handle, SelMode, Sketch};
use glam::{DQuat, DVec3};
use nurbs_kernel::{Feature, FeatureId, Loft, Op, Prim, Profile, Revolve};

/// What a curve makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurveMake {
    Nothing,
    Fill,
    Revolve,
}

/// The outline drawn (Path, Surface on its plane) as a profile in its box:
/// straight sides (corner fillets kept) or, `curve`, the spline through
/// its points (Shift + click made some sharp). With its frame and box.
pub fn sketch_profile(k: &Sketch, closed: bool, curve: bool) -> Option<(Profile, DVec3, DQuat, f64, f64)> {
    let lo = k.pts.iter().fold([f64::MAX; 2], |a, q| [a[0].min(q[0]), a[1].min(q[1])]);
    let hi = k.pts.iter().fold([f64::MIN; 2], |a, q| [a[0].max(q[0]), a[1].max(q[1])]);
    let (w, dp) = (hi[0] - lo[0], hi[1] - lo[1]);
    if w.max(dp) < 0.5 {
        return None;
    }
    // a straight path along X or Z: its box is 0.01 mm across
    let (w, dp) = (w.max(0.01), dp.max(0.01));
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
    let n = k.pts.len();
    let points = k.pts.iter().map(|q| [(q[0] - c[0]) / w, (q[1] - c[1]) / dp]).collect();
    let profile = if curve {
        Profile { points, smooth: (0..n).map(|i| !k.sharp.get(i).copied().unwrap_or(false)).collect(), corner: vec![], tension: vec![], open: !closed, spline: true }
    } else {
        Profile { points, smooth: k.smooth.clone(), corner: k.corner.clone(), tension: vec![], open: !closed, spline: false }
    };
    Some((profile, k.plane.at(c), basis_rotation(k.plane.u, k.plane.n), w, dp))
}

/// A path's points in its frame (mm), put back as its box and unit points:
/// the box round them, its origin moved to their middle (its turn kept).
pub fn reframe(f: &mut Feature, local: &[[f64; 2]]) {
    let lo = local.iter().fold([f64::MAX; 2], |a, q| [a[0].min(q[0]), a[1].min(q[1])]);
    let hi = local.iter().fold([f64::MIN; 2], |a, q| [a[0].max(q[0]), a[1].max(q[1])]);
    let (w, d) = ((hi[0] - lo[0]).max(0.01), (hi[1] - lo[1]).max(0.01));
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
    f.origin = f.matrix().transform_point3(DVec3::new(c[0], 0.0, c[1]));
    f.prim = Prim::Path { x: w, z: d };
    if let Some(pr) = f.profile.as_mut() {
        pr.points = local.iter().map(|q| [(q[0] - c[0]) / w, (q[1] - c[1]) / d]).collect();
    }
}

/// Where point `i` of a path is shown (world): a cut corner at its cut's
/// middle, else the point.
pub fn path_point_pos(f: &Feature, i: usize) -> Option<DVec3> {
    let (Prim::Path { x, z }, Some(pr)) = (f.prim, f.profile.as_ref()) else { return None };
    let q = *pr.scaled(x, z).get(i)?;
    let q = pr.cuts(x, z).get(i).copied().flatten().map(|c| c.m).unwrap_or(q);
    Some(f.matrix().transform_point3(DVec3::new(q[0], 0.0, q[1])))
}

impl AppState {
    /// The path point shown at `v` (any of its points: corners, curve
    /// points, a spline's), on a curve drawn by hand or a 3ds Max shape.
    pub fn path_point(&self, v: DVec3) -> Option<(FeatureId, usize)> {
        for f in self.doc.features.iter().filter(|f| f.visible && f.is_path() && f.shape_bool.is_none()) {
            let Some(pr) = &f.profile else { continue };
            for i in 0..pr.points.len() {
                if path_point_pos(f, i).map(|q| q.distance(v) < 1e-3).unwrap_or(false) {
                    return Some((f.id, i));
                }
            }
        }
        None
    }

    /// The picked vertices when they are all path points (Vertex mode).
    pub fn picked_path_points(&self) -> Option<Vec<(FeatureId, usize)>> {
        if self.sel_mode != SelMode::Vertex || self.vert_sel.is_empty() {
            return None;
        }
        self.vert_sel.iter().map(|v| self.path_point(*v)).collect()
    }

    /// The move gizmo dragged on picked path points: they move in their
    /// curve's plane (one undo step "Move points").
    pub fn begin_path_points_drag(&mut self, h: Handle, p: [f32; 2], pts: Vec<(FeatureId, usize)>) {
        if !matches!(h, Handle::Axis(_) | Handle::Plane(_) | Handle::Free) {
            return;
        }
        let Some(pivot) = self.pivot() else { return };
        let Some(grab) = self.handle_param(h, pivot, p) else { return };
        let mut ids: Vec<FeatureId> = pts.iter().map(|x| x.0).collect();
        ids.dedup();
        let start: Vec<Feature> = self.doc.features.iter().filter(|f| ids.contains(&f.id)).cloned().collect();
        self.begin_live("Move points");
        self.drag = Some(Drag { handle: h, grab, start, line: None, ops: vec![], carried: Default::default(), pivot, custom: false, keep: vec![], cage: None, pts, groups: vec![], objects: vec![], grow: vec![], legacy: false });
    }

    pub(crate) fn update_path_points_drag(&mut self, d: &Drag, p: [f32; 2]) {
        let Some(now) = self.handle_param(d.handle, d.pivot, p) else { return };
        let raw = now - d.grab;
        let delta = DVec3::new(self.snap(raw.x), self.snap(raw.y), self.snap(raw.z));
        let mut out: Vec<Feature> = vec![];
        for f0 in &d.start {
            let (Prim::Path { x, z }, Some(pr)) = (f0.prim, f0.profile.as_ref()) else { continue };
            let ld = f0.rot.inverse() * delta;
            let mut local = pr.scaled(x, z);
            for (id, i) in &d.pts {
                if *id == f0.id {
                    if let Some(q) = local.get_mut(*i) {
                        q[0] += ld.x;
                        q[1] += ld.z;
                    }
                }
            }
            let mut f = f0.clone();
            // a 3ds Max shape edited by hand becomes a plain path
            f.curve = None;
            reframe(&mut f, &local);
            out.push(f);
        }
        self.live(|doc| {
            for o in &out {
                if let Some(f) = doc.feature_mut(o.id) {
                    *f = o.clone();
                }
            }
        });
        // the picks follow their points
        self.vert_sel = d.pts.iter().filter_map(|(id, i)| self.doc.feature(*id).and_then(|f| path_point_pos(f, *i))).collect();
        self.gizmo_readout = Some(format!("{:.2}, {:.2}, {:.2}", delta.x, delta.y, delta.z));
    }

    /// A spline's points made sharp (a V) or smooth again.
    pub fn set_points_sharp(&mut self, pts: &[(FeatureId, usize)], sharp: bool) {
        self.edit(if sharp { "Sharp point" } else { "Smooth point" }, |d| {
            for (id, i) in pts {
                if let Some(pr) = d.feature_mut(*id).and_then(|f| f.profile.as_mut()).filter(|p| p.spline) {
                    if pr.smooth.len() < pr.points.len() {
                        pr.smooth.resize(pr.points.len(), true);
                    }
                    pr.smooth[*i] = !sharp;
                }
            }
        });
    }

    /// What a curve makes: nothing, a flat face, a turn round an axis.
    pub fn set_curve_make(&mut self, id: FeatureId, make: CurveMake) {
        let Some(f) = self.doc.feature(id) else { return };
        if make == CurveMake::Fill && !self.doc.is_closed_curve(id) {
            self.toast("Only a closed curve can be filled: close it first", true);
            return;
        }
        let label = match make {
            CurveMake::Nothing => "Curve only",
            CurveMake::Fill => "Fill",
            CurveMake::Revolve => "Revolve",
        };
        let keep = f.revolve;
        self.edit(label, |d| {
            if let Some(f) = d.feature_mut(id) {
                f.fill = make == CurveMake::Fill;
                f.revolve = if make == CurveMake::Revolve { Some(keep.unwrap_or_default()) } else { None };
            }
        });
    }

    /// A path's line: straight sides or the spline through its points.
    pub fn set_spline(&mut self, id: FeatureId, on: bool) {
        self.edit(if on { "Curve" } else { "Straight sides" }, |d| {
            if let Some(pr) = d.feature_mut(id).and_then(|f| {
                f.curve = None;
                f.profile.as_mut()
            }) {
                pr.spline = on;
                let n = pr.points.len();
                // a spline's points are round unless made sharp; straight
                // sides: all corners
                pr.smooth = vec![on; n];
                pr.corner.clear();
                pr.tension.clear();
            }
        });
    }

    /// Revolve's settings changed (a slider drag is one undo step).
    pub fn set_revolve(&mut self, id: FeatureId, r: Revolve, label: &str, start: bool) {
        if start || !self.is_live() {
            self.end_live();
            self.begin_live(label);
        }
        self.live(|d| {
            if let Some(f) = d.feature_mut(id) {
                f.revolve = Some(r);
            }
        });
    }

    /// The curves picked (2 or more, all closed or all open), joined by a
    /// loft in the order picked: a solid (closed) as the top bar says, or a
    /// sheet.
    pub fn make_loft(&mut self) {
        let ids: Vec<FeatureId> = self.selection.clone();
        if ids.len() < 2 || !ids.iter().all(|id| self.doc.feature(*id).map(|f| f.is_path()).unwrap_or(false)) {
            self.toast("Pick 2 curves or more for a loft", true);
            return;
        }
        let closed = ids.iter().filter(|id| self.doc.is_closed_curve(**id)).count();
        if closed != 0 && closed != ids.len() {
            self.toast("A loft's curves are all closed or all open", true);
            return;
        }
        let Some(first) = self.doc.feature(ids[0]).cloned() else { return };
        let op = self.mode;
        let mut id = 0;
        self.edit("Loft", |d| {
            let name = d.fresh_name("Loft");
            id = d.add(op, first.prim, first.origin, first.rot);
            if let Some(f) = d.feature_mut(id) {
                f.name = name;
                f.loft = Some(Loft { sections: ids.clone(), ruled: false });
            }
            d.regen_curves();
        });
        self.selection = vec![id];
        self.sel_mode = SelMode::Object;
    }

    /// A loft straight between its curves, or smooth through them.
    pub fn set_loft_ruled(&mut self, id: FeatureId, ruled: bool) {
        self.edit(if ruled { "Loft ruled" } else { "Loft smooth" }, |d| {
            if let Some(l) = d.feature_mut(id).and_then(|f| f.loft.as_mut()) {
                l.ruled = ruled;
            }
        });
    }

    /// A curve of a loft moved one place earlier.
    pub fn loft_up(&mut self, id: FeatureId, k: usize) {
        self.edit("Loft order", |d| {
            if let Some(l) = d.feature_mut(id).and_then(|f| f.loft.as_mut()) {
                if k > 0 && k < l.sections.len() {
                    l.sections.swap(k, k - 1);
                }
            }
        });
    }

    /// The loft a curve is in.
    pub fn loft_of(&self, id: FeatureId) -> Option<FeatureId> {
        self.doc.loft_of(id)
    }

    /// The Add or Carve a solid made from curves takes (the top bar's).
    pub fn curve_solid_op(&self, f: &Feature) -> Option<Op> {
        (self.doc.makes(f) == nurbs_kernel::Makes::Solid && (f.revolve.is_some() || f.loft.is_some())).then_some(f.op)
    }
}
