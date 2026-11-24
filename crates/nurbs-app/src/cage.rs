//! The scale cage (R; Romain 20 nov., 11 h 44: "un Scale Tool en mode cage
//! à poignées"): a box round what is selected, in the active shape's own
//! frame (a component mode: the world's), with a grip in the middle of
//! each face (one axis), of each edge (two) and at each corner (all
//! three). A drag scales about the side facing the grip, which stays;
//! Shift keeps the proportions, Alt scales from the middle. The new size
//! goes on the grid (snap on), or the grip on the point of interest under
//! the pointer.

use crate::state::{closest_on_line, AppState, BasePlane, SelMode};
use crate::snap::Snapped;
use glam::{DQuat, DVec3};
use nurbs_kernel::FeatureId;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CageBox {
    /// Its axes; `lo`, `hi` are along them (world points turned back).
    pub frame: DQuat,
    pub lo: DVec3,
    pub hi: DVec3,
}

impl CageBox {
    /// A grip's place along its axes: -1 the low side, 1 the high, 0 the middle.
    pub fn local(&self, s: [i8; 3]) -> DVec3 {
        let pick = |a: usize| match s[a] {
            -1 => self.lo[a],
            1 => self.hi[a],
            _ => (self.lo[a] + self.hi[a]) * 0.5,
        };
        DVec3::new(pick(0), pick(1), pick(2))
    }

    pub fn point(&self, s: [i8; 3]) -> DVec3 {
        self.frame * self.local(s)
    }

    pub fn axis(&self, a: usize) -> DVec3 {
        self.frame * DVec3::AXES[a]
    }

    /// Its grips: none across an axis it has no size along (a flat face).
    pub fn grips(&self) -> Vec<[i8; 3]> {
        let size = self.hi - self.lo;
        let mut out = vec![];
        for x in -1i8..=1 {
            for y in -1i8..=1 {
                for z in -1i8..=1 {
                    let s = [x, y, z];
                    if s == [0, 0, 0] || (0..3).any(|a| s[a] != 0 && size[a] < 1e-6) {
                        continue;
                    }
                    out.push(s);
                }
            }
        }
        out
    }

    /// Its twelve edges (world).
    pub fn edges(&self) -> Vec<(DVec3, DVec3)> {
        let mut out = vec![];
        for a in 0..3 {
            let (b, c) = ((a + 1) % 3, (a + 2) % 3);
            for (sb, sc) in [(-1i8, -1i8), (-1, 1), (1, -1), (1, 1)] {
                let mut s0 = [0i8; 3];
                s0[a] = -1;
                s0[b] = sb;
                s0[c] = sc;
                let mut s1 = s0;
                s1[a] = 1;
                out.push((self.point(s0), self.point(s1)));
            }
        }
        out
    }
}

impl AppState {
    /// The scale cage round what is selected: the shapes (Object mode, in
    /// the active one's frame) or the picked components (the world's).
    pub fn cage(&self) -> Option<CageBox> {
        let (frame, pts): (DQuat, Vec<DVec3>) = if self.sel_mode == SelMode::Object {
            let ids = self.doc.with_operands(&self.selection);
            let fs: Vec<&nurbs_kernel::Feature> = self.doc.features.iter().filter(|f| ids.contains(&f.id) && f.is_solid()).collect();
            if fs.is_empty() {
                return None;
            }
            // round the part as built (its faces' edges), not the shapes'
            // boxes: a face moved off its shape (Romain 24 nov., 11 h 04)
            // left the box beside the part
            let mesh = self.element_points(&ids);
            (self.scale_frame()?, if mesh.is_empty() { fs.iter().flat_map(|f| self.seen_corners(f)).collect() } else { mesh })
        } else if self.gizmo == crate::state::GizmoMode::Universal {
            // Y: in the components' own frame (their normal), so a face's
            // box is flat along its normal and its arrow there moves it
            (self.comp_frame(), self.picked_points().iter().map(|p| p.as_dvec3()).collect())
        } else {
            (DQuat::IDENTITY, self.picked_points().iter().map(|p| p.as_dvec3()).collect())
        };
        if pts.is_empty() {
            return None;
        }
        let back = frame.inverse();
        let (lo, hi) = pts.iter().map(|p| back * *p).fold((DVec3::splat(f64::MAX), DVec3::splat(f64::MIN)), |(lo, hi), q| (lo.min(q), hi.max(q)));
        Some(CageBox { frame, lo, hi })
    }

    /// The points of the part's edges round the faces of these elements
    /// (their shapes and operations), as the kernel made them.
    pub fn element_points(&self, ids: &[FeatureId]) -> Vec<DVec3> {
        let Some(m) = self.model.as_ref() else { return vec![] };
        let map = crate::draw::element_map(self, m);
        let mine: Vec<bool> = m.faces.iter().map(|f| f.tag.map(|t| ids.contains(map.get(&t.feature).unwrap_or(&t.feature))).unwrap_or(false)).collect();
        let on = |i: i32| i >= 0 && mine.get(i as usize).copied().unwrap_or(false);
        m.edges.iter().filter(|e| on(e.faces[0]) || on(e.faces[1])).flat_map(|e| e.pts.iter().map(|q| glam::Vec3::from(*q).as_dvec3())).collect()
    }

    /// Where a grip goes under the pointer (world), and whether a point of
    /// interest caught it: a face's grip along its axis, an edge's in the
    /// plane of its two axes, a corner's in the plane facing the eye.
    pub fn cage_target(&self, c: &CageBox, s: [i8; 3], p: [f32; 2], skip: &[FeatureId]) -> Option<(DVec3, bool)> {
        if let Some(sp) = self.pick_snap(p, skip) {
            return Some((sp.pos, true));
        }
        let (ro, rd) = self.ray(p);
        let h0 = c.point(s);
        let on: Vec<usize> = (0..3).filter(|&a| s[a] != 0).collect();
        let t = match on.as_slice() {
            [a] => {
                let ax = c.axis(*a);
                closest_on_line(h0, ax, ro, rd).map(|t| h0 + ax * t)
            }
            [a, b] => {
                let n = c.axis(3 - a - b);
                BasePlane::new(h0, n, None).hit(ro, rd)
            }
            _ => {
                let n = (self.view().eye.as_dvec3() - h0).try_normalize()?;
                BasePlane::new(h0, n, None).hit(ro, rd)
            }
        }?;
        Some((t, false))
    }

    /// The scale along the cage's axes a grip dragged to the pointer gives,
    /// and the point it is about (world). The readout shows the new sizes.
    pub fn cage_scale(&mut self, c: &CageBox, s: [i8; 3], p: [f32; 2], skip: &[FeatureId]) -> Option<(DVec3, DVec3)> {
        let (target, caught) = self.cage_target(c, s, p, skip)?;
        // Ctrl held (Romain 24 nov., 9 h 25: "si CTRL maintenu avec une
        // poignée fait le scale dans les deux sens") or Alt: about the middle
        let centre = self.alt_held || self.snap_off;
        let mid = (c.lo + c.hi) * 0.5;
        let mut al = mid;
        if !centre {
            for a in 0..3 {
                if s[a] > 0 {
                    al[a] = c.lo[a];
                } else if s[a] < 0 {
                    al[a] = c.hi[a];
                }
            }
        }
        let tl = c.frame.inverse() * target;
        let h0 = c.local(s);
        let both = if centre { 2.0 } else { 1.0 };
        let mut k = DVec3::ONE;
        for a in (0..3).filter(|&a| s[a] != 0) {
            let d0 = h0[a] - al[a];
            if d0.abs() < 1e-9 {
                continue;
            }
            let mut d1 = (tl[a] - al[a]) * d0.signum();
            // the new size on the grid (a point of interest caught: exact)
            if !caught {
                d1 = self.snap(d1 * both) / both;
            }
            k[a] = d1 / d0.abs();
        }
        if self.shift_held {
            // the proportions kept: along the line from the point it is
            // about through the grip
            let u = h0 - al;
            let mut t = (tl - al).dot(u) / u.length_squared().max(1e-12);
            // free of a point of interest: where the pointer's ray passes
            // nearest that line in space (the corner's own plane facing the
            // eye gave far less than the pointer showed when the line runs
            // into the depth)
            if !caught {
                let (ro, rd) = self.ray(p);
                let (aw, uw) = (c.frame * al, c.frame * u);
                if let Some(tt) = uw.try_normalize().and_then(|d| closest_on_line(aw, d, ro, rd)) {
                    t = tt / uw.length();
                }
            }
            if self.settings.snap && !caught {
                t = (t * 100.0).round() / 100.0;
            }
            k = DVec3::splat(t);
        }
        let k = k.map(|x| x.clamp(0.02, 100.0));
        let size = (c.hi - c.lo) * k;
        let fmt = |v: f64| {
            let t = format!("{:.2}", v);
            t.trim_end_matches('0').trim_end_matches('.').to_string()
        };
        let parts: Vec<String> = (0..3).filter(|&a| c.hi[a] - c.lo[a] > 1e-6).map(|a| fmt(size[a])).collect();
        self.gizmo_readout = Some(format!("{} mm", parts.join(" × ")));
        self.snap_shown = caught.then(|| Snapped { pos: target, kind: crate::snap::SnapKind::End, source: None, guides: vec![] });
        Some((k, c.frame * al))
    }
}
