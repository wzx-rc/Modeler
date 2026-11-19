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
    /// The wall as read: (height along the axis from the base, distance
    /// from the inner rectangle) of its vertices.
    wall: Vec<(f64, f64)>,
    edge: f64,
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

impl Scene {
    /// The triangles joined to the one pressed within `reach` × the drag's
    /// length of it; the axis by mean shift of their normals.
    fn gather(m: &Mesh, topo: &Topo, t0: u32, p0: DVec3, p1: DVec3, reach: f64, h: f64) -> Option<Scene> {
        let len = (p1 - p0).length();
        let reach = len * reach + h * 4.0;
        let seg3 = |x: DVec3| {
            let ab = p1 - p0;
            let t = ((x - p0).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
            (x - (p0 + ab * t)).length()
        };
        let mut tris: Vec<u32> = vec![t0];
        let mut seen: HashSet<u32> = HashSet::new();
        seen.insert(t0);
        let mut k = 0;
        while k < tris.len() && tris.len() < 800_000 {
            let t = tris[k];
            k += 1;
            for &u in &topo.adj[t as usize] {
                if u != NONE && !seen.contains(&u) && seg3(m.tri_centre(u as usize)) <= reach {
                    seen.insert(u);
                    tris.push(u);
                }
            }
        }
        // the axis: the most common normal (the face, the floor), wide,
        // then narrower
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

    /// The nearest strong peak of weighted values (`vals`: value, weight)
    /// between `lo` and `hi` to `want`.
    fn peak_near(&self, vals: &[(f64, f64)], lo: f64, hi: f64, want: f64, h: f64) -> Option<f64> {
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
        (1..nb - 1)
            .filter(|&i| sm[i] >= sm[i - 1] && sm[i] >= sm[i + 1] && sm[i] >= (top * 0.25).max(4.0))
            .map(|i| lo + (i as f64 + 0.5) * bin)
            .min_by(|a, b| (a - want).abs().partial_cmp(&(b - want).abs()).unwrap())
    }

    /// The walls facing along `dir` (a unit direction on the plane) on its
    /// side of `c`, facing back (cut in) or out (raised), whose other
    /// coordinate is within `span` of `c`'s: the nearest strong peak of
    /// their distances from `c` along `dir` to `want`.
    fn wall_along(&self, c: DVec2, dir: DVec2, span: f64, cut: bool, want: f64, hi: f64, h: f64) -> Option<f64> {
        let across = DVec2::new(-dir.y, dir.x);
        let vals: Vec<(f64, f64)> = (0..self.q.len())
            .filter_map(|i| {
                let d = self.q[i] - c;
                let (x, y) = (d.dot(dir), d.dot(across));
                let nn = self.n2[i];
                let k = nn.dot(dir);
                (self.wallness[i] > 0.0 && x > 0.0 && y.abs() < span && k.abs() > 0.8 * nn.length() && (k < 0.0) == cut).then_some((x, self.wallness[i]))
            })
            .collect();
        self.peak_near(&vals, 0.0, hi, want, h)
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
        let flat = |i: usize| self.nz[i] > 0.95;
        let dist = |i: usize| rect_dist(self.q[i], a2, b2, w).0 - r;
        let ins: Vec<f64> = (0..nv).filter(|&i| flat(i) && dist(i) < -0.4 * s).map(|i| self.hh[i]).collect();
        let band = (0.45 * s).min((8.0 * h).max(1.5));
        let outs: Vec<f64> = (0..nv).filter(|&i| flat(i) && { let d = dist(i); d > 0.15 * s && d < 0.15 * s + band + 4.0 * h }).map(|i| self.hh[i]).collect();
        let (h_out, h_in) = (median(outs.clone()), median(ins.clone()));
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
            eprintln!("slot: A {:.3},{:.3} B {:.3},{:.3}, w {w:.3}, r {r:.3}, {:.3} × {:.3}, floor {h_in:?} face {h_out:?} → depth {depth:.3} ({}), {} wall points ({} inside, {} round)", a2.x, a2.y, b2.x, b2.y, (b2 - a2).length() + 2.0 * r, 2.0 * (w + r), if inward { "cut in" } else { "raised" }, wall.len(), ins.len(), outs.len());
        }
        Ok(Slot { a: to3(a2), b: to3(b2), w, axis, r, depth, inward, kind, wall, edge: h })
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
        let sc = Scene::gather(m, topo, t0, p0, p1, 0.75, h).ok_or(across)?;
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
        let sides = |cut: bool| -> Option<[f64; 4]> {
            let mut out = [0.0; 4];
            for (k, (dir, want, other)) in [(ex, wx, wy), (-ex, wx, wy), (ey, wy, wx), (-ey, wy, wx)].into_iter().enumerate() {
                let span = (other * 0.5).max(want * 0.25).max(h * 4.0);
                out[k] = sc.wall_along(DVec2::ZERO, dir, span, cut, want, want * 1.6 + h * 8.0 + other * 0.3, h)?;
            }
            Some(out)
        };
        let score = |s: &[f64; 4]| (s[0] - wx).abs() + (s[1] - wx).abs() + (s[2] - wy).abs() + (s[3] - wy).abs();
        let pick = [true, false].into_iter().filter_map(|cut| Some((cut, sides(cut)?))).min_by(|a, b| score(&a.1).partial_cmp(&score(&b.1)).unwrap());
        if debug() {
            eprintln!("box: axis {:.3},{:.3},{:.3}, {} triangles, drag ±{hl:.3}, turn {:.1}°, wants {wx:.3} {wy:.3}, cut in {:?}, raised {:?}", sc.n.x, sc.n.y, sc.n.z, sc.tris, turn.to_degrees(), sides(true), sides(false));
        }
        let Some((cut, s)) = pick else {
            return Err("No block there: drag across a block with rounded corners, corner to corner".into());
        };
        let c = ex * (s[0] - s[1]) * 0.5 + ey * (s[2] - s[3]) * 0.5;
        let (hx, hy) = ((s[0] + s[1]) * 0.5, (s[2] + s[3]) * 0.5);
        let r = hx.min(hy) * 0.3;
        // the centre line along the long way
        let (long, hlong, hshort) = if hx >= hy { (ex, hx, hy) } else { (ey, hy, hx) };
        let (a2, b2) = (c - long * (hlong - r), c + long * (hlong - r));
        let (a2, b2, w, r) = sc.fit(a2, b2, hshort - r, r, Kind::Box, cut, h);
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
            let vals: Vec<(f64, f64)> = (0..sc.q.len())
                .filter_map(|i| {
                    let l = sc.q[i].length();
                    if sc.wallness[i] <= 0.0 || l < 1e-9 {
                        return None;
                    }
                    let nn = sc.n2[i];
                    let k = nn.dot(sc.q[i] / l);
                    (k.abs() > 0.8 * nn.length() && (k < 0.0) == cut).then_some((l, sc.wallness[i]))
                })
                .collect();
            let r0 = sc.peak_near(&vals, 0.0, hl * 1.6 + h * 8.0, hl, h)?;
            let (c, _, _, r) = sc.fit(DVec2::ZERO, DVec2::ZERO, 0.0, r0, Kind::Round, cut, h);
            // the wall must go round it: in at least 6 of 8 sectors
            let mut sectors = [0.0f64; 8];
            for (i, _) in sc.near(c, c, 0.0, r, r * 0.15 + h * 2.0, cut) {
                let d = sc.q[i] - c;
                let s = ((d.y.atan2(d.x) / std::f64::consts::TAU + 1.0) * 8.0) as usize % 8;
                sectors[s] += sc.wallness[i];
            }
            let round = sectors.iter().filter(|&&x| x >= 2.0).count();
            if debug() {
                eprintln!("round: {}: peak {r0:.3} → centre {:.3},{:.3} r {r:.3}, wall in {round} of 8 sectors {sectors:.0?}", if cut { "cut in" } else { "raised" }, c.x, c.y);
            }
            (round >= 6 && r > hl * 0.4 && r < hl * 2.0).then_some((c, r, (r - hl).abs()))
        };
        if debug() {
            eprintln!("round: axis {:.3},{:.3},{:.3}, {} triangles, drag ±{hl:.3}", sc.n.x, sc.n.y, sc.n.z, sc.tris);
        }
        let pick = [true, false].into_iter().filter_map(|cut| ring(cut).map(|x| (cut, x))).min_by(|a, b| a.1 .2.partial_cmp(&b.1 .2).unwrap());
        let Some((cut, (c, r, _))) = pick else {
            return Err("No round part there: drag across a hole or a boss, rim to rim".into());
        };
        sc.finish(c, c, 0.0, r, cut, Kind::Round, h)
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

    #[test]
    fn every_row_is_a_slot_outline() {
        let s = Slot { a: DVec3::new(1.0, 2.0, 0.0), b: DVec3::new(7.0, 4.0, 0.0), w: 0.0, axis: DVec3::new(0.0, 0.0, -1.0), r: 2.0, depth: 3.0, inward: true, kind: Kind::Slot, wall: vec![], edge: 0.1 };
        rows_are_outlines(&s, 13);
    }

    #[test]
    fn every_row_is_a_box_outline() {
        let s = Slot { a: DVec3::new(1.0, 2.0, 0.0), b: DVec3::new(7.0, 4.0, 0.0), w: 1.5, axis: DVec3::new(0.0, 0.0, 1.0), r: 0.8, depth: 3.0, inward: false, kind: Kind::Box, wall: vec![], edge: 0.1 };
        rows_are_outlines(&s, 17);
    }

    #[test]
    fn every_row_is_a_circle() {
        let c = DVec3::new(3.0, -1.0, 0.0);
        let s = Slot { a: c, b: c, w: 0.0, axis: DVec3::new(0.0, 0.0, 1.0), r: 1.3, depth: 2.0, inward: false, kind: Kind::Round, wall: vec![], edge: 0.1 };
        rows_are_outlines(&s, 9);
    }
}
