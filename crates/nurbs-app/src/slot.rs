//! The Slot tool drawn by hand (Romain, 20 nov. 5 h 55: "Le Slot tool doit
//! fonctionner mais sans scan mode par défaut, l'ajouter en option"): a
//! drag on a face or the ground from one end of the slot to the other,
//! then the pointer sets its width, a click, then its height as any shape.
//! Made as a Sketch step: a rectangle along the drag whose corners are
//! rounded at half its width (round ends). With ON SCAN (the operation
//! window) the drag reads the scan instead (`scan.rs`).

use glam::DVec3;

use crate::state::{AppState, BasePlane, Sketch};

#[derive(Clone, Debug)]
pub struct SlotDraw {
    pub plane: BasePlane,
    /// The slot's two ends (plane coordinates).
    pub a: [f64; 2],
    pub b: [f64; 2],
    /// Its width (mm).
    pub w: f64,
    /// The length is set: the pointer sets the width, a click ends it.
    pub width: bool,
}

impl SlotDraw {
    pub fn len(&self) -> f64 {
        (self.b[0] - self.a[0]).hypot(self.b[1] - self.a[1])
    }

    /// The slot's outline in the world (its round ends sampled), for the
    /// overlay while it is drawn.
    pub fn outline(&self) -> Vec<DVec3> {
        let (a, b) = (self.plane.at(self.a), self.plane.at(self.b));
        let e = b - a;
        let l = e.length();
        if l < 1e-9 {
            return vec![];
        }
        let e = e / l;
        let s = self.plane.n.cross(e).normalize_or_zero();
        let r = (self.w * 0.5).min(l * 0.5).max(0.0);
        let (ca, cb) = (a + e * r, b - e * r);
        let mut out = vec![];
        let n = 24;
        for i in 0..=n {
            // the end at B: from +s round to −s
            let t = std::f64::consts::PI * i as f64 / n as f64;
            out.push(cb + (s * t.cos() + e * t.sin()) * r);
        }
        for i in 0..=n {
            let t = std::f64::consts::PI * i as f64 / n as f64;
            out.push(ca + (-s * t.cos() - e * t.sin()) * r);
        }
        out.push(out[0]);
        out
    }
}

impl AppState {
    /// A press with the Slot tool by hand: the first end of the slot on
    /// the face or ground under it; in the width phase, the end.
    pub fn slot_press(&mut self, p: [f32; 2]) {
        if self.slot_draw.as_ref().map(|s| s.width).unwrap_or(false) {
            self.slot_finish(p);
            return;
        }
        let Some((plane, a, _)) = self.start_plane(p) else { return };
        self.selection.clear();
        self.slot_draw = Some(SlotDraw { plane, a, b: a, w: 0.0, width: false });
    }

    /// The pointer moves: the other end (dragged), else the width.
    pub fn slot_move(&mut self, p: [f32; 2]) {
        let Some(s) = self.slot_draw.clone() else { return };
        if !s.width {
            if let Some((mut b, _)) = self.snap_on_plane(&s.plane, p, &[]) {
                // Shift (Romain 20 nov., 9 h 27: "Shift maintenu permet de
                // tracer en ligne droite"): the slot's line locked level,
                // upright or at 45° on its plane, its length on the grid step
                let w = [b[0] - s.a[0], b[1] - s.a[1]];
                if self.shift_held && w[0].hypot(w[1]) > 1e-9 {
                    let step = std::f64::consts::FRAC_PI_4;
                    let ang = (w[1].atan2(w[0]) / step).round() * step;
                    let u = [ang.cos(), ang.sin()];
                    let len = self.snap((w[0] * u[0] + w[1] * u[1]).max(0.0));
                    b = [s.a[0] + u[0] * len, s.a[1] + u[1] * len];
                }
                if let Some(d) = &mut self.slot_draw {
                    d.b = b;
                }
            }
            return;
        }
        let (ro, rd) = self.ray(p);
        let Some(q) = s.plane.hit(ro, rd.normalize()) else { return };
        let q = s.plane.to_plane(q);
        let l = s.len();
        let e = [(s.b[0] - s.a[0]) / l, (s.b[1] - s.a[1]) / l];
        let d = ((q[0] - s.a[0]) * -e[1] + (q[1] - s.a[1]) * e[0]).abs();
        let w = self.snap(2.0 * d).clamp(0.2, l);
        if let Some(dr) = &mut self.slot_draw {
            dr.w = w;
        }
    }

    /// Let go after the length: on to the width (a third of the length
    /// until the pointer moves). Too short: nothing.
    pub fn slot_release(&mut self, p: [f32; 2]) {
        let Some(s) = self.slot_draw.clone() else { return };
        if s.width {
            return;
        }
        let at = s.plane.at(s.a);
        if s.len() < (self.mm_per_point(at) * 6.0).max(0.4) {
            self.slot_draw = None;
            self.toast("Slot: drag from one end of the slot to the other, then set its width", false);
            return;
        }
        let w = self.snap(s.len() / 3.0).max(0.2);
        if let Some(d) = &mut self.slot_draw {
            d.width = true;
            d.w = w;
        }
        let _ = p;
    }

    /// The width set: the slot made as a Sketch outline (corners rounded at
    /// half its width) and on to its height, as a sketch is.
    fn slot_finish(&mut self, p: [f32; 2]) {
        let Some(s) = self.slot_draw.take() else { return };
        let l = s.len();
        let (a, b) = (s.plane.at(s.a), s.plane.at(s.b));
        let e = (b - a) / l;
        // the plane turned along the slot: its rectangle square to it
        let plane = BasePlane { origin: (a + b) * 0.5, u: e, n: s.plane.n, v: e.cross(s.plane.n), face: s.plane.face, scan: s.plane.scan, tangent: s.plane.tangent };
        let (x, z) = (l * 0.5, s.w * 0.5);
        let r = s.w * 0.5;
        self.sketch = Some(Sketch {
            plane,
            pts: vec![[-x, -z], [x, -z], [x, z], [-x, z]],
            smooth: vec![false; 4],
            held: None,
            corner: vec![r; 4],
            grab: None,
            free: vec![None; 4],
            sharp: vec![false; 4],
        });
        self.sketch_close(p);
        if let Some(id) = self.creation.as_ref().map(|c| c.id) {
            let name = if self.mode == nurbs_kernel::Op::Carve { "Slot" } else { "Pill" };
            self.live(|d| {
                if let Some(f) = d.feature_mut(id) {
                    f.name = name.into();
                }
            });
        }
    }
}
