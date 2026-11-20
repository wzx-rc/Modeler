//! Pattern / Array (Romain's choice, 20 nov., 16 h 44, among the tools
//! proposed: "une forme et ses opérations répétées en ligne ou en cercle
//! (évents, rivets), modifiables après"): a step that makes its shapes and
//! their operations again in a line, in rows or round an axis. The kernel
//! makes the copies (`build::arrayed_doc`) with the original's tags: a
//! click on a copy names the original, and an operation made on a copy is
//! moved before the step and made on all of them (`Doc::keep_arrays`).

use crate::state::{AppState, SelMode};
use glam::DVec3;
use nurbs_kernel::{Array, ArrayKind, Feature, FeatureId};

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
