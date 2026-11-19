//! The Slot and Box tools (Romain, 18 nov. 2026, 16 h 38: "même principe
//! avec pilule, cube etc… auto détection + placement de primary shape
//! primitives"; 17 h 56, on the four vents laid: "c'est exactement ça…
//! essaie avec d'autres formes isolées comme ça").
//!
//! Slot: a slot with round ends (the vents of the gun's front block), or a
//! raised pill; a drag along it, from one end to the other. Box: a block
//! with rounded corners, raised (a button, a plate) or cut in (a pocket); a
//! drag across it, corner to corner. Both are a rounded rectangle: an inner
//! rectangle (a centre line A B, thickened by `w` across it; `w` = 0 for a
//! slot) and everything within `r` of it. The Circle tool (a hole, a boss,
//! a button, a rivet; a drag across it, rim to rim) uses the same with a
//! centre line of no length: a disc.
//!
//! Around the drag, the flat face gives the axis; the walls around it the
//! first outline (for a slot: the walls on each side of the drag and at its
//! ends; for a box: the walls' directions give its turn, then its four
//! sides); then the outline is fitted on the whole wall (least squares, the
//! points far from it counting less and less, the sloped walls the AI makes
//! counting by their slope), and the depth read from the floor inside and
//! the face around. Then a NURBS surface is laid on the wall: every row an
//! exact rounded rectangle round the same inner rectangle (straight sides,
//! quarter circles), its offset read on the scan, straight, tapered or
//! curved down the wall, like the Circle's rings. The scan does not change.

use crate::circle::{fit_profile, median, vertex_normal, ProfileKind};
use crate::mesh::{Mesh, Topo, NONE};
use crate::nurbs::{solve, Surface};
use glam::{DVec2, DVec3};
use std::collections::HashSet;

fn debug() -> bool {
    std::env::var("RECTIFY_DEBUG").is_ok()
}

/// The distance from `q` to the rectangle round the segment `a` `b`, `w`
/// on each side of it (`w` = 0: the segment), and the nearest point on it.
fn rect_dist(q: DVec2, a: DVec2, b: DVec2, w: f64) -> (f64, DVec2) {
    let ab = b - a;
    let l = ab.length();
    let e = if l > 1e-12 { ab / l } else { DVec2::X };
    let f = DVec2::new(-e.y, e.x);
    let m = (a + b) * 0.5;
    let d = q - m;
    let (x, y) = (d.dot(e), d.dot(f));
    let c = m + e * x.clamp(-l * 0.5, l * 0.5) + f * y.clamp(-w.max(0.0), w.max(0.0));
    ((q - c).length(), c)
}

/// The signed distance from `q` to that rectangle: negative inside it, by
/// how far from its border.
fn rect_sd(q: DVec2, a: DVec2, b: DVec2, w: f64) -> f64 {
    let ab = b - a;
    let l = ab.length();
    let e = if l > 1e-12 { ab / l } else { DVec2::X };
    let d = q - (a + b) * 0.5;
    let (dx, dy) = (d.dot(e).abs() - l * 0.5, d.perp_dot(e).abs() - w.max(0.0));
    DVec2::new(dx.max(0.0), dy.max(0.0)).length() + dx.max(dy).min(0.0)
}

/// What was looked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Round ends (the Slot tool).
    Slot,
    /// Four sides, rounded corners (the Box tool).
    Box,
    /// A disc: the centre line has no length (the Circle tool).
    Round,
}

/// A slot, a box or a round part found on the scan.
#[derive(Clone, Debug)]
pub struct Slot {
    /// The centre line's ends, on the base plane (the face it is cut in, or
    /// the ground round it when raised).
    pub a: DVec3,
    pub b: DVec3,
    /// Half the inner rectangle's width across the centre line (0: a slot).
    pub w: f64,
    /// From the base along the wall: into a slot, up a raised one.
    pub axis: DVec3,
    /// The corners' radius (a slot: half its width) (mm).
    pub r: f64,
    /// The wall's depth found, from the base to the floor (mm).
    pub depth: f64,
    /// Cut in (the wall faces the inside) rather than raised.
    pub inward: bool,
    /// Which tool found it.
    pub kind: Kind,
    /// A round part in steps (a button on its ring, a counterbore), from
    /// the outer one in: each round the same centre, smaller, from one level
    /// to the next. Empty: one step (`depth` is then its own; else `depth`
    /// runs to the innermost step's top or floor).
    pub tiers: Vec<Tier>,
    /// The wall as read: (height along the axis from the base, distance
    /// from the inner rectangle) of its vertices.
    wall: Vec<(f64, f64)>,
    edge: f64,
}

/// One step of a round part in steps.
#[derive(Clone, Debug)]
pub struct Tier {
    /// Its centre, on the outer step's base plane.
    pub centre: DVec3,
    pub r: f64,
    /// From and to along the axis, from the outer step's base (mm).
    pub from: f64,
    pub to: f64,
}

/// The NURBS surface's settings, like the Circle's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlotParams {
    /// From the base along the axis (mm).
    pub depth: f64,
    /// The offset down the wall: one (`Cylinder`: straight walls), even
    /// (`Cone`: tapered), or a cubic (`Curve`).
    pub profile: ProfileKind,
    /// Rows down the wall: `levels` + 1, each an exact outline.
    pub levels: usize,
}

/// The surface laid on the wall, and how well it sits there.
#[derive(Clone, Debug)]
pub struct Laid {
    pub surface: Surface,
    /// How far the wall's points are from the surface (mm, root mean square).
    pub off: f64,
    /// How many of the wall's points it was fitted to.
    pub points: usize,
}

/// The scan around the drag, in the drag's frame.
#[derive(Clone)]
struct Scene {
    /// The drag's middle, along it (e), across it (f), the face's normal (n).
    o: DVec3,
    e: DVec3,
    f: DVec3,
    n: DVec3,
    /// Per vertex: on the plane (x along the drag, y across), along the
    /// normal from the drag's middle, the normal's part along the axis and
    /// on the plane, how much of a wall it is.
    q: Vec<DVec2>,
    hh: Vec<f64>,
    nz: Vec<f64>,
    n2: Vec<DVec2>,
    wallness: Vec<f64>,
    /// Half the drag's length on the plane.
    hl: f64,
    tris: usize,
}

/// A wall found along a direction: how far, and the flat levels on each
/// side of it (inside, towards the drag's middle, and outside), along the
/// axis: the four sides of one block go from one level to another, a plate
/// under a button or a groove beside a pocket between others.
#[derive(Clone, Copy, Debug)]
struct Wall {
    at: f64,
    inner: f64,
    outer: f64,
}

impl Scene {
    /// The triangles joined to the one pressed within `reach` × the drag's
    /// length of it; the axis by mean shift of their normals.
    fn gather(m: &Mesh, topo: &Topo, t0: u32, p0: DVec3, p1: DVec3, reach: f64, h: f64) -> Option<Scene> {
        Self::gather_on(m, topo, t0, p0, p1, reach, h, false)
    }

    /// The same; `planar`: then gathered again with the reach measured on
    /// the face's plane (a drag on a pocket's floor reaches the face round
    /// it however deep the pocket).
    #[allow(clippy::too_many_arguments)]
    fn gather_on(m: &Mesh, topo: &Topo, t0: u32, p0: DVec3, p1: DVec3, reach: f64, h: f64, planar: bool) -> Option<Scene> {
        let len = (p1 - p0).length();
        let reach = len * reach + h * 4.0;
        let collect = |n: Option<DVec3>| {
            let flat = |v: DVec3| match n {
                Some(n) => v.reject_from(n),
                None => v,
            };
            let (a, ab) = (flat(p0), flat(p1 - p0));
            let seg = |x: DVec3| {
                let x = flat(x);
                let t = if ab.length_squared() > 1e-18 { ((x - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
                (x - (a + ab * t)).length()
            };
            let mut tris: Vec<u32> = vec![t0];
            let mut seen: HashSet<u32> = HashSet::new();
            seen.insert(t0);
            let mut k = 0;
            while k < tris.len() && tris.len() < 800_000 {
                let t = tris[k];
                k += 1;
                for &u in &topo.adj[t as usize] {
                    if u != NONE && !seen.contains(&u) && seg(m.tri_centre(u as usize)) <= reach {
                        seen.insert(u);
                        tris.push(u);
                    }
                }
            }
            tris
        };
        // the axis: the most common normal (the face, the floor), wide,
        // then narrower
        let axis = |tris: &[u32]| {
            let nrm: Vec<DVec3> = tris.iter().map(|&t| m.tri_normal(t as usize)).collect();
            let area: Vec<f64> = tris.iter().map(|&t| m.tri_area(t as usize)).collect();
            let mut n = nrm.iter().zip(&area).map(|(n, a)| *n * *a).sum::<DVec3>().normalize_or_zero();
            if n == DVec3::ZERO {
                return None;
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
            Some(n)
        };
        let mut tris = collect(None);
        let mut n = axis(&tris)?;
        if planar {
            tris = collect(Some(n));
            n = axis(&tris)?;
        }
        let o = (p0 + p1) * 0.5;
        let e = (p1 - p0).reject_from(n).normalize_or_zero();
        if e == DVec3::ZERO {
            return None;
        }
        let f = n.cross(e);
        let mut verts: Vec<u32> = tris.iter().flat_map(|&t| m.tris[t as usize]).collect();
        verts.sort_unstable();
        verts.dedup();
        let mut sc = Scene { o, e, f, n, q: vec![], hh: vec![], nz: vec![], n2: vec![], wallness: vec![], hl: (p1 - p0).reject_from(n).length() * 0.5, tris: tris.len() };
        for &v in &verts {
            let x = m.pos[v as usize] - o;
            let vn = vertex_normal(m, topo, v);
            sc.q.push(DVec2::new(x.dot(e), x.dot(f)));
            sc.hh.push(x.dot(n));
            sc.nz.push(vn.dot(n));
            let n2 = DVec2::new(vn.dot(e), vn.dot(f));
            sc.n2.push(n2);
            // how much of a wall: nothing on the flat parts (the face, the
            // floor), fully where the normal is 65° or more from the axis.
            // The AI often makes the walls sloped, a fillet into a fillet
            // with no straight part: the middle of the slope is the wall.
            sc.wallness.push(if n2.length() < 0.3 { 0.0 } else { ((0.92 - vn.dot(n).abs()) / 0.5).clamp(0.0, 1.0) });
        }
        if std::env::var("RECTIFY_SLOT_DUMP").is_ok() {
            let mut rows: std::collections::BTreeMap<i64, (f64, f64, f64, usize, f64)> = Default::default();
            for i in 0..sc.q.len() {
                if sc.q[i].x.abs() < sc.hl * 0.4 {
                    let k = (sc.q[i].y / 0.1).floor() as i64;
                    let r = rows.entry(k).or_default();
                    r.0 += sc.hh[i];
                    r.1 += sc.nz[i];
                    r.2 += sc.n2[i].y;
                    r.3 += 1;
                    r.4 += sc.wallness[i];
                }
            }
            for (k, r) in rows {
                let c = r.3 as f64;
                eprintln!("dump v {:6.2}: h {:7.3} nz {:6.3} ny {:6.3} n {:4} wall {:5.1}", k as f64 * 0.1, r.0 / c, r.1 / c, r.2 / c, r.3, r.4);
            }
        }
        Some(sc)
    }

    /// The strong peaks of weighted values (`vals`: value, weight) between
    /// `lo` and `hi`, and the histogram's bin.
    fn peaks(&self, vals: &[(f64, f64)], lo: f64, hi: f64, h: f64) -> (Vec<f64>, f64) {
        let bin = (h * 0.5).max((hi - lo) / 240.0);
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
        // local maxima (a flat top counts once), the strongest first, none
        // within 3 bins of a stronger one
        let mut cand: Vec<usize> = (1..nb - 1).filter(|&i| sm[i] > sm[i - 1] && sm[i] >= sm[i + 1] && sm[i] >= (top * 0.25).max(4.0)).collect();
        cand.sort_by(|&a, &b| sm[b].total_cmp(&sm[a]));
        let mut kept: Vec<usize> = vec![];
        for i in cand {
            if kept.iter().all(|&k| k.abs_diff(i) > 3) {
                kept.push(i);
            }
        }
        kept.sort_unstable();
        (kept.into_iter().map(|i| lo + (i as f64 + 0.5) * bin).collect(), bin)
    }

    /// The nearest strong peak of weighted values (`vals`: value, weight)
    /// between `lo` and `hi` to `want`.
    fn peak_near(&self, vals: &[(f64, f64)], lo: f64, hi: f64, want: f64, h: f64) -> Option<f64> {
        self.peaks(vals, lo, hi, h).0.into_iter().min_by(|a, b| (a - want).abs().total_cmp(&(b - want).abs()))
    }

    /// The walls facing along `dir` (a unit direction on the plane) on its
    /// side of `c`, facing back (cut in) or out (raised), whose other
    /// coordinate is within `span` of `c`'s: the nearest strong peak of
    /// their distances from `c` along `dir` to `want`.
    #[allow(clippy::too_many_arguments)]
    fn wall_along(&self, c: DVec2, dir: DVec2, span: f64, cut: bool, want: f64, hi: f64, h: f64) -> Option<f64> {
        self.walls_along(c, dir, span, cut, hi, h).into_iter().map(|w| w.at).min_by(|a, b| (a - want).abs().total_cmp(&(b - want).abs()))
    }

    /// Every strong peak of those walls' distances, with the flat levels
    /// just inside and just outside it (else the wall's own height there).
    fn walls_along(&self, c: DVec2, dir: DVec2, span: f64, cut: bool, hi: f64, h: f64) -> Vec<Wall> {
        let across = DVec2::new(-dir.y, dir.x);
        let (idx, vals): (Vec<usize>, Vec<(f64, f64)>) = (0..self.q.len())
            .filter_map(|i| {
                let d = self.q[i] - c;
                let (x, y) = (d.dot(dir), d.dot(across));
                let nn = self.n2[i];
                let k = nn.dot(dir);
                (self.wallness[i] > 0.0 && x > 0.0 && y.abs() < span && k.abs() > 0.8 * nn.length() && (k < 0.0) == cut).then_some((i, (x, self.wallness[i])))
            })
            .unzip();
        let (peaks, bin) = self.peaks(&vals, 0.0, hi, h);
        let (d1, d2) = ((h * 3.0).max(0.4), (h * 3.0).max(0.4) + (h * 6.0).max(1.0));
        peaks
            .into_iter()
            .map(|at| {
                let hs: Vec<f64> = idx.iter().zip(&vals).filter(|(_, v)| (v.0 - at).abs() < bin * 2.0 && v.1 > 0.3).map(|(&i, _)| self.hh[i]).collect();
                let own = median(hs).unwrap_or(0.0);
                let level = |from: f64, to: f64| {
                    let hs: Vec<f64> = (0..self.q.len())
                        .filter(|&i| {
                            let d = self.q[i] - c;
                            let x = d.dot(dir);
                            self.nz[i] > 0.95 && x > from && x < to && d.dot(across).abs() < span
                        })
                        .map(|i| self.hh[i])
                        .collect();
                    median(hs).unwrap_or(own)
                };
                Wall { at, inner: level((at - d2).max(0.0), at - d1), outer: level(at + d1, at + d2) }
            })
            .collect()
    }

    /// The flat face round the rounded rectangle and the flat floor (or
    /// top) inside it: the medians of their heights along the axis.
    fn levels(&self, a2: DVec2, b2: DVec2, w: f64, r: f64, h: f64) -> (Option<f64>, Option<f64>) {
        let s = r + w.min((b2 - a2).length() * 0.5);
        let nv = self.q.len();
        let flat = |i: usize| self.nz[i] > 0.95;
        let dist = |i: usize| rect_sd(self.q[i], a2, b2, w) - r;
        let ins: Vec<f64> = (0..nv).filter(|&i| flat(i) && dist(i) < -0.4 * s).map(|i| self.hh[i]).collect();
        let band = (0.45 * s).min((8.0 * h).max(1.5));
        let outs: Vec<f64> = (0..nv).filter(|&i| flat(i) && { let d = dist(i); d > 0.15 * s && d < 0.15 * s + band + 4.0 * h }).map(|i| self.hh[i]).collect();
        (median(outs), median(ins))
    }

    /// The scene with only the inner part of the wall counted: from a fifth
    /// to three fifths of the way from the floor (or top) inside to the face
    /// round it, so a groove along a pocket's rim, a chamfer there or a
    /// plate under a button (all near the face round it) does not pull the
    /// outline.
    fn mid_wall(&self, a2: DVec2, b2: DVec2, w: f64, r: f64, h: f64) -> Option<Scene> {
        let lv = self.levels(a2, b2, w, r, h);
        if debug() {
            eprintln!("mid wall: A {a2:.3?} B {b2:.3?} w {w:.3} r {r:.3}: levels {lv:.3?}");
        }
        let (Some(ho), Some(hi)) = lv else { return None };
        let d = (hi - ho).abs();
        if d < h * 4.0 {
            return None;
        }
        let (lo, top) = (hi + (ho - hi) * 0.2, hi + (ho - hi) * 0.6);
        let (lo, top) = (lo.min(top), lo.max(top));
        let mut sc = self.clone();
        for i in 0..sc.q.len() {
            if sc.hh[i] < lo || sc.hh[i] > top {
                sc.wallness[i] = 0.0;
            }
        }
        if debug() {
            eprintln!("mid wall: face {ho:.3}, inside {hi:.3}, counted from {lo:.3} to {top:.3}");
        }
        Some(sc)
    }

    /// The centre the round walls near `c0` turn round: the point nearest
    /// the lines along their normals (weighted by how much of a wall, less
    /// and less when far), so a drag a little off the middle of a round
    /// part, and the steps of a button, all give one centre.
    fn centre_of_walls(&self, c0: DVec2, reach: f64, cut: bool, h: f64) -> Option<DVec2> {
        let pts: Vec<(DVec2, DVec2, f64)> = (0..self.q.len())
            .filter_map(|i| {
                let d = self.q[i] - c0;
                let nl = self.n2[i].length();
                if self.wallness[i] <= 0.0 || d.length() > reach || nl < 1e-9 {
                    return None;
                }
                let u = self.n2[i] / nl;
                // facing the middle (cut in) or away from it
                ((u.dot(d) < 0.0) == cut).then_some((self.q[i], u, self.wallness[i]))
            })
            .collect();
        if pts.len() < 12 {
            return None;
        }
        let mut c = c0;
        for k in 0..12 {
            let sigma = (reach * 0.3 * 0.7f64.powi(k)).max(h * 2.0);
            let (mut a, mut b) = ([[0.0f64; 2]; 2], DVec2::ZERO);
            for &(q, u, w) in &pts {
                let off = (c - q).perp_dot(u);
                let w = w / (1.0 + (off / sigma).powi(2));
                // the projection across the line
                let m = [[1.0 - u.x * u.x, -u.x * u.y], [-u.x * u.y, 1.0 - u.y * u.y]];
                for i in 0..2 {
                    for j in 0..2 {
                        a[i][j] += w * m[i][j];
                    }
                }
                b += DVec2::new(m[0][0] * q.x + m[0][1] * q.y, m[1][0] * q.x + m[1][1] * q.y) * w;
            }
            let det = a[0][0] * a[1][1] - a[0][1] * a[1][0];
            if det.abs() < 1e-12 {
                return None;
            }
            let next = DVec2::new(a[1][1] * b.x - a[0][1] * b.y, a[0][0] * b.y - a[1][0] * b.x) / det;
            if (next - c0).length() > reach * 0.5 {
                return None;
            }
            c = next;
        }
        Some(c)
    }

    /// In how many of 8 sectors round `c` the wall of radius `r` stands.
    fn goes_round(&self, c: DVec2, r: f64, cut: bool, h: f64) -> usize {
        let mut sectors = [0.0f64; 8];
        for (i, _) in self.near(c, c, 0.0, r, r * 0.15 + h * 2.0, cut) {
            let d = self.q[i] - c;
            let s = ((d.y.atan2(d.x) / std::f64::consts::TAU + 1.0) * 8.0) as usize % 8;
            sectors[s] += self.wallness[i];
        }
        sectors.iter().filter(|&&x| x >= 2.0).count()
    }

    /// The median height of the flat-ish points between `lo` and `hi` from
    /// `c` (at least 8 of them).
    fn ring_level(&self, c: DVec2, lo: f64, hi: f64) -> Option<f64> {
        let hs: Vec<f64> = (0..self.q.len())
            .filter(|&i| {
                let d = (self.q[i] - c).length();
                self.nz[i] > 0.9 && d >= lo && d <= hi
            })
            .map(|i| self.hh[i])
            .collect();
        if hs.len() < 8 {
            return None;
        }
        median(hs)
    }

    /// The steps inside the round part `c`, `r`: walls facing the same way
    /// round about the same centre, going round, each smaller, with a level
    /// of their own between them (raised: each higher; cut in: each
    /// deeper). For each: its centre, its radius, the level between it and
    /// the step outside it (its base), then the innermost top or floor.
    fn round_steps(&self, c: DVec2, r: f64, base: f64, cut: bool, h: f64) -> (Vec<(DVec2, f64, f64)>, f64) {
        let way = if cut { -1.0 } else { 1.0 };
        let gap = (r * 0.12).max(h * 3.0);
        let vals: Vec<(f64, f64)> = (0..self.q.len())
            .filter_map(|i| {
                let d = self.q[i] - c;
                let l = d.length();
                let nn = self.n2[i];
                if self.wallness[i] <= 0.0 || l < 1e-9 || l > r - gap {
                    return None;
                }
                let k = nn.dot(d / l);
                (k.abs() > 0.8 * nn.length() && (k < 0.0) == cut).then_some((l, self.wallness[i]))
            })
            .collect();
        let (mut peaks, _) = self.peaks(&vals, 0.0, r, h);
        peaks.sort_by(|a, b| b.total_cmp(a));
        let band = |inner: f64, outer: f64| ((outer - inner) * 0.2).max(h * 2.0);
        let (mut cur_c, mut cur_r, mut level) = (c, r, base);
        let mut out = vec![];
        for p in peaks {
            if p > cur_r - gap || p < (h * 3.0).max(r * 0.12) {
                continue;
            }
            let (ck, _, _, rk) = self.fit(cur_c, cur_c, 0.0, p, Kind::Round, cut, h);
            if (ck - c).length() > r * 0.2 || rk > cur_r - gap || rk < (h * 3.0).max(r * 0.12) || self.goes_round(ck, rk, cut, h) < 6 {
                continue;
            }
            let b = band(rk, cur_r);
            let (Some(between), Some(inside)) = (self.ring_level(ck, rk + b, cur_r - b), self.ring_level(ck, 0.0, rk - b)) else { continue };
            // a step: the level between above (raised) or below (cut in)
            // the one outside it, the inside further on
            if (between - level) * way < h * 2.0 || (inside - between) * way < h * 2.0 {
                continue;
            }
            if debug() {
                eprintln!("round: step r {rk:.3} at {:.3},{:.3}, level outside it {between:.3}, inside {inside:.3}", ck.x, ck.y);
            }
            out.push((ck, rk, between));
            cur_c = ck;
            cur_r = rk;
            level = between;
        }
        let top = self.ring_level(cur_c, 0.0, cur_r - band(0.0, cur_r) * 0.5).unwrap_or(level);
        (out, top)
    }

    /// The vertices of the wall round the rounded rectangle (`a`, `b`, `w`,
    /// `r`) within `band` of it, facing its inside (cut in) or away: their
    /// position, their weight (how much of a wall), their distance from the
    /// inner rectangle.
    fn near(&self, a: DVec2, b: DVec2, w: f64, r: f64, band: f64, cut: bool) -> Vec<(usize, f64)> {
        (0..self.q.len())
            .filter_map(|i| {
                if self.wallness[i] <= 0.0 {
                    return None;
                }
                let (d, c) = rect_dist(self.q[i], a, b, w);
                if d < 1e-9 || (d - r).abs() >= band {
                    return None;
                }
                let k = self.n2[i].dot((self.q[i] - c) / d) / self.n2[i].length();
                (if cut { k < -0.85 } else { k > 0.85 }).then_some((i, d))
            })
            .collect()
    }

    /// The rounded rectangle fitted on its wall: the centre line's ends (a
    /// disc: its centre alone), its half width across (a box) and the
    /// radius, by Gauss-Newton on the distance to the wall's points
    /// (weighted by how much of a wall, and less and less when far).
    #[allow(clippy::too_many_arguments)]
    fn fit(&self, mut a: DVec2, mut b: DVec2, mut w: f64, mut r: f64, kind: Kind, cut: bool, h: f64) -> (DVec2, DVec2, f64, f64) {
        let free_w = kind == Kind::Box;
        let round = kind == Kind::Round;
        let np = match kind {
            Kind::Round => 3,
            Kind::Slot => 5,
            Kind::Box => 6,
        };
        // the scale of the wall's band: across a slot or a box, its half
        // width; round a disc, less than its radius (the shapes next to a
        // big ring do not pull it)
        let size = if round { r * 0.4 } else { r + w };
        let mut pts = self.near(a, b, w, r, size * 0.5, cut);
        for k in 0..14 {
            if pts.len() < 12 {
                break;
            }
            let sigma = (size * 0.15 * 0.75f64.powi(k)).max(h * 0.75);
            // a disc: [centre, r]; else [A, B, r, w]
            let res = |p: &[f64], q: DVec2| {
                if round {
                    (q - DVec2::new(p[0], p[1])).length() - p[2]
                } else {
                    rect_dist(q, DVec2::new(p[0], p[1]), DVec2::new(p[2], p[3]), if free_w { p[5] } else { w }).0 - p[4]
                }
            };
            let p0 = if round { [a.x, a.y, r, 0.0, 0.0, 0.0] } else { [a.x, a.y, b.x, b.y, r, w] };
            let mut ata = vec![vec![0.0; np]; np];
            let mut atb = vec![0.0; np];
            let step = (h * 0.01).max(1e-6);
            for &(i, _) in &pts {
                let q = self.q[i];
                let f0 = res(&p0, q);
                let wt = self.wallness[i] / (1.0 + (f0 / sigma).powi(2));
                let mut g = [0.0; 6];
                for (j, gj) in g.iter_mut().enumerate().take(np) {
                    let (mut pp, mut pm) = (p0, p0);
                    pp[j] += step;
                    pm[j] -= step;
                    *gj = (res(&pp, q) - res(&pm, q)) / (2.0 * step);
                }
                for i2 in 0..np {
                    for j in 0..np {
                        ata[i2][j] += wt * g[i2] * g[j];
                    }
                    atb[i2] -= wt * g[i2] * f0;
                }
            }
            // a little damping (a round end: its centre line's ends move
            // freely together)
            for (i2, row) in ata.iter_mut().enumerate() {
                row[i2] += 1e-6 + row[i2] * 1e-3;
            }
            let Some(dx) = solve(ata, atb) else { break };
            let big = dx.iter().fold(0.0f64, |m, x| m.max(x.abs()));
            if big > size {
                break;
            }
            if round {
                a += DVec2::new(dx[0], dx[1]);
                b = a;
                r = (r + dx[2]).max(h * 0.5);
            } else {
                a += DVec2::new(dx[0], dx[1]);
                b += DVec2::new(dx[2], dx[3]);
                r = (r + dx[4]).max(h * 0.5);
            }
            if free_w {
                w = (w + dx[5]).max(0.0);
            }
            if k == 6 {
                // the wall again, round the outline fitted so far
                let band = if round { size * 0.5 } else { (r + w) * 0.3 };
                pts = self.near(a, b, w, r, band + h * 2.0, cut);
            }
        }
        (a, b, w, r)
    }

    /// The slot or box from its outline on the plane: its depth (the flat
    /// floor inside, the face round it), the wall as read.
    #[allow(clippy::too_many_arguments)]
    fn finish(&self, mut a2: DVec2, mut b2: DVec2, w: f64, r: f64, cut: bool, kind: Kind, h: f64) -> Result<Slot, String> {
        let none = match kind {
            Kind::Box => "No block there: drag across a block with rounded corners, corner to corner",
            Kind::Slot => "No slot there: drag along a slot, from one end to the other",
            Kind::Round => "No round part there: drag across a hole or a boss, rim to rim",
        };
        if !(r > h * 0.4) || (b2 - a2).length() > self.hl * 4.0 + r * 4.0 {
            return Err(none.into());
        }
        // the centre line runs from the press towards the release
        if (b2 - a2).x < 0.0 {
            std::mem::swap(&mut a2, &mut b2);
        }
        let nv = self.q.len();
        // the size inside: from the inner rectangle's middle to the wall
        // across its narrow way
        let s = r + w.min((b2 - a2).length() * 0.5);
        let dist = |i: usize| rect_sd(self.q[i], a2, b2, w) - r;
        let (mut h_out, h_in) = self.levels(a2, b2, w, r, h);
        if kind == Kind::Round {
            // round a round part, the level just outside its wall: a cap
            // stands on its ring, not on the face round the ring
            let lo = r + (h * 2.0).max(r * 0.08);
            if let Some(v) = self.ring_level(a2, lo, lo + (h * 4.0).max(r * 0.25)) {
                h_out = Some(v);
            }
        }
        let wall_h: Vec<f64> = self.near(a2, b2, w, r, s * 0.2 + h * 2.0, cut).iter().map(|&(i, _)| self.hh[i]).collect();
        let (base, sign, depth) = match (h_out, h_in) {
            // the floor below the face for a wall facing in, the top above
            // it for a wall facing out (else what is inside is another
            // thing: a hole in a boss, a pin in a hole)
            (Some(ho), Some(hi)) if (ho - hi).abs() > h * 2.0 && (hi < ho) == cut => (ho, if hi < ho { -1.0 } else { 1.0 }, (ho - hi).abs()),
            (Some(ho), _) => {
                // no floor (through, or too narrow): the wall's far end
                let mut hs = wall_h.clone();
                hs.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let (lo, hi) = (hs.get(hs.len() / 50).copied().unwrap_or(ho), hs.get(hs.len().saturating_sub(1 + hs.len() / 50)).copied().unwrap_or(ho));
                if cut {
                    (ho, -1.0, (ho - lo) * 1.1)
                } else {
                    // raised with something sunk in its top (a ring round a
                    // hole): its highest points inside
                    let mut tops: Vec<f64> = (0..nv).filter(|&i| dist(i) < 0.0).map(|i| self.hh[i]).collect();
                    tops.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    match tops.get(tops.len().saturating_sub(1 + tops.len() / 50)) {
                        Some(&top) if top > ho => (ho, 1.0, top - ho),
                        _ => (ho, 1.0, (hi - ho) * 1.1),
                    }
                }
            }
            _ => {
                return Err(match kind {
                    Kind::Box => "No flat face round it: drag across a block standing on a face, or a pocket cut in one",
                    Kind::Slot => "No flat face round it: drag along a slot cut in a face",
                    Kind::Round => "No flat face round it: drag across a hole cut in a face, or a boss standing on one",
                }
                .into())
            }
        };
        if depth < h * 2.0 {
            return Err(none.into());
        }
        let inward = sign < 0.0;
        let axis = self.n * sign;
        let to3 = |p: DVec2| self.o + self.e * p.x + self.f * p.y + self.n * base;
        // the wall as read, for the surface: its slope included
        let win = (s * 0.5).max(h * 4.0);
        let wall: Vec<(f64, f64)> = self.near(a2, b2, w, r, win, cut).iter().map(|&(i, d)| ((self.hh[i] - base) * sign, d)).collect();
        if debug() {
            eprintln!("slot: A {:.3},{:.3} B {:.3},{:.3}, w {w:.3}, r {r:.3}, {:.3} × {:.3}, floor {h_in:?} face {h_out:?} → depth {depth:.3} ({}), {} wall points", a2.x, a2.y, b2.x, b2.y, (b2 - a2).length() + 2.0 * r, 2.0 * (w + r), if inward { "cut in" } else { "raised" }, wall.len());
        }
        Ok(Slot { a: to3(a2), b: to3(b2), w, axis, r, depth, inward, kind, tiers: vec![], wall, edge: h })
    }
}

impl Slot {
    /// Finds the slot under a drag from `p0` to `p1` (on the scan, `t0` the
    /// triangle under `p0`), from one end of the slot to the other. `h`: the
    /// scan's edge length.
    pub fn detect(m: &Mesh, topo: &Topo, t0: u32, p0: DVec3, p1: DVec3, h: f64) -> Result<Slot, String> {
        let along = "Drag along the slot, from one end to the other";
        if (p1 - p0).length() < h * 8.0 {
            return Err(along.into());
        }
        let sc = Scene::gather(m, topo, t0, p0, p1, 0.6, h).ok_or(along)?;
        let hl = sc.hl;
        // the sides: on each side of the drag's middle, the walls facing
        // across it, the nearest strong peak of their distances. Facing the
        // drag: a slot cut in; facing away: a raised pill; the pair of walls
        // nearest the drag.
        let side = |sign: f64, cut: bool| sc.wall_along(DVec2::ZERO, DVec2::new(0.0, sign), hl * 0.4, cut, 0.0, hl * 1.5, h);
        let pick = [true, false].into_iter().filter_map(|cut| Some((cut, side(1.0, cut)?, side(-1.0, cut)?))).min_by(|a, b| (a.1 + a.2).partial_cmp(&(b.1 + b.2)).unwrap());
        if debug() {
            eprintln!("slot: axis {:.3},{:.3},{:.3}, {} triangles, drag ±{hl:.3}, cut in +{:?} −{:?}, raised +{:?} −{:?}", sc.n.x, sc.n.y, sc.n.z, sc.tris, side(1.0, true), side(-1.0, true), side(1.0, false), side(-1.0, false));
        }
        let Some((cut, vp, vm)) = pick else {
            return Err("No slot there: drag along a slot, from one end to the other".into());
        };
        let r = (vp + vm) * 0.5;
        let cv = (vp - vm) * 0.5;
        // the ends: the walls facing along the drag, near its ends
        let end = |sign: f64| sc.wall_along(DVec2::new(0.0, cv), DVec2::new(sign, 0.0), r * 0.5, cut, hl, hl + r * 2.0 + h * 4.0, h);
        let up = end(1.0).unwrap_or(hl);
        let um = -end(-1.0).unwrap_or(hl);
        let (a2, b2) = if up - um > r * 2.0 { (DVec2::new(um + r, cv), DVec2::new(up - r, cv)) } else { let c = DVec2::new((up + um) * 0.5, cv); (c, c) };
        if debug() {
            eprintln!("slot: {}, sides +{vp:.3} −{vm:.3}, ends {um:.3} {up:.3}", if cut { "cut in" } else { "raised" });
        }
        let (a2, b2, _, r) = sc.fit(a2, b2, 0.0, r, Kind::Slot, cut, h);
        sc.finish(a2, b2, 0.0, r, cut, Kind::Slot, h)
    }

    /// Finds the block with rounded corners under a drag across it, corner
    /// to corner (raised: across its top; cut in: across its floor).
    pub fn detect_box(m: &Mesh, topo: &Topo, t0: u32, p0: DVec3, p1: DVec3, h: f64) -> Result<Slot, String> {
        let across = "Drag across the block, from one corner to the other";
        if (p1 - p0).length() < h * 8.0 {
            return Err(across.into());
        }
        let sc = Scene::gather_on(m, topo, t0, p0, p1, 0.75, h, true).ok_or(across)?;
        let hl = sc.hl;
        // its turn: the walls' directions, folded on a quarter turn (four
        // times the angle), the most common
        use std::f64::consts::{FRAC_PI_2, TAU};
        let nb = 90;
        let mut hist = vec![0.0f64; nb];
        for i in 0..sc.q.len() {
            if sc.wallness[i] > 0.0 && sc.q[i].length() < hl * 1.3 {
                let a = sc.n2[i].y.atan2(sc.n2[i].x).rem_euclid(FRAC_PI_2);
                hist[((a / FRAC_PI_2 * nb as f64) as usize).min(nb - 1)] += sc.wallness[i];
            }
        }
        let sm: Vec<f64> = (0..nb).map(|i| (0..5).map(|k| hist[(i + nb + k - 2) % nb]).sum()).collect();
        let top = (0..nb).max_by(|&a, &b| sm[a].partial_cmp(&sm[b]).unwrap()).unwrap_or(0);
        let a0 = (top as f64 + 0.5) / nb as f64 * FRAC_PI_2;
        // refined: the mean of four times the angle near the peak
        let mut acc = DVec2::ZERO;
        for i in 0..sc.q.len() {
            if sc.wallness[i] > 0.0 && sc.q[i].length() < hl * 1.3 {
                let a4 = sc.n2[i].y.atan2(sc.n2[i].x) * 4.0;
                let d = (a4 - a0 * 4.0 + TAU * 0.5).rem_euclid(TAU) - TAU * 0.5;
                if d.abs() < 20f64.to_radians() {
                    acc += DVec2::new(a4.cos(), a4.sin()) * sc.wallness[i];
                }
            }
        }
        let turn = if acc.length() > 1e-9 { acc.y.atan2(acc.x) / 4.0 } else { a0 };
        let ex = DVec2::new(turn.cos(), turn.sin());
        let ey = DVec2::new(-ex.y, ex.x);
        // the drag in the block's turn: its half extents, wanted for the
        // sides
        let (d0, d1) = (DVec2::new(-hl, 0.0), DVec2::new(hl, 0.0));
        let (wx, wy) = ((d1 - d0).dot(ex).abs() * 0.5, (d1 - d0).dot(ey).abs() * 0.5);
        // its four sides: for each, the strong walls nearest the drag's
        // extents (three at most); of their combinations, the one nearest
        // the drag whose four walls rise from one floor (or fall from one
        // top) to one face (a plate under a button, a groove beside a
        // pocket are walls too, between other levels). The level inside
        // counts most: it is the block's own, the face round it may change
        // from side to side.
        let wants = [wx, wx, wy, wy];
        let dirs = [ex, -ex, ey, -ey];
        let sides = |cut: bool| -> Option<[Vec<Wall>; 4]> {
            let mut out: [Vec<Wall>; 4] = Default::default();
            for k in 0..4 {
                let (want, other) = (wants[k], wants[(k + 2) % 4]);
                let span = (other * 0.5).max(want * 0.25).max(h * 4.0);
                let mut ws = sc.walls_along(DVec2::ZERO, dirs[k], span, cut, want * 1.6 + h * 8.0 + other * 0.3, h);
                ws.sort_by(|a, b| (a.at - want).abs().total_cmp(&(b.at - want).abs()));
                ws.truncate(3);
                if ws.is_empty() {
                    return None;
                }
                out[k] = ws;
            }
            Some(out)
        };
        let best = |c: &[Vec<Wall>; 4]| -> (f64, [Wall; 4]) {
            let mut best = (f64::INFINITY, [c[0][0]; 4]);
            for i0 in &c[0] {
                for i1 in &c[1] {
                    for i2 in &c[2] {
                        for i3 in &c[3] {
                            let ws = [*i0, *i1, *i2, *i3];
                            let hi = ws.iter().map(|w| w.inner).sum::<f64>() * 0.25;
                            let ho = ws.iter().map(|w| w.outer).sum::<f64>() * 0.25;
                            let score = (0..4).map(|k| (ws[k].at - wants[k]).abs() + 2.0 * (ws[k].inner - hi).abs() + 0.5 * (ws[k].outer - ho).abs()).sum::<f64>();
                            if score < best.0 {
                                best = (score, ws);
                            }
                        }
                    }
                }
            }
            best
        };
        if debug() {
            let w3 = |d: DVec2| sc.e * d.x + sc.f * d.y;
            eprintln!("box: axis {:.3},{:.3},{:.3}, {} triangles, drag ±{hl:.3}, turn {:.1}°, wants {wx:.3} {wy:.3}, sides along {:.2?}", sc.n.x, sc.n.y, sc.n.z, sc.tris, turn.to_degrees(), dirs.map(w3));
        }
        let found: Vec<(bool, f64, [Wall; 4])> = [true, false]
            .into_iter()
            .filter_map(|cut| {
                let c = sides(cut)?;
                let (score, ws) = best(&c);
                if debug() {
                    eprintln!("box: {}: candidates {:.2?}, best {:.2?} (score {score:.2})", if cut { "cut in" } else { "raised" }, c.iter().map(|v| v.iter().map(|w| (w.at, w.inner, w.outer)).collect::<Vec<_>>()).collect::<Vec<_>>(), ws.iter().map(|w| (w.at, w.inner, w.outer)).collect::<Vec<_>>());
                }
                Some((cut, score, ws))
            })
            .collect();
        let Some(&(cut, _, ws)) = found.iter().min_by(|a, b| a.1.total_cmp(&b.1)) else {
            return Err("No block there: drag across a block with rounded corners, corner to corner".into());
        };
        let s = ws.map(|w| w.at);
        let c = ex * (s[0] - s[1]) * 0.5 + ey * (s[2] - s[3]) * 0.5;
        let (hx, hy) = ((s[0] + s[1]) * 0.5, (s[2] + s[3]) * 0.5);
        let r = hx.min(hy) * 0.3;
        // the centre line along the long way
        let (long, hlong, hshort) = if hx >= hy { (ex, hx, hy) } else { (ey, hy, hx) };
        let (a2, b2) = (c - long * (hlong - r), c + long * (hlong - r));
        // fitted on the middle of the wall only
        let mid = sc.mid_wall(a2, b2, hshort - r, r, h);
        let (a2, b2, w, r) = mid.as_ref().unwrap_or(&sc).fit(a2, b2, hshort - r, r, Kind::Box, cut, h);
        if debug() {
            eprintln!("box: fitted A {a2:.3?} B {b2:.3?} w {w:.3} r {r:.3}");
        }
        sc.finish(a2, b2, w, r, cut, Kind::Box, h)
    }

    /// Finds the round part (a hole, a boss, a button, a rivet) under a drag
    /// across it, from one side of its rim to the other.
    pub fn detect_round(m: &Mesh, topo: &Topo, t0: u32, p0: DVec3, p1: DVec3, h: f64) -> Result<Slot, String> {
        let across = "Drag across the round part, from one side of its rim to the other";
        if (p1 - p0).length() < h * 6.0 {
            return Err(across.into());
        }
        let sc = Scene::gather(m, topo, t0, p0, p1, 0.75, h).ok_or(across)?;
        let hl = sc.hl;
        // the rim: the walls facing the drag's middle (a hole) or away from
        // it (a boss), the nearest strong peak of their distances to half
        // the drag; then the centre and the radius fitted on that wall
        let ring = |cut: bool| -> Option<(DVec2, f64, f64)> {
            // the distances from the centre the walls within the drag turn
            // round (a drag a little off the middle smears them round the
            // drag's middle), else from the drag's middle
            let reach = hl * 1.6 + h * 8.0;
            let est = sc.centre_of_walls(DVec2::ZERO, hl * 1.2 + h * 2.0, cut, h);
            for c0 in est.into_iter().chain([DVec2::ZERO]) {
                let vals: Vec<(f64, f64)> = (0..sc.q.len())
                    .filter_map(|i| {
                        let d = sc.q[i] - c0;
                        let l = d.length();
                        if sc.wallness[i] <= 0.0 || l < 1e-9 {
                            return None;
                        }
                        let nn = sc.n2[i];
                        let k = nn.dot(d / l);
                        (k.abs() > 0.8 * nn.length() && (k < 0.0) == cut).then_some((l, sc.wallness[i]))
                    })
                    .collect();
                let Some(r0) = sc.peak_near(&vals, 0.0, reach, hl, h) else { continue };
                let (c, _, _, r) = sc.fit(c0, c0, 0.0, r0, Kind::Round, cut, h);
                // the wall must go round it, in at least 6 of 8 sectors,
                // round about the drag's middle
                let round = sc.goes_round(c, r, cut, h);
                if debug() {
                    eprintln!("round: {}: from {:.3},{:.3}, peak {r0:.3} → centre {:.3},{:.3} r {r:.3}, wall in {round} of 8 sectors", if cut { "cut in" } else { "raised" }, c0.x, c0.y, c.x, c.y);
                }
                if round >= 6 && r > hl * 0.4 && r < hl * 2.0 && c.length() <= hl * 0.7 {
                    return Some((c, r, (r - hl).abs()));
                }
            }
            None
        };
        if debug() {
            eprintln!("round: axis {:.3},{:.3},{:.3}, {} triangles, drag ±{hl:.3}", sc.n.x, sc.n.y, sc.n.z, sc.tris);
        }
        let pick = [true, false].into_iter().filter_map(|cut| ring(cut).map(|x| (cut, x))).min_by(|a, b| a.1 .2.partial_cmp(&b.1 .2).unwrap());
        let Some((cut, (c, r, _))) = pick else {
            return Err("No round part there: drag across a hole or a boss, rim to rim".into());
        };
        let mut s = sc.finish(c, c, 0.0, r, cut, Kind::Round, h)?;
        // steps inside it (a button on its ring, a counterbore's hole)
        let base = (s.a - sc.o).dot(sc.n);
        let sign = s.axis.dot(sc.n).signum();
        let (steps, top) = sc.round_steps(c, r, base, s.inward, h);
        if !steps.is_empty() {
            let to3 = |p: DVec2| sc.o + sc.e * p.x + sc.f * p.y + sc.n * base;
            let along = |v: f64| (v - base) * sign;
            let mut tiers = vec![Tier { centre: to3(c), r, from: 0.0, to: along(steps[0].2) }];
            for (k, &(ck, rk, lv)) in steps.iter().enumerate() {
                let to = steps.get(k + 1).map(|x| x.2).unwrap_or(top);
                tiers.push(Tier { centre: to3(ck), r: rk, from: along(lv), to: along(to) });
            }
            if debug() {
                eprintln!("round: {} steps: {:?}", tiers.len(), tiers.iter().map(|t| format!("Ø {:.3} from {:.3} to {:.3}", t.r * 2.0, t.from, t.to)).collect::<Vec<_>>());
            }
            s.tiers = tiers;
        }
        Ok(s)
    }

    /// Its length and width, end to end and side to side (mm).
    pub fn length(&self) -> f64 {
        (self.b - self.a).length() + self.r * 2.0
    }

    pub fn width(&self) -> f64 {
        (self.w + self.r) * 2.0
    }

    /// The wall read between the base and `depth`: (height 0..1, distance
    /// from the inner rectangle).
    fn data(&self, depth: f64) -> Vec<(f64, f64)> {
        let inset = (self.edge * 1.5).min(depth * 0.1);
        self.wall.iter().filter(|w| w.0 >= inset && w.0 <= depth - inset).map(|w| (w.0 / depth, w.1)).collect()
    }

    /// A NURBS surface on the wall, from the base to `p.depth`: every row an
    /// exact outline round the inner rectangle.
    pub fn nurbs(&self, p: SlotParams) -> Laid {
        let depth = p.depth.max(self.edge * 2.0);
        let data = self.data(depth);
        let (profile, off) = fit_profile(&data, p.profile, p.levels, depth, self.r, self.edge);
        let (section, w, ku) = self.outline(profile.pts.iter().map(|q| q.x).sum::<f64>() / profile.pts.len() as f64);
        Laid { surface: Surface::sweep(&section, &w, ku, 2, self.axis, &profile), off, points: data.len() }
    }

    /// The simplest profile that follows the wall about as well as the
    /// others: straight, else tapered, else a curve (with its fewest
    /// levels). Each one is read for itself.
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

    /// The outline as control points, each a point of the inner rectangle
    /// and the direction out from it (the outline at offset ρ: point +
    /// direction × ρ), their weights and the knots (by length at offset
    /// `r`): degree 2, a side, a quarter circle, an end (none for a slot), a
    /// quarter circle, the other side…; starting at A's side. A slot: 13
    /// points; a box: 17; a disc (no sides, no ends): 9.
    fn outline(&self, r: f64) -> (Vec<(DVec3, DVec3)>, Vec<f64>, Vec<f64>) {
        let ab = self.b - self.a;
        let l = ab.length();
        let e = if l > 1e-9 { ab / l } else { self.axis.any_orthonormal_vector() };
        let f = self.axis.cross(e);
        let w = self.w;
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let q = std::f64::consts::FRAC_PI_2 * r.max(1e-6);
        let tiny = q * 1e-3;
        let (a0, a1, b0, b1) = (self.a - f * w, self.a + f * w, self.b - f * w, self.b + f * w);
        let mut sec = vec![(a0, -f)];
        let mut wt = vec![1.0];
        let mut lens = vec![];
        let side = |sec: &mut Vec<(DVec3, DVec3)>, wt: &mut Vec<f64>, lens: &mut Vec<f64>, to: DVec3, dir: DVec3, len: f64, skip: bool| {
            if skip {
                return;
            }
            let from = sec.last().unwrap().0;
            sec.push(((from + to) * 0.5, dir));
            sec.push((to, dir));
            wt.extend([1.0, 1.0]);
            lens.push(len.max(tiny));
        };
        let quarter = |sec: &mut Vec<(DVec3, DVec3)>, wt: &mut Vec<f64>, lens: &mut Vec<f64>, at: DVec3, corner: DVec3, end: DVec3| {
            sec.push((at, corner));
            sec.push((at, end));
            wt.extend([s, 1.0]);
            lens.push(q);
        };
        let no_ends = w < 1e-9;
        let no_sides = l < 1e-9;
        side(&mut sec, &mut wt, &mut lens, b0, -f, l, no_sides);
        quarter(&mut sec, &mut wt, &mut lens, b0, e - f, e);
        side(&mut sec, &mut wt, &mut lens, b1, e, 2.0 * w, no_ends);
        quarter(&mut sec, &mut wt, &mut lens, b1, e + f, f);
        side(&mut sec, &mut wt, &mut lens, a1, f, l, no_sides);
        quarter(&mut sec, &mut wt, &mut lens, a1, f - e, -e);
        side(&mut sec, &mut wt, &mut lens, a0, -e, 2.0 * w, no_ends);
        quarter(&mut sec, &mut wt, &mut lens, a0, -e - f, -f);
        let total: f64 = lens.iter().sum();
        let mut ku = vec![0.0; 3];
        let mut acc = 0.0;
        for li in &lens[..lens.len() - 1] {
            acc += li / total;
            ku.push(acc);
            ku.push(acc);
        }
        ku.extend([1.0; 3]);
        (sec, wt, ku)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::{uniform_knots, Curve};

    fn rows_are_outlines(s: &Slot, count: usize) {
        let prof = Curve { degree: 3, knots: uniform_knots(3, 2), pts: vec![DVec3::new(2.0, 0.0, 0.0), DVec3::new(2.2, 1.0, 0.0), DVec3::new(1.9, 2.0, 0.0), DVec3::new(2.1, 2.5, 0.0), DVec3::new(2.0, 3.0, 0.0)], w: vec![1.0; 5] };
        let (sec, w, ku) = s.outline(2.0);
        assert_eq!(sec.len(), count);
        assert_eq!(ku.len(), sec.len() + 3);
        let surf = Surface::sweep(&sec, &w, ku, 2, s.axis, &prof);
        let (a2, b2) = (DVec2::new(s.a.x, s.a.y), DVec2::new(s.b.x, s.b.y));
        for v in [0.0, 0.31, 0.5, 0.9, 1.0] {
            let p0 = surf.eval(0.0, v);
            let r0 = rect_dist(DVec2::new(p0.x, p0.y), a2, b2, s.w).0;
            for k in 0..=120 {
                let p = surf.eval(k as f64 / 120.0, v);
                assert!((p.z - p0.z).abs() < 1e-9);
                assert!((rect_dist(DVec2::new(p.x, p.y), a2, b2, s.w).0 - r0).abs() < 1e-9, "v {v} k {k}");
            }
        }
    }

    /// A height field sampled every `step` over a square of half side
    /// `half`, as a scan: two triangles per cell.
    fn height_field(half: f64, step: f64, z: impl Fn(f64, f64) -> f64) -> Mesh {
        let n = (2.0 * half / step).round() as usize + 1;
        let mut pos = vec![];
        for j in 0..n {
            for i in 0..n {
                let (x, y) = (-half + i as f64 * step, -half + j as f64 * step);
                pos.push(DVec3::new(x, y, z(x, y)));
            }
        }
        let mut tris = vec![];
        for j in 0..n - 1 {
            for i in 0..n - 1 {
                let v = (j * n + i) as u32;
                let (a, b, c, d) = (v, v + 1, v + n as u32 + 1, v + n as u32);
                tris.push([a, b, c]);
                tris.push([a, c, d]);
            }
        }
        Mesh { pos, tris, unit_mm: 1.0, ..Default::default() }
    }

    /// A rounded rectangle's signed distance (centre, half sizes, corner
    /// radius).
    fn rounded_sd(x: f64, y: f64, c: DVec2, hx: f64, hy: f64, r: f64) -> f64 {
        let (dx, dy) = ((x - c.x).abs() - (hx - r), (y - c.y).abs() - (hy - r));
        DVec2::new(dx.max(0.0), dy.max(0.0)).length() + dx.max(dy).min(0.0) - r
    }

    /// 0 outside, 1 inside, a smooth wall `width` wide across the border
    /// (the AI's walls: a fillet into a fillet).
    fn inside(sd: f64, width: f64) -> f64 {
        let t = (0.5 - sd / width).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// The Box dragged corner to corner, `margin` outside the corners
    /// (`lo`, `hi`): its size, centre, corner radius, depth.
    fn drag_box(m: &Mesh, lo: DVec2, hi: DVec2, margin: f64) -> Result<Slot, String> {
        let topo = Topo::new(m);
        let near = |p: DVec2| (0..m.tris.len()).min_by(|&a, &b| (m.tri_centre(a).truncate() - p).length().total_cmp(&(m.tri_centre(b).truncate() - p).length())).unwrap();
        let (p0, p1) = (lo - DVec2::splat(margin), hi + DVec2::splat(margin));
        let (t0, t1) = (near(p0), near(p1));
        Slot::detect_box(m, &topo, t0 as u32, m.tri_centre(t0), m.tri_centre(t1), 0.15)
    }

    fn box_size(s: &Slot) -> (f64, f64) {
        let l = (s.b - s.a).length();
        let (long, short) = (l + 2.0 * s.r, 2.0 * (s.w + s.r));
        if (s.b - s.a).x.abs() >= (s.b - s.a).y.abs() || l < 1e-9 { (long, short) } else { (short, long) }
    }

    #[test]
    fn box_on_a_plate() {
        // a button 5.8 × 5.2 (corners 1.3), 2.5 high, on a plate 1.5 high
        // whose -X and -Y edges pass 1.5 and 1.2 mm outside it: from a
        // drag past the corners, the plate's edges are nearer the drag than
        // the button's walls on two sides
        let m = height_field(14.0, 0.15, |x, y| {
            1.5 * inside(rounded_sd(x, y, DVec2::new(14.0, 14.0), 18.4, 18.0, 0.8), 0.6) + 2.5 * inside(rounded_sd(x, y, DVec2::ZERO, 2.9, 2.6, 1.3), 0.7)
        });
        for margin in [-0.5, 0.0, 0.8, 1.2, 1.8] {
            let s = drag_box(&m, DVec2::new(-2.9, -2.6), DVec2::new(2.9, 2.6), margin).unwrap_or_else(|e| panic!("margin {margin}: {e}"));
            let (sx, sy) = box_size(&s);
            let c = (s.a + s.b) * 0.5;
            assert!((sx - 5.8).abs() < 0.12 && (sy - 5.2).abs() < 0.12, "margin {margin}: {sx:.3} × {sy:.3}");
            assert!(c.truncate().length() < 0.06, "margin {margin}: centre {c:.3?}");
            assert!(!s.inward && (s.depth - 2.5).abs() < 0.1, "margin {margin}: depth {:.3}", s.depth);
        }
    }

    #[test]
    fn pocket_with_a_groove() {
        // a pocket 3.6 × 4 (corners 0.5), 3.3 deep, and a rounded groove
        // 1.2 deep, 2 wide, running along Y across its +X side
        let m = height_field(10.0, 0.15, |x, y| {
            let pocket = -3.3 * inside(rounded_sd(x, y, DVec2::ZERO, 1.8, 2.0, 0.5), 0.5);
            let t = ((x - 1.9) / 1.0).abs();
            let groove = if t < 1.0 { -1.2 * (1.0 - t * t) } else { 0.0 };
            pocket.min(groove)
        });
        for margin in [-0.5, 0.0, 0.4, 0.8, 1.2] {
            let s = drag_box(&m, DVec2::new(-1.8, -2.0), DVec2::new(1.8, 2.0), margin).unwrap_or_else(|e| panic!("margin {margin}: {e}"));
            let (sx, sy) = box_size(&s);
            let c = (s.a + s.b) * 0.5;
            assert!((sx - 3.6).abs() < 0.15 && (sy - 4.0).abs() < 0.15, "margin {margin}: {sx:.3} × {sy:.3}");
            assert!(c.truncate().length() < 0.08, "margin {margin}: centre {c:.3?}");
            assert!(s.inward && (s.depth - 3.3).abs() < 0.12, "margin {margin}: depth {:.3}", s.depth);
        }
    }

    /// The Circle dragged across from `a` to `b`.
    fn drag_round(m: &Mesh, a: DVec2, b: DVec2) -> Result<Slot, String> {
        let topo = Topo::new(m);
        let near = |p: DVec2| (0..m.tris.len()).min_by(|&x, &y| (m.tri_centre(x).truncate() - p).length().total_cmp(&(m.tri_centre(y).truncate() - p).length())).unwrap();
        let (t0, t1) = (near(a), near(b));
        Slot::detect_round(m, &topo, t0 as u32, m.tri_centre(t0), m.tri_centre(t1), 0.15)
    }

    #[test]
    fn stepped_button() {
        // a ring Ø 10, 1.3 high, and on it a cap Ø 7, 2.2 higher, both
        // walls sloped (a fillet into a fillet)
        let disc = |x: f64, y: f64, r: f64| DVec2::new(x, y).length() - r;
        let m = height_field(9.0, 0.15, |x, y| 1.3 * inside(disc(x, y, 5.0), 0.6) + 2.2 * inside(disc(x, y, 3.5), 0.6));
        // rim to rim of the ring, through the middle, then 1 mm off it
        for (a, b) in [(DVec2::new(-5.3, 0.0), DVec2::new(5.3, 0.0)), (DVec2::new(-5.2, 1.0), DVec2::new(5.2, 1.0))] {
            let s = drag_round(&m, a, b).unwrap();
            assert_eq!(s.tiers.len(), 2, "{a} {b}: {:?}", s.tiers);
            let (t0, t1) = (&s.tiers[0], &s.tiers[1]);
            assert!((t0.r - 5.0).abs() < 0.1 && (t1.r - 3.5).abs() < 0.1, "{a} {b}: radii {:.3} {:.3}", t0.r, t1.r);
            assert!(t0.from.abs() < 1e-9 && (t0.to - 1.3).abs() < 0.1 && (t1.from - 1.3).abs() < 0.1 && (t1.to - 3.5).abs() < 0.1, "{a} {b}: {:?}", s.tiers);
            assert!(t1.centre.truncate().length() < 0.05, "{a} {b}: centre {:.3?}", t1.centre);
        }
        // across the cap only: the cap, standing on the ring
        let s = drag_round(&m, DVec2::new(-3.8, 0.0), DVec2::new(3.8, 0.0)).unwrap();
        assert!(s.tiers.is_empty() && (s.r - 3.5).abs() < 0.1 && (s.depth - 2.2).abs() < 0.1, "cap: r {:.3} depth {:.3} {:?}", s.r, s.depth, s.tiers);
    }

    #[test]
    fn counterbore() {
        // a hole Ø 8, 1 deep, then Ø 4, 3 deeper
        let disc = |x: f64, y: f64, r: f64| DVec2::new(x, y).length() - r;
        let m = height_field(8.0, 0.15, |x, y| -1.0 * inside(disc(x, y, 4.0), 0.4) - 3.0 * inside(disc(x, y, 2.0), 0.4));
        let s = drag_round(&m, DVec2::new(-4.2, 0.3), DVec2::new(4.2, 0.3)).unwrap();
        assert!(s.inward && s.tiers.len() == 2, "{:?}", s.tiers);
        let (t0, t1) = (&s.tiers[0], &s.tiers[1]);
        assert!((t0.r - 4.0).abs() < 0.1 && (t1.r - 2.0).abs() < 0.1, "radii {:.3} {:.3}", t0.r, t1.r);
        assert!((t0.to - 1.0).abs() < 0.1 && (t1.from - 1.0).abs() < 0.1 && (t1.to - 4.0).abs() < 0.1, "{:?}", s.tiers);
        // a plain hole: one step
        let m = height_field(8.0, 0.15, |x, y| -3.0 * inside(disc(x, y, 3.0), 0.4));
        let s = drag_round(&m, DVec2::new(-3.2, 0.0), DVec2::new(3.2, 0.0)).unwrap();
        assert!(s.inward && s.tiers.is_empty() && (s.r - 3.0).abs() < 0.1 && (s.depth - 3.0).abs() < 0.1, "hole: r {:.3} depth {:.3} {:?}", s.r, s.depth, s.tiers);
    }

    #[test]
    fn every_row_is_a_slot_outline() {
        let s = Slot { a: DVec3::new(1.0, 2.0, 0.0), b: DVec3::new(7.0, 4.0, 0.0), w: 0.0, axis: DVec3::new(0.0, 0.0, -1.0), r: 2.0, depth: 3.0, inward: true, kind: Kind::Slot, tiers: vec![], wall: vec![], edge: 0.1 };
        rows_are_outlines(&s, 13);
    }

    #[test]
    fn every_row_is_a_box_outline() {
        let s = Slot { a: DVec3::new(1.0, 2.0, 0.0), b: DVec3::new(7.0, 4.0, 0.0), w: 1.5, axis: DVec3::new(0.0, 0.0, 1.0), r: 0.8, depth: 3.0, inward: false, kind: Kind::Box, tiers: vec![], wall: vec![], edge: 0.1 };
        rows_are_outlines(&s, 17);
    }

    #[test]
    fn every_row_is_a_circle() {
        let c = DVec3::new(3.0, -1.0, 0.0);
        let s = Slot { a: c, b: c, w: 0.0, axis: DVec3::new(0.0, 0.0, 1.0), r: 1.3, depth: 2.0, inward: false, kind: Kind::Round, tiers: vec![], wall: vec![], edge: 0.1 };
        rows_are_outlines(&s, 9);
    }
}
