//! The Path tool (Romain, 18 nov. 2026, 16 h 01: "que je puisse facilement
//! tracer des lignes et créer des PATH… en semi-auto détection du scan";
//! 16 h 13: "un tool de path pareil qui comprend les plans").
//!
//! Points clicked on the scan, joined into one NURBS curve. Around each
//! point the scan's flat faces are found (`planes_near`: the triangles
//! grouped by their normals, each group's connected parts that are wide and
//! flat enough, a plane fitted to each). A point near where two faces meet
//! goes onto their edge: the line where the two planes cross, sharp, not the
//! rounded scan; near three faces, onto their corner; else onto its face's
//! plane (`snap`). Between two points on one face the path is a straight
//! line on it; from one face to another it folds over their edge (the
//! shortest way across the two planes); elsewhere it follows the scan, one
//! smooth B-spline through the points (`build`). The scan does not change.

use crate::mesh::{fit_plane, Mesh, Surface, Topo, NONE};
use crate::nurbs::Curve;
use glam::DVec3;
use std::collections::{HashMap, VecDeque};

fn debug() -> bool {
    std::env::var("RECTIFY_DEBUG").is_ok()
}

/// A flat face of the scan found near a point.
#[derive(Clone, Debug)]
pub struct Plane {
    /// A point on it (its part found, centred) and its unit normal (the
    /// scan's side: out of the material).
    pub c: DVec3,
    pub n: DVec3,
    /// How far its triangles lie from it (mm, RMS) and their area (mm²).
    pub rms: f64,
    pub area: f64,
    /// Its triangles' centres (at most a few thousand, spread): how far a
    /// point is from the face.
    pts: Vec<DVec3>,
}

impl Plane {
    /// Signed distance from the plane.
    pub fn dist(&self, x: DVec3) -> f64 {
        self.n.dot(x - self.c)
    }

    pub fn onto(&self, x: DVec3) -> DVec3 {
        x - self.n * self.dist(x)
    }

    /// How far `x` is from the face's triangles found (their centres).
    pub fn gap(&self, x: DVec3) -> f64 {
        self.pts.iter().map(|p| (*p - x).length_squared()).fold(f64::INFINITY, f64::min).sqrt()
    }

    /// The same face found from another point: parallel (4°) and level.
    pub fn same(&self, o: &Plane, tol: f64) -> bool {
        self.n.dot(o.n) >= 4f64.to_radians().cos() && self.dist(o.c).abs() <= tol && o.dist(self.c).abs() <= tol
    }

    /// The angle between two faces' normals (degrees: 90 at a box's edge).
    pub fn angle(&self, o: &Plane) -> f64 {
        self.n.dot(o.n).clamp(-1.0, 1.0).acos().to_degrees()
    }
}

/// Where two planes cross: a point (the nearest to `near`) and the unit
/// direction.
pub fn meet(a: &Plane, b: &Plane, near: DVec3) -> Option<(DVec3, DVec3)> {
    let (n1, n2) = (a.n, b.n);
    let e = n1.cross(n2);
    if e.length() < 1e-6 {
        return None;
    }
    let e = e.normalize();
    let (a11, a12, a22) = (n1.dot(n1), n1.dot(n2), n2.dot(n2));
    let (b1, b2) = (n1.dot(a.c - near), n2.dot(b.c - near));
    let det = a11 * a22 - a12 * a12;
    if det.abs() < 1e-12 {
        return None;
    }
    let al = (b1 * a22 - b2 * a12) / det;
    let be = (a11 * b2 - a12 * b1) / det;
    Some((near + n1 * al + n2 * be, e))
}

/// Where three planes cross.
fn meet3(a: &Plane, b: &Plane, c: &Plane) -> Option<DVec3> {
    let det = a.n.dot(b.n.cross(c.n));
    if det.abs() < 0.05 {
        return None;
    }
    let (d1, d2, d3) = (a.n.dot(a.c), b.n.dot(b.c), c.n.dot(c.c));
    Some((b.n.cross(c.n) * d1 + c.n.cross(a.n) * d2 + a.n.cross(b.n) * d3) / det)
}

/// A face bends less than this (1 / mm: a radius of 20 mm and more).
const MAX_BEND: f64 = 0.06;

/// A unit normal's cell on a cube around the sphere (6 faces of `B` × `B`).
const B: usize = 24;
fn cell(n: DVec3) -> usize {
    let a = n.abs();
    let (face, u, v) = if a.x >= a.y && a.x >= a.z {
        (if n.x > 0.0 { 0 } else { 1 }, n.y / a.x, n.z / a.x)
    } else if a.y >= a.z {
        (if n.y > 0.0 { 2 } else { 3 }, n.x / a.y, n.z / a.y)
    } else {
        (if n.z > 0.0 { 4 } else { 5 }, n.x / a.z, n.y / a.z)
    };
    let q = |x: f64| (((x + 1.0) * 0.5 * B as f64) as usize).min(B - 1);
    face * B * B + q(u) * B + q(v)
}

/// The flat faces of the scan near `p` (within `reach`, connected to the
/// triangle `t0`). `h`: the scan's edge length. Largest first.
pub fn planes_near(m: &Mesh, topo: &Topo, t0: u32, p: DVec3, reach: f64, h: f64) -> Vec<Plane> {
    // the triangles around, connected to the one under the pointer
    let mut tris: Vec<u32> = vec![t0];
    let mut index: HashMap<u32, usize> = HashMap::new();
    index.insert(t0, 0);
    let mut k = 0;
    while k < tris.len() && tris.len() < 150_000 {
        let t = tris[k];
        k += 1;
        for &u in &topo.adj[t as usize] {
            if u != NONE && !index.contains_key(&u) && (m.tri_centre(u as usize) - p).length() <= reach {
                index.insert(u, tris.len());
                tris.push(u);
            }
        }
    }
    let n = tris.len();
    let nrm: Vec<DVec3> = tris.iter().map(|&t| m.tri_normal(t as usize)).collect();
    let area: Vec<f64> = tris.iter().map(|&t| m.tri_area(t as usize)).collect();
    let ctr: Vec<DVec3> = tris.iter().map(|&t| m.tri_centre(t as usize)).collect();
    let mut used = vec![false; n];
    let mut out: Vec<Plane> = vec![];
    // a face must be this wide (across its narrowest way) and this flat
    let min_w = (h * 2.5).max(0.25);
    let max_rms = (h * 0.5).max(0.06);
    for _ in 0..8 {
        // the densest normals left (a cell and the 8 around it)
        let mut hist = vec![0.0f64; 6 * B * B];
        for i in 0..n {
            if !used[i] {
                hist[cell(nrm[i])] += area[i];
            }
        }
        let dens = |c: usize| -> f64 {
            let (f, r) = (c / (B * B), c % (B * B));
            let (u, v) = ((r / B) as i64, (r % B) as i64);
            let mut s = 0.0;
            for du in -1..=1i64 {
                for dv in -1..=1i64 {
                    let (uu, vv) = (u + du, v + dv);
                    if uu >= 0 && vv >= 0 && uu < B as i64 && vv < B as i64 {
                        s += hist[f * B * B + uu as usize * B + vv as usize];
                    }
                }
            }
            s
        };
        let Some(best) = (0..hist.len()).filter(|&c| hist[c] > 0.0).max_by(|&a, &b| dens(a).partial_cmp(&dens(b)).unwrap()) else { break };
        if dens(best) < min_w * min_w {
            break;
        }
        let mut mode = (0..n).filter(|&i| !used[i] && cell(nrm[i]) == best).map(|i| nrm[i] * area[i]).sum::<DVec3>().normalize_or_zero();
        if mode == DVec3::ZERO {
            break;
        }
        // mean shift, narrow
        let cos_ms = 5f64.to_radians().cos();
        for _ in 0..12 {
            let s: DVec3 = (0..n).filter(|&i| !used[i] && nrm[i].dot(mode) >= cos_ms).map(|i| nrm[i] * area[i]).sum();
            let next = s.normalize_or_zero();
            if next == DVec3::ZERO || next.dot(mode) > 1.0 - 1e-12 {
                break;
            }
            mode = next;
        }
        let cos_in = 10f64.to_radians().cos();
        let members: Vec<usize> = (0..n).filter(|&i| !used[i] && nrm[i].dot(mode) >= cos_in).collect();
        for &i in &members {
            used[i] = true;
        }
        // its connected parts (through the triangles' sides)
        let mut part = vec![usize::MAX; n];
        let is_member: Vec<bool> = {
            let mut v = vec![false; n];
            for &i in &members {
                v[i] = true;
            }
            v
        };
        let mut parts: Vec<Vec<usize>> = vec![];
        for &s in &members {
            if part[s] != usize::MAX {
                continue;
            }
            let id = parts.len();
            let mut q = VecDeque::from([s]);
            part[s] = id;
            let mut comp = vec![];
            while let Some(i) = q.pop_front() {
                comp.push(i);
                for &u in &topo.adj[tris[i] as usize] {
                    if let Some(&j) = index.get(&u) {
                        if is_member[j] && part[j] == usize::MAX {
                            part[j] = id;
                            q.push_back(j);
                        }
                    }
                }
            }
            parts.push(comp);
        }
        for comp in parts {
            let a: f64 = comp.iter().map(|&i| area[i]).sum();
            if a < min_w * min_w * 2.0 {
                continue;
            }
            // the plane: the mode's normal through the median, then fitted
            // on the triangles close to it
            let mut offs: Vec<f64> = comp.iter().map(|&i| mode.dot(ctr[i])).collect();
            offs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let d0 = offs[offs.len() / 2];
            let mut dev: Vec<f64> = offs.iter().map(|o| (o - d0).abs()).collect();
            dev.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mad = dev[dev.len() / 2].max(h * 0.02);
            let close: Vec<(DVec3, f64)> = comp.iter().filter(|&&i| (mode.dot(ctr[i]) - d0).abs() <= mad * 3.0 + h * 0.05).map(|&i| (ctr[i], area[i])).collect();
            let (mut c, mut nn) = fit_plane(&close).unwrap_or((mode * d0, mode));
            if nn.dot(mode) < 0.0 {
                nn = -nn;
            }
            if nn.dot(mode) < 4f64.to_radians().cos() {
                nn = mode;
                c = close.iter().map(|(x, w)| *x * *w).sum::<DVec3>() / close.iter().map(|(_, w)| *w).sum::<f64>().max(1e-12);
            }
            // wide enough both ways (a narrow strip of a rounded edge is
            // not a face) and flat
            let u = nn.any_orthonormal_vector();
            let v = nn.cross(u);
            let mut cov = [[0.0f64; 2]; 2];
            let wsum: f64 = close.iter().map(|(_, w)| *w).sum();
            for (x, w) in &close {
                let d = *x - c;
                let (a1, a2) = (d.dot(u), d.dot(v));
                cov[0][0] += w * a1 * a1;
                cov[0][1] += w * a1 * a2;
                cov[1][1] += w * a2 * a2;
            }
            let (s00, s01, s11) = (cov[0][0] / wsum, cov[0][1] / wsum, cov[1][1] / wsum);
            let tr = s00 + s11;
            let det = s00 * s11 - s01 * s01;
            let lo = (tr * 0.5 - (tr * tr * 0.25 - det).max(0.0).sqrt()).max(0.0);
            let narrow = lo.sqrt() * 2.0;
            let rms = (close.iter().map(|(x, w)| w * nn.dot(*x - c).powi(2)).sum::<f64>() / wsum).sqrt();
            // how much it bends: the normals' turn against the position
            // (least squares, per mm); a band of a fillet seen within the
            // normals' window bends like the fillet (1 / its radius), a
            // face does not
            let bend = {
                let pick: Vec<usize> = comp.iter().copied().filter(|&i| (mode.dot(ctr[i]) - d0).abs() <= mad * 3.0 + h * 0.05).collect();
                let wsum: f64 = pick.iter().map(|&i| area[i]).sum::<f64>().max(1e-12);
                let pa = |i: usize| ((ctr[i] - c).dot(u), (ctr[i] - c).dot(v));
                let pb = |i: usize| (nrm[i].dot(u), nrm[i].dot(v));
                let (ma, mb) = pick.iter().fold(((0.0, 0.0), (0.0, 0.0)), |acc, &i| {
                    let (a, b) = (pa(i), pb(i));
                    ((acc.0 .0 + a.0 * area[i], acc.0 .1 + a.1 * area[i]), (acc.1 .0 + b.0 * area[i], acc.1 .1 + b.1 * area[i]))
                });
                let (ma, mb) = ((ma.0 / wsum, ma.1 / wsum), (mb.0 / wsum, mb.1 / wsum));
                let (mut saa, mut sba) = ([[0.0f64; 2]; 2], [[0.0f64; 2]; 2]);
                for &i in &pick {
                    let (a, b) = (pa(i), pb(i));
                    let a = [a.0 - ma.0, a.1 - ma.1];
                    let b = [b.0 - mb.0, b.1 - mb.1];
                    for r in 0..2 {
                        for q in 0..2 {
                            saa[r][q] += area[i] * a[r] * a[q];
                            sba[r][q] += area[i] * b[r] * a[q];
                        }
                    }
                }
                let det = saa[0][0] * saa[1][1] - saa[0][1] * saa[1][0];
                if det.abs() < 1e-18 {
                    0.0
                } else {
                    let inv = [[saa[1][1] / det, -saa[0][1] / det], [-saa[1][0] / det, saa[0][0] / det]];
                    let k = [[sba[0][0] * inv[0][0] + sba[0][1] * inv[1][0], sba[0][0] * inv[0][1] + sba[0][1] * inv[1][1]], [sba[1][0] * inv[0][0] + sba[1][1] * inv[1][0], sba[1][0] * inv[0][1] + sba[1][1] * inv[1][1]]];
                    // the largest singular value
                    let (a11, a12, a22) = (k[0][0] * k[0][0] + k[1][0] * k[1][0], k[0][0] * k[0][1] + k[1][0] * k[1][1], k[0][1] * k[0][1] + k[1][1] * k[1][1]);
                    let tr = a11 + a22;
                    let dt = a11 * a22 - a12 * a12;
                    (tr * 0.5 + (tr * tr * 0.25 - dt).max(0.0).sqrt()).sqrt()
                }
            };
            if debug() {
                eprintln!("plane? n {:.3},{:.3},{:.3} area {:.2} mm² width {:.2} rms {:.4} bend {:.3}/mm", nn.x, nn.y, nn.z, a, narrow, rms, bend);
            }
            if narrow < min_w || rms > max_rms || bend > MAX_BEND {
                continue;
            }
            let step = (comp.len() / 3000).max(1);
            let pts = comp.iter().step_by(step).map(|&i| ctr[i]).collect();
            out.push(Plane { c, n: nn, rms, area: a, pts });
        }
    }
    out.sort_by(|a, b| b.area.partial_cmp(&a.area).unwrap());
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    /// On the scan, no face there (or the faces not asked for).
    Scan,
    /// On a flat face's plane.
    Plane,
    /// On the edge where two faces' planes cross.
    Edge,
    /// On the corner where three cross.
    Corner,
}

impl SnapKind {
    pub fn label(self) -> &'static str {
        match self {
            SnapKind::Scan => "Scan",
            SnapKind::Plane => "Face",
            SnapKind::Edge => "Edge",
            SnapKind::Corner => "Corner",
        }
    }
}

/// A point of a path: where it goes and what it lies on.
#[derive(Clone, Debug)]
pub struct Snap {
    pub kind: SnapKind,
    pub p: DVec3,
    /// The scan under the pointer, and its triangle.
    pub hit: DVec3,
    pub tri: u32,
    /// The planes it lies on: one (Plane), two (Edge), three (Corner).
    pub planes: Vec<Plane>,
    /// An edge: its direction and how far it runs along the two faces on
    /// each side of `p` (mm), to draw it.
    pub dir: DVec3,
    pub run: (f64, f64),
    /// An edge or a corner: the angle between the faces (degrees; the
    /// smallest of a corner's three).
    pub angle: f64,
}

impl Snap {
    pub fn scan(hit: DVec3, tri: u32) -> Snap {
        Snap { kind: SnapKind::Scan, p: hit, hit, tri, planes: vec![], dir: DVec3::ZERO, run: (0.0, 0.0), angle: 0.0 }
    }
}

/// Faces that turn less than this are not an edge (degrees).
const MIN_TURN: f64 = 20.0;

/// The point under the pointer (`hit` on the scan, in triangle `tri`):
/// onto a corner, an edge or a face near it. `catch`: how far the pointer
/// catches (mm at that depth); `h`: the scan's edge length.
pub fn snap(m: &Mesh, topo: &Topo, tri: u32, hit: DVec3, catch: f64, h: f64) -> Snap {
    // on a wide rounded edge the faces are further: looked for further
    let reach = (catch * 6.0).max(h * 30.0);
    let mut s = snap_within(m, topo, tri, hit, catch, h, reach);
    for k in [2.0, 4.0] {
        if s.kind != SnapKind::Scan {
            break;
        }
        s = snap_within(m, topo, tri, hit, catch, h, reach * k);
    }
    s
}

fn snap_within(m: &Mesh, topo: &Topo, tri: u32, hit: DVec3, catch: f64, h: f64, reach: f64) -> Snap {
    let planes = planes_near(m, topo, tri, hit, reach, h);
    let tol = (h * 1.5).max(0.08);
    let gap_max = (reach * 0.45).max(h * 6.0);
    let turn = |a: &Plane, b: &Plane| {
        let t = a.angle(b);
        (MIN_TURN..=180.0 - MIN_TURN).contains(&t).then_some(t)
    };
    // a corner: three faces whose planes cross near the pointer, each face
    // reaching there
    let mut best: Option<(f64, Snap)> = None;
    for i in 0..planes.len() {
        for j in i + 1..planes.len() {
            for k in j + 1..planes.len() {
                let (a, b, c) = (&planes[i], &planes[j], &planes[k]);
                let (Some(t1), Some(t2), Some(t3)) = (turn(a, b), turn(b, c), turn(a, c)) else { continue };
                let Some(x) = meet3(a, b, c) else { continue };
                let gap = a.gap(x).max(b.gap(x)).max(c.gap(x));
                let d = (x - hit).length();
                if gap > gap_max || d > (catch + gap * 0.6).min(catch * 3.0) {
                    continue;
                }
                if best.as_ref().map(|(bd, _)| d < *bd).unwrap_or(true) {
                    let s = Snap { kind: SnapKind::Corner, p: x, hit, tri, planes: vec![a.clone(), b.clone(), c.clone()], dir: DVec3::ZERO, run: (0.0, 0.0), angle: t1.min(t2).min(t3) };
                    best = Some((d, s));
                }
            }
        }
    }
    if let Some((_, s)) = best {
        return s;
    }
    // an edge: two faces whose planes cross near the pointer
    for i in 0..planes.len() {
        for j in i + 1..planes.len() {
            let (a, b) = (&planes[i], &planes[j]);
            let Some(t) = turn(a, b) else { continue };
            let Some((p0, e)) = meet(a, b, hit) else { continue };
            let q = p0 + e * e.dot(hit - p0);
            let gap = a.gap(q).max(b.gap(q));
            let d = (q - hit).length();
            if debug() {
                eprintln!("edge? {t:.0}° gap {gap:.2} (max {gap_max:.2}) d {d:.2} (max {:.2})", catch + gap);
            }
            if gap > gap_max || d > (catch + gap * 0.6).min(catch * 3.0) {
                continue;
            }
            if best.as_ref().map(|(bd, _)| d < *bd).unwrap_or(true) {
                // how far the edge runs along both faces, each way
                let step = (reach / 80.0).max(h);
                let along = |sign: f64| -> f64 {
                    let mut s = 0.0;
                    while s < reach * 1.5 {
                        let x = q + e * (sign * (s + step));
                        if a.gap(x).max(b.gap(x)) > gap + h * 4.0 {
                            break;
                        }
                        s += step;
                    }
                    s
                };
                let run = (along(-1.0), along(1.0));
                let s = Snap { kind: SnapKind::Edge, p: q, hit, tri, planes: vec![a.clone(), b.clone()], dir: e, run, angle: t };
                best = Some((d, s));
            }
        }
    }
    if let Some((_, s)) = best {
        return s;
    }
    // a face: the one the pointer is on
    let own = planes.iter().filter(|pl| pl.dist(hit).abs() <= tol + pl.rms * 3.0 && pl.gap(hit) <= h * 3.0).min_by(|a, b| a.gap(hit).partial_cmp(&b.gap(hit)).unwrap());
    if let Some(pl) = own {
        return Snap { kind: SnapKind::Plane, p: pl.onto(hit), hit, tri, planes: vec![pl.clone()], dir: DVec3::ZERO, run: (0.0, 0.0), angle: 0.0 };
    }
    Snap::scan(hit, tri)
}

/// The shortest way over the scan's edges from the triangle `ta` (point
/// `xa`) to `tb` (`xb`), kept near the two (A*): the vertices passed,
/// without the ends. None when it cannot get there.
fn surface_walk(m: &Mesh, topo: &Topo, ta: u32, xa: DVec3, tb: u32, xb: DVec3) -> Option<Vec<DVec3>> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let nearest = |t: u32, x: DVec3| m.tris[t as usize].iter().copied().min_by(|&u, &v| (m.pos[u as usize] - x).length_squared().partial_cmp(&(m.pos[v as usize] - x).length_squared()).unwrap()).unwrap();
    let (s, g) = (nearest(ta, xa), nearest(tb, xb));
    if s == g {
        return Some(vec![]);
    }
    let goal = m.pos[g as usize];
    let len = (xb - xa).length();
    let room = (len * 2.2).max(len + 2.0);
    let mut cost: HashMap<u32, f64> = HashMap::new();
    let mut prev: HashMap<u32, u32> = HashMap::new();
    let mut heap = BinaryHeap::new();
    cost.insert(s, 0.0);
    heap.push((Reverse((m.pos[s as usize] - goal).length().to_bits()), s));
    let mut seen = 0usize;
    while let Some((_, v)) = heap.pop() {
        if v == g {
            break;
        }
        seen += 1;
        if seen > 400_000 {
            return None;
        }
        let cv = cost[&v];
        let xv = m.pos[v as usize];
        for &w in topo.verts_of(v as usize) {
            let xw = m.pos[w as usize];
            if (xw - xa).length() + (xw - xb).length() > room {
                continue;
            }
            let c = cv + (xw - xv).length();
            if cost.get(&w).map(|&o| c < o).unwrap_or(true) {
                cost.insert(w, c);
                prev.insert(w, v);
                heap.push((Reverse((c + (xw - goal).length()).to_bits()), w));
            }
        }
    }
    if !prev.contains_key(&g) {
        return None;
    }
    let mut out = vec![];
    let mut v = prev[&g];
    while v != s {
        out.push(m.pos[v as usize]);
        v = prev[&v];
    }
    out.reverse();
    Some(out)
}

/// Points every `step` along a polyline (its ends kept).
fn resample(line: &[DVec3], step: f64) -> Vec<DVec3> {
    let mut out = vec![line[0]];
    let mut carry = 0.0;
    for w in line.windows(2) {
        let (a, b) = (w[0], w[1]);
        let l = (b - a).length();
        let mut t = step - carry;
        while t < l {
            out.push(a + (b - a) * (t / l));
            t += step;
        }
        carry = l - (t - step);
    }
    if (*out.last().unwrap() - *line.last().unwrap()).length() > step * 0.3 {
        out.push(*line.last().unwrap());
    } else {
        *out.last_mut().unwrap() = *line.last().unwrap();
    }
    out
}

/// The closest point of the scan to `x`, looked for near first, then
/// further (up to `max`): a search far away at once is slow.
fn onto_scan(m: &Mesh, scan: &Surface, x: DVec3, h: f64, max: f64) -> Option<DVec3> {
    let mut r = h * 4.0;
    loop {
        if let Some((_, y)) = scan.closest(m, x, r) {
            return Some(y);
        }
        if r >= max {
            return None;
        }
        r = (r * 2.5).min(max.max(h * 4.0));
    }
}

/// The path's settings, changed live.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathParams {
    /// The parts that follow the scan: 0 close to it, 1 smooth and calm.
    pub smooth: f64,
    /// The last point joined back to the first.
    pub closed: bool,
    /// The points caught by the faces, their edges and corners (else the
    /// scan as clicked).
    pub planes: bool,
}

impl Default for PathParams {
    fn default() -> Self {
        PathParams { smooth: 0.5, closed: false, planes: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PieceKind {
    /// Straight, on a face (or along an edge).
    Line,
    /// Two straight lines folded over the edge between two faces.
    Fold,
    /// Following the scan.
    Scan,
}

/// A path made: one NURBS curve (degree 3, corners where its pieces meet).
#[derive(Clone, Debug)]
pub struct Built {
    pub curve: Curve,
    /// Where each piece starts in the curve's parameter (and the end).
    pub breaks: Vec<f64>,
    /// What each segment between two points became.
    pub kinds: Vec<PieceKind>,
    /// How far the scan is from the parts that follow it (mm, RMS), and the
    /// curve's length.
    pub off: f64,
    pub length: f64,
}

impl Built {
    /// The curve as polylines, one per piece (straight pieces: their ends).
    pub fn polylines(&self) -> Vec<Vec<DVec3>> {
        self.breaks.windows(2).map(|w| self.curve.sample_range(w[0], w[1], 48)).collect()
    }
}

/// The path through `pts` (in order). `scan`: the closest point on the
/// scan; `h`: its edge length.
pub fn build(m: &Mesh, topo: &Topo, scan: &Surface, pts: &[Snap], p: &PathParams, h: f64) -> Option<Built> {
    let n = pts.len();
    if n < 2 {
        return None;
    }
    let pos = |i: usize| if p.planes { pts[i].p } else { pts[i].hit };
    let none: Vec<Plane> = vec![];
    let planes = |i: usize| if p.planes { &pts[i].planes } else { &none };
    let mut segs: Vec<(usize, usize)> = (0..n - 1).map(|i| (i, i + 1)).collect();
    if p.closed && n >= 3 {
        segs.push((n - 1, 0));
    }
    let tol = (h * 2.0).max(0.1);
    let fold_tol = (h * 8.0).max(1.2);
    let near_scan = |x: DVec3, r: f64| scan.closest(m, x, r).is_some();
    // each segment: straight on a shared face, folded over an edge, or on
    // the scan
    enum Seg {
        Line,
        Fold(DVec3),
        Scan,
    }
    let mut kinds: Vec<Seg> = vec![];
    for &(i, j) in &segs {
        let (a, b) = (pos(i), pos(j));
        // one face; an AI's faces are a little warped: two faces found
        // nearly parallel (10°) are one when the straight line between the
        // points lies on the scan
        let len = (b - a).length();
        let lean = tol.max(len * 0.025);
        let shared = planes(i).iter().any(|pa| planes(j).iter().any(|pb| pa.same(pb, tol)))
            || (planes(i).iter().any(|pa| planes(j).iter().any(|pb| pa.n.dot(pb.n) >= 10f64.to_radians().cos()))
                && (1..16).all(|k| {
                    let x = a + (b - a) * (k as f64 / 16.0);
                    scan.closest(m, x, lean).is_some()
                }));
        if shared || (a - b).length() < h {
            kinds.push(Seg::Line);
            continue;
        }
        let mut fold: Option<(f64, DVec3)> = None;
        for pa in planes(i) {
            for pb in planes(j) {
                if pa.angle(pb) < 15.0 || pa.angle(pb) > 165.0 {
                    continue;
                }
                let Some((p0, e)) = meet(pa, pb, (a + b) * 0.5) else { continue };
                // the shortest way across both planes: unfolded, a straight
                // line meets the edge here
                let (ta, tb) = (e.dot(a - p0), e.dot(b - p0));
                let (da, db) = ((a - p0 - e * ta).length(), (b - p0 - e * tb).length());
                if da < 1e-6 && db < 1e-6 {
                    continue;
                }
                let t = ta + (tb - ta) * da / (da + db);
                let x = p0 + e * t;
                let len = (x - a).length() + (b - x).length();
                if !(near_scan(x, fold_tol) && near_scan((a + x) * 0.5, fold_tol) && near_scan((x + b) * 0.5, fold_tol)) {
                    continue;
                }
                if fold.map(|(l, _)| len < l).unwrap_or(true) {
                    fold = Some((len, x));
                }
            }
        }
        kinds.push(match fold {
            Some((_, x)) => Seg::Fold(x),
            None => Seg::Scan,
        });
    }
    // the pieces: lines, and runs of scan segments fitted as one smooth curve
    let mut pieces: Vec<Curve> = vec![];
    let mut lens: Vec<f64> = vec![];
    let mut off2 = (0.0f64, 0usize);
    let mut s = 0;
    while s < segs.len() {
        match kinds[s] {
            Seg::Line => {
                let (a, b) = (pos(segs[s].0), pos(segs[s].1));
                pieces.push(Curve::line(a, b));
                lens.push((b - a).length());
                s += 1;
            }
            Seg::Fold(x) => {
                let (a, b) = (pos(segs[s].0), pos(segs[s].1));
                pieces.push(Curve::line(a, x));
                lens.push((x - a).length());
                pieces.push(Curve::line(x, b));
                lens.push((b - x).length());
                s += 1;
            }
            Seg::Scan => {
                let mut e = s;
                while e < segs.len() && matches!(kinds[e], Seg::Scan) {
                    e += 1;
                }
                // samples along the scan between the points: the shortest
                // way over its surface (around a hole, not into it); else
                // along the straight line, put on the scan
                let between = |i: usize, j: usize| -> Vec<DVec3> {
                    let (a, b) = (pos(i), pos(j));
                    let len = (b - a).length();
                    match surface_walk(m, topo, pts[i].tri, pts[i].hit, pts[j].tri, pts[j].hit) {
                        Some(walk) => {
                            // the edges' zigzag taken out: evened along its
                            // length, relaxed and put back on the scan
                            let mut line = vec![a];
                            line.extend(walk);
                            line.push(b);
                            let mut w = resample(&line, (len / 200.0).max(h));
                            for _ in 0..24 {
                                let prev = w.clone();
                                for k in 1..w.len().saturating_sub(1) {
                                    let x = prev[k] * 0.5 + (prev[k - 1] + prev[k + 1]) * 0.25;
                                    w[k] = onto_scan(m, scan, x, h, h * 8.0).unwrap_or(x);
                                }
                            }
                            w.pop();
                            w.remove(0);
                            w
                        }
                        None => {
                            let cnt = ((len / h.max(len / 160.0)).ceil() as usize).clamp(8, 160);
                            (1..cnt).map(|q| a + (b - a) * (q as f64 / cnt as f64)).map(|x| onto_scan(m, scan, x, h, len).unwrap_or(x)).collect()
                        }
                    }
                };
                // a closed path all on the scan goes round smoothly: fitted
                // with a segment more at each end, then cut where it passes
                // the first point
                let wrap = p.closed && s == 0 && e == segs.len() && segs.len() >= 3;
                let mut samples: Vec<(DVec3, f64)> = vec![];
                let mut cut = (0usize, 0usize);
                if wrap {
                    let last = *segs.last().unwrap();
                    samples.extend(between(last.0, last.1).into_iter().map(|x| (x, 1.0)));
                    cut.0 = samples.len();
                    samples.push((pos(0), 1e3));
                    for &(i, j) in &segs {
                        samples.extend(between(i, j).into_iter().map(|x| (x, 1.0)));
                        samples.push((pos(j), 1e3));
                    }
                    cut.1 = samples.len() - 1;
                    samples.extend(between(segs[0].0, segs[0].1).into_iter().map(|x| (x, 1.0)));
                } else {
                    for (k, &(i, j)) in segs[s..e].iter().enumerate() {
                        if k == 0 {
                            samples.push((pos(i), 1e6));
                        }
                        samples.extend(between(i, j).into_iter().map(|x| (x, 1.0)));
                        // a clicked point inside the run: passed close by;
                        // the run's end: pinned
                        samples.push((pos(j), if k + 1 == e - s { 1e6 } else { 1e3 }));
                    }
                }
                let mut acc = vec![0.0];
                for w in samples.windows(2) {
                    acc.push(acc.last().unwrap() + (w[1].0 - w[0].0).length());
                }
                let all = acc.last().copied().unwrap_or(0.0).max(1e-9);
                let data: Vec<(f64, DVec3, f64)> = samples.iter().zip(&acc).map(|((x, w), t)| (t / all, *x, *w)).collect();
                let total = if wrap { acc[cut.1] - acc[cut.0] } else { all };
                // spans: more than points, fewer as it gets smoother
                let spacing = h * (3.0 + 60.0 * p.smooth * p.smooth);
                let spans = ((all / spacing).round() as usize).clamp((e - s).max(1) + if wrap { 2 } else { 0 }, 160);
                let n_ctrl = spans + 3;
                let lam = 10f64.powf(-4.0 + 5.0 * p.smooth) * data.len() as f64 / n_ctrl as f64;
                let mut c = Curve::fit_3d(&data, 3, spans, lam).unwrap_or_else(|| Curve::line(pos(segs[s].0), pos(segs[e - 1].1)));
                if wrap && c.degree == 3 {
                    let (ta, tb) = (data[cut.0].0, data[cut.1].0);
                    if let Some((_, right)) = c.split(ta) {
                        if let Some((mid, _)) = right.split(tb) {
                            c = mid;
                            let n = c.pts.len();
                            c.pts[0] = pos(0);
                            c.pts[n - 1] = pos(0);
                        }
                    }
                }
                // how far the scan is from it
                for x in c.sample(spans * 8) {
                    if let Some(y) = onto_scan(m, scan, x, h, total) {
                        off2.0 += (y - x).length_squared();
                        off2.1 += 1;
                    }
                }
                pieces.push(c);
                lens.push(total);
                s = e;
            }
        }
    }
    let (curve, breaks) = Curve::join(&pieces, &lens);
    let kinds = kinds
        .iter()
        .map(|k| match k {
            Seg::Line => PieceKind::Line,
            Seg::Fold(_) => PieceKind::Fold,
            Seg::Scan => PieceKind::Scan,
        })
        .collect();
    let off = if off2.1 > 0 { (off2.0 / off2.1 as f64).sqrt() } else { 0.0 };
    let length = lens.iter().sum();
    Some(Built { curve, breaks, kinds, off, length })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box of `s` mm, cut into small triangles (`k` per side).
    fn boxy(s: f64, k: usize) -> Mesh {
        let mut m = Mesh::default();
        let faces = [(DVec3::X, DVec3::Y, DVec3::Z), (-DVec3::X, DVec3::Z, DVec3::Y), (DVec3::Y, DVec3::Z, DVec3::X), (-DVec3::Y, DVec3::X, DVec3::Z), (DVec3::Z, DVec3::X, DVec3::Y), (-DVec3::Z, DVec3::Y, DVec3::X)];
        let mut index: HashMap<[i64; 3], u32> = HashMap::new();
        let mut vid = |m: &mut Mesh, p: DVec3| -> u32 {
            let key = [(p.x * 1e6).round() as i64, (p.y * 1e6).round() as i64, (p.z * 1e6).round() as i64];
            *index.entry(key).or_insert_with(|| {
                m.pos.push(p);
                (m.pos.len() - 1) as u32
            })
        };
        for (n, u, v) in faces {
            let at = |i: usize, j: usize| (n + u * (i as f64 / k as f64 * 2.0 - 1.0) + v * (j as f64 / k as f64 * 2.0 - 1.0)) * (s * 0.5);
            for i in 0..k {
                for j in 0..k {
                    let a = vid(&mut m, at(i, j));
                    let b = vid(&mut m, at(i + 1, j));
                    let c = vid(&mut m, at(i + 1, j + 1));
                    let d = vid(&mut m, at(i, j + 1));
                    m.tris.push([a, b, c]);
                    m.tris.push([a, c, d]);
                }
            }
        }
        m.unit_mm = 1.0;
        m
    }

    fn tri_at(m: &Mesh, p: DVec3) -> u32 {
        (0..m.tris.len()).min_by(|&a, &b| (m.tri_centre(a) - p).length().partial_cmp(&(m.tri_centre(b) - p).length()).unwrap()).unwrap() as u32
    }

    #[test]
    fn a_box_gives_faces_edges_and_corners() {
        let m = boxy(20.0, 40);
        let topo = Topo::new(&m);
        let h = m.edge_median();
        // on a face
        let x = DVec3::new(10.0, 2.0, 3.0);
        let s = snap(&m, &topo, tri_at(&m, x), x, 0.5, h);
        assert_eq!(s.kind, SnapKind::Plane);
        assert!((s.p.x - 10.0).abs() < 1e-6);
        // near an edge
        let x = DVec3::new(10.0, 9.8, 3.0);
        let s = snap(&m, &topo, tri_at(&m, x), x, 0.5, h);
        assert_eq!(s.kind, SnapKind::Edge);
        assert!((s.p - DVec3::new(10.0, 10.0, 3.0)).length() < 1e-6, "{:?}", s.p);
        assert!((s.angle - 90.0).abs() < 1e-6);
        // near a corner
        let x = DVec3::new(10.0, 9.8, 9.7);
        let s = snap(&m, &topo, tri_at(&m, x), x, 0.6, h);
        assert_eq!(s.kind, SnapKind::Corner);
        assert!((s.p - DVec3::new(10.0, 10.0, 10.0)).length() < 1e-6, "{:?}", s.p);
    }

    #[test]
    fn a_path_folds_over_an_edge() {
        let m = boxy(20.0, 40);
        let topo = Topo::new(&m);
        let scan = Surface::new(&m);
        let h = m.edge_median();
        let a = DVec3::new(10.0, 2.0, -4.0);
        let b = DVec3::new(4.0, 10.0, 6.0);
        let sa = snap(&m, &topo, tri_at(&m, a), a, 0.5, h);
        let sb = snap(&m, &topo, tri_at(&m, b), b, 0.5, h);
        let built = build(&m, &topo, &scan, &[sa, sb], &PathParams::default(), h).unwrap();
        assert_eq!(built.kinds, vec![PieceKind::Fold]);
        // the corner on the edge, where the unfolded line crosses it
        let x = built.curve.eval(built.breaks[1]);
        assert!((x.x - 10.0).abs() < 1e-6 && (x.y - 10.0).abs() < 1e-6, "{x:?}");
        // unfolded: 8 to the edge on one face, 6 on the other, z from -4 to 6
        let z = -4.0 + 10.0 * 8.0 / 14.0;
        assert!((x.z - z).abs() < 1e-6, "{x:?} vs z {z}");
    }

    #[test]
    fn join_keeps_the_pieces() {
        let a = Curve::line(DVec3::ZERO, DVec3::X);
        let data: Vec<(f64, DVec3, f64)> = (0..=40).map(|k| {
            let t = k as f64 / 40.0;
            (t, DVec3::new(1.0 + t, (t * 3.0).sin(), 0.0), if k == 0 || k == 40 { 1e6 } else { 1.0 })
        }).collect();
        let b = Curve::fit_3d(&data, 3, 6, 1e-6).unwrap();
        let (c, breaks) = Curve::join(&[a.clone(), b.clone()], &[1.0, 1.5]);
        assert_eq!(c.knots.len(), c.pts.len() + 4);
        for k in 0..=20 {
            let t = k as f64 / 20.0;
            assert!((c.eval(breaks[0] + (breaks[1] - breaks[0]) * t) - a.eval(t)).length() < 1e-9);
            assert!((c.eval(breaks[1] + (breaks[2] - breaks[1]) * t) - b.eval(t)).length() < 1e-9);
        }
        assert!((b.eval(0.0) - DVec3::X).length() < 1e-4);
    }
}
