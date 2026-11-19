//! The Circle tool (Romain, 18 nov. 2026: "un cercle qui détecte le plan, la
//! base, on set la hauteur, les niveaux de subdiv et ça vient se snap auto sur
//! la forme, un peu comme une projection, sauf qu'elle force à conserver des
//! cerclages parfaits").
//!
//! A drag across a round part (a hole, a boss, a button), from one side of
//! its rim to the other. Under the first point, the flat face it sits on:
//! the plane, whose normal is the axis. The circle drawn is then fitted on
//! the round wall next to it (centre and radius), and the wall's height
//! found. Then (Romain, 18 nov., 16 h 01: "garde l'outil de cerclage… mais
//! fais le résultat en NURBS") a NURBS surface is laid on the wall: every
//! ring an exact circle on the axis, its radius read on the scan (lines cast
//! straight out from the axis), the profile a cylinder, a cone or a curve.
//! The scan itself does not change.

use crate::mesh::{fit_plane, Mesh, Topo};
use crate::nurbs::{greville, uniform_knots, Curve, Surface};
use glam::{DVec2, DVec3};
use std::collections::{HashSet, VecDeque};

#[derive(Clone, Debug)]
pub struct Circle {
    /// The centre of the base, on the plane.
    pub c: DVec3,
    /// The axis: the plane's normal, from the base along the wall.
    pub a: DVec3,
    /// The base radius (mm).
    pub r: f64,
    /// The wall's height found on the mesh (mm).
    pub height: f64,
    /// The vertices around the part (where the rings are read and moved).
    pub region: Vec<u32>,
    /// Per vertex of `region`: its normal.
    normals: Vec<DVec3>,
    /// The mesh's edge length.
    edge: f64,
}

fn debug() -> bool {
    std::env::var("RECTIFY_DEBUG").is_ok()
}

pub(crate) fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(v[v.len() / 2])
}

pub(crate) fn vertex_normal(m: &Mesh, topo: &Topo, v: u32) -> DVec3 {
    topo.tris_of(v as usize).iter().map(|&t| m.tri_cross(t as usize)).sum::<DVec3>().normalize_or_zero()
}

/// A circle through points of a plane (Kåsa: algebraic least squares).
fn fit_circle(pts: &[DVec2]) -> Option<(DVec2, f64)> {
    if pts.len() < 3 {
        return None;
    }
    // x² + y² + D x + E y + F = 0
    let mut ata = [[0.0f64; 3]; 3];
    let mut atb = [0.0f64; 3];
    for p in pts {
        let row = [p.x, p.y, 1.0];
        let b = -(p.x * p.x + p.y * p.y);
        for i in 0..3 {
            for j in 0..3 {
                ata[i][j] += row[i] * row[j];
            }
            atb[i] += row[i] * b;
        }
    }
    let det = |m: &[[f64; 3]; 3]| m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let d0 = det(&ata);
    if d0.abs() < 1e-12 {
        return None;
    }
    let mut sol = [0.0; 3];
    for k in 0..3 {
        let mut mk = ata;
        for i in 0..3 {
            mk[i][k] = atb[i];
        }
        sol[k] = det(&mk) / d0;
    }
    let c = DVec2::new(-sol[0] * 0.5, -sol[1] * 0.5);
    let r2 = c.length_squared() - sol[2];
    (r2 > 0.0).then(|| (c, r2.sqrt()))
}

impl Circle {
    /// Two directions square to the axis.
    pub fn basis(&self) -> (DVec3, DVec3) {
        let u = self.a.any_orthonormal_vector();
        (u, self.a.cross(u))
    }

    /// Height along the axis, distance from it, and the unit direction out.
    pub fn local(&self, x: DVec3) -> (f64, f64, DVec3) {
        let d = x - self.c;
        let hh = d.dot(self.a);
        let radial = d - self.a * hh;
        let rho = radial.length();
        (hh, rho, if rho > 1e-12 { radial / rho } else { self.basis().0 })
    }

    /// Finds the round part under a drag from `p0` (on the rim, `seeds` the
    /// vertices of its triangle) to `p1` (across it). `h`: the mesh's edge
    /// length.
    pub fn detect(m: &Mesh, topo: &Topo, seeds: &[u32], p0: DVec3, p1: DVec3, h: f64) -> Result<Circle, String> {
        if (p1 - p0).length() < h * 6.0 {
            return Err("Drag across the round part, from one side of its rim to the other".into());
        }
        // 1. the flat face under the first point
        let n_seed: DVec3 = seeds.iter().map(|&v| vertex_normal(m, topo, v)).sum::<DVec3>().normalize_or_zero();
        let r_plane = ((p1 - p0).length() * 0.2).max(h * 4.0);
        let cos15 = 15f64.to_radians().cos();
        let mut seen: HashSet<u32> = HashSet::new();
        let mut q: VecDeque<u32> = VecDeque::new();
        let mut flat = vec![];
        for &s in seeds {
            if seen.insert(s) {
                q.push_back(s);
            }
        }
        while let Some(v) = q.pop_front() {
            let x = m.pos[v as usize];
            flat.push((x, 1.0));
            for &w in topo.verts_of(v as usize) {
                if seen.contains(&w) {
                    continue;
                }
                let y = m.pos[w as usize];
                if (y - p0).length() <= r_plane && vertex_normal(m, topo, w).dot(n_seed) >= cos15 {
                    seen.insert(w);
                    q.push_back(w);
                }
            }
        }
        let (pc, mut n) = fit_plane(&flat).unwrap_or((p0, n_seed));
        if n.dot(n_seed) < 0.0 {
            n = -n;
        }
        if flat.len() < 6 {
            n = n_seed;
        }
        // 2. the circle drawn, on that plane
        let onto = |x: DVec3| x - n * n.dot(x - pc);
        let (q0, q1) = (onto(p0), onto(p1));
        let c = (q0 + q1) * 0.5;
        let r = (q1 - q0).length() * 0.5;
        if r < h * 3.0 {
            return Err("Drag across the round part, from one side of its rim to the other".into());
        }
        // 3. the mesh around it: from the first point, inside a cylinder
        let reach_r = r * 1.8;
        let reach_h = (r * 3.0).max(h * 20.0);
        let inside = |x: DVec3| {
            let d = x - c;
            let hh = d.dot(n);
            (d - n * hh).length() <= reach_r && hh.abs() <= reach_h
        };
        let mut seen: HashSet<u32> = HashSet::new();
        let mut region = vec![];
        let mut q: VecDeque<u32> = seeds.iter().copied().filter(|&s| inside(m.pos[s as usize])).collect();
        for &s in &q {
            seen.insert(s);
        }
        while let Some(v) = q.pop_front() {
            region.push(v);
            for &w in topo.verts_of(v as usize) {
                if !seen.contains(&w) && inside(m.pos[w as usize]) {
                    seen.insert(w);
                    q.push_back(w);
                }
            }
        }
        let normals: Vec<DVec3> = region.iter().map(|&v| vertex_normal(m, topo, v)).collect();
        // the axis: the direction the flat parts around face (the panel, the
        // floor, the top), the most common normal near the press's (wide,
        // then narrower): a press on a slope or a lip does not tilt it
        let mut n = n;
        for deg in [20.0f64, 12.0, 8.0, 5.0, 5.0] {
            let k = deg.to_radians();
            let mut sum = DVec3::ZERO;
            for nn in &normals {
                let cosang = nn.dot(n);
                if cosang > 0.0 {
                    let ang = cosang.min(1.0).acos();
                    sum += *nn * (-(ang / k).powi(2)).exp();
                }
            }
            if sum.length() > 1e-9 {
                n = sum.normalize();
            }
        }
        if debug() {
            eprintln!("circle: seed n {:.3},{:.3},{:.3}, flat {} → plane n, then axis {:.3},{:.3},{:.3}; p0 {:.2},{:.2},{:.2} p1 {:.2},{:.2},{:.2}", n_seed.x, n_seed.y, n_seed.z, flat.len(), n.x, n.y, n.z, p0.x, p0.y, p0.z, p1.x, p1.y, p1.z);
        }
        let c = c - n * n.dot(c - pc);
        let mut circle = Circle { c, a: n, r, height: 0.0, region, normals, edge: h };
        // (the same two directions square to the axis whichever way it turns)
        let u = n.any_orthonormal_vector();
        let v = n.cross(u);
        let to2 = |ci: &Circle, x: DVec3| {
            let d = x - ci.c;
            DVec2::new(d.dot(u), d.dot(v))
        };
        // 4. the walls near the rim, on both sides of the plane: normals
        // square to the axis, within half a radius of the circle drawn and
        // not far along the axis (the rest of the part around does not count)
        let win = r.max(h * 10.0);
        let near: Vec<usize> = (0..circle.region.len())
            .filter(|&i| {
                let (hh, rho, _) = circle.local(m.pos[circle.region[i] as usize]);
                circle.normals[i].dot(n).abs() < 0.5 && hh.abs() > h * 0.5 && hh.abs() < win && (rho - r).abs() < r * 0.6
            })
            .collect();
        if near.len() < 12 {
            return Err("No round wall there: drag across a hole or a boss, rim to rim".into());
        }
        // 5. the centre: on a round wall every normal points at the axis.
        // The point nearest the wall's normal lines (the drag rarely goes
        // through the middle), lines far from it counting less and less:
        // the other shapes around do not pull it.
        let lines: Vec<(DVec2, DVec2)> = near
            .iter()
            .filter_map(|&i| {
                let nn = circle.normals[i];
                let d2 = DVec2::new(nn.dot(u), nn.dot(v));
                (d2.length() > 0.5).then(|| (to2(&circle, m.pos[circle.region[i] as usize]), d2.normalize()))
            })
            .collect();
        let mut cen = DVec2::ZERO;
        for k in 0..7 {
            let sigma = (r * 0.3 * 0.6f64.powi(k)).max(h * 1.5);
            let (mut a11, mut a12, mut a22, mut b1, mut b2) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for (p, d) in &lines {
                let e = DVec2::new(-d.y, d.x);
                let dist = (cen - *p).dot(e);
                let w = 1.0 / (1.0 + (dist / sigma).powi(2));
                a11 += w * e.x * e.x;
                a12 += w * e.x * e.y;
                a22 += w * e.y * e.y;
                let pe = p.dot(e);
                b1 += w * e.x * pe;
                b2 += w * e.y * pe;
            }
            let det = a11 * a22 - a12 * a12;
            if det.abs() < 1e-12 {
                break;
            }
            let next = DVec2::new((b1 * a22 - b2 * a12) / det, (a11 * b2 - a12 * b1) / det);
            if next.length() > r * 0.5 {
                break;
            }
            cen = next;
        }
        circle.c += u * cen.x + v * cen.y;
        // the rim: where it was pressed (where it was let go only tells
        // across what)
        let (r_press, r_end) = (circle.local(p0).1, circle.local(p1).1);
        // 6. the wall: on each side of the plane, the distances from the axis
        // of the vertices facing it in a histogram; the peak nearest the rim
        // pressed, on the side where it is nearest
        let bin = (h * 0.5).max(r_press / 60.0);
        let lo = r_press * 0.4;
        let nb = ((r_press * 1.2 / bin).ceil() as usize).max(3);
        let facing_at = |ci: &Circle, i: usize| -> Option<(f64, f64)> {
            let (hh, rho, out) = ci.local(m.pos[ci.region[i] as usize]);
            let nn = ci.normals[i];
            let flat = nn - ci.a * nn.dot(ci.a);
            (nn.dot(ci.a).abs() < 0.5 && nn.dot(out).abs() > 0.85 * flat.length()).then_some((hh, rho))
        };
        let mut best: Option<(f64, f64, f64)> = None; // (sign, peak, votes)
        for sign in [1.0, -1.0] {
            let mut hist = vec![0.0f64; nb];
            for &i in &near {
                if let Some((hh, rho)) = facing_at(&circle, i) {
                    let b = ((rho - lo) / bin) as isize;
                    if hh * sign > h * 0.5 && (0..nb as isize).contains(&b) {
                        hist[b as usize] += 1.0;
                    }
                }
            }
            let sm: Vec<f64> = (0..nb).map(|i| hist[i.saturating_sub(1)] + 2.0 * hist[i] + hist[(i + 1).min(nb - 1)]).collect();
            let top = sm.iter().cloned().fold(0.0, f64::max);
            let peak = (1..nb - 1)
                .filter(|&i| sm[i] >= sm[i - 1] && sm[i] >= sm[i + 1] && sm[i] >= (top * 0.05).max(8.0))
                .map(|i| (lo + (i as f64 + 0.5) * bin, sm[i]))
                .min_by(|a, b| (a.0 - r_press).abs().partial_cmp(&(b.0 - r_press).abs()).unwrap());
            if debug() {
                eprintln!("circle: side {sign}: peak {peak:?} (top {top})");
            }
            if let Some((pr, votes)) = peak {
                let better = match best {
                    None => true,
                    Some((_, bp, bv)) => {
                        let (d, bd) = ((pr - r_press).abs(), (bp - r_press).abs());
                        if (d - bd).abs() < bin * 3.0 {
                            votes > bv
                        } else {
                            d < bd
                        }
                    }
                };
                if better {
                    best = Some((sign, pr, votes));
                }
            }
        }
        let Some((sign, peak, _)) = best else {
            return Err("No round wall there: drag across a hole or a boss, rim to rim".into());
        };
        circle.a = n * sign;
        circle.r = peak;
        if debug() {
            eprintln!("circle: axis {:.3},{:.3},{:.3}, rim r {r_press:.3} (end {r_end:.3}), centre moved {:.3}, {} lines, wall r {peak:.3}", circle.a.x, circle.a.y, circle.a.z, cen.length(), lines.len());
        }
        // centre and radius fitted on that wall alone (twice, tighter)
        let facing = |ci: &Circle, band: f64| -> Vec<(DVec3, f64, f64)> {
            (0..ci.region.len())
                .filter_map(|i| {
                    let (hh, rho) = facing_at(ci, i)?;
                    (hh > h * 0.5 && hh < win && (rho - ci.r).abs() < band).then(|| (m.pos[ci.region[i] as usize], hh, rho))
                })
                .collect()
        };
        for band in [bin * 4.0 + h, bin * 2.5 + h] {
            let pts: Vec<DVec2> = facing(&circle, band).iter().map(|x| to2(&circle, x.0)).collect();
            if let Some((c2, r2)) = fit_circle(&pts) {
                if (r2 - circle.r).abs() < band * 2.0 && c2.length() < band * 2.0 {
                    circle.c += u * c2.x + v * c2.y;
                    circle.r = r2;
                }
            }
        }
        // the walls, at any height (for the height below)
        let wall = |ci: &Circle, band: f64| -> Vec<(DVec3, f64, f64)> {
            ci.region
                .iter()
                .enumerate()
                .filter_map(|(i, &vx)| {
                    let x = m.pos[vx as usize];
                    let (hh, rho, _) = ci.local(x);
                    (ci.normals[i].dot(ci.a).abs() < 0.5 && (rho - ci.r).abs() < band).then_some((x, hh, rho))
                })
                .collect()
        };
        let r = circle.r;
        // 6. the wall's height: its vertices, from the base up, step by step
        // until a gap
        let hs: Vec<f64> = wall(&circle, (bin * 3.0 + h).min(r * 0.12 + h * 2.0)).iter().map(|x| x.1).filter(|hh| *hh > -h).collect();
        let step = (h * 2.0).max(r / 30.0);
        let bins = ((reach_h / step).ceil() as usize).max(1);
        let mut count = vec![0usize; bins + 1];
        for hh in &hs {
            let b = (hh.max(0.0) / step) as usize;
            if b <= bins {
                count[b] += 1;
            }
        }
        let mut top = 0usize;
        let mut gap = 0;
        for (b, &k) in count.iter().enumerate() {
            if k > 0 {
                top = b + 1;
                gap = 0;
            } else {
                gap += 1;
                if gap > 1 && top > 0 {
                    break;
                }
            }
        }
        circle.height = (top as f64 * step).max(step);
        Ok(circle)
    }

    /// The rings: `levels` + 1 perfect circles from the base to `height`,
    /// each radius the median distance from the axis of the wall's vertices
    /// at that height. (height, radius) from the base up.
    pub fn rings(&self, m: &Mesh, height: f64, levels: usize) -> Vec<(f64, f64)> {
        let levels = levels.max(1);
        let half = height / levels as f64 * 0.5;
        let mut out = vec![];
        let mut prev = self.r;
        for i in 0..=levels {
            let hi = height * i as f64 / levels as f64;
            let rs: Vec<f64> = self
                .region
                .iter()
                .enumerate()
                .filter_map(|(k, &v)| {
                    let (hh, rho, _) = self.local(m.pos[v as usize]);
                    ((hh - hi).abs() <= half && self.normals[k].dot(self.a).abs() < 0.4 && (rho - prev).abs() < (self.r * 0.12).max(self.edge * 4.0)).then_some(rho)
                })
                .collect();
            let ri = median(rs).unwrap_or(prev);
            out.push((hi, ri));
            prev = ri;
        }
        out
    }
}

/// How the wall goes along the axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileKind {
    /// One radius: a true cylinder.
    Cylinder,
    /// The radius changing evenly: a cone.
    Cone,
    /// A curve through the wall's radii (a cubic).
    Curve,
}

/// The NURBS surface's settings (Romain, 18 nov.: the plane and the base
/// found, the height and the subdivisions set, laid on the shape with
/// perfect rings).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoundParams {
    /// From the base along the axis (mm).
    pub height: f64,
    pub profile: ProfileKind,
    /// Rows along the height: the surface has `levels` + 1 rings, each an
    /// exact circle (its knot lines across), evenly from the base up.
    pub levels: usize,
}

/// The NURBS surface laid on the round part, and how well it sits there.
#[derive(Clone, Debug)]
pub struct Round {
    pub surface: Surface,
    /// The share of the lines cast that found the scan.
    pub found: f64,
    /// How far the scan is from the surface where they were cast (mm, root
    /// mean square): how far from round the AI made it.
    pub off: f64,
    /// The surface faces the axis (a hole) rather than away from it (a boss).
    pub inward: bool,
}

/// The wall as read: (height 0..1, distance from the axis) of each line
/// cast that met it.
struct Wall {
    data: Vec<(f64, f64)>,
    found: f64,
    inward: bool,
    height: f64,
}

/// The triangles around the part, binned by angle and height, for casting
/// lines straight out from the axis onto them.
struct Caster {
    tris: Vec<[DVec3; 3]>,
    bins: Vec<Vec<u32>>,
    nt: usize,
    nh: usize,
    h0: f64,
    h1: f64,
}

impl Caster {
    fn new(ci: &Circle, m: &Mesh, topo: &Topo) -> Caster {
        use std::f64::consts::{PI, TAU};
        let mut ts: Vec<u32> = ci.region.iter().flat_map(|&v| topo.tris_of(v as usize).iter().copied()).collect();
        ts.sort_unstable();
        ts.dedup();
        let (u, v) = ci.basis();
        let tris: Vec<[DVec3; 3]> = ts.iter().map(|&t| m.tri_pts(t as usize)).collect();
        let loc: Vec<[(f64, f64, f64); 3]> = tris
            .iter()
            .map(|p| {
                p.map(|x| {
                    let (hh, rho, _) = ci.local(x);
                    let q = x - ci.c;
                    (hh, rho, q.dot(v).atan2(q.dot(u)))
                })
            })
            .collect();
        let h0 = loc.iter().flatten().map(|x| x.0).fold(f64::INFINITY, f64::min);
        let h1 = loc.iter().flatten().map(|x| x.0).fold(f64::NEG_INFINITY, f64::max);
        let (nt, nh) = (256usize, 96usize);
        let mut bins = vec![vec![]; nt * nh];
        let hbin = |hh: f64| (((hh - h0) / (h1 - h0).max(1e-9) * nh as f64) as isize).clamp(0, nh as isize - 1) as usize;
        let tbin = |t: f64| ((((t + PI) / TAU) * nt as f64) as isize).rem_euclid(nt as isize) as usize;
        for (i, c) in loc.iter().enumerate() {
            let (lo, hi) = (c.iter().map(|x| x.0).fold(f64::INFINITY, f64::min), c.iter().map(|x| x.0).fold(f64::NEG_INFINITY, f64::max));
            // the smallest arc holding the three angles (all of them when
            // the triangle is around the axis)
            let mut a: Vec<f64> = c.iter().map(|x| x.2).collect();
            a.sort_by(|x, y| x.partial_cmp(y).unwrap());
            let gaps = [a[1] - a[0], a[2] - a[1], a[0] + TAU - a[2]];
            let (k, gap) = gaps.iter().enumerate().fold((0, 0.0), |best, (k, &g)| if g > best.1 { (k, g) } else { best });
            let span = TAU - gap;
            let start = a[(k + 1) % 3];
            let (t0, steps) = if span > PI * 0.9 { (0, nt) } else { (tbin(start), ((span / TAU) * nt as f64).ceil() as usize + 2) };
            for hb in hbin(lo)..=hbin(hi) {
                for s in 0..steps.min(nt) {
                    let tb = (t0 + nt + s - usize::from(steps < nt)) % nt;
                    bins[hb * nt + tb].push(i as u32);
                }
            }
        }
        Caster { tris, bins, nt, nh, h0, h1 }
    }

    /// Where the line out from the axis at height `hh` and angle `t` meets
    /// the mesh nearest to `guess` (within `win`): the distance from the
    /// axis and the triangle's normal.
    fn cast(&self, ci: &Circle, hh: f64, t: f64, guess: f64, win: f64) -> Option<(f64, DVec3)> {
        use std::f64::consts::{PI, TAU};
        if hh < self.h0 || hh > self.h1 {
            return None;
        }
        let (u, v) = ci.basis();
        let o = ci.c + ci.a * hh;
        let d = u * t.cos() + v * t.sin();
        let hb = (((hh - self.h0) / (self.h1 - self.h0).max(1e-9) * self.nh as f64) as usize).min(self.nh - 1);
        let tb = ((((t + PI) / TAU) * self.nt as f64) as isize).rem_euclid(self.nt as isize) as usize;
        let mut best: Option<(f64, DVec3)> = None;
        for &i in &self.bins[hb * self.nt + tb] {
            let [a, b, c] = self.tris[i as usize];
            // Möller-Trumbore, the whole line
            let e1 = b - a;
            let e2 = c - a;
            let p = d.cross(e2);
            let det = e1.dot(p);
            if det.abs() < 1e-14 {
                continue;
            }
            let inv = 1.0 / det;
            let s = o - a;
            let bu = s.dot(p) * inv;
            if !(-1e-9..=1.0 + 1e-9).contains(&bu) {
                continue;
            }
            let q = s.cross(e1);
            let bv = d.dot(q) * inv;
            if bv < -1e-9 || bu + bv > 1.0 + 1e-9 {
                continue;
            }
            let dist = e2.dot(q) * inv;
            if dist <= 0.0 || (dist - guess).abs() > win {
                continue;
            }
            if best.map(|x| (dist - guess).abs() < (x.0 - guess).abs()).unwrap_or(true) {
                best = Some((dist, e1.cross(e2).normalize_or_zero()));
            }
        }
        best
    }
}

impl Circle {
    /// A NURBS surface on the round part's wall, from the base to
    /// `p.height`: every ring an exact circle on the axis. The wall is read
    /// by lines cast straight out from the axis at many heights and angles
    /// (the hit nearest to the wall's radius there); the profile, a radius
    /// by height, is fitted to all the hits (a second pass without those
    /// far from the first).
    pub fn nurbs(&self, m: &Mesh, topo: &Topo, p: RoundParams) -> Round {
        let wall = self.read_wall(m, topo, p.height);
        self.lay(&wall, p)
    }

    /// The fewest levels that follow the wall about as well as many do
    /// (Romain, 18 nov., 16 h 34: "pourquoi il y a autant de subdivisions
    /// sur la cage ?"): one for a cylinder or a cone (more rings do not
    /// change them); for a curve, the first count within 0.01 mm (or 5 %)
    /// of what 16 levels give.
    pub fn fewest_levels(&self, m: &Mesh, topo: &Topo, height: f64, profile: ProfileKind) -> usize {
        if profile != ProfileKind::Curve {
            return 1;
        }
        let wall = self.read_wall(m, topo, height);
        let off = |levels: usize| self.lay(&wall, RoundParams { height, profile, levels }).off;
        let best = off(16);
        let enough = best + (best * 0.05).max(0.01);
        (1..16).find(|&l| off(l) <= enough).unwrap_or(16)
    }

    /// The wall read by lines cast out from the axis.
    fn read_wall(&self, m: &Mesh, topo: &Topo, height: f64) -> Wall {
        use rayon::prelude::*;
        use std::f64::consts::TAU;
        let height = height.max(self.edge * 2.0);
        let caster = Caster::new(self, m, topo);
        // a height every ~1.5 edges (at most 120), 64 angles
        let nh = ((height / (self.edge * 1.5)).round() as usize).clamp(4, 120);
        let guess = self.rings(m, height, nh);
        let win = (self.r * 0.2).max(self.edge * 4.0);
        let inset = (self.edge * 1.5).min(height * 0.1);
        let na = 64;
        let (u, v) = self.basis();
        // (height 0..1, distance from the axis, the scan's normal · out)
        let hits: Vec<(f64, f64, f64)> = (0..=nh)
            .into_par_iter()
            .flat_map_iter(|i| {
                let hi = (height * i as f64 / nh as f64).clamp(inset, height - inset);
                let g = guess[i].1;
                let caster = &caster;
                (0..na).filter_map(move |j| {
                    let t = TAU * j as f64 / na as f64;
                    caster.cast(self, hi, t, g, win).map(|(d, n)| (hi / height, d, n.dot(u * t.cos() + v * t.sin())))
                })
            })
            .collect();
        let found = hits.len() as f64 / ((nh + 1) * na) as f64;
        let inward = hits.iter().map(|x| x.2).sum::<f64>() < 0.0;
        let mut data: Vec<(f64, f64)> = hits.iter().map(|x| (x.0, x.1)).collect();
        if data.is_empty() {
            data = guess.iter().map(|g| (g.0 / height, g.1)).collect();
        }
        Wall { data, found, inward, height }
    }

    /// The surface on the wall read.
    fn lay(&self, wall: &Wall, p: RoundParams) -> Round {
        let (u, v) = self.basis();
        let (profile, off) = fit_profile(&wall.data, p.profile, p.levels, wall.height, self.r, self.edge);
        let surface = Surface::revolve(self.c, self.a, u, v, &profile);
        Round { surface, found: wall.found, off, inward: wall.inward }
    }
}

/// A wall's profile fitted to `data` (height 0..1, distance from the axis
/// or the centre line): one radius (`Cylinder`), the radius changing evenly
/// (`Cone`) or a cubic (`Curve`), `levels` + 1 rows its knots (straight
/// between them, or a cubic). Fitted twice, the second time without the
/// points far from the first fit (another wall, a detail on this one). The
/// curve's control points: (radius, height up to `height`); and how far the
/// points kept are from it (mm, root mean square).
pub(crate) fn fit_profile(data: &[(f64, f64)], kind: ProfileKind, levels: usize, height: f64, r0: f64, edge: f64) -> (Curve, f64) {
    let levels = levels.clamp(1, 64);
    let fit = |data: &[(f64, f64)]| -> (usize, Vec<f64>) {
        match kind {
            ProfileKind::Cylinder => (1, vec![median(data.iter().map(|x| x.1).collect()).unwrap_or(r0); levels + 1]),
            ProfileKind::Cone => {
                let ab = Curve::fit_1d(data, 1, 1).unwrap_or(vec![r0; 2]);
                (1, (0..=levels).map(|i| ab[0] + (ab[1] - ab[0]) * i as f64 / levels as f64).collect())
            }
            ProfileKind::Curve => (3, Curve::fit_1d(data, 3, levels).unwrap_or(vec![r0; 3 + levels])),
        }
    };
    let spans_of = |deg: usize, n: usize| n - deg;
    let eval = |deg: usize, ctrl: &[f64], t: f64| -> f64 {
        let c = Curve { degree: deg, knots: uniform_knots(deg, spans_of(deg, ctrl.len())), pts: ctrl.iter().map(|&r| DVec3::new(r, 0.0, 0.0)).collect(), w: vec![1.0; ctrl.len()] };
        c.eval(t).x
    };
    let mut data = data.to_vec();
    let (mut deg, mut ctrl) = fit(&data);
    let res: Vec<f64> = data.iter().map(|&(t, r)| (r - eval(deg, &ctrl, t)).abs()).collect();
    let cut = (median(res.clone()).unwrap_or(0.0) * 4.0).max(edge * 0.5);
    let kept: Vec<(f64, f64)> = data.iter().zip(&res).filter(|(_, &e)| e <= cut).map(|(d, _)| *d).collect();
    if kept.len() >= 8 {
        (deg, ctrl) = fit(&kept);
        data = kept;
    }
    let off = if data.is_empty() { 0.0 } else { (data.iter().map(|&(t, r)| (r - eval(deg, &ctrl, t)).powi(2)).sum::<f64>() / data.len() as f64).sqrt() };
    let knots = uniform_knots(deg, spans_of(deg, ctrl.len()));
    let hs = greville(&knots, deg);
    let profile = Curve { degree: deg, knots, pts: ctrl.iter().zip(&hs).map(|(&r, &g)| DVec3::new(r.max(0.0), g * height, 0.0)).collect(), w: vec![1.0; ctrl.len()] };
    (profile, off)
}
