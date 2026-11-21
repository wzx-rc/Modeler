//! A fillet's radius read on the scan (Forme NURBS retopo, step 6): the
//! scan cut in slices across a sharp edge of the part; in each slice the
//! scan's two faces (parallel to the part's, at their own offsets) and the
//! arc tangent to both are fitted; the radius is the slices' median, their
//! spread says whether it is a constant fillet.
//!
//! A slice keeps the scan's vertices within a window round the edge whose
//! normals turn from one face's to the other's (30° either side), so the
//! back of a thin wall or another feature's wall does not count. The
//! window starts at 24 grains (at least 3 mm) and grows when the arc found
//! fills more than 55 % of it (to 1.8 times the arc's run along a face),
//! never past 0.9 of either face's reach.

use crate::bvh::Bvh;
use crate::mesh::{Mesh, Topo};
use glam::DVec3;
use std::collections::HashSet;

/// A point of the part's sharp edge and its two faces there.
#[derive(Clone, Copy, Debug)]
pub struct EdgeSample {
    pub p: DVec3,
    /// The edge's direction there.
    pub t: DVec3,
    /// Each face's outward normal there.
    pub n: [DVec3; 2],
    /// Each face's direction away from the edge (square to it).
    pub d: [DVec3; 2],
    /// How far each face runs from the edge (mm).
    pub reach: [f64; 2],
}

/// One slice's reading.
#[derive(Clone, Debug)]
pub struct Slice {
    /// The part's edge point it was taken at.
    pub p: DVec3,
    pub r: f64,
    /// The scan's faces' offsets from the part's, along their normals.
    pub off: [f64; 2],
    pub centre: DVec3,
    /// Where the arc meets the two faces.
    pub ends: [DVec3; 2],
    /// How far each scan face turns from the part's (degrees).
    pub turn: [f64; 2],
    /// The scan's distance to the fitted profile (RMS, mm).
    pub rms: f64,
    pub points: usize,
    pub on_arc: usize,
}

impl Slice {
    /// Points along the arc, `n` + 1 of them.
    pub fn arc(&self, n: usize) -> Vec<DVec3> {
        let a = self.ends[0] - self.centre;
        let b = self.ends[1] - self.centre;
        let phi = (a.dot(b) / (a.length() * b.length()).max(1e-12)).clamp(-1.0, 1.0).acos();
        if phi < 1e-6 {
            return vec![self.ends[0], self.ends[1]];
        }
        (0..=n)
            .map(|i| {
                let s = i as f64 / n as f64;
                self.centre + (a * ((1.0 - s) * phi).sin() + b * (s * phi).sin()) / phi.sin()
            })
            .collect()
    }
}

/// What the scan says along the edge.
#[derive(Clone, Debug)]
pub struct FilletRead {
    /// The slices' median radius.
    pub r: f64,
    /// The smallest and largest radius kept.
    pub lo: f64,
    pub hi: f64,
    /// A convex edge (the fillet takes material away), else a concave one.
    pub convex: bool,
    /// The slices kept, in the samples' order.
    pub slices: Vec<Slice>,
    /// How many slices were tried.
    pub tried: usize,
}

impl FilletRead {
    /// The radius changes along the edge more than a constant fillet's
    /// would on a scan.
    pub fn varies(&self) -> bool {
        self.hi - self.lo > (0.25 * self.r).max(0.1)
    }
}

fn debug() -> bool {
    std::env::var("FORME_SCAN_DEBUG").is_ok()
}

/// Reads the scan across the edge at each sample. `h`: the scan's median
/// edge length (its grain).
pub fn read(m: &Mesh, topo: &Topo, bvh: &Bvh, samples: &[EdgeSample], h: f64) -> Result<FilletRead, String> {
    if samples.is_empty() {
        return Err("no edge to read".into());
    }
    let mut kept: Vec<Slice> = vec![];
    let mut why: Vec<String> = vec![];
    let mut convex = 0i32;
    for s in samples {
        let c = is_convex(s);
        convex += if c { 1 } else { -1 };
        match read_slice(m, topo, bvh, s, h, c) {
            Ok(sl) => {
                if debug() {
                    eprintln!("fillet slice at {:.2?}: r {:.3} off {:.3?} turn {:.1?}° rms {:.3} points {} on arc {}", s.p, sl.r, sl.off, sl.turn, sl.rms, sl.points, sl.on_arc);
                }
                kept.push(sl)
            }
            Err(e) => {
                if debug() {
                    eprintln!("fillet slice at {:.2?}: {e}", s.p);
                }
                why.push(e)
            }
        }
    }
    let need = 3.max(samples.len().div_ceil(3)).min(samples.len());
    let fail = |why: &[String]| -> String {
        // the reason given most often
        let mut best = ("the scan does not show a fillet along this edge".to_string(), 0);
        for w in why {
            let n = why.iter().filter(|x| *x == w).count();
            if n > best.1 {
                best = (w.clone(), n);
            }
        }
        best.0
    };
    if kept.len() < need {
        return Err(fail(&why));
    }
    let median = |v: &[f64]| {
        let mut v = v.to_vec();
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let med = median(&kept.iter().map(|s| s.r).collect::<Vec<_>>());
    // slices far from the others (a vent or a rivet crossing the edge there)
    let tol = (0.3 * med).max(2.0 * h);
    kept.retain(|s| (s.r - med).abs() <= tol);
    if kept.len() < need {
        return Err("the slices disagree: the radius is not steady along this edge".into());
    }
    let rs: Vec<f64> = kept.iter().map(|s| s.r).collect();
    let r = median(&rs);
    if r < 1.5 * h {
        return Err(format!("sharp on the scan: no fillet here (under {:.2} mm)", 1.5 * h));
    }
    Ok(FilletRead { r, lo: rs.iter().copied().fold(f64::MAX, f64::min), hi: rs.iter().copied().fold(0.0, f64::max), convex: convex >= 0, slices: kept, tried: samples.len() })
}

/// A convex edge: each face runs to the inner side of the other's plane.
fn is_convex(s: &EdgeSample) -> bool {
    s.d[0].dot(s.n[1]) + s.d[1].dot(s.n[0]) < 0.0
}

/// The slice's frame: the normals and directions made square to the edge.
fn frame(s: &EdgeSample) -> Option<(DVec3, [DVec3; 2], [DVec3; 2])> {
    let t = s.t.try_normalize()?;
    let flat = |v: DVec3| (v - t * v.dot(t)).try_normalize();
    let n = [flat(s.n[0])?, flat(s.n[1])?];
    let d = [flat(s.d[0] - n[0] * s.d[0].dot(n[0]))?, flat(s.d[1] - n[1] * s.d[1].dot(n[1]))?];
    Some((t, n, d))
}

fn read_slice(m: &Mesh, topo: &Topo, bvh: &Bvh, s: &EdgeSample, h: f64, convex: bool) -> Result<Slice, String> {
    let Some((t, n, _)) = frame(s) else { return Err("the edge's faces are not clear here".into()) };
    if n[0].dot(n[1]) > 0.985 {
        return Err("the faces are nearly flush: no fillet to read".into());
    }
    let wmax = s.reach[0].min(s.reach[1]) * 0.9;
    if wmax < 4.0 * h {
        return Err("the faces are too narrow here to read the scan".into());
    }
    let mut w = wmax.min((24.0 * h).max(3.0));
    loop {
        let pts = gather(m, topo, bvh, s.p, t, n, w, h);
        if pts.len() < 12 {
            return Err("the scan is not along this edge".into());
        }
        let f = match fit(&pts, n, convex, h, w) {
            // the window holds only the fillet: wider
            Err(e) if e == NO_FACES && w < wmax - 1e-9 => {
                w = wmax.min(w * 2.0);
                continue;
            }
            r => r?,
        };
        // how far the arc runs along each face from the edge: the window
        // must see the faces flat past it
        let tl = f.r * (n[0].dot(n[1]).clamp(-1.0, 1.0).acos() * 0.5).tan();
        if tl > 0.55 * w && w < wmax - 1e-9 {
            w = wmax.min(1.8 * tl + 4.0 * h);
            continue;
        }
        if tl + (3.0 * h).max(0.15 * tl) > 1.2 * w {
            return Err("the fillet is wider than the faces round it".into());
        }
        if f.o[0].abs().max(f.o[1].abs()) > (0.5 * w).max(1.0) {
            return Err("the scan's faces are far from the part's here".into());
        }
        return Ok(Slice { p: s.p, r: f.r, off: f.o, centre: s.p + f.centre, ends: [s.p + f.ends[0], s.p + f.ends[1]], turn: [f.turn[0].to_degrees(), f.turn[1].to_degrees()], rms: f.rms, points: pts.len(), on_arc: f.on_arc });
    }
}

/// The scan's vertices in a slab across the edge at `p`, within `w` of it,
/// in the slice's plane (relative to `p`), with their normals; only those
/// whose normal turns between the two faces'.
fn gather(m: &Mesh, topo: &Topo, bvh: &Bvh, p: DVec3, t: DVec3, n: [DVec3; 2], w: f64, h: f64) -> Vec<(DVec3, DVec3)> {
    let Some((tri, _)) = bvh.closest(m, p, w) else { return vec![] };
    let slab = (0.12 * w).clamp(2.0 * h, 0.8);
    let reach = w * 1.25;
    let theta = n[0].dot(n[1]).clamp(-1.0, 1.0).acos();
    let slack = 30f64.to_radians();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut stack: Vec<u32> = m.tris[tri as usize].to_vec();
    for &v in &stack {
        seen.insert(v);
    }
    let mut out = vec![];
    while let Some(v) = stack.pop() {
        let x = m.pos[v as usize] - p;
        if x.length() > reach {
            continue;
        }
        for &u in topo.verts_of(v as usize) {
            if seen.insert(u) {
                stack.push(u);
            }
        }
        let along = x.dot(t);
        if along.abs() > slab {
            continue;
        }
        let nv = topo.tris_of(v as usize).iter().map(|&k| m.tri_cross(k as usize)).sum::<DVec3>();
        let Some(nv) = nv.try_normalize() else { continue };
        if nv.dot(t).abs() > 0.7 {
            continue;
        }
        let nf = (nv - t * nv.dot(t)).normalize();
        let a = nf.dot(n[0]).clamp(-1.0, 1.0).acos();
        let b = nf.dot(n[1]).clamp(-1.0, 1.0).acos();
        if a + b - theta > 2.0 * slack {
            continue;
        }
        out.push((x - t * along, nf));
    }
    out
}

const NO_FACES: &str = "the scan's faces are not found next to this edge";

struct Fit {
    o: [f64; 2],
    /// Each scan face's turn from the part's (radians).
    turn: [f64; 2],
    r: f64,
    centre: DVec3,
    ends: [DVec3; 2],
    rms: f64,
    on_arc: usize,
}

/// The two faces (their offsets and a small turn of each in the slice) and
/// the radius that fit the slice's points best (robust Levenberg-Marquardt,
/// Cauchy weights). The part's faces may be turned a few degrees from the
/// scan's (a plane taken as a world axis within 5°).
fn fit(pts: &[(DVec3, DVec3)], n: [DVec3; 2], convex: bool, h: f64, w: f64) -> Result<Fit, String> {
    let g0 = n[0].dot(n[1]);
    // each face's turn in the slice: towards the other's normal
    let m = [(n[1] - n[0] * g0).normalize(), (n[0] - n[1] * g0).normalize()];
    let turn = |k: usize, a: f64| n[k] * a.cos() + m[k] * a.sin();
    let sgn = if convex { -1.0 } else { 1.0 };
    const MAX_TURN: f64 = 0.42;
    // the distance from q to the profile (two lines and the arc tangent to
    // both), and whether q faces the arc; x = [o0, o1, a0, a1, r]
    struct Prof {
        n: [DVec3; 2],
        g: f64,
        den: f64,
    }
    impl Prof {
        fn at(&self, a: f64, b: f64) -> DVec3 {
            self.n[0] * ((a - self.g * b) / self.den) + self.n[1] * ((b - self.g * a) / self.den)
        }
    }
    let prof = |x: &[f64; 5]| {
        let nn = [turn(0, x[2]), turn(1, x[3])];
        let g = nn[0].dot(nn[1]);
        Prof { n: nn, g, den: (1.0 - g * g).max(1e-9) }
    };
    let res = |pf: &Prof, q: DVec3, x: &[f64; 5]| -> (f64, bool) {
        let (o, r) = ([x[0], x[1]], x[4]);
        let h0 = pf.n[0].dot(q) - o[0];
        let h1 = pf.n[1].dot(q) - o[1];
        let c = pf.at(o[0] + sgn * r, o[1] + sgn * r);
        let u = q - c;
        let (un0, un1) = (u.dot(pf.n[0]), u.dot(pf.n[1]));
        let (l, mu) = ((un0 - pf.g * un1) / pf.den, (un1 - pf.g * un0) / pf.den);
        if convex {
            if l >= 0.0 && mu >= 0.0 {
                (u.length() - r, true)
            } else {
                (h0.max(h1), false)
            }
        } else if l <= 0.0 && mu <= 0.0 {
            (r - u.length(), true)
        } else {
            (h0.min(h1), false)
        }
    };
    let median = |mut v: Vec<f64>| -> Option<f64> {
        if v.len() < 5 {
            return None;
        }
        v.sort_by(f64::total_cmp);
        Some(v[v.len() / 2])
    };
    // the faces: the points facing like them, their mean normal and offset
    // (the scan's face may lean from the part's: its normal found by mean
    // shift from the part's, in narrowing windows)
    let face = |k: usize| -> Option<(f64, f64)> {
        let mut mean = n[k];
        for win in [20f64, 12.0, 7.0] {
            let c = win.to_radians().cos();
            mean = pts.iter().filter(|(_, nq)| nq.dot(mean) > c).map(|(_, nq)| *nq).sum::<DVec3>().try_normalize()?;
        }
        let near: Vec<&(DVec3, DVec3)> = pts.iter().filter(|(_, nq)| nq.dot(mean) > 7f64.to_radians().cos()).collect();
        let a = mean.dot(m[k]).atan2(mean.dot(n[k])).clamp(-MAX_TURN, MAX_TURN);
        let nk = turn(k, a);
        Some((median(near.iter().map(|(q, _)| q.dot(nk)).collect())?, a))
    };
    let (Some((o0, a0)), Some((o1, a1))) = (face(0), face(1)) else { return Err(NO_FACES.into()) };
    // the radius from the gap between their corner and the scan, along the
    // bisector
    let mut x = [o0, o1, a0, a1, 0.0];
    let pf = prof(&x);
    let b = (pf.n[0] + pf.n[1]).normalize();
    let corner = pf.at(o0, o1);
    let half = ((1.0 + pf.g) * 0.5).sqrt();
    let gap = median(pts.iter().filter(|(_, nq)| nq.dot(b) > 10f64.to_radians().cos()).map(|(q, _)| -sgn * (corner - *q).dot(b)).collect::<Vec<_>>()).unwrap_or(0.0);
    x[4] = (gap.max(0.0) / (1.0 / half - 1.0)).min(0.4 * w);
    let c = (1.5 * h).max(0.06);
    let clampx = |y: [f64; 5]| [y[0], y[1], y[2].clamp(-MAX_TURN, MAX_TURN), y[3].clamp(-MAX_TURN, MAX_TURN), y[4].max(0.0)];
    let cost = |x: &[f64; 5]| -> f64 {
        let pf = prof(x);
        pts.iter()
            .map(|(q, _)| {
                let e = res(&pf, *q, x).0;
                (1.0 + (e / c).powi(2)).ln()
            })
            .sum()
    };
    let mut cur = cost(&x);
    let mut lambda = 1e-3;
    for _ in 0..60 {
        let mut a = [[0.0f64; 5]; 5];
        let mut rhs = [0.0f64; 5];
        let pf = prof(&x);
        let pfs: Vec<([f64; 5], Prof)> = (0..5)
            .map(|k| {
                let mut y = x;
                y[k] += 1e-6;
                (y, prof(&y))
            })
            .collect();
        for (q, _) in pts {
            let e = res(&pf, *q, &x).0;
            let wgt = 1.0 / (1.0 + (e / c).powi(2));
            let mut j = [0.0; 5];
            for k in 0..5 {
                j[k] = (res(&pfs[k].1, *q, &pfs[k].0).0 - e) / 1e-6;
            }
            for i in 0..5 {
                rhs[i] -= wgt * j[i] * e;
                for k in 0..5 {
                    a[i][k] += wgt * j[i] * j[k];
                }
            }
        }
        let mut step = None;
        for _ in 0..8 {
            let mut aa = a;
            for i in 0..5 {
                aa[i][i] += lambda * a[i][i].max(1e-9);
            }
            let Some(dx) = solve_n(aa, rhs) else {
                lambda *= 10.0;
                continue;
            };
            let y = clampx([x[0] + dx[0], x[1] + dx[1], x[2] + dx[2], x[3] + dx[3], x[4] + dx[4]]);
            let cy = cost(&y);
            if cy <= cur {
                step = Some((y, dx));
                cur = cy;
                lambda = (lambda * 0.3).max(1e-7);
                break;
            }
            lambda *= 10.0;
        }
        let Some((y, dx)) = step else { break };
        x = y;
        if dx.iter().map(|v| v.abs()).fold(0.0, f64::max) < 1e-7 {
            break;
        }
    }
    let pf = prof(&x);
    let (o, r) = ([x[0], x[1]], x[4]);
    let mut sw = 0.0;
    let mut se = 0.0;
    let mut on_arc = 0;
    for (q, _) in pts {
        let (e, arc) = res(&pf, *q, &x);
        if e.abs() < 3.0 * c {
            sw += 1.0;
            se += e * e;
            if arc && e.abs() < 2.0 * c {
                on_arc += 1;
            }
        }
    }
    let rms = if sw > 0.0 { (se / sw).sqrt() } else { f64::MAX };
    if sw < 12.0 {
        return Err("too little of the scan fits a fillet here".into());
    }
    if rms > (0.12 * r).max(h).max(0.05) {
        return Err(format!("the scan is not a round fillet here (off by {rms:.2} mm)"));
    }
    if r >= 1.5 * h && on_arc < 4 {
        return Err("too little of the scan on the fillet here".into());
    }
    let centre = pf.at(o[0] + sgn * r, o[1] + sgn * r);
    let ends = [centre - pf.n[0] * (sgn * r), centre - pf.n[1] * (sgn * r)];
    // the offsets reported along the part's own normals, at the edge
    let off = [pf.at(o[0], o[1]).dot(n[0]), pf.at(o[0], o[1]).dot(n[1])];
    Ok(Fit { o: off, r, centre, ends, rms, on_arc, turn: [x[2], x[3]] })
}

/// Gaussian elimination with partial pivoting.
fn solve_n<const N: usize>(mut a: [[f64; N]; N], mut b: [f64; N]) -> Option<[f64; N]> {
    for col in 0..N {
        let piv = (col..N).max_by(|&i, &k| a[i][col].abs().total_cmp(&a[k][col].abs()))?;
        if a[piv][col].abs() < 1e-18 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..N {
            let f = a[row][col] / a[col][col];
            for k in col..N {
                a[row][k] -= f * a[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0; N];
    for row in (0..N).rev() {
        let s: f64 = (row + 1..N).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - s) / a[row][row];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec2;

    /// A closed outline in XY (counter-clockwise), each corner rounded at
    /// its radius (0: sharp), sampled every `step`.
    fn rounded(corners: &[(DVec2, f64)], step: f64) -> Vec<DVec2> {
        let k = corners.len();
        let mut out = vec![];
        for i in 0..k {
            let (p, r) = corners[i];
            let a = (p - corners[(i + k - 1) % k].0).normalize();
            let b = (corners[(i + 1) % k].0 - p).normalize();
            let left = a.perp_dot(b) > 0.0;
            let alpha = (-a).dot(b).clamp(-1.0, 1.0).acos();
            let tl = if r > 0.0 { r / (alpha * 0.5).tan() } else { 0.0 };
            let tin = p - a * tl;
            let tout = p + b * tl;
            if r > 0.0 {
                let c = tin + a.perp() * if left { r } else { -r };
                let a0 = (tin - c).to_angle();
                let mut a1 = (tout - c).to_angle();
                if left {
                    while a1 < a0 {
                        a1 += std::f64::consts::TAU;
                    }
                } else {
                    while a1 > a0 {
                        a1 -= std::f64::consts::TAU;
                    }
                }
                let n = (((a1 - a0).abs() * r) / step).ceil().max(2.0) as usize;
                for j in 0..n {
                    let t = a0 + (a1 - a0) * j as f64 / n as f64;
                    out.push(c + DVec2::from_angle(t) * r);
                }
            } else {
                out.push(p);
            }
            // the side to the next corner
            let (q, rq) = corners[(i + 1) % k];
            let c2 = corners[(i + 2) % k].0;
            let b2 = (c2 - q).normalize();
            let alpha2 = (-b).dot(b2).clamp(-1.0, 1.0).acos();
            let tl2 = if rq > 0.0 { rq / (alpha2 * 0.5).tan() } else { 0.0 };
            let end = q - b * tl2;
            let len = (end - tout).length();
            let n = (len / step).ceil().max(1.0) as usize;
            for j in 0..n {
                out.push(tout + (end - tout) * (j as f64 / n as f64));
            }
        }
        out
    }

    /// The outline swept along +Z from 0 to `len`, a little noise along
    /// the normals (an open tube: no caps).
    fn prism(outline: &[DVec2], len: f64, step: f64, noise: f64) -> Mesh {
        let k = outline.len();
        let rings = (len / step).ceil() as usize + 1;
        let mut seed = 12345u64;
        let mut rnd = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0
        };
        let mut pos = vec![];
        for j in 0..rings {
            let z = len * j as f64 / (rings - 1) as f64;
            for i in 0..k {
                let a = outline[(i + k - 1) % k];
                let b = outline[(i + 1) % k];
                let nn = (b - a).perp().normalize() * -1.0;
                let p = outline[i] + nn * (noise * rnd());
                pos.push(DVec3::new(p.x, p.y, z));
            }
        }
        let mut tris = vec![];
        for j in 0..rings - 1 {
            for i in 0..k {
                let a = (j * k + i) as u32;
                let b = (j * k + (i + 1) % k) as u32;
                let c = ((j + 1) * k + (i + 1) % k) as u32;
                let d = ((j + 1) * k + i) as u32;
                tris.push([a, b, c]);
                tris.push([a, c, d]);
            }
        }
        Mesh { pos, tris, unit_mm: 1.0, ..Default::default() }
    }

    /// Samples along a vertical sharp edge at `p` between the faces whose
    /// outward normals are `n0`, `n1` (directions away from the edge `d0`,
    /// `d1`), z 5 to 15.
    fn edge(p: DVec2, n: [DVec2; 2], d: [DVec2; 2], reach: f64) -> Vec<EdgeSample> {
        (0..10)
            .map(|i| {
                let z = 5.0 + i as f64 * 10.0 / 9.0;
                let v = |q: DVec2| DVec3::new(q.x, q.y, 0.0);
                EdgeSample { p: DVec3::new(p.x, p.y, z), t: DVec3::Z, n: [v(n[0]), v(n[1])], d: [v(d[0]), v(d[1])], reach: [reach, reach] }
            })
            .collect()
    }

    fn read_on(m: &Mesh, s: &[EdgeSample]) -> Result<FilletRead, String> {
        let topo = Topo::new(m);
        let bvh = Bvh::new(m);
        read(m, &topo, &bvh, s, m.edge_median())
    }

    #[test]
    fn square_corners() {
        // a 30 mm square, its corners rounded at 0.5, 2, 5 and 8
        let c = |x: f64, y: f64, r: f64| (DVec2::new(x, y), r);
        let o = rounded(&[c(-15.0, -15.0, 0.5), c(15.0, -15.0, 2.0), c(15.0, 15.0, 5.0), c(-15.0, 15.0, 8.0)], 0.15);
        let m = prism(&o, 20.0, 0.15, 0.02);
        let (x, y) = (DVec2::X, DVec2::Y);
        for (p, n, d, want) in [
            (DVec2::new(-15.0, -15.0), [-x, -y], [y, x], 0.5),
            (DVec2::new(15.0, -15.0), [x, -y], [y, -x], 2.0),
            (DVec2::new(15.0, 15.0), [x, y], [-y, -x], 5.0),
            (DVec2::new(-15.0, 15.0), [-x, y], [-y, x], 8.0),
        ] {
            let f = read_on(&m, &edge(p, n, d, 30.0)).unwrap_or_else(|e| panic!("r {want}: {e}"));
            println!("r {want}: read {:.3} ({:.3} to {:.3}), {} slices", f.r, f.lo, f.hi, f.slices.len());
            assert!(f.convex);
            assert!((f.r - want).abs() < 0.02 + 0.01 * want, "r {want}: read {}", f.r);
            assert!(!f.varies());
        }
    }

    #[test]
    fn hexagon_and_offset() {
        // a hexagon (120° corners) rounded at 1.5, the part's faces 0.3 mm
        // inside the scan's
        let corners: Vec<(DVec2, f64)> = (0..6).map(|i| (DVec2::from_angle(i as f64 * std::f64::consts::FRAC_PI_3) * 15.0, 1.5)).collect();
        let m = prism(&rounded(&corners, 0.15), 20.0, 0.15, 0.02);
        let p = corners[0].0;
        let s0 = (corners[0].0 - corners[5].0).normalize();
        let s1 = (corners[1].0 - corners[0].0).normalize();
        let n = [-s0.perp(), -s1.perp()];
        let d = [-s0, s1];
        // the part's corner moved in by 0.3 along both normals
        let inset = (n[0] + n[1]).normalize() * (0.3 / (n[0].dot((n[0] + n[1]).normalize())));
        let f = read_on(&m, &edge(p - inset, n, d, 14.0)).unwrap();
        println!("hexagon: {:.3}, offsets {:.3?}", f.r, f.slices[0].off);
        assert!((f.r - 1.5).abs() < 0.04, "{}", f.r);
        assert!((f.slices[0].off[0] - 0.3).abs() < 0.03);
    }

    #[test]
    fn concave_corner() {
        // an L whose inner corner is rounded at 1.5 (the others sharp)
        let c = |x: f64, y: f64, r: f64| (DVec2::new(x, y), r);
        let o = rounded(&[c(0.0, 0.0, 0.0), c(30.0, 0.0, 0.0), c(30.0, 10.0, 0.0), c(10.0, 10.0, 1.5), c(10.0, 30.0, 0.0), c(0.0, 30.0, 0.0)], 0.15);
        let m = prism(&o, 20.0, 0.15, 0.02);
        let f = read_on(&m, &edge(DVec2::new(10.0, 10.0), [DVec2::Y, DVec2::X], [DVec2::X, DVec2::Y], 20.0)).unwrap();
        println!("concave: {:.3}", f.r);
        assert!(!f.convex);
        assert!((f.r - 1.5).abs() < 0.04, "{}", f.r);
    }

    #[test]
    fn sharp_corner() {
        let c = |x: f64, y: f64| (DVec2::new(x, y), 0.0);
        let o = rounded(&[c(-15.0, -15.0), c(15.0, -15.0), c(15.0, 15.0), c(-15.0, 15.0)], 0.15);
        let m = prism(&o, 20.0, 0.15, 0.01);
        let e = read_on(&m, &edge(DVec2::new(15.0, 15.0), [DVec2::X, DVec2::Y], [-DVec2::Y, -DVec2::X], 30.0)).unwrap_err();
        println!("sharp: {e}");
        assert!(e.contains("sharp"), "{e}");
    }

    #[test]
    fn part_turned_a_little() {
        // the part's faces turned 3° from the scan's (a plane snapped to a
        // world axis), a noisier scan
        let c = |x: f64, y: f64, r: f64| (DVec2::new(x, y), r);
        let o = rounded(&[c(-15.0, -15.0, 1.0), c(15.0, -15.0, 1.0), c(15.0, 15.0, 3.0), c(-15.0, 15.0, 1.0)], 0.15);
        let m = prism(&o, 20.0, 0.15, 0.05);
        let turn = |v: DVec2| DVec2::from_angle(3f64.to_radians()).rotate(v);
        let (x, y) = (DVec2::X, DVec2::Y);
        let f = read_on(&m, &edge(DVec2::new(15.0, 15.0), [turn(x), turn(y)], [turn(-y), turn(-x)], 30.0)).unwrap();
        println!("turned 3°: {:.3} ({:.3} to {:.3}) rms {:.3} turn {:.2?}", f.r, f.lo, f.hi, f.slices[0].rms, f.slices[0].turn);
        assert!((f.r - 3.0).abs() < 0.06, "{}", f.r);
    }
}
