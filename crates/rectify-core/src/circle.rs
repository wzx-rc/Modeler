//! The Circle tool (Romain, 18 nov. 2026: "un cercle qui détecte le plan, la
//! base, on set la hauteur, les niveaux de subdiv et ça vient se snap auto sur
//! la forme, un peu comme une projection, sauf qu'elle force à conserver des
//! cerclages parfaits").
//!
//! A drag across a round part (a hole, a boss, a button), from one side of
//! its rim to the other. Under the first point, the flat face it sits on:
//! the plane, whose normal is the axis. The circle drawn is then fitted on
//! the round wall next to it (centre and radius), and the wall's height
//! found. Along the height, rings (the levels): each a perfect circle on the
//! axis, its radius read on the mesh at that height. The wall's vertices are
//! then put on them, straight out from the axis: every slice comes out truly
//! round, the profile follows the rings.

use crate::mesh::{fit_plane, Mesh, Topo};
use crate::moves::Moves;
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

fn smooth(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(v[v.len() / 2])
}

fn vertex_normal(m: &Mesh, topo: &Topo, v: u32) -> DVec3 {
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

    /// The radius at a height, between the rings: a smooth curve through
    /// them (Catmull-Rom), so the profile has no kink at a ring.
    pub fn radius_at(rings: &[(f64, f64)], hh: f64) -> f64 {
        let Some(first) = rings.first() else { return 0.0 };
        if hh <= first.0 {
            return first.1;
        }
        let n = rings.len();
        for i in 0..n.saturating_sub(1) {
            if hh <= rings[i + 1].0 {
                let (h0, h1) = (rings[i].0, rings[i + 1].0);
                let t = (hh - h0) / (h1 - h0).max(1e-12);
                let p1 = rings[i].1;
                let p2 = rings[i + 1].1;
                let p0 = if i > 0 { rings[i - 1].1 } else { 2.0 * p1 - p2 };
                let p3 = if i + 2 < n { rings[i + 2].1 } else { 2.0 * p2 - p1 };
                let (t2, t3) = (t * t, t * t * t);
                return 0.5 * (2.0 * p1 + (p2 - p0) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2 + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3);
            }
        }
        rings.last().unwrap().1
    }

    /// How far from round the wall is: the root mean square distance of its
    /// vertices (between the base and `height`) from the rings (mm).
    pub fn out_of_round(&self, m: &Mesh, rings: &[(f64, f64)]) -> f64 {
        let height = rings.last().map(|r| r.0).unwrap_or(0.0);
        let r0 = rings.iter().map(|r| r.1).fold(0.0, f64::max);
        let (mut s, mut k) = (0.0, 0usize);
        for (i, &v) in self.region.iter().enumerate() {
            let (hh, rho, _) = self.local(m.pos[v as usize]);
            if hh < 0.0 || hh > height || self.normals[i].dot(self.a).abs() > 0.5 {
                continue;
            }
            let d = rho - Self::radius_at(rings, hh);
            if d.abs() < r0 * 0.15 {
                s += d * d;
                k += 1;
            }
        }
        if k == 0 {
            0.0
        } else {
            (s / k as f64).sqrt()
        }
    }

    /// The wall's vertices put on the rings, straight out from the axis.
    /// `reach`: how far from its ring a vertex may be and still move.
    pub fn apply(&self, m: &Mesh, topo: &Topo, rings: &[(f64, f64)], reach: f64) -> Moves {
        use rayon::prelude::*;
        let height = rings.last().map(|r| r.0).unwrap_or(0.0).max(1e-6);
        let (u, v) = self.basis();
        let n = self.region.len();
        let local: Vec<(f64, f64, f64)> = self
            .region
            .iter()
            .map(|&vx| {
                let x = m.pos[vx as usize];
                let (hh, rho, _) = self.local(x);
                let q = x - self.c;
                (hh, rho, q.dot(v).atan2(q.dot(u)))
            })
            .collect();
        // the wall itself: between the base and the top, a wall's normal,
        // near its ring. There each vertex goes exactly onto its ring.
        let (core, delta): (Vec<f64>, Vec<f64>) = (0..n)
            .map(|i| {
                let (hh, rho, _) = local[i];
                if hh < 0.0 || hh > height {
                    return (0.0, 0.0);
                }
                let d = Self::radius_at(rings, hh) - rho;
                let near = smooth((reach - d.abs()) / (reach * 0.5));
                let wall = smooth((0.5 - self.normals[i].dot(self.a).abs()) / 0.2);
                (near * wall, d)
            })
            .unzip();
        // how far out of round the wall is, by angle and height: a grid of
        // its corrections (an AI mesh's wall is oval or off-centre, smoothly)
        let r_mean = rings.iter().map(|r| r.1).sum::<f64>() / rings.len().max(1) as f64;
        let nt = ((std::f64::consts::TAU * r_mean / (self.edge * 3.0)) as usize).clamp(24, 360);
        let nh = rings.len().max(2);
        let cell = |t: f64, hh: f64| -> (usize, usize) {
            let ti = (((t + std::f64::consts::PI) / std::f64::consts::TAU * nt as f64) as usize).min(nt - 1);
            let hi = ((hh.clamp(0.0, height) / height * (nh - 1) as f64).round() as usize).min(nh - 1);
            (ti, hi)
        };
        let mut sum = vec![0.0f64; nt * nh];
        let mut wsum = vec![0.0f64; nt * nh];
        for i in 0..n {
            if core[i] > 0.5 {
                let (ti, hi) = cell(local[i].2, local[i].0);
                sum[hi * nt + ti] += delta[i] * core[i];
                wsum[hi * nt + ti] += core[i];
            }
        }
        let mut grid: Vec<Option<f64>> = (0..nt * nh).map(|k| (wsum[k] > 0.0).then(|| sum[k] / wsum[k])).collect();
        // holes in the grid filled from their neighbours (around, then up and down)
        for _ in 0..(nt + nh) {
            let mut any = false;
            let prev = grid.clone();
            for hi in 0..nh {
                for ti in 0..nt {
                    if prev[hi * nt + ti].is_some() {
                        continue;
                    }
                    let mut acc = (0.0, 0);
                    for (dt, dh) in [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)] {
                        let (t2, h2) = ((ti as isize + dt).rem_euclid(nt as isize) as usize, hi as isize + dh);
                        if h2 < 0 || h2 >= nh as isize {
                            continue;
                        }
                        if let Some(x) = prev[h2 as usize * nt + t2] {
                            acc.0 += x;
                            acc.1 += 1;
                        }
                    }
                    if acc.1 > 0 {
                        grid[hi * nt + ti] = Some(acc.0 / acc.1 as f64);
                        any = true;
                    }
                }
            }
            if !any {
                break;
            }
        }
        let mut g: Vec<f64> = grid.iter().map(|x| x.unwrap_or(0.0)).collect();
        // a little smoother around
        for _ in 0..2 {
            g = (0..nt * nh).map(|k| (g[k] * 2.0 + g[(k / nt) * nt + (k % nt + 1) % nt] + g[(k / nt) * nt + (k % nt + nt - 1) % nt]) / 4.0).collect();
        }
        // past the wall (its fillets, the faces around) the same correction,
        // the one at the wall's nearest end at that angle, fading with the
        // distance from that end in the profile: on a flat face a move out
        // from the axis only slides the vertices along it
        let fd = (r_mean * 0.25).max(self.edge * 6.0);
        let mut disp: Vec<f64> = (0..n)
            .into_par_iter()
            .map(|i| {
                let (hh, rho, t) = local[i];
                let hc = hh.clamp(0.0, height);
                let (ti, hi) = cell(t, hc);
                let corr = g[hi * nt + ti];
                let dist = ((hh - hc).powi(2) + (rho - Self::radius_at(rings, hc)).powi(2)).sqrt();
                let fade = smooth((fd - dist) / fd);
                core[i] * delta[i] + (1.0 - core[i]) * corr * fade
            })
            .collect();
        // evened out a little outside the wall: no step where it starts
        let index: std::collections::HashMap<u32, usize> = self.region.iter().enumerate().map(|(i, &vx)| (vx, i)).collect();
        let nbs: Vec<Vec<usize>> = self.region.iter().map(|&vx| topo.verts_of(vx as usize).iter().filter_map(|x| index.get(x).copied()).collect()).collect();
        for _ in 0..4 {
            disp = (0..n)
                .into_par_iter()
                .map(|i| {
                    let nb = &nbs[i];
                    let mean = if nb.is_empty() { disp[i] } else { nb.iter().map(|&j| disp[j]).sum::<f64>() / nb.len() as f64 };
                    core[i] * delta[i] + (1.0 - core[i]) * (disp[i] + mean) * 0.5
                })
                .collect();
        }
        let targets = self.region.iter().enumerate().filter(|(i, _)| disp[*i].abs() > 1e-5).map(|(i, &vx)| {
            let x = m.pos[vx as usize];
            let (_, _, out) = self.local(x);
            (vx, x + out * disp[i])
        });
        Moves::new(m, targets.collect::<Vec<_>>())
    }
}
