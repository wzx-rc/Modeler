//! Forme's tools reading the reference mesh (retopology, plan point 22).
//!
//! Step 2, the Cylinder read from the scan: with the Cylinder tool (C), a
//! drag that starts on the reference (`scan_under`) goes across a
//! round part of it, rim to rim, as Forme Rectify's Circle. Released, the
//! scan is read there once (`rectify_core::slot::Slot::detect_round`: the
//! wall going round, its centre, radius, axis and depth) and a real
//! Cylinder step goes in the history, sized and turned as read: a Carve for
//! a hole (begun a little outside its rim, so it cuts cleanly), an Add for a
//! boss, a button, a rivet. Nothing round there: nothing made, and a toast
//! says so.

use crate::state::AppState;
use glam::{DQuat, DVec3};
use nurbs_kernel::{Op, Prim};
use rectify_core::planes::dominant_normal;
use rectify_core::slot::Slot;

/// A drag across a round part of the scan.
#[derive(Clone, Debug)]
pub struct ScanRound {
    /// The triangle pressed, and the scan under the press and under the
    /// pointer: the diameter drawn.
    pub tri: u32,
    pub a: DVec3,
    pub b: DVec3,
    /// The way most of the scan faces round the press (towards the eye):
    /// the ring is drawn square to it.
    pub n: DVec3,
}

impl ScanRound {
    /// The drawn ring's axis: the plane's normal turned square to the
    /// diameter, so the ring goes through both ends.
    pub fn ring_normal(&self) -> DVec3 {
        let u = (self.b - self.a).normalize_or_zero();
        let m = (self.n - u * self.n.dot(u)).normalize_or_zero();
        if m == DVec3::ZERO {
            self.n
        } else {
            m
        }
    }

    pub fn radius(&self) -> f64 {
        (self.b - self.a).length() * 0.5
    }

    /// The ring drawn (closed polyline).
    pub fn ring(&self, segs: usize) -> Vec<DVec3> {
        let c = (self.a + self.b) * 0.5;
        let r = self.radius();
        let n = self.ring_normal();
        let x = (self.a - c).normalize_or_zero();
        let x = if x == DVec3::ZERO { n.any_orthonormal_vector() } else { x };
        let y = n.cross(x);
        (0..=segs).map(|i| (i as f64 / segs as f64) * std::f64::consts::TAU).map(|t| c + (x * t.cos() + y * t.sin()) * r).collect()
    }
}

/// What the scan gave for a round part: the cylinder step to make.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoundRead {
    pub op: Op,
    pub radius: f64,
    pub height: f64,
    /// The centre of the cylinder's base, and its axis (+Y turned there).
    pub origin: DVec3,
    pub axis: DVec3,
    /// How deep the hole or how high the boss was read (mm).
    pub depth: f64,
}

/// A hole's cylinder begins this far outside its rim (mm, at most).
const CLEAR: f64 = 0.5;

/// An axis read within this of a world axis is taken as that axis (an
/// AI-made mesh is a little off: the pistol's holes read 1.4° off X).
const AXIS_SNAP_DEG: f64 = 3.0;

/// The world axis near `a` (within `AXIS_SNAP_DEG`), else `a`.
pub fn snap_axis(a: DVec3) -> DVec3 {
    let cos = AXIS_SNAP_DEG.to_radians().cos();
    for w in [DVec3::X, DVec3::Y, DVec3::Z] {
        let c = a.dot(w);
        if c.abs() >= cos {
            return w * c.signum();
        }
    }
    a
}

/// From the scan's round part to a cylinder step.
pub fn round_step(s: &Slot) -> Option<RoundRead> {
    let axis = snap_axis(s.axis.normalize_or_zero());
    if axis == DVec3::ZERO || !(s.r > 0.0) {
        return None;
    }
    // a depth the scan cannot hold (a wall read past its end): none
    if !s.depth.is_finite() || s.depth > (s.r * 20.0).max(30.0) || s.depth < 0.05 {
        return None;
    }
    Some(if s.inward {
        let clear = (s.r * 0.1).clamp(0.1, CLEAR);
        RoundRead { op: Op::Carve, radius: s.r, height: s.depth + clear, origin: s.a - axis * clear, axis, depth: s.depth }
    } else {
        RoundRead { op: Op::Add, radius: s.r, height: s.depth, origin: s.a, axis, depth: s.depth }
    })
}

impl AppState {
    /// The reference under a point of the screen when it is shown: the
    /// triangle and the point. The part modelled on it lies on it or a
    /// little off it: the scan counts unless the part is clearly in front
    /// (by 2 % of the scan's size, at least 0.5 mm); seen through (Ghost),
    /// it always counts.
    pub fn scan_under(&self, p: [f32; 2]) -> Option<(u32, DVec3)> {
        let r = self.reference.as_ref().filter(|_| self.ref_shown)?;
        let (o, d) = self.ray(p);
        let d = d.normalize();
        let (t, x, s) = r.hit_tri(o, d)?;
        if !self.ref_ghost {
            let tol = ((r.hi - r.lo).length() * 0.02).max(0.5);
            if let Some(h) = self.pick_at(p) {
                if (h.t as f64) < s - tol {
                    return None;
                }
            }
        }
        Some((t, x))
    }

    /// The Cylinder pressed on the scan: true when it was (the drag goes
    /// across a round part of it), false to create as usual.
    pub fn scan_round_press(&mut self, p: [f32; 2]) -> bool {
        let Some((t, x)) = self.scan_under(p) else { return false };
        let Some(r) = self.reference.clone() else { return false };
        // the face round the rim, not the fillet or the wall the press is
        // on: the ring is drawn square to it
        let reach = (self.mm_per_point(x) * 60.0).clamp(1.0, 20.0);
        let mut n = dominant_normal(&r.mesh, &r.topo, t, x, reach);
        if n.dot(self.ray(p).1) > 0.0 {
            n = -n;
        }
        self.scan_round = Some(ScanRound { tri: t, a: x, b: x, n });
        true
    }

    /// The pointer moving: the ring follows it (the scan under it, else
    /// the plane through the press).
    pub fn scan_round_move(&mut self, p: [f32; 2]) {
        let Some(sr) = &self.scan_round else { return };
        let (o, d) = self.ray(p);
        let d = d.normalize();
        let on_scan = self.reference.as_ref().and_then(|r| r.hit(o, d)).map(|h| h.0);
        let on_plane = {
            let den = d.dot(sr.n);
            (den.abs() > 1e-9).then(|| o + d * ((sr.a - o).dot(sr.n) / den)).filter(|q| (*q - o).dot(d) > 0.0)
        };
        if let (Some(b), Some(sr)) = (on_scan.or(on_plane), &mut self.scan_round) {
            sr.b = b;
        }
    }

    /// Released: the scan read once there; a cylinder step made as read.
    pub fn scan_round_release(&mut self) {
        let Some(sr) = self.scan_round.take() else { return };
        let Some(r) = self.reference.clone() else { return };
        let rad = sr.radius();
        if rad * 2.0 < self.mm_per_point(sr.a) * 4.0 {
            return;
        }
        let c = (sr.a + sr.b) * 0.5;
        let found = Slot::detect_round(&r.mesh, &r.topo, sr.tri, sr.a, sr.b, r.edge);
        if std::env::var_os("FORME_SCAN_DEBUG").is_some() {
            match &found {
                Ok(s) => eprintln!("scan round: drawn Ø {:.2} at {c:.2?}; found Ø {:.2} at {:.2?} axis {:.3?} depth {:.2} inward {}", rad * 2.0, s.r * 2.0, s.a, s.axis, s.depth, s.inward),
                Err(e) => eprintln!("scan round: drawn Ø {:.2} at {c:.2?}: {e}", rad * 2.0),
            }
        }
        // about the size drawn, about where it was drawn
        let found = found.ok().filter(|s| {
            let off = s.a - c;
            let off = off - s.axis * s.axis.dot(off);
            (s.r - rad).abs() <= rad * 0.5 && off.length() <= rad
        });
        let Some(read) = found.as_ref().and_then(round_step) else {
            let why = if found.is_some() { "its depth could not be read" } else { "no wall goes round the circle drawn" };
            self.toast(&format!("Nothing round read there ({why}) · drag across it, rim to rim"), true);
            return;
        };
        let kind = if read.op == Op::Carve { "Hole" } else { "Boss" };
        let rot = DQuat::from_rotation_arc(DVec3::Y, read.axis);
        let mat = self.material;
        let mut id = 0;
        self.edit(&format!("{kind} from the scan"), |d| {
            id = d.add(read.op, Prim::Cylinder { radius: read.radius, height: read.height }, read.origin, rot);
            let name = d.fresh_name(kind);
            if let Some(f) = d.feature_mut(id) {
                f.name = name;
                f.material = mat;
            }
        });
        self.selection = vec![id];
        let fmt = |v: f64| crate::widgets::fmt_mm(v as f32);
        let way = if read.op == Op::Carve { "deep" } else { "high" };
        self.toast(&format!("{kind} read on the scan: Ø {} mm, {} mm {way}", fmt(read.radius * 2.0), fmt(read.depth)), false);
    }

    pub fn scan_round_cancel(&mut self) {
        self.scan_round = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_snaps_near_world_axes() {
        // the pistol's muzzle hole read 1.4° off -X: taken as -X
        let a = DVec3::new(-1.0, 0.005, 0.024).normalize();
        assert_eq!(snap_axis(a), -DVec3::X);
        // 5° off Z: kept as read
        let b = DVec3::new(0.0, 5f64.to_radians().sin(), 5f64.to_radians().cos());
        assert_eq!(snap_axis(b), b);
    }
}
