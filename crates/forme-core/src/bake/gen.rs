//! Clean parametric tessellation of one primitive at one iso level, in its
//! local frame: surfaces of revolution (meridian × angle), rounded boxes,
//! quad spheres and rounded extrusions (stacked offset contours + n-gon caps).
//! Every vertex is finally snapped on the exact iso surface.

use super::field::{prim_grad, prim_project, prim_raw, sd_polygon, sd_regular_polygon, PrimDef};
use glam::{DVec2, DVec3};
use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, TAU};

#[derive(Clone, Copy, Debug)]
pub struct GenSettings {
    /// Maximum chord error (mm).
    pub tol: f64,
    /// Maximum edge length on flat or straight parts (mm).
    pub max_edge: f64,
    /// Forced segment count around surfaces of revolution (0 = automatic).
    pub segs: usize,
}

#[derive(Clone, Debug, Default)]
pub struct PolyMesh {
    pub pos: Vec<DVec3>,
    pub faces: Vec<Vec<u32>>,
}

impl PolyMesh {
    fn add(&mut self, p: DVec3) -> u32 {
        self.pos.push(p);
        self.pos.len() as u32 - 1
    }

    /// Merges vertices closer than `eps` and drops degenerate faces.
    pub fn weld(&mut self, eps: f64) {
        let cell = eps * 4.0;
        let key = |p: DVec3| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64, (p.z / cell).floor() as i64);
        let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
        let mut remap = vec![0u32; self.pos.len()];
        let mut out: Vec<DVec3> = vec![];
        for (i, p) in self.pos.iter().enumerate() {
            let k = key(*p);
            let mut found = None;
            'search: for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        if let Some(list) = grid.get(&(k.0 + dx, k.1 + dy, k.2 + dz)) {
                            for &j in list {
                                if out[j as usize].distance(*p) <= eps {
                                    found = Some(j);
                                    break 'search;
                                }
                            }
                        }
                    }
                }
            }
            remap[i] = match found {
                Some(j) => j,
                None => {
                    out.push(*p);
                    let j = out.len() as u32 - 1;
                    grid.entry(k).or_default().push(j);
                    j
                }
            };
        }
        self.pos = out;
        for f in &mut self.faces {
            for v in f.iter_mut() {
                *v = remap[*v as usize];
            }
        }
        self.clean_faces();
        // drop vertices no face uses
        let mut used = vec![u32::MAX; self.pos.len()];
        let mut pos = vec![];
        for f in &mut self.faces {
            for v in f.iter_mut() {
                if used[*v as usize] == u32::MAX {
                    used[*v as usize] = pos.len() as u32;
                    pos.push(self.pos[*v as usize]);
                }
                *v = used[*v as usize];
            }
        }
        self.pos = pos;
    }

    pub fn clean_faces(&mut self) {
        for f in &mut self.faces {
            f.dedup();
            while f.len() > 1 && f.first() == f.last() {
                f.pop();
            }
        }
        self.faces.retain(|f| {
            if f.len() < 3 {
                return false;
            }
            let mut s = f.clone();
            s.sort();
            s.dedup();
            s.len() >= 3
        });
    }

    fn face_normal(&self, f: &[u32]) -> DVec3 {
        let mut n = DVec3::ZERO;
        for i in 0..f.len() {
            let a = self.pos[f[i] as usize];
            let b = self.pos[f[(i + 1) % f.len()] as usize];
            n += a.cross(b);
        }
        n
    }
}

fn arc_step(radius: f64, tol: f64) -> f64 {
    if radius <= tol * 2.0 {
        FRAC_PI_2
    } else {
        (2.0 * (1.0 - tol / radius).acos()).clamp(0.02, FRAC_PI_2)
    }
}

/// Local bounds of a primitive at an iso level (conservative).
pub(crate) fn prim_bounds(pr: &PrimDef, level: f64) -> (DVec3, DVec3) {
    let s = pr.size;
    let he = match pr.kind {
        0 | 2 => s,
        1 | 6 | 7 => DVec3::new(s.x, s.y, s.x),
        3 => DVec3::new(s.x, s.y + s.x, s.x),
        4 => DVec3::new(s.x + s.z, s.z, s.x + s.z),
        5 => {
            let r = s.x.max(s.z);
            DVec3::new(r, s.y, r)
        }
        _ => {
            let mut lo = DVec2::splat(f64::MAX);
            let mut hi = DVec2::splat(f64::MIN);
            for q in &pr.poly {
                lo = lo.min(*q);
                hi = hi.max(*q);
            }
            let pad = DVec3::splat(level.abs() + pr.shell.abs() + pr.round + 0.2);
            return if pr.revolve {
                let r = lo.x.abs().max(hi.x.abs());
                (DVec3::new(-r, lo.y, -r) - pad, DVec3::new(r, hi.y, r) + pad)
            } else {
                (DVec3::new(lo.x, lo.y, -s.z) - pad, DVec3::new(hi.x, hi.y, s.z) + pad)
            };
        }
    };
    let grow = if pr.taper < 0.0 { 1.0 - pr.taper } else { 1.0 };
    let he = DVec3::new(he.x * grow, he.y, he.z * grow) + DVec3::splat(level.abs() + pr.shell.abs() + 0.2);
    (-he, he)
}

fn fine_feature(pr: &PrimDef, level: f64) -> f64 {
    let mut m = f64::MAX;
    for v in [pr.round, pr.chamfer, pr.size.x, pr.size.y, pr.size.z, level.abs(), pr.shell] {
        if v > 1e-6 {
            m = m.min(v);
        }
    }
    m
}

// ---------------------------------------------------------------------------
// 2D contours (dual contouring on a grid, corners kept, then simplified)
// ---------------------------------------------------------------------------

fn grad2(f: &dyn Fn(DVec2) -> f64, p: DVec2, h: f64) -> DVec2 {
    DVec2::new(f(p + DVec2::X * h) - f(p - DVec2::X * h), f(p + DVec2::Y * h) - f(p - DVec2::Y * h)) / (2.0 * h)
}

fn newton2(f: &dyn Fn(DVec2) -> f64, mut p: DVec2) -> DVec2 {
    for _ in 0..30 {
        let v = f(p);
        if v.abs() < 1e-12 {
            break;
        }
        let g = grad2(f, p, 1e-7);
        let gg = g.length_squared();
        if gg < 1e-14 {
            break;
        }
        let step = g * (v / gg);
        let q = p - step;
        if f(q).abs() > v.abs() {
            p -= step * 0.5;
        } else {
            p = q;
        }
    }
    p
}

/// Closed loops of the zero set of `f`, inside (f < 0) on the left.
pub fn trace_loops(f: &dyn Fn(DVec2) -> f64, lo: DVec2, hi: DVec2, h: f64) -> Vec<Vec<DVec2>> {
    let nx = (((hi.x - lo.x) / h).ceil() as usize).max(2);
    let ny = (((hi.y - lo.y) / h).ceil() as usize).max(2);
    let hx = (hi.x - lo.x) / nx as f64;
    let hy = (hi.y - lo.y) / ny as f64;
    let corner = |i: usize, j: usize| lo + DVec2::new(i as f64 * hx, j as f64 * hy);
    let mut val = vec![0.0f64; (nx + 1) * (ny + 1)];
    for j in 0..=ny {
        for i in 0..=nx {
            let mut v = f(corner(i, j));
            if v == 0.0 {
                v = 1e-13;
            }
            val[j * (nx + 1) + i] = v;
        }
    }
    let v = |i: usize, j: usize| val[j * (nx + 1) + i];
    let cross = |a: DVec2, va: f64, b: DVec2, vb: f64| -> DVec2 {
        // regula falsi on the edge
        let (mut a, mut va, mut b, mut vb) = (a, va, b, vb);
        let mut p = a + (b - a) * (va / (va - vb));
        for _ in 0..8 {
            let vp = f(p);
            if vp.abs() < 1e-13 {
                break;
            }
            if (vp < 0.0) == (va < 0.0) {
                a = p;
                va = vp;
            } else {
                b = p;
                vb = vp;
            }
            p = a + (b - a) * (va / (va - vb));
        }
        p
    };
    // one vertex per cell with a sign change
    let mut cell_vertex: HashMap<(usize, usize), usize> = HashMap::new();
    let mut verts: Vec<DVec2> = vec![];
    for j in 0..ny {
        for i in 0..nx {
            let c = [v(i, j), v(i + 1, j), v(i + 1, j + 1), v(i, j + 1)];
            let pos = [corner(i, j), corner(i + 1, j), corner(i + 1, j + 1), corner(i, j + 1)];
            let mut pts = vec![];
            for e in 0..4 {
                let (a, b) = (e, (e + 1) % 4);
                if (c[a] < 0.0) != (c[b] < 0.0) {
                    pts.push(cross(pos[a], c[a], pos[b], c[b]));
                }
            }
            if pts.is_empty() {
                continue;
            }
            let mass = pts.iter().fold(DVec2::ZERO, |s, p| s + *p) / pts.len() as f64;
            let (mut a11, mut a12, mut a22, mut b1, mut b2) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for p in &pts {
                let n = grad2(f, *p, h * 1e-3).normalize_or_zero();
                let d = n.dot(*p - mass);
                a11 += n.x * n.x;
                a12 += n.x * n.y;
                a22 += n.y * n.y;
                b1 += n.x * d;
                b2 += n.y * d;
            }
            let lam = 1e-3;
            a11 += lam;
            a22 += lam;
            let det = a11 * a22 - a12 * a12;
            let mut x = mass;
            if det.abs() > 1e-18 {
                x += DVec2::new((a22 * b1 - a12 * b2) / det, (a11 * b2 - a12 * b1) / det);
            }
            let pad = DVec2::new(hx, hy) * 0.5;
            x = x.clamp(pos[0] - pad, pos[2] + pad);
            cell_vertex.insert((i, j), verts.len());
            verts.push(x);
        }
    }
    // oriented segments between cells across each crossed grid edge
    let mut next: HashMap<usize, Vec<usize>> = HashMap::new();
    for j in 1..ny {
        for i in 0..nx {
            let (a, b) = (v(i, j), v(i + 1, j));
            if (a < 0.0) != (b < 0.0) {
                let below = cell_vertex[&(i, j - 1)];
                let above = cell_vertex[&(i, j)];
                if a < 0.0 {
                    next.entry(below).or_default().push(above);
                } else {
                    next.entry(above).or_default().push(below);
                }
            }
        }
    }
    for j in 0..ny {
        for i in 1..nx {
            let (a, b) = (v(i, j), v(i, j + 1));
            if (a < 0.0) != (b < 0.0) {
                let left = cell_vertex[&(i - 1, j)];
                let right = cell_vertex[&(i, j)];
                if b < 0.0 {
                    next.entry(left).or_default().push(right);
                } else {
                    next.entry(right).or_default().push(left);
                }
            }
        }
    }
    let mut loops = vec![];
    let mut keys: Vec<usize> = next.keys().copied().collect();
    keys.sort();
    for start in keys {
        while next.get(&start).map(|l| !l.is_empty()).unwrap_or(false) {
            let mut lp = vec![start];
            let mut cur = start;
            for _ in 0..verts.len() + 2 {
                let Some(list) = next.get_mut(&cur) else { break };
                let Some(n) = list.pop() else { break };
                if n == start {
                    break;
                }
                lp.push(n);
                cur = n;
            }
            if lp.len() >= 3 {
                loops.push(lp.iter().map(|&i| verts[i]).collect());
            }
        }
    }
    loops
}

/// A simplified contour point (exact on the curve) and whether it is a corner.
#[derive(Clone, Copy, Debug)]
pub struct CPt {
    pub p: DVec2,
    pub corner: bool,
}

fn seg_dist(p: DVec2, a: DVec2, b: DVec2) -> f64 {
    let e = b - a;
    let t = ((p - a).dot(e) / e.length_squared().max(1e-30)).clamp(0.0, 1.0);
    (a + e * t).distance(p)
}

fn dp(pts: &[DVec2], i0: usize, i1: usize, tol: f64, keep: &mut Vec<bool>) {
    if i1 <= i0 + 1 {
        return;
    }
    let (a, b) = (pts[i0], pts[i1]);
    let mut best = (0.0, 0);
    for i in i0 + 1..i1 {
        let d = seg_dist(pts[i], a, b);
        if d > best.0 {
            best = (d, i);
        }
    }
    if best.0 > tol {
        keep[best.1] = true;
        dp(pts, i0, best.1, tol, keep);
        dp(pts, best.1, i1, tol, keep);
    }
}

fn turning(a: DVec2, b: DVec2, c: DVec2) -> f64 {
    let u = (b - a).normalize_or_zero();
    let v = (c - b).normalize_or_zero();
    u.perp_dot(v).atan2(u.dot(v))
}

/// Simplifies a traced polyline: corners kept, curved parts within `tol`,
/// points projected on the curve, long segments split to `max_edge`.
/// `closed`: loop; else the first and last points are fixed (and exact).
pub fn simplify(f: &dyn Fn(DVec2) -> f64, pts: &[DVec2], closed: bool, tol: f64, max_edge: f64) -> Vec<CPt> {
    let n = pts.len();
    if n < 2 {
        return pts.iter().map(|&p| CPt { p, corner: true }).collect();
    }
    let mut corner = vec![false; n];
    for i in 0..n {
        let (ia, ic) = if closed {
            ((i + n - 1) % n, (i + 1) % n)
        } else if i == 0 || i == n - 1 {
            corner[i] = true;
            continue;
        } else {
            (i - 1, i + 1)
        };
        if turning(pts[ia], pts[i], pts[ic]).abs() > 15f64.to_radians() {
            corner[i] = true;
        }
    }
    // unroll a loop so that it starts on a corner (or anywhere if none);
    // `anchor` = points DP must keep (corners, ends, split points)
    let (seq, cor, anchor): (Vec<DVec2>, Vec<bool>, Vec<bool>) = if closed {
        let start = corner.iter().position(|c| *c).unwrap_or(0);
        let s: Vec<DVec2> = (0..=n).map(|k| pts[(start + k) % n]).collect();
        let c: Vec<bool> = (0..=n).map(|k| corner[(start + k) % n]).collect();
        let mut a = c.clone();
        if !corner.iter().any(|c| *c) {
            let far = (1..n).max_by(|&x, &y| s[0].distance(s[x]).partial_cmp(&s[0].distance(s[y])).unwrap()).unwrap();
            a[far] = true;
        }
        a[0] = true;
        a[n] = true;
        (s, c, a)
    } else {
        let mut c = corner.clone();
        c[0] = false;
        c[n - 1] = false;
        (pts.to_vec(), c, corner.clone())
    };
    let m = seq.len();
    let mut keep = anchor.clone();
    let mut last = 0;
    for i in 1..m {
        if anchor[i] {
            dp(&seq, last, i, tol * 0.5, &mut keep);
            last = i;
        }
    }
    let mut out: Vec<CPt> = vec![];
    let lim = if closed { m - 1 } else { m };
    for i in 0..lim {
        if keep[i] {
            let fixed = !closed && (i == 0 || i == m - 1);
            let p = if cor[i] || fixed { seq[i] } else { newton2(f, seq[i]) };
            out.push(CPt { p, corner: cor[i] });
        }
    }
    // split long segments
    let mut res: Vec<CPt> = vec![];
    let cnt = out.len();
    let segs = if closed { cnt } else { cnt - 1 };
    for i in 0..cnt {
        res.push(out[i]);
        if i >= segs {
            continue;
        }
        let a = out[i].p;
        let b = out[(i + 1) % cnt].p;
        let l = a.distance(b);
        if l > max_edge {
            let k = (l / max_edge).ceil() as usize;
            for s in 1..k {
                let q = a + (b - a) * (s as f64 / k as f64);
                res.push(CPt { p: newton2(f, q), corner: false });
            }
        }
    }
    res
}

// ---------------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------------

pub fn generate(pr: &PrimDef, level: f64, st: &GenSettings) -> PolyMesh {
    let mut m = match pr.kind {
        0 => rounded_box(pr, level, st),
        2 => quad_sphere(pr, level, st),
        6 => extrusion(pr, level, st),
        8 if !pr.revolve => extrusion(pr, level, st),
        _ => revolve(pr, level, st),
    };
    let scale = prim_bounds(pr, 0.0).1.max_element().max(1.0);
    m.weld(scale * 1e-9);
    for p in &mut m.pos {
        *p = prim_project(pr, *p, level);
    }
    m.weld(scale * 1e-9);
    // outward orientation from the exact gradient
    for f in &mut m.faces {
        let pos = &m.pos;
        let mut n = DVec3::ZERO;
        let mut c = DVec3::ZERO;
        for i in 0..f.len() {
            let a = pos[f[i] as usize];
            let b = pos[f[(i + 1) % f.len()] as usize];
            n += a.cross(b);
            c += a;
        }
        c /= f.len() as f64;
        if n.length_squared() < 1e-24 {
            continue;
        }
        if n.dot(prim_grad(pr, c)) < 0.0 {
            f.reverse();
        }
    }
    m
}

/// Grid of the zero set for the meridian / section tracing.
fn section_grid(lo: DVec2, hi: DVec2, fine: f64) -> f64 {
    let span = (hi - lo).max_element();
    let area = (hi - lo).x * (hi - lo).y;
    let mut h = (span / 400.0).clamp(0.004, 0.08);
    h = h.min(fine / 4.0);
    h.max((area / 3.0e6).sqrt()).max(span / 4000.0)
}

fn revolve(pr: &PrimDef, level: f64, st: &GenSettings) -> PolyMesh {
    let g = |q: DVec2| prim_raw(pr, DVec3::new(q.x, q.y, 0.0)) - level;
    let (blo, bhi) = prim_bounds(pr, level);
    let r = bhi.x.max(-blo.x).max(bhi.z).max(-blo.z) + 0.3;
    let lo = DVec2::new(-r, blo.y - 0.3);
    let hi = DVec2::new(r, bhi.y + 0.3);
    let h = section_grid(lo, hi, fine_feature(pr, level));
    // even cell count across x so that x = 0 is a grid line
    let nx = ((2.0 * r / h).ceil() as usize).div_ceil(2) * 2;
    let r = nx as f64 * h * 0.5;
    let lo = DVec2::new(-r, lo.y);
    let hi = DVec2::new(r, hi.y);
    let loops = trace_loops(&g, lo, hi, h);
    let mut mesh = PolyMesh::default();
    let mut meridians: Vec<(Vec<CPt>, bool)> = vec![];
    for lp in loops {
        let n = lp.len();
        if lp.iter().all(|p| p.x > 0.0) {
            meridians.push((simplify(&g, &lp, true, st.tol, st.max_edge), true));
            continue;
        }
        if lp.iter().all(|p| p.x <= 0.0) {
            continue;
        }
        // cut the symmetric loop on the axis, keep the x >= 0 half (pole to pole)
        let crossings: Vec<usize> = (0..n).filter(|&i| (lp[i].x < 0.0) != (lp[(i + 1) % n].x < 0.0)).collect();
        if crossings.len() != 2 {
            continue;
        }
        let axis_point = |i: usize| -> DVec2 {
            let a = lp[i];
            let b = lp[(i + 1) % n];
            let t = a.x / (a.x - b.x);
            let mut y = a.y + (b.y - a.y) * t;
            for _ in 0..40 {
                let v = g(DVec2::new(0.0, y));
                let d = (g(DVec2::new(0.0, y + 1e-7)) - g(DVec2::new(0.0, y - 1e-7))) / 2e-7;
                if d.abs() < 1e-12 || v.abs() < 1e-13 {
                    break;
                }
                y -= (v / d).clamp(-h, h);
            }
            DVec2::new(0.0, y)
        };
        let start = crossings.iter().copied().find(|&i| lp[i].x < 0.0).unwrap_or(crossings[0]);
        let mut half = vec![axis_point(start)];
        let mut i = (start + 1) % n;
        while lp[i].x >= 0.0 {
            half.push(lp[i]);
            i = (i + 1) % n;
        }
        half.push(axis_point((i + n - 1) % n));
        meridians.push((simplify(&g, &half, false, st.tol, st.max_edge), false));
    }
    let rmax = meridians.iter().flat_map(|(m, _)| m.iter().map(|c| c.p.x)).fold(0.0f64, f64::max);
    if rmax <= 0.0 {
        return mesh;
    }
    let mut segs = (TAU / arc_step(rmax, st.tol)).ceil() as usize;
    segs = segs.max((TAU * rmax / st.max_edge).ceil() as usize).max(12);
    segs = segs.div_ceil(8) * 8;
    if st.segs >= 3 {
        segs = st.segs;
    }
    for (mer, closed) in meridians {
        let rows: Vec<Vec<u32>> = mer
            .iter()
            .map(|c| {
                if c.p.x.abs() < 1e-9 {
                    let id = mesh.add(DVec3::new(0.0, c.p.y, 0.0));
                    vec![id; segs]
                } else {
                    (0..segs)
                        .map(|j| {
                            let a = TAU * j as f64 / segs as f64;
                            mesh.add(DVec3::new(c.p.x * a.cos(), c.p.y, c.p.x * a.sin()))
                        })
                        .collect()
                }
            })
            .collect();
        let nr = rows.len();
        // flat discs ending on the axis become one n-gon (like a CAD cap)
        let (mut first, mut last) = (0usize, nr - 1);
        if !closed && nr >= 3 {
            let eps = 1e-9 * (1.0 + rmax);
            let mut k = 0;
            while k + 1 < nr - 1 && (mer[k + 1].p.y - mer[0].p.y).abs() < eps {
                k += 1;
            }
            if k >= 1 {
                quad_disc(&mut mesh, &rows[k], mer[k].p.x, mer[k].p.y);
                first = k;
            }
            let mut k = nr - 1;
            while k > first + 1 && (mer[k - 1].p.y - mer[nr - 1].p.y).abs() < eps {
                k -= 1;
            }
            if k < nr - 1 {
                quad_disc(&mut mesh, &rows[k], mer[k].p.x, mer[k].p.y);
                last = k;
            }
        }
        let pairs = if closed { nr } else { last };
        for i in first..pairs {
            let (ra, rb) = (&rows[i], &rows[(i + 1) % nr]);
            for j in 0..segs {
                let j2 = (j + 1) % segs;
                mesh.faces.push(vec![ra[j], ra[j2], rb[j2], rb[j]]);
            }
        }
    }
    mesh.clean_faces();
    mesh
}

/// All-quad disc filling a ring of N (multiple of 8) vertices at radius `r`,
/// height `y`: a square grid bent by the Shirley–Chiu concentric mapping,
/// whose border lands exactly on the ring's uniform angles.
fn quad_disc(mesh: &mut PolyMesh, ring: &[u32], r: f64, y: f64) {
    let n = ring.len();
    let m = n / 4;
    if m < 2 || n % 8 != 0 {
        mesh.faces.push(ring.to_vec());
        return;
    }
    let step = TAU / n as f64;
    let mut ids = vec![0u32; (m + 1) * (m + 1)];
    for j in 0..=m {
        for i in 0..=m {
            let a = 2.0 * i as f64 / m as f64 - 1.0;
            let b = 2.0 * j as f64 / m as f64 - 1.0;
            let (rr, phi) = if a == 0.0 && b == 0.0 {
                (0.0, 0.0)
            } else if a.abs() > b.abs() {
                (a, FRAC_PI_4 * (b / a))
            } else {
                (b, FRAC_PI_2 - FRAC_PI_4 * (a / b))
            };
            let on_border = i == 0 || j == 0 || i == m || j == m;
            ids[j * (m + 1) + i] = if on_border {
                // angle of the border point, matched to the ring
                let ang = if rr < 0.0 { phi + std::f64::consts::PI } else { phi };
                let k = ((ang / step).round() as i64).rem_euclid(n as i64) as usize;
                ring[k]
            } else {
                mesh.add(DVec3::new(r * rr * phi.cos(), y, r * rr * phi.sin()))
            };
        }
    }
    for j in 0..m {
        for i in 0..m {
            let w = m + 1;
            mesh.faces.push(vec![ids[j * w + i], ids[j * w + i + 1], ids[(j + 1) * w + i + 1], ids[(j + 1) * w + i]]);
        }
    }
}

fn quad_sphere(pr: &PrimDef, level: f64, st: &GenSettings) -> PolyMesh {
    let r = pr.size.max(DVec3::splat(1e-3)) + DVec3::splat(level);
    let r = r.max(DVec3::splat(1e-3));
    let rmax = r.max_element();
    let mut n = ((FRAC_PI_2) / arc_step(rmax, st.tol)).ceil() as usize;
    n = n.max((FRAC_PI_2 * rmax / st.max_edge).ceil() as usize).max(2);
    let mut mesh = PolyMesh::default();
    let axes = [DVec3::X, DVec3::Y, DVec3::Z];
    let coord = |i: usize| -> f64 {
        if i == 0 {
            -1.0
        } else if i == n {
            1.0
        } else {
            (FRAC_PI_4 * (2.0 * i as f64 / n as f64 - 1.0)).tan()
        }
    };
    for a in 0..3 {
        for s in [-1.0, 1.0] {
            let na = axes[a] * s;
            let b = axes[(a + 1) % 3];
            let c = axes[(a + 2) % 3];
            let mut ids = vec![0u32; (n + 1) * (n + 1)];
            for j in 0..=n {
                for i in 0..=n {
                    let d = (na + b * coord(i) + c * coord(j)).normalize();
                    ids[j * (n + 1) + i] = mesh.add(d * r);
                }
            }
            for j in 0..n {
                for i in 0..n {
                    let q = [ids[j * (n + 1) + i], ids[j * (n + 1) + i + 1], ids[(j + 1) * (n + 1) + i + 1], ids[(j + 1) * (n + 1) + i]];
                    mesh.faces.push(q.to_vec());
                }
            }
        }
    }
    mesh.weld(rmax * 1e-9);
    mesh
}

/// Box with one fillet (or chamfer) per edge. The grid is built in the
/// sharp box's parameter space, with a band of rows at each end of each
/// axis as wide as the largest fillet around that face; every vertex is
/// then cast from the inner point of its band onto the exact surface. One
/// mapping for all faces: seams and corners stay watertight.
fn rounded_box_edges(pr: &PrimDef, st: &GenSettings) -> PolyMesh {
    let size = pr.size;
    let er = |e: usize| pr.edges[e];
    // edge between face (a, sa) and face (b, sb): parallel to the third axis
    let edge_between = |a: usize, sa: f64, b: usize, sb: f64| -> usize {
        let c = 3 - a - b;
        let (u, v) = ((c + 1) % 3, (c + 2) % 3);
        let sgn = |ax: usize| if ax == a { sa } else { sb };
        let k = match (sgn(u) >= 0.0, sgn(v) >= 0.0) {
            (true, true) => 0,
            (false, true) => 1,
            (false, false) => 2,
            (true, false) => 3,
        };
        c * 4 + k
    };
    let mut band = [[0.0f64; 2]; 3];
    for ax in 0..3 {
        for (ei, end) in [-1.0f64, 1.0].iter().enumerate() {
            let mut m = 0.0f64;
            for o in 0..3 {
                if o != ax {
                    for so in [-1.0, 1.0] {
                        let r = er(edge_between(ax, *end, o, so)).abs();
                        let cap = size[ax].min(size[o]);
                        m = m.max(r.min(cap));
                    }
                }
            }
            band[ax][ei] = m.min(size[ax] * 0.999);
        }
    }
    let samples = |ax: usize| -> Vec<f64> {
        let (b0, b1) = (band[ax][0], band[ax][1]);
        let (lo, hi) = (-(size[ax] - b0), size[ax] - b1);
        let mut v = vec![];
        let k = (((hi - lo) / st.max_edge).ceil() as usize).max(1);
        for i in 0..=k {
            v.push(lo + (hi - lo) * i as f64 / k as f64);
        }
        for (bw, sign) in [(b0, -1.0), (b1, 1.0)] {
            if bw <= 1e-12 {
                continue;
            }
            let m = ((FRAC_PI_4 / arc_step(bw, st.tol)).ceil() as usize).max(1);
            for j in 1..=m {
                let t = if j == m { 1.0 } else { (FRAC_PI_4 * j as f64 / m as f64).tan() };
                let x = sign * (size[ax] - bw + bw * t);
                if sign > 0.0 {
                    v.push(x);
                } else {
                    v.insert(0, x);
                }
            }
        }
        v
    };
    let s3 = [samples(0), samples(1), samples(2)];
    let map = |y: DVec3| -> DVec3 {
        let mut c = y;
        for ax in 0..3 {
            let lim = if y[ax] >= 0.0 { size[ax] - band[ax][1] } else { size[ax] - band[ax][0] };
            c[ax] = y[ax].clamp(-lim, lim);
        }
        let d = y - c;
        let len = d.length();
        if len < 1e-12 {
            return y;
        }
        let dir = d / len;
        // the sharp box contains the filleted one: the surface is on [c, y]
        let (mut a, mut b) = (0.0, len);
        for _ in 0..60 {
            let m = 0.5 * (a + b);
            if prim_raw(pr, c + dir * m) < 0.0 {
                a = m;
            } else {
                b = m;
            }
        }
        c + dir * (0.5 * (a + b))
    };
    let mut mesh = PolyMesh::default();
    for a in 0..3 {
        for s in [-1.0, 1.0] {
            let b = (a + 1) % 3;
            let c = (a + 2) % 3;
            let (sb, scn) = (&s3[b], &s3[c]);
            let mut ids = vec![0u32; sb.len() * scn.len()];
            for (j, &vc) in scn.iter().enumerate() {
                for (i, &vb) in sb.iter().enumerate() {
                    let mut x = DVec3::ZERO;
                    x[a] = s * size[a];
                    x[b] = vb;
                    x[c] = vc;
                    ids[j * sb.len() + i] = mesh.add(map(x));
                }
            }
            for j in 0..scn.len() - 1 {
                for i in 0..sb.len() - 1 {
                    let w = sb.len();
                    mesh.faces.push(vec![ids[j * w + i], ids[j * w + i + 1], ids[(j + 1) * w + i + 1], ids[(j + 1) * w + i]]);
                }
            }
        }
    }
    mesh
}

fn rounded_box(pr: &PrimDef, level: f64, st: &GenSettings) -> PolyMesh {
    if pr.edges.len() == 12 && level.abs() < 1e-12 {
        return rounded_box_edges(pr, st);
    }
    let size = pr.size;
    let r = pr.round.min(size.min_element());
    let mut inner = size - DVec3::splat(r);
    let mut rho = r + level;
    if rho < 0.0 {
        inner += DVec3::splat(rho);
        rho = 0.0;
    }
    let ch = pr.chamfer;
    if ch > rho {
        inner -= DVec3::splat(ch - rho);
        rho = ch;
    }
    let inner = inner.max(DVec3::ZERO);
    let m = if rho > 0.0 { if ch > 0.0 && pr.round < ch { 1 } else { ((FRAC_PI_4) / arc_step(rho, st.tol)).ceil() as usize } } else { 0 };
    let samples = |a: f64| -> Vec<f64> {
        let mut v = vec![];
        if a > 1e-12 {
            let k = ((2.0 * a / st.max_edge).ceil() as usize).max(1);
            for i in 0..=k {
                v.push(-a + 2.0 * a * i as f64 / k as f64);
            }
        } else {
            v.push(0.0);
        }
        for j in 1..=m {
            let t = if j == m { 1.0 } else { (FRAC_PI_4 * j as f64 / m as f64).tan() };
            v.push(a + rho * t);
            v.insert(0, -(a + rho * t));
        }
        v
    };
    let s3 = [samples(inner.x), samples(inner.y), samples(inner.z)];
    let map = |x: DVec3| -> DVec3 {
        if rho <= 0.0 {
            return x;
        }
        let c = x.clamp(-inner, inner);
        c + (x - c).normalize() * rho
    };
    let mut mesh = PolyMesh::default();
    for a in 0..3 {
        for s in [-1.0, 1.0] {
            let b = (a + 1) % 3;
            let c = (a + 2) % 3;
            let (sb, scn) = (&s3[b], &s3[c]);
            let mut ids = vec![0u32; sb.len() * scn.len()];
            for (j, &vc) in scn.iter().enumerate() {
                for (i, &vb) in sb.iter().enumerate() {
                    let mut x = DVec3::ZERO;
                    x[a] = s * (inner[a] + rho);
                    x[b] = vb;
                    x[c] = vc;
                    ids[j * sb.len() + i] = mesh.add(map(x));
                }
            }
            for j in 0..scn.len() - 1 {
                for i in 0..sb.len() - 1 {
                    let w = sb.len();
                    mesh.faces.push(vec![ids[j * w + i], ids[j * w + i + 1], ids[(j + 1) * w + i + 1], ids[(j + 1) * w + i]]);
                }
            }
        }
    }
    mesh
}

// --- polygon utilities for caps ---------------------------------------------

fn signed_area(p: &[DVec2]) -> f64 {
    let n = p.len();
    (0..n).map(|i| p[i].perp_dot(p[(i + 1) % n])).sum::<f64>() * 0.5
}

/// Convex (collinear points allowed), whatever the orientation.
pub fn is_convex(p: &[DVec2]) -> bool {
    let n = p.len();
    let o = if signed_area(p) >= 0.0 { 1.0 } else { -1.0 };
    let scale = p.iter().fold(0.0f64, |m, q| m.max(q.abs().max_element())).max(1e-9);
    (0..n).all(|i| (p[i] - p[(i + n - 1) % n]).perp_dot(p[(i + 1) % n] - p[i]) * o >= -1e-9 * scale * scale)
}

/// Ear clipping of a simple polygon (any orientation). Returns triangles in
/// the polygon's own orientation.
pub fn ear_clip(p: &[DVec2]) -> Vec<[usize; 3]> {
    let n = p.len();
    if n < 3 {
        return vec![];
    }
    let ccw = signed_area(p) > 0.0;
    let mut idx: Vec<usize> = if ccw { (0..n).collect() } else { (0..n).rev().collect() };
    let mut tris = vec![];
    let scale = p.iter().fold(0.0f64, |m, q| m.max(q.abs().max_element())).max(1e-9);
    let eps = scale * scale * 1e-14;
    let inside = |a: DVec2, b: DVec2, c: DVec2, q: DVec2| -> bool {
        (b - a).perp_dot(q - a) > -eps && (c - b).perp_dot(q - b) > -eps && (a - c).perp_dot(q - c) > -eps
    };
    let mut guard = 0;
    while idx.len() > 3 && guard < n * n {
        guard += 1;
        let m = idx.len();
        let mut best: Option<(usize, f64)> = None;
        for i in 0..m {
            let (ia, ib, ic) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let (a, b, c) = (p[ia], p[ib], p[ic]);
            let cr = (b - a).perp_dot(c - b);
            if cr <= eps {
                continue;
            }
            let blocked = idx.iter().any(|&k| k != ia && k != ib && k != ic && p[k] != a && p[k] != b && p[k] != c && inside(a, b, c, p[k]));
            if blocked {
                continue;
            }
            // prefer well shaped ears
            let q = cr / ((b - a).length_squared() + (c - b).length_squared() + (a - c).length_squared()).max(1e-30);
            if best.map(|(_, bq)| q > bq).unwrap_or(true) {
                best = Some((i, q));
            }
        }
        let i = match best {
            Some((i, _)) => i,
            None => {
                // degenerate input: clip the most convex vertex anyway
                (0..m)
                    .max_by(|&x, &y| {
                        let t = |i: usize| (p[idx[i]] - p[idx[(i + m - 1) % m]]).perp_dot(p[idx[(i + 1) % m]] - p[idx[i]]);
                        t(x).partial_cmp(&t(y)).unwrap()
                    })
                    .unwrap()
            }
        };
        tris.push([idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]]);
        idx.remove(i);
    }
    if idx.len() == 3 {
        tris.push([idx[0], idx[1], idx[2]]);
    }
    if !ccw {
        for t in &mut tris {
            t.swap(0, 2);
        }
    }
    tris
}

/// Hertel–Mehlhorn: merges triangles across diagonals while the result stays convex.
pub fn convex_merge(p: &[DVec2], tris: &[[usize; 3]]) -> Vec<Vec<usize>> {
    let mut polys: Vec<Vec<usize>> = tris.iter().map(|t| t.to_vec()).collect();
    let orient = if signed_area(p) >= 0.0 { 1.0 } else { -1.0 };
    let convex = |poly: &[usize]| -> bool {
        let m = poly.len();
        (0..m).all(|i| {
            let (a, b, c) = (p[poly[(i + m - 1) % m]], p[poly[i]], p[poly[(i + 1) % m]]);
            (b - a).perp_dot(c - b) * orient >= -1e-12
        })
    };
    loop {
        let mut merged = false;
        'outer: for i in 0..polys.len() {
            for j in i + 1..polys.len() {
                let (a, b) = (&polys[i], &polys[j]);
                // shared edge u->v in a, v->u in b
                for ea in 0..a.len() {
                    let (u, v) = (a[ea], a[(ea + 1) % a.len()]);
                    if let Some(eb) = (0..b.len()).find(|&k| b[k] == v && b[(k + 1) % b.len()] == u) {
                        let mut np = vec![];
                        for k in 0..a.len() {
                            np.push(a[(ea + 1 + k) % a.len()]);
                            if k == a.len() - 1 {
                                break;
                            }
                        }
                        // np starts at v ... ends at u; append b's path from u to v (exclusive)
                        let mut k = (eb + 2) % b.len();
                        while b[k] != v {
                            np.push(b[k]);
                            k = (k + 1) % b.len();
                        }
                        if convex(&np) {
                            polys[i] = np;
                            polys.remove(j);
                            merged = true;
                            break 'outer;
                        }
                    }
                }
            }
        }
        if !merged {
            break;
        }
    }
    polys
}

/// Rounded extrusion (prism, extruded profile): stacked offset contours of the
/// section, quarter-circle rows on the rounded edges, convex n-gon caps.
fn extrusion(pr: &PrimDef, level: f64, st: &GenSettings) -> PolyMesh {
    let (e_fn, hin, rr, axis_y): (Box<dyn Fn(DVec2) -> f64 + '_>, f64, f64, bool) = if pr.kind == 6 {
        let r = pr.round.min(pr.size.y);
        let rp = pr.size.x - r;
        let sides = pr.sides.max(3.0);
        (Box::new(move |q: DVec2| sd_regular_polygon(q, rp, sides)), pr.size.y - r, r, true)
    } else {
        let round = pr.round;
        let poly = &pr.poly;
        (Box::new(move |q: DVec2| sd_polygon(poly, q) + round), (pr.size.z - round).max(0.0), round, false)
    };
    let to3 = |q: DVec2, hgt: f64| -> DVec3 {
        if axis_y {
            DVec3::new(q.x, hgt, q.y)
        } else {
            DVec3::new(q.x, q.y, hgt)
        }
    };
    let k = rr + level;
    let mut mesh = PolyMesh::default();
    let (blo, bhi) = prim_bounds(pr, level);
    let (lo, hi) = if axis_y {
        (DVec2::new(blo.x, blo.z) - DVec2::splat(0.3), DVec2::new(bhi.x, bhi.z) + DVec2::splat(0.3))
    } else {
        (DVec2::new(blo.x, blo.y) - DVec2::splat(0.3), DVec2::new(bhi.x, bhi.y) + DVec2::splat(0.3))
    };
    let h = section_grid(lo, hi, fine_feature(pr, level));
    let outer_level = k;
    let fo = |q: DVec2| e_fn(q) - outer_level;
    let loops = trace_loops(&fo, lo, hi, h);
    let m_arc = if k > 0.0 { ((FRAC_PI_2) / arc_step(k, st.tol)).ceil() as usize } else { 0 };
    let height = if k > 0.0 { hin } else { hin + k };
    if height < 0.0 {
        return mesh;
    }
    for lp in loops {
        if signed_area(&lp) <= 0.0 {
            continue; // holes are not produced by these sections
        }
        let outer = simplify(&fo, &lp, true, st.tol, st.max_edge);
        let n = outer.len();
        if n < 3 {
            continue;
        }
        // per outer point: one or more columns (reflex corners fan out on an arc)
        struct Col {
            base: DVec2,
            kind: u8, // 0 smooth, 1 convex corner, 2 reflex corner
            n1: DVec2,
            n2: DVec2,
            t: f64,
        }
        let mut cols: Vec<Col> = vec![];
        for i in 0..n {
            let a = outer[(i + n - 1) % n].p;
            let b = outer[i].p;
            let c = outer[(i + 1) % n].p;
            let e1 = (b - a).normalize_or_zero();
            let e2 = (c - b).normalize_or_zero();
            let n1 = DVec2::new(e1.y, -e1.x);
            let n2 = DVec2::new(e2.y, -e2.x);
            if !outer[i].corner || k <= 0.0 {
                cols.push(Col { base: b, kind: 0, n1, n2, t: 0.0 });
            } else if e1.perp_dot(e2) > 0.0 {
                cols.push(Col { base: b, kind: 1, n1, n2, t: 0.0 });
            } else {
                let span = n1.angle_to(n2).abs();
                let cnt = ((span / arc_step(k, st.tol)).ceil() as usize).max(1) + 1;
                for s in 0..cnt {
                    cols.push(Col { base: b, kind: 2, n1, n2, t: s as f64 / (cnt - 1) as f64 });
                }
            }
        }
        // rows of the rounded band: theta 0 (outer contour) .. pi/2 (cap edge)
        let nrows = m_arc + 1;
        let mut rows2d: Vec<Vec<DVec2>> = vec![cols.iter().map(|c| c.base).collect()];
        for j in 1..nrows {
            let th = FRAC_PI_2 * j as f64 / m_arc as f64;
            let lj = k * th.cos();
            let delta = k - lj;
            let prev = rows2d[j - 1].clone();
            let row: Vec<DVec2> = cols
                .iter()
                .enumerate()
                .map(|(ci, c)| match c.kind {
                    1 => c.base - (c.n1 + c.n2) * (delta / (1.0 + c.n1.dot(c.n2)).max(0.05)),
                    2 => {
                        let a0 = c.n1.y.atan2(c.n1.x);
                        let u = a0 + c.n1.angle_to(c.n2) * c.t;
                        c.base - DVec2::new(u.cos(), u.sin()) * delta
                    }
                    _ => {
                        let target = |q: DVec2| e_fn(q) - lj;
                        newton2(&target, prev[ci])
                    }
                })
                .collect();
            rows2d.push(row);
        }
        // stack of (row, height) from bottom cap to top cap
        let mut stack: Vec<(usize, f64)> = vec![];
        let wall = |stack: &mut Vec<(usize, f64)>, from: f64, to: f64| {
            let cnt = (((to - from) / st.max_edge).ceil() as usize).max(1);
            for s in 0..=cnt {
                stack.push((0, from + (to - from) * s as f64 / cnt as f64));
            }
        };
        if k > 0.0 {
            for j in (1..nrows).rev() {
                let th = FRAC_PI_2 * j as f64 / m_arc as f64;
                stack.push((j, -(hin + k * th.sin())));
            }
            if hin > 1e-9 {
                wall(&mut stack, -hin, hin);
            } else {
                stack.push((0, 0.0));
            }
            for j in 1..nrows {
                let th = FRAC_PI_2 * j as f64 / m_arc as f64;
                stack.push((j, hin + k * th.sin()));
            }
        } else {
            wall(&mut stack, -height, height);
        }
        let ncol = cols.len();
        let grid: Vec<Vec<u32>> = stack.iter().map(|&(j, hgt)| rows2d[j].iter().map(|q| mesh.add(to3(*q, hgt))).collect()).collect();
        for r in 0..grid.len() - 1 {
            for i in 0..ncol {
                let i2 = (i + 1) % ncol;
                mesh.faces.push(vec![grid[r][i], grid[r][i2], grid[r + 1][i2], grid[r + 1][i]]);
            }
        }
        // caps from the last row (distinct points only)
        let cap_row = &rows2d[nrows - 1];
        let mut cap_idx: Vec<usize> = vec![];
        for i in 0..ncol {
            if cap_idx.last().map(|&l| cap_row[l].distance(cap_row[i]) < 1e-6).unwrap_or(false) {
                continue;
            }
            cap_idx.push(i);
        }
        while cap_idx.len() > 1 && cap_row[cap_idx[0]].distance(cap_row[*cap_idx.last().unwrap()]) < 1e-6 {
            cap_idx.pop();
        }
        if cap_idx.len() >= 3 {
            let pts: Vec<DVec2> = cap_idx.iter().map(|&i| cap_row[i]).collect();
            let polys = if is_convex(&pts) { vec![(0..pts.len()).collect()] } else { convex_merge(&pts, &ear_clip(&pts)) };
            for (ri, flip) in [(0usize, true), (grid.len() - 1, false)] {
                for poly in &polys {
                    let mut f: Vec<u32> = poly.iter().map(|&pi| grid[ri][cap_idx[pi]]).collect();
                    if flip {
                        f.reverse();
                    }
                    mesh.faces.push(f);
                }
            }
        }
    }
    mesh.clean_faces();
    mesh
}

impl PolyMesh {
    /// Number of boundary (open) edges, 0 for a closed manifold.
    pub fn open_edges(&self) -> usize {
        let mut count: HashMap<(u32, u32), i32> = HashMap::new();
        for f in &self.faces {
            for i in 0..f.len() {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                *count.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        count.values().filter(|&&c| c != 2).count()
    }

    pub fn normal_of(&self, f: &[u32]) -> DVec3 {
        self.face_normal(f)
    }
}
