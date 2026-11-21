//! The scan's flat faces near a point (the Pen's plane, the planes under
//! the Circle). Around the point the scan's triangles are grouped by their
//! normals; each group's connected parts that are wide and flat enough get a
//! plane fitted to them (`planes_near`). The plane to draw on is the flat
//! face under the pointer, else the scan's tangent plane there
//! (`plane_under`). The scan does not change.

use crate::mesh::{fit_plane, Mesh, Topo, NONE};
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
    /// A plane through `c` facing `n` (no face of the scan behind it).
    pub fn through(c: DVec3, n: DVec3) -> Plane {
        Plane { c, n: n.normalize_or_zero(), rms: 0.0, area: 0.0, pts: vec![c] }
    }

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
    planes_near_bend(m, topo, t0, p, reach, h, MAX_BEND)
}

/// The same, faces bending up to `max_bend` (1 / mm) kept: the short walls
/// round a raised part, filleted at both ends, bend more in the window.
pub fn planes_near_bend(m: &Mesh, topo: &Topo, t0: u32, p: DVec3, reach: f64, h: f64, max_bend: f64) -> Vec<Plane> {
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
            if narrow < min_w || rms > max_rms || bend > max_bend {
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

/// The plane to draw on at `p` (on the triangle `t0` under the pointer):
/// the flat face there when there is one (found within `reach`), else the
/// scan's tangent plane at `p` (the normals of its triangles round it,
/// averaged). The bool: a flat face of the scan.
pub fn plane_under(m: &Mesh, topo: &Topo, t0: u32, p: DVec3, reach: f64, h: f64) -> (Plane, bool) {
    let local = tangent_under(m, topo, t0, p, reach, h);
    let tol = (h * 1.5).max(0.08);
    let flat = planes_near(m, topo, t0, p, reach, h)
        .into_iter()
        .filter(|pl| pl.dist(p).abs() <= tol + pl.rms * 3.0 && pl.gap(p) <= (h * 4.0).max(reach * 0.15) && pl.n.dot(local) > 20f64.to_radians().cos())
        .min_by(|a, b| a.dist(p).abs().partial_cmp(&b.dist(p).abs()).unwrap());
    if let Some(pl) = flat {
        return (pl, true);
    }
    (Plane { c: p, n: local, rms: 0.0, area: 0.0, pts: vec![p] }, false)
}

/// The scan's tangent plane's normal at `p` (on the triangle `t0`): the
/// normals of its triangles round it, a few edges wide (at least 0.12 of
/// `reach`), averaged by area. Cheap: `plane_under` without the search for
/// a flat face.
pub fn tangent_under(m: &Mesh, topo: &Topo, t0: u32, p: DVec3, reach: f64, h: f64) -> DVec3 {
    let r = (h * 5.0).max(reach * 0.12);
    let mut tris = vec![t0];
    let mut seen: HashMap<u32, ()> = HashMap::from([(t0, ())]);
    let mut k = 0;
    while k < tris.len() && tris.len() < 20_000 {
        let t = tris[k];
        k += 1;
        for &u in &topo.adj[t as usize] {
            if u != NONE && !seen.contains_key(&u) && (m.tri_centre(u as usize) - p).length() <= r {
                seen.insert(u, ());
                tris.push(u);
            }
        }
    }
    let local = tris.iter().map(|&t| m.tri_cross(t as usize)).sum::<DVec3>().normalize_or_zero();
    if local == DVec3::ZERO {
        m.tri_normal(t0 as usize)
    } else {
        local
    }
}

/// The way most of the scan faces within `reach` of `p` (the triangles
/// joined to `t0`): area-weighted, narrowed by mean shift from 30° to 5°.
/// Round a hole or a boss this is the face it is cut in or stands on, even
/// when `p` is on its rim's fillet or wall.
pub fn dominant_normal(m: &Mesh, topo: &Topo, t0: u32, p: DVec3, reach: f64) -> DVec3 {
    let mut tris = vec![t0];
    let mut seen: HashMap<u32, ()> = HashMap::from([(t0, ())]);
    let mut k = 0;
    while k < tris.len() && tris.len() < 200_000 {
        let t = tris[k];
        k += 1;
        for &u in &topo.adj[t as usize] {
            if u != NONE && !seen.contains_key(&u) && (m.tri_centre(u as usize) - p).length() <= reach {
                seen.insert(u, ());
                tris.push(u);
            }
        }
    }
    let crs: Vec<DVec3> = tris.iter().map(|&t| m.tri_cross(t as usize)).collect();
    let mut n = crs.iter().copied().sum::<DVec3>().normalize_or_zero();
    if n == DVec3::ZERO {
        return m.tri_normal(t0 as usize);
    }
    for deg in [30.0f64, 20.0, 12.0, 8.0, 5.0, 5.0] {
        let kk = deg.to_radians();
        let mut sum = DVec3::ZERO;
        for c in &crs {
            let a = c.length();
            if a < 1e-18 {
                continue;
            }
            let nn = *c / a;
            let cs = nn.dot(n);
            if cs > 0.0 {
                let ang = cs.min(1.0).acos();
                sum += nn * (a * (-(ang / kk).powi(2)).exp());
            }
        }
        if sum.length() > 1e-12 {
            n = sum.normalize();
        }
    }
    n
}
