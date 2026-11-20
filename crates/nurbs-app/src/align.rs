//! Align (3ds Max's Align, Alt + A; Romain 20 nov. 2026, 15 h 42: "un petit
//! outil d'alignement qui serait une petite fenêtre qui apparaît dans le
//! style de ce que fait 3ds Max"). The shapes selected, Align, a click on
//! the target shape; a small window then sets which axes line up and which
//! point of each (Min, Center, Pivot, Max), and whether the shapes take the
//! target's orientation. Shown live on the part; Apply keeps it and goes
//! on (the axes cleared, as in Max), OK keeps it and closes, Cancel puts
//! everything back. One undo step per Apply / OK.

use crate::state::*;
use glam::{DQuat, DVec3};
use nurbs_kernel::{Doc, Feature, FeatureId};

/// Which point of a shape's box lines up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignPoint {
    Min,
    Center,
    /// The shape's own pivot: its base's centre (the pivot put by hand
    /// with D when there is one).
    Pivot,
    Max,
}

impl AlignPoint {
    pub const ALL: [AlignPoint; 4] = [AlignPoint::Min, AlignPoint::Center, AlignPoint::Pivot, AlignPoint::Max];
    pub fn label(self) -> &'static str {
        match self {
            AlignPoint::Min => "Min",
            AlignPoint::Center => "Center",
            AlignPoint::Pivot => "Pivot",
            AlignPoint::Max => "Max",
        }
    }
}

/// A shape's box in the world (from the part's faces) and its pivot.
#[derive(Clone, Debug)]
pub struct AlignBox {
    pub lo: DVec3,
    pub hi: DVec3,
    pub pivot: DVec3,
    /// The points it was read from (a turned shape's box is read again
    /// from them turned).
    pub pts: Vec<DVec3>,
}

impl AlignBox {
    fn point(&self, p: AlignPoint) -> DVec3 {
        match p {
            AlignPoint::Min => self.lo,
            AlignPoint::Center => (self.lo + self.hi) * 0.5,
            AlignPoint::Pivot => self.pivot,
            AlignPoint::Max => self.hi,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Align {
    /// The elements moved (shapes and curves, with their operations).
    pub ids: Vec<FeatureId>,
    /// The shape they line up with (None: still to be clicked).
    pub target: Option<FeatureId>,
    pub axes: [bool; 3],
    pub cur: AlignPoint,
    pub tgt: AlignPoint,
    /// The shapes take the target's orientation.
    pub orient: bool,
    /// The document when the window opened, or at the last Apply (Cancel
    /// goes back to it; OK makes one undo step from it).
    pub base: Doc,
    pub sel: SelState,
    /// The moved elements' boxes and the target's, at `base`.
    pub boxes: Vec<(FeatureId, AlignBox)>,
    pub tbox: Option<AlignBox>,
    /// The boxes are read again from the next model of `base` (after an
    /// Apply that turned the shapes).
    pub stale: bool,
    /// Where the target was clicked (the window opens by it).
    pub at: [f32; 2],
}

impl AppState {
    /// Align (Alt + A): the selected shapes, then the target to click.
    pub fn begin_align(&mut self) {
        self.abort_gestures();
        let mut ids: Vec<FeatureId> = self.selection.iter().map(|id| self.element_of(*id)).filter(|id| self.doc.feature(*id).map(|f| f.is_solid() || f.is_path()).unwrap_or(false)).collect();
        ids.sort();
        ids.dedup();
        if ids.is_empty() {
            self.toast("Align: select the shapes to move first, then click the shape to align them to", true);
            return;
        }
        self.set_sel_mode(SelMode::Object);
        self.align = Some(Align {
            ids,
            target: None,
            axes: [true; 3],
            cur: AlignPoint::Center,
            tgt: AlignPoint::Center,
            orient: false,
            base: self.doc.clone(),
            sel: self.sel_state(),
            boxes: vec![],
            tbox: None,
            stale: false,
            at: [0.0; 2],
        });
        self.tool = Tool::Align;
    }

    /// The click on the target: the window opens and the shapes line up.
    pub fn align_pick(&mut self, p: [f32; 2]) {
        let Some(a) = self.align.as_ref() else { return };
        let ids = a.ids.clone();
        let hit = self.pick_path(p).or_else(|| self.pick_at(p).and_then(|h| self.feature_of_face(h.face))).map(|id| self.element_of(id));
        let Some(t) = hit else {
            self.toast("Align: click a shape", false);
            return;
        };
        if ids.contains(&t) {
            self.toast("Align: that one moves, click another shape", false);
            return;
        }
        let boxes: Vec<(FeatureId, AlignBox)> = ids.iter().filter_map(|id| self.align_box(*id).map(|b| (*id, b))).collect();
        let tbox = self.align_box(t);
        let (base, sel) = (self.doc.clone(), self.sel_state());
        if let Some(a) = self.align.as_mut() {
            a.target = Some(t);
            a.boxes = boxes;
            a.tbox = tbox;
            a.at = p;
            a.base = base;
            a.sel = sel;
        }
        self.tool = Tool::Select;
        self.record("tool", "Align window");
        self.align_update();
    }

    /// An element's box in the world: its shape's outline with the faces
    /// of its operations, or its curve's points; its pivot.
    pub fn align_box(&self, id: FeatureId) -> Option<AlignBox> {
        let f = self.doc.feature(id)?;
        // the shape's own outline (its faces in the part may be merged
        // with a flush neighbour's, which then takes them), and the faces
        // its operations made (an extrusion goes past its box)
        let mut pts: Vec<DVec3> = if f.is_path() { self.curve_lines(f).concat() } else { f.outline().concat() };
        if let Some(m) = self.model.as_ref().filter(|_| f.is_solid() && f.op == nurbs_kernel::Op::Add) {
            let ops: Vec<bool> = m.faces.iter().map(|g| g.tag.map(|t| t.feature != id && self.element_of(t.feature) == id).unwrap_or(false)).collect();
            for (t, tri) in m.triangles.iter().enumerate() {
                if ops.get(m.tri_face[t] as usize).copied().unwrap_or(false) {
                    pts.extend(tri.iter().map(|&v| glam::Vec3::from(m.positions[v as usize]).as_dvec3()));
                }
            }
        }
        if pts.is_empty() {
            let (a, b) = f.bounds();
            pts = vec![a, b];
        }
        // the mesh's corners shared by its triangles: once each
        pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)).then(a.z.total_cmp(&b.z)));
        pts.dedup();
        let lo = pts.iter().fold(DVec3::splat(f64::MAX), |a, p| a.min(*p));
        let hi = pts.iter().fold(DVec3::splat(f64::MIN), |a, p| a.max(*p));
        // the pivot put by hand (D) when it belongs to this one alone
        let custom = self.custom_pivot.filter(|c| self.selection == [id] && c.0 == self.pivot_key()).map(|c| c.1);
        Some(AlignBox { lo, hi, pivot: custom.unwrap_or(f.origin), pts })
    }

    /// The document as the window's settings put it: each moved element
    /// turned to the target's orientation (round its pivot) and moved so
    /// its point meets the target's along the axes ticked.
    pub fn align_update(&mut self) {
        let Some(a) = self.align.clone() else { return };
        let (Some(t), Some(tb)) = (a.target, a.tbox.clone()) else { return };
        let Some(target) = a.base.feature(t).cloned() else { return };
        let mut doc = a.base.clone();
        for (id, b) in &a.boxes {
            let Some(f) = a.base.feature(*id).cloned() else { continue };
            // the turn (round its pivot) and its box turned with it
            let turn = if a.orient && f.is_solid() && !f.is_path() { target.rot * f.rot.inverse() } else { DQuat::IDENTITY };
            let piv = b.pivot;
            let from = if turn != DQuat::IDENTITY {
                // its points turned: the box it will have
                let mut lo = DVec3::splat(f64::MAX);
                let mut hi = DVec3::splat(f64::MIN);
                for c in &b.pts {
                    let q = piv + turn * (*c - piv);
                    lo = lo.min(q);
                    hi = hi.max(q);
                }
                AlignBox { lo, hi, pivot: piv, pts: vec![] }.point(a.cur)
            } else {
                b.point(a.cur)
            };
            let to = tb.point(a.tgt);
            let mut d = DVec3::ZERO;
            for k in 0..3 {
                if a.axes[k] {
                    d[k] = to[k] - from[k];
                }
            }
            let map = move |p: DVec3| piv + turn * (p - piv) + d;
            let mut moved = vec![*id];
            moved.extend(a.base.with_operands(&[*id]).into_iter().filter(|x| x != id));
            for m in moved {
                if let Some(g) = doc.feature_mut(m) {
                    g.origin = map(g.origin);
                    if turn != DQuat::IDENTITY {
                        g.rot = (turn * g.rot).normalize();
                    }
                }
                for op in self.element_ops(m) {
                    if let Some(o) = a.base.feature(op) {
                        let c = carried(o, &map);
                        if let Some(g) = doc.feature_mut(op) {
                            *g = c;
                        }
                    }
                }
            }
        }
        if doc != self.doc {
            self.doc = doc;
            self.touched();
        }
    }

    /// Keeps what the window shows as one undo step ("Align").
    fn align_commit(&mut self) {
        let Some(a) = self.align.as_ref() else { return };
        let (base, sel) = (a.base.clone(), a.sel.clone());
        if self.doc != base {
            self.push_undo("Align", base.clone(), sel);
            self.after_change("Align", &base);
        }
    }

    /// OK: kept, the window closes.
    pub fn align_ok(&mut self) {
        self.align_commit();
        self.align = None;
        self.tool = Tool::Select;
    }

    /// Apply: kept, the window stays with its axes cleared (as in 3ds
    /// Max), the boxes read again where the shapes are now.
    pub fn align_apply(&mut self) {
        self.align_commit();
        let doc = self.doc.clone();
        let sel = self.sel_state();
        if let Some(a) = self.align.as_mut() {
            // moved only: the boxes move with them; turned: read again
            // from the part once the kernel has built it
            let turned = a.orient;
            for (id, b) in a.boxes.iter_mut() {
                let (Some(f0), Some(f1)) = (a.base.feature(*id), doc.feature(*id)) else { continue };
                let d = f1.origin - f0.origin;
                b.lo += d;
                b.hi += d;
                b.pivot += d;
                for p in b.pts.iter_mut() {
                    *p += d;
                }
            }
            a.stale = turned;
            a.base = doc;
            a.sel = sel;
            a.axes = [false; 3];
            a.orient = false;
        }
    }

    /// Cancel (Esc): everything as it was.
    pub fn align_cancel(&mut self) {
        if let Some(a) = self.align.take() {
            if a.target.is_some() && self.doc != a.base {
                self.doc = a.base;
                self.touched();
            }
        }
        self.tool = Tool::Select;
    }

    /// Once per frame: after an Apply that turned the shapes, their boxes
    /// are read again from the part's model of that document.
    pub fn align_tick(&mut self) {
        let Some(a) = self.align.as_ref() else { return };
        if !a.stale || a.target.is_none() || self.model.as_ref().map(|m| m.serial / 4 != self.version).unwrap_or(true) || self.doc != a.base {
            return;
        }
        let ids = a.ids.clone();
        let t = a.target;
        let boxes: Vec<(FeatureId, AlignBox)> = ids.iter().filter_map(|id| self.align_box(*id).map(|b| (*id, b))).collect();
        let tbox = t.and_then(|t| self.align_box(t));
        if let Some(a) = self.align.as_mut() {
            a.boxes = boxes;
            a.tbox = tbox.or(a.tbox.take());
            a.stale = false;
        }
    }

    /// Whether the window is open (the target clicked).
    pub fn align_open(&self) -> bool {
        self.align.as_ref().map(|a| a.target.is_some()).unwrap_or(false)
    }

    /// The names the window shows: what moves, and onto what.
    pub fn align_names(&self) -> (String, String) {
        let Some(a) = self.align.as_ref() else { return Default::default() };
        let name = |id: FeatureId| self.doc.feature(id).map(|f: &Feature| f.name.clone()).unwrap_or_default();
        let what = if a.ids.len() == 1 { name(a.ids[0]) } else { format!("{} shapes", a.ids.len()) };
        (what, a.target.map(name).unwrap_or_default())
    }
}
