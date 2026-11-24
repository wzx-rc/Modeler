//! Cables (Romain 24 nov., 8 h 21: "Également un premier système de
//! Câble"): the Cable tool (H) puts points on the part's faces or the
//! ground, each lifted off the surface by the cable's radius along its
//! normal; Enter, a double-click or a right click makes the cable, a round
//! section swept along a smooth curve through them, sagging between them
//! by its slack (`nurbs_kernel::Cable`, built by the kernel's `focc_cable`).
//! A cable is an Object of its own: nothing is fused with it.

use crate::snap::SnapKind;
use crate::state::*;
use glam::{DQuat, DVec3};
use nurbs_kernel::{Cable, FeatureId, Op, Prim};

/// The next cable's radius and slack (the selected cable's in its window).
#[derive(Clone, Copy, Debug)]
pub struct CableSettings {
    pub radius: f64,
    pub slack: f64,
}

impl Default for CableSettings {
    fn default() -> Self {
        CableSettings { radius: 1.5, slack: 0.04 }
    }
}

impl AppState {
    /// Arms the Cable tool (H).
    pub fn arm_cable(&mut self) {
        self.abort_gestures();
        self.tool = Tool::Cable;
    }

    /// Where a cable's point goes for the pointer at `p`: on the face under
    /// it (a point of interest near it catches: a hole's or a boss's
    /// centre, a corner, an edge's middle), lifted by the radius along the
    /// face's normal towards the eye; else on the ground, lifted up.
    pub fn cable_point_at(&self, p: [f32; 2]) -> Option<(DVec3, DVec3)> {
        let r = self.cable.radius;
        let (ro, _) = self.ray(p);
        if let Some(s) = self.pick_snap(p, &[]).filter(|s| matches!(s.kind, SnapKind::Center | SnapKind::End | SnapKind::Mid | SnapKind::FaceCenter)) {
            let under = self.pick_at(p).map(|h| h.face);
            let face = self.snap_face(s, under);
            let n = face.and_then(|f| self.normal_near(f, s.pos)).unwrap_or(DVec3::Y);
            let n = if n.dot(ro - s.pos) < 0.0 { -n } else { n };
            return Some((s.pos + n * r, n));
        }
        if let Some(h) = self.pick_at(p) {
            let at = h.point.as_dvec3();
            let n = self.normal_near(h.face, at).unwrap_or(h.normal.as_dvec3()).normalize_or(DVec3::Y);
            let n = if n.dot(ro - at) < 0.0 { -n } else { n };
            return Some((at + n * r, n));
        }
        let g = BasePlane::ground();
        let (ro, rd) = self.ray(p);
        g.hit(ro, rd).map(|q| (q + DVec3::Y * r, DVec3::Y))
    }

    /// A click with the Cable tool: one more point.
    pub fn cable_click(&mut self, p: [f32; 2]) {
        let Some((q, _)) = self.cable_point_at(p) else { return };
        let pts = self.cable_draw.get_or_insert_with(Vec::new);
        if pts.last().map(|l| l.distance(q) < 1e-3).unwrap_or(false) {
            return;
        }
        pts.push(q);
    }

    /// The last point taken off (Backspace, Ctrl + Z while drawing).
    pub fn cable_back(&mut self) {
        if let Some(pts) = self.cable_draw.as_mut() {
            pts.pop();
            if pts.is_empty() {
                self.cable_draw = None;
            }
        }
    }

    /// The curve shown while a cable is drawn: its points and the pointer's.
    pub fn cable_preview(&self, pointer: Option<[f32; 2]>) -> Vec<DVec3> {
        let mut pts = self.cable_draw.clone().unwrap_or_default();
        if let Some(q) = pointer.and_then(|p| self.cable_point_at(p)) {
            pts.push(q.0);
        }
        Cable::spine(&pts, self.cable.radius, self.cable.slack)
    }

    /// The cable drawn is made (2 points at least): one undo step "Cable",
    /// selected, the tool still armed for the next one.
    pub fn cable_finish(&mut self) -> Option<FeatureId> {
        let pts = self.cable_draw.take()?;
        if pts.len() < 2 {
            self.toast("Cable: click two points at least", false);
            return None;
        }
        let set = self.cable;
        let mut id = 0;
        self.edit("Cable", |d| {
            let name = d.fresh_name("Cable");
            id = d.add(Op::Add, Prim::Box { x: 1.0, y: 1.0, z: 1.0 }, pts[0], DQuat::IDENTITY);
            if let Some(f) = d.feature_mut(id) {
                f.name = name;
                f.cable = Some(Cable { points: vec![], radius: set.radius, slack: set.slack });
                f.set_cable_points(&pts);
            }
        });
        self.selection = vec![id];
        self.group_sel = None;
        Some(id)
    }

    /// The selected cable's radius or slack set, its points kept (its box
    /// fitted again).
    pub fn set_cable(&mut self, id: FeatureId, radius: Option<f64>, slack: Option<f64>, live: bool) {
        if let Some(r) = radius {
            self.cable.radius = r;
        }
        if let Some(s) = slack {
            self.cable.slack = s;
        }
        let apply = move |d: &mut nurbs_kernel::Doc| {
            let Some(f) = d.feature_mut(id) else { return };
            let pts = f.cable_points();
            if let Some(c) = f.cable.as_mut() {
                if let Some(r) = radius {
                    c.radius = r.max(0.05);
                }
                if let Some(s) = slack {
                    c.slack = s.clamp(0.0, 1.0);
                }
            }
            f.set_cable_points(&pts);
        };
        if live {
            if !self.is_live() {
                self.begin_live("Cable");
            }
            self.live(apply);
        } else {
            self.edit("Cable", apply);
        }
    }
}
