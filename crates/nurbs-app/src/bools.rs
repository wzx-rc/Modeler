//! A 2D boolean's own corners (Romain 23 nov., 7 h 12: "Il faut créer des
//! points à fillet aux intersections", a rounded rectangle and a circle
//! joined): where its curves cross, the result's sharp corners are
//! vertices in Vertex mode, rounded or cut like a sketch's corners
//! (`ShapeBool.corners`, made by the kernel on its region, `focc_fillet2d`;
//! an Extrude of it keeps them).

use crate::state::{AppState, SelMode};
use glam::DVec3;
use nurbs_kernel::{BoolCorner, FeatureId};

/// How far apart two ends of the result's lines may be and still meet.
const MEET: f64 = 1e-3;

impl AppState {
    /// Every 2D boolean's own corners (a flat one, not pulled into a
    /// solid, nor inside another boolean): its result's sharp corners and
    /// those already cut. The boolean, the corner (world), its cut (0:
    /// none).
    pub fn bool_corners(&self) -> Vec<(FeatureId, DVec3, f64)> {
        let mut out = vec![];
        for f in self.doc.features.iter().filter(|f| f.visible && !f.suspended && f.is_path()) {
            let Some(b) = &f.shape_bool else { continue };
            if self.curve_inside(f.id).is_some() {
                continue;
            }
            let m = f.matrix();
            let cut: Vec<(DVec3, f64)> = b.corners.iter().filter(|c| c.size.abs() > 1e-9).map(|c| (m.transform_point3(c.at), c.size)).collect();
            out.extend(cut.iter().map(|(p, s)| (f.id, *p, *s)));
            // the ends of the result's lines where two meet at an angle
            let lines = self.curve_lines(f);
            let mut ends: Vec<(DVec3, DVec3)> = vec![];
            for l in lines.iter().filter(|l| l.len() >= 2) {
                let n = l.len();
                let away = |a: DVec3, b: DVec3| (b - a).normalize_or_zero();
                ends.push((l[0], away(l[0], l[1])));
                ends.push((l[n - 1], away(l[n - 1], l[n - 2])));
            }
            for i in 0..ends.len() {
                let (p, d) = ends[i];
                let meet: Vec<&(DVec3, DVec3)> = ends.iter().enumerate().filter(|(j, e)| *j != i && e.0.distance(p) < MEET).map(|(_, e)| e).collect();
                if meet.len() != 1 || ends[..i].iter().any(|e| e.0.distance(p) < MEET) {
                    continue;
                }
                // sharp: the two lines do not go on straight through it
                if d.dot(meet[0].1) < -(3.0f64.to_radians().cos()) {
                    continue;
                }
                if out.iter().any(|(id, q, _)| *id == f.id && q.distance(p) < nurbs_kernel::doc::BOOL_CORNER_TOL) {
                    continue;
                }
                out.push((f.id, p, 0.0));
            }
        }
        out
    }

    /// The picked vertices that are a 2D boolean's corners: the boolean,
    /// the corner (world), its cut.
    pub fn picked_bool_corners(&self) -> Vec<(FeatureId, DVec3, f64)> {
        if self.sel_mode != SelMode::Vertex {
            return vec![];
        }
        let all = self.bool_corners();
        self.vert_sel.iter().filter_map(|v| all.iter().find(|c| c.1.distance(*v) < MEET).copied()).collect()
    }

    /// Rounds (or cuts, `chamfer`) the picked corners of 2D booleans
    /// (0: sharp again); live while a slider is dragged (`start` begins a
    /// new undo step).
    pub fn set_bool_corners(&mut self, size: f64, chamfer: bool, start: bool) {
        let picked = self.picked_bool_corners();
        if picked.is_empty() {
            return;
        }
        if start || !self.is_live() {
            self.end_live();
            self.begin_live(if chamfer { "Chamfer corner" } else { "Fillet corner" });
        }
        let v = if chamfer { -size.max(0.0) } else { size.max(0.0) };
        self.live(|d| {
            for (id, p, _) in &picked {
                let Some(f) = d.feature_mut(*id) else { continue };
                let at = f.matrix().inverse().transform_point3(*p);
                let Some(b) = f.shape_bool.as_mut() else { continue };
                match b.corners.iter().position(|c| c.at.distance(at) < MEET) {
                    Some(k) if v.abs() < 1e-9 => {
                        b.corners.remove(k);
                    }
                    Some(k) => b.corners[k].size = v,
                    None if v.abs() > 1e-9 => b.corners.push(BoolCorner { at, size: v }),
                    None => {}
                }
            }
        });
    }

    /// The largest cut the picked corners' booleans may take (half their
    /// smaller side).
    pub fn bool_corner_max(&self) -> f64 {
        self.picked_bool_corners()
            .iter()
            .filter_map(|(id, _, _)| self.doc.feature(*id))
            .map(|f| match f.prim {
                nurbs_kernel::Prim::Path { x, z } => x.min(z) * 0.5,
                _ => 20.0,
            })
            .fold(f64::MAX, f64::min)
            .clamp(0.1, 500.0)
    }
}
