//! The Slot tool (Romain, 18 nov. 2026, 16 h 38: "même principe avec
//! pilule, cube etc… auto détection + placement de primary shape
//! primitives"): a slot with round ends, like the vents of the gun's front
//! block, or a raised pill.
//!
//! A drag along the slot, from one end to the other. Around it, the flat
//! face the slot is cut in gives the axis; the walls on each side of the
//! drag its width, the walls at its ends its length; then the centre line
//! (two ends) and the radius are fitted on the whole wall (least squares,
//! the points far from it counting less and less), and the depth read from
//! the floor inside and the face around. Then a NURBS surface is laid on the
//! wall: every row an exact slot outline (two straight sides, two half
//! circles) round the same centre line, its half width read on the scan,
//! straight, tapered or curved down the wall, like the Circle's rings. The
//! scan does not change.

use crate::circle::{fit_profile, median, vertex_normal, ProfileKind};
use crate::mesh::{Mesh, Topo, NONE};
use crate::nurbs::{solve, Surface};
use glam::{DVec2, DVec3};
use std::collections::HashMap;

fn debug() -> bool {
    std::env::var("RECTIFY_DEBUG").is_ok()
}

/// The distance from `q` to the segment `a` `b`, and where along it (0..1).
fn seg_dist(q: DVec2, a: DVec2, b: DVec2) -> (f64, f64) {
    let ab = b - a;
    let l2 = ab.length_squared();
    let t = if l2 > 1e-18 { ((q - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    ((q - (a + ab * t)).length(), t)
}

/// A slot found on the scan.
#[derive(Clone, Debug)]
pub struct Slot {
    /// The centre line's ends, on the base plane (the face the slot is cut
    /// in, or the ground round a raised pill).
    pub a: DVec3,
    pub b: DVec3,
    /// From the base along the wall: into a slot, up a raised pill.
    pub axis: DVec3,
    /// Half the width: the round ends' radius (mm).
    pub r: f64,
    /// The wall's depth found, from the base to the floor (mm).
    pub depth: f64,
    /// A slot cut in (the wall faces the centre line) rather than a raised
    /// pill.
    pub inward: bool,
    /// The wall as read: (height along the axis from the base, distance
    /// from the centre line) of its vertices.
    wall: Vec<(f64, f64)>,
    edge: f64,
}

/// The NURBS surface's settings, like the Circle's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlotParams {
    /// From the base along the axis (mm).
    pub depth: f64,
    /// The half width down the wall: one (`Cylinder`: straight walls), even
    /// (`Cone`: tapered), or a cubic (`Curve`).
    pub profile: ProfileKind,
    /// Rows down the wall: `levels` + 1, each an exact slot outline.
    pub levels: usize,
}

/// The surface laid on the slot's wall, and how well it sits there.
#[derive(Clone, Debug)]
pub struct Laid {
    pub surface: Surface,
    /// How far the wall's points are from the surface (mm, root mean square).
    pub off: f64,
    /// How many of the wall's points it was fitted to.
    pub points: usize,
}

/// The vertices around the drag, in the slot's frame.
struct Around {
    /// On the plane: along the drag (x) and across it (y).
    q: Vec<DVec2>,
    /// Along the axis (the face's normal), from the drag's middle.
    hh: Vec<f64>,
    /// The normal: its part along the axis, its part on the plane.
    nz: Vec<f64>,
    n2: Vec<DVec2>,
}

impl Slot {
    /// Finds the slot under a drag from `p0` to `p1` (on the scan, `t0` the
    /// triangle under `p0`), from one end of the slot to the other. `h`: the
    /// scan's edge length.
    pub fn detect(m: &Mesh, topo: &Topo, t0: u32, p0: DVec3, p1: DVec3, h: f64) -> Result<Slot, String> {
        let len = (p1 - p0).length();
        let along = "Drag along the slot, from one end to the other";
        if len < h * 8.0 {
            return Err(along.into());
        }
        // 1. the scan around the drag: the triangles joined to the one
        // pressed, within reach of the drag
        let reach = len * 0.6 + h * 4.0;
        let seg3 = |x: DVec3| {
            let ab = p1 - p0;
            let t = ((x - p0).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
            (x - (p0 + ab * t)).length()
        };
        let mut tris: Vec<u32> = vec![t0];
        let mut seen: HashMap<u32, ()> = HashMap::new();
        seen.insert(t0, ());
        let mut k = 0;
        while k < tris.len() && tris.len() < 600_000 {
            let t = tris[k];
            k += 1;
            for &u in &topo.adj[t as usize] {
                if u != NONE && !seen.contains_key(&u) && seg3(m.tri_centre(u as usize)) <= reach {
                    seen.insert(u, ());
                    tris.push(u);
                }
            }
        }
        // 2. the axis: the most common normal (the face the slot is cut in,
        // and its floor), wide, then narrower
        let nrm: Vec<DVec3> = tris.iter().map(|&t| m.tri_normal(t as usize)).collect();
        let area: Vec<f64> = tris.iter().map(|&t| m.tri_area(t as usize)).collect();
        let mut n = nrm.iter().zip(&area).map(|(n, a)| *n * *a).sum::<DVec3>().normalize_or_zero();
        if n == DVec3::ZERO {
            return Err(along.into());
        }
        for deg in [30.0f64, 20.0, 12.0, 8.0, 5.0, 5.0] {
            let kk = deg.to_radians();
            let mut sum = DVec3::ZERO;
            for (nn, a) in nrm.iter().zip(&area) {
                let c = nn.dot(n);
                if c > 0.0 {
                    let ang = c.min(1.0).acos();
                    sum += *nn * (*a * (-(ang / kk).powi(2)).exp());
                }
            }
            if sum.length() > 1e-12 {
                n = sum.normalize();
            }
        }
        // the frame: along the drag (on the plane), across it
        let o = (p0 + p1) * 0.5;
        let e = (p1 - p0).reject_from(n).normalize_or_zero();
        if e == DVec3::ZERO {
            return Err(along.into());
        }
        let f = n.cross(e);
        let mut verts: Vec<u32> = tris.iter().flat_map(|&t| m.tris[t as usize]).collect();
        verts.sort_unstable();
        verts.dedup();
        let mut ar = Around { q: vec![], hh: vec![], nz: vec![], n2: vec![] };
        for &v in &verts {
            let x = m.pos[v as usize] - o;
            let vn = vertex_normal(m, topo, v);
            ar.q.push(DVec2::new(x.dot(e), x.dot(f)));
            ar.hh.push(x.dot(n));
            ar.nz.push(vn.dot(n));
            ar.n2.push(DVec2::new(vn.dot(e), vn.dot(f)));
        }
        let nv = verts.len();
        // how much of a wall each vertex is: nothing on the flat parts (the
        // face, the floor), fully where its normal is 65° or more from the
        // axis. The AI often makes the walls sloped, a fillet into a fillet
        // with no straight part: the middle of the slope is the wall.
        let wallness: Vec<f64> = (0..nv).map(|i| if ar.n2[i].length() < 0.3 { 0.0 } else { ((0.92 - ar.nz[i].abs()) / 0.5).clamp(0.0, 1.0) }).collect();
        let hl = (p1 - p0).reject_from(n).length() * 0.5;
        if std::env::var("RECTIFY_SLOT_DUMP").is_ok() {
            let mut rows: std::collections::BTreeMap<i64, (f64, f64, f64, usize, f64)> = Default::default();
            for i in 0..nv {
                if ar.q[i].x.abs() < hl * 0.4 {
                    let k = (ar.q[i].y / 0.1).floor() as i64;
                    let e = rows.entry(k).or_default();
                    e.0 += ar.hh[i];
                    e.1 += ar.nz[i];
                    e.2 += ar.n2[i].y;
                    e.3 += 1;
                    e.4 += wallness[i];
                }
            }
            for (k, e) in rows {
                let c = e.3 as f64;
                eprintln!("dump v {:6.2}: h {:7.3} nz {:6.3} ny {:6.3} n {:4} wall {:5.1}", k as f64 * 0.1, e.0 / c, e.1 / c, e.2 / c, e.3, e.4);
            }
        }
        // 3. the sides: on each side of the drag's middle, the walls facing
        // across it, the nearest strong peak of their distances (weighted by
        // how much of a wall they are). Facing the drag: a slot cut in;
        // facing away: a raised pill; the pair of walls nearest the drag.
        let bin = (h * 0.5).max(hl / 120.0);
        let peak_near = |vals: &[(f64, f64)], lo: f64, hi: f64, want: f64| -> Option<f64> {
            let nb = (((hi - lo) / bin).ceil() as usize).max(3);
            let mut hist = vec![0.0f64; nb];
            for &(x, w) in vals {
                let b = ((x - lo) / bin) as isize;
                if (0..nb as isize).contains(&b) {
                    hist[b as usize] += w;
                }
            }
            let sm: Vec<f64> = (0..nb).map(|i| hist[i.saturating_sub(1)] + 2.0 * hist[i] + hist[(i + 1).min(nb - 1)]).collect();
            let top = sm.iter().cloned().fold(0.0, f64::max);
            (1..nb - 1)
                .filter(|&i| sm[i] >= sm[i - 1] && sm[i] >= sm[i + 1] && sm[i] >= (top * 0.25).max(4.0))
                .map(|i| lo + (i as f64 + 0.5) * bin)
                .min_by(|a, b| (a - want).abs().partial_cmp(&(b - want).abs()).unwrap())
        };
        let side = |sign: f64, cut: bool| -> Option<f64> {
            let vals: Vec<(f64, f64)> = (0..nv)
                .filter(|&i| wallness[i] > 0.0 && ar.q[i].x.abs() < hl * 0.4 && ar.q[i].y * sign > 0.0 && ar.n2[i].y.abs() > 0.8 * ar.n2[i].length() && (ar.n2[i].y * sign < 0.0) == cut)
                .map(|i| (ar.q[i].y.abs(), wallness[i]))
                .collect();
            peak_near(&vals, 0.0, hl * 1.5, 0.0)
        };
        let pick = [true, false].into_iter().filter_map(|cut| Some((cut, side(1.0, cut)?, side(-1.0, cut)?))).min_by(|a, b| (a.1 + a.2).partial_cmp(&(b.1 + b.2)).unwrap());
        if debug() {
            eprintln!("slot: axis {:.3},{:.3},{:.3}, {} triangles, drag ±{hl:.3}, cut in +{:?} −{:?}, raised +{:?} −{:?}", n.x, n.y, n.z, tris.len(), side(1.0, true), side(-1.0, true), side(1.0, false), side(-1.0, false));
        }
        let Some((cut, vp, vm)) = pick else {
            return Err("No slot there: drag along a slot, from one end to the other".into());
        };
        let mut r = (vp + vm) * 0.5;
        let cv = (vp - vm) * 0.5;
        // 4. the ends: the walls facing along the drag, near its ends
        let end = |sign: f64| -> Option<f64> {
            let vals: Vec<(f64, f64)> = (0..nv)
                .filter(|&i| wallness[i] > 0.0 && (ar.q[i].y - cv).abs() < r * 0.5 && ar.q[i].x * sign > 0.0 && ar.n2[i].x.abs() > 0.8 * ar.n2[i].length() && (ar.n2[i].x * sign < 0.0) == cut)
                .map(|i| (ar.q[i].x.abs(), wallness[i]))
                .collect();
            peak_near(&vals, 0.0, hl + r * 2.0 + h * 4.0, hl)
        };
        let up = end(1.0).unwrap_or(hl);
        let um = -end(-1.0).unwrap_or(hl);
        let (mut a2, mut b2) = if up - um > r * 2.0 { (DVec2::new(um + r, cv), DVec2::new(up - r, cv)) } else { let c = DVec2::new((up + um) * 0.5, cv); (c, c) };
        if debug() {
            eprintln!("slot: {}, sides +{vp:.3} −{vm:.3}, ends {um:.3} {up:.3}", if cut { "cut in" } else { "raised" });
        }
        // 5. the centre line and the radius fitted on the whole wall: the
        // walls facing the centre line (or away from it), near the slot
        // first found; the points far from it counting less and less
        let facing = |i: usize, a2: DVec2, b2: DVec2| -> Option<(f64, f64)> {
            if wallness[i] <= 0.0 {
                return None;
            }
            let (d, t) = seg_dist(ar.q[i], a2, b2);
            let c = a2 + (b2 - a2) * t;
            let radial = (ar.q[i] - c) / d.max(1e-12);
            let k = ar.n2[i].dot(radial) / ar.n2[i].length();
            (if cut { k < -0.85 } else { k > 0.85 }).then_some((d, wallness[i]))
        };
        let near = |a2: DVec2, b2: DVec2, r: f64, band: f64| -> Vec<(DVec2, f64)> { (0..nv).filter_map(|i| facing(i, a2, b2).filter(|(d, _)| (d - r).abs() < band).map(|(_, w)| (ar.q[i], w))).collect() };
        let mut pts = near(a2, b2, r, r * 0.5);
        if pts.len() < 24 {
            return Err("No slot there: drag along a slot, from one end to the other".into());
        }
        for k in 0..14 {
            let sigma = (r * 0.15 * 0.75f64.powi(k)).max(h * 0.75);
            let mut ata = vec![vec![0.0; 5]; 5];
            let mut atb = vec![0.0; 5];
            for (q, wq) in &pts {
                let ab = b2 - a2;
                let l2 = ab.length_squared();
                let t = if l2 > 1e-18 { ((*q - a2).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
                let c = a2 + ab * t;
                let d = (*q - c).length();
                if d < 1e-9 {
                    continue;
                }
                let u = (*q - c) / d;
                let res = d - r;
                let w = wq / (1.0 + (res / sigma).powi(2));
                // d(res) / d(Ax, Ay, Bx, By, r)
                let g = [-(1.0 - t) * u.x, -(1.0 - t) * u.y, -t * u.x, -t * u.y, -1.0];
                for i in 0..5 {
                    for j in 0..5 {
                        ata[i][j] += w * g[i] * g[j];
                    }
                    atb[i] -= w * g[i] * res;
                }
            }
            // a little damping (a round slot: the ends move freely together)
            for (i, row) in ata.iter_mut().enumerate() {
                row[i] += 1e-6 + row[i] * 1e-3;
            }
            let Some(dx) = solve(ata, atb) else { break };
            let step = DVec2::new(dx[0], dx[1]).length().max(DVec2::new(dx[2], dx[3]).length()).max(dx[4].abs());
            if step > r {
                break;
            }
            a2 += DVec2::new(dx[0], dx[1]);
            b2 += DVec2::new(dx[2], dx[3]);
            r += dx[4];
            if k == 6 {
                // the wall again, round the slot fitted so far
                pts = near(a2, b2, r, r * 0.35 + h * 2.0);
            }
        }
        if !(r > h * 2.0) || (b2 - a2).length() > hl * 4.0 {
            return Err("No slot there: drag along a slot, from one end to the other".into());
        }
        // the centre line runs from the press towards the release
        if (b2 - a2).x < 0.0 {
            std::mem::swap(&mut a2, &mut b2);
        }
        // 6. the depth: the flat floor inside (the top of a raised pill)
        // and the face round it
        let flat = |i: usize| ar.nz[i] > 0.95;
        let ins: Vec<f64> = (0..nv).filter(|&i| flat(i) && seg_dist(ar.q[i], a2, b2).0 < r * 0.6).map(|i| ar.hh[i]).collect();
        let outs: Vec<f64> = (0..nv).filter(|&i| flat(i) && { let d = seg_dist(ar.q[i], a2, b2).0; d > r * 1.15 && d < r * 1.6 + h * 4.0 }).map(|i| ar.hh[i]).collect();
        let h_out = median(outs.clone());
        let h_in = median(ins.clone());
        let wall_h: Vec<f64> = (0..nv).filter_map(|i| facing(i, a2, b2).filter(|(d, _)| (d - r).abs() < r * 0.2 + h * 2.0).map(|_| ar.hh[i])).collect();
        let (base, sign, depth) = match (h_out, h_in) {
            (Some(ho), Some(hi)) if (ho - hi).abs() > h * 2.0 => (ho, if hi < ho { -1.0 } else { 1.0 }, (ho - hi).abs()),
            (Some(ho), _) => {
                // no floor (through, or too narrow): the wall's far end
                let mut hs = wall_h.clone();
                hs.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let (lo, hi) = (hs.get(hs.len() / 50).copied().unwrap_or(ho), hs.get(hs.len().saturating_sub(1 + hs.len() / 50)).copied().unwrap_or(ho));
                if cut {
                    (ho, -1.0, (ho - lo) * 1.1)
                } else {
                    (ho, 1.0, (hi - ho) * 1.1)
                }
            }
            _ => return Err("No flat face round it: drag along a slot cut in a face".into()),
        };
        if depth < h * 2.0 {
            return Err("No slot there: drag along a slot, from one end to the other".into());
        }
        let inward = sign < 0.0;
        let axis = n * sign;
        let to3 = |p: DVec2| o + e * p.x + f * p.y + n * base;
        // 7. the wall as read, for the surface: its slope included
        let win = (r * 0.5).max(h * 4.0);
        let wall: Vec<(f64, f64)> = (0..nv).filter_map(|i| facing(i, a2, b2).filter(|(d, _)| (d - r).abs() < win).map(|(d, _)| ((ar.hh[i] - base) * sign, d))).collect();
        if debug() {
            eprintln!("slot: A {:.3},{:.3} B {:.3},{:.3}, r {r:.3}, length {:.3}, floor {h_in:?} face {h_out:?} → depth {depth:.3} ({}), {} wall points ({} inside, {} round)", a2.x, a2.y, b2.x, b2.y, (b2 - a2).length() + 2.0 * r, if inward { "cut in" } else { "raised" }, wall.len(), ins.len(), outs.len());
        }
        Ok(Slot { a: to3(a2), b: to3(b2), axis, r, depth, inward, wall, edge: h })
    }

    /// The slot's length, end to end (mm).
    pub fn length(&self) -> f64 {
        (self.b - self.a).length() + self.r * 2.0
    }

    /// The wall read between the base and `depth`: (height 0..1, distance
    /// from the centre line).
    fn data(&self, depth: f64) -> Vec<(f64, f64)> {
        let inset = (self.edge * 1.5).min(depth * 0.1);
        self.wall.iter().filter(|w| w.0 >= inset && w.0 <= depth - inset).map(|w| (w.0 / depth, w.1)).collect()
    }

    /// A NURBS surface on the slot's wall, from the base to `p.depth`: every
    /// row an exact slot outline round the centre line.
    pub fn nurbs(&self, p: SlotParams) -> Laid {
        let depth = p.depth.max(self.edge * 2.0);
        let data = self.data(depth);
        let (profile, off) = fit_profile(&data, p.profile, p.levels, depth, self.r, self.edge);
        let (section, w, ku) = self.outline(profile.pts.iter().map(|q| q.x).sum::<f64>() / profile.pts.len() as f64);
        Laid { surface: Surface::sweep(&section, &w, ku, 2, self.axis, &profile), off, points: data.len() }
    }

    /// The simplest profile that follows the wall about as well as the
    /// others: straight, else tapered, else a curve (with its fewest
    /// levels). Each slot is read for itself.
    pub fn best_profile(&self, depth: f64) -> (ProfileKind, usize) {
        let off = |k: ProfileKind, levels: usize| self.nurbs(SlotParams { depth, profile: k, levels }).off;
        let curve = self.fewest_levels(depth, ProfileKind::Curve);
        let offs = [(ProfileKind::Cylinder, 1, off(ProfileKind::Cylinder, 1)), (ProfileKind::Cone, 1, off(ProfileKind::Cone, 1)), (ProfileKind::Curve, curve, off(ProfileKind::Curve, curve))];
        let best = offs.iter().map(|x| x.2).fold(f64::INFINITY, f64::min);
        if debug() {
            eprintln!("slot: off straight {:.3}, tapered {:.3}, curve {:.3} ({curve} levels)", offs[0].2, offs[1].2, offs[2].2);
        }
        offs.iter().find(|x| x.2 <= best * 1.3 + 0.01).map(|x| (x.0, x.1)).unwrap_or((ProfileKind::Cylinder, 1))
    }

    /// The fewest levels that follow the wall about as well as many do, as
    /// for the Circle.
    pub fn fewest_levels(&self, depth: f64, profile: ProfileKind) -> usize {
        if profile != ProfileKind::Curve {
            return 1;
        }
        let off = |levels: usize| self.nurbs(SlotParams { depth, profile, levels }).off;
        let best = off(16);
        let enough = best + (best * 0.05).max(0.01);
        (1..16).find(|&l| off(l) <= enough).unwrap_or(16)
    }

    /// The slot's outline as control points, each the centre line's point
    /// and the direction out from it (the outline at radius ρ: point +
    /// direction × ρ), their weights and the knots (by length at radius
    /// `r`): degree 2, 13 points, a straight side, a half circle, the other
    /// side, the other half circle; starting at A's side.
    fn outline(&self, r: f64) -> (Vec<(DVec3, DVec3)>, Vec<f64>, Vec<f64>) {
        let ab = self.b - self.a;
        let l = ab.length();
        let e = if l > 1e-9 { ab / l } else { self.axis.any_orthonormal_vector() };
        let f = self.axis.cross(e);
        let (a, b) = (self.a, self.b);
        let m = (a + b) * 0.5;
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let sec = vec![(a, -f), (m, -f), (b, -f), (b, e - f), (b, e), (b, e + f), (b, f), (m, f), (a, f), (a, f - e), (a, -e), (a, -e - f), (a, -f)];
        let w = vec![1.0, 1.0, 1.0, s, 1.0, s, 1.0, 1.0, 1.0, s, 1.0, s, 1.0];
        // the pieces' lengths: side, quarter, quarter, side, quarter, quarter
        let q = std::f64::consts::FRAC_PI_2 * r.max(1e-6);
        let side = l.max(q * 1e-3);
        let lens = [side, q, q, side, q, q];
        let total: f64 = lens.iter().sum();
        let mut ku = vec![0.0; 3];
        let mut acc = 0.0;
        for li in &lens[..5] {
            acc += li / total;
            ku.push(acc);
            ku.push(acc);
        }
        ku.extend([1.0; 3]);
        (sec, w, ku)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::{uniform_knots, Curve};

    #[test]
    fn every_row_is_a_slot_outline() {
        let s = Slot { a: DVec3::new(1.0, 2.0, 0.0), b: DVec3::new(7.0, 4.0, 0.0), axis: DVec3::new(0.0, 0.0, -1.0), r: 2.0, depth: 3.0, inward: true, wall: vec![], edge: 0.1 };
        let prof = Curve { degree: 3, knots: uniform_knots(3, 2), pts: vec![DVec3::new(2.0, 0.0, 0.0), DVec3::new(2.2, 1.0, 0.0), DVec3::new(1.9, 2.0, 0.0), DVec3::new(2.1, 2.5, 0.0), DVec3::new(2.0, 3.0, 0.0)], w: vec![1.0; 5] };
        let (sec, w, ku) = s.outline(2.0);
        let surf = Surface::sweep(&sec, &w, ku, 2, s.axis, &prof);
        let (a2, b2) = (DVec2::new(s.a.x, s.a.y), DVec2::new(s.b.x, s.b.y));
        for v in [0.0, 0.31, 0.5, 0.9, 1.0] {
            let p0 = surf.eval(0.0, v);
            let r0 = seg_dist(DVec2::new(p0.x, p0.y), a2, b2).0;
            for k in 0..=80 {
                let p = surf.eval(k as f64 / 80.0, v);
                assert!((p.z - p0.z).abs() < 1e-9);
                assert!((seg_dist(DVec2::new(p.x, p.y), a2, b2).0 - r0).abs() < 1e-9, "v {v} k {k}");
            }
        }
    }
}
