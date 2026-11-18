//! The shapes a region can be: plane, cylinder, sphere. Fitting (least
//! squares on the faces' centres and normals), distance, projection.

use crate::mesh::{add_outer, eigen_sym};
use glam::DVec3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Prim {
    /// n · x = d, n pointing out of the matter.
    Plane { n: DVec3, d: f64 },
    /// Axis through `p` along unit `a`, radius `r`.
    Cylinder { p: DVec3, a: DVec3, r: f64 },
    Sphere { c: DVec3, r: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Plane,
    Cylinder,
    Sphere,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Plane => "Plane",
            Kind::Cylinder => "Cylinder",
            Kind::Sphere => "Sphere",
        }
    }
}

/// One sample of a region: a face's centre, normal and area.
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub p: DVec3,
    pub n: DVec3,
    pub w: f64,
}

impl Prim {
    pub fn kind(&self) -> Kind {
        match self {
            Prim::Plane { .. } => Kind::Plane,
            Prim::Cylinder { .. } => Kind::Cylinder,
            Prim::Sphere { .. } => Kind::Sphere,
        }
    }

    /// Distance from `x` to the surface (unsigned).
    pub fn dist(&self, x: DVec3) -> f64 {
        match *self {
            Prim::Plane { n, d } => (n.dot(x) - d).abs(),
            Prim::Cylinder { p, a, r } => {
                let v = x - p;
                ((v - a * a.dot(v)).length() - r).abs()
            }
            Prim::Sphere { c, r } => ((x - c).length() - r).abs(),
        }
    }

    /// The surface's normal direction at (the projection of) `x`, up to sign
    /// for cylinders and spheres.
    pub fn normal_at(&self, x: DVec3) -> DVec3 {
        match *self {
            Prim::Plane { n, .. } => n,
            Prim::Cylinder { p, a, .. } => {
                let v = x - p;
                (v - a * a.dot(v)).normalize_or(DVec3::X)
            }
            Prim::Sphere { c, .. } => (x - c).normalize_or(DVec3::X),
        }
    }

    /// Whether a face (centre, normal) lies on the surface within the
    /// tolerances (`cos_max`: cosine of the largest angle).
    pub fn fits(&self, s: &Sample, tol: f64, cos_max: f64) -> bool {
        if self.dist(s.p) > tol {
            return false;
        }
        let m = self.normal_at(s.p);
        match self {
            Prim::Plane { .. } => m.dot(s.n) > cos_max,
            _ => m.dot(s.n).abs() > cos_max,
        }
    }

    pub fn project(&self, x: DVec3) -> DVec3 {
        match *self {
            Prim::Plane { n, d } => x - n * (n.dot(x) - d),
            Prim::Cylinder { p, a, r } => {
                let v = x - p;
                let along = a * a.dot(v);
                let rad = v - along;
                let l = rad.length();
                if l < 1e-12 {
                    return x;
                }
                p + along + rad * (r / l)
            }
            Prim::Sphere { c, r } => {
                let v = x - c;
                let l = v.length();
                if l < 1e-12 {
                    return x;
                }
                c + v * (r / l)
            }
        }
    }

    /// Root mean square distance of the samples.
    pub fn rms(&self, s: &[Sample]) -> f64 {
        let (mut e, mut w) = (0.0, 0.0);
        for x in s {
            let d = self.dist(x.p);
            e += x.w * d * d;
            w += x.w;
        }
        if w > 0.0 {
            (e / w).sqrt()
        } else {
            0.0
        }
    }

    pub fn describe(&self) -> String {
        match *self {
            Prim::Plane { n, .. } => format!("normal {:.2} {:.2} {:.2}", n.x, n.y, n.z),
            Prim::Cylinder { a, r, .. } => format!("R {:.2} mm · axis {:.2} {:.2} {:.2}", r, a.x, a.y, a.z),
            Prim::Sphere { r, .. } => format!("R {:.2} mm", r),
        }
    }
}

pub fn total_weight(s: &[Sample]) -> f64 {
    s.iter().map(|x| x.w).sum()
}

fn centroid(s: &[Sample]) -> DVec3 {
    let w = total_weight(s).max(1e-300);
    s.iter().map(|x| x.p * x.w).sum::<DVec3>() / w
}

/// Least squares plane, its normal turned like the faces'.
pub fn fit_plane(s: &[Sample]) -> Option<Prim> {
    if s.len() < 3 {
        return None;
    }
    let c = centroid(s);
    let mut cov = [[0.0; 3]; 3];
    let mut nsum = DVec3::ZERO;
    for x in s {
        add_outer(&mut cov, x.p - c, x.w);
        nsum += x.n * x.w;
    }
    let (_, v) = eigen_sym(cov);
    let mut n = v[0];
    if n.dot(nsum) < 0.0 {
        n = -n;
    }
    Some(Prim::Plane { n, d: n.dot(c) })
}

/// A plane with a given normal: only its offset is fitted.
pub fn fit_plane_dir(s: &[Sample], n: DVec3) -> Prim {
    Prim::Plane { n, d: n.dot(centroid(s)) }
}

/// The axis direction of a cylinder: the normals are all square to it.
/// Also returns the share of the normals' spread across it (0 for a plane).
pub fn axis_of(s: &[Sample]) -> (DVec3, [f64; 3]) {
    let mut m = [[0.0; 3]; 3];
    let mut w = 0.0;
    for x in s {
        add_outer(&mut m, x.n, x.w);
        w += x.w;
    }
    let (l, v) = eigen_sym(m);
    let w = w.max(1e-300);
    (v[0], [l[0] / w, l[1] / w, l[2] / w])
}

/// 2D coordinates in the plane square to `a`.
fn frame(a: DVec3) -> (DVec3, DVec3) {
    let u = a.any_orthonormal_vector();
    (u, a.cross(u))
}

/// Circle through 2D points (Kåsa, then a few Gauss-Newton steps on the
/// true distances). Fixed radius when `r` is given.
fn fit_circle(pts: &[(f64, f64, f64)], r: Option<f64>) -> Option<(f64, f64, f64)> {
    if pts.len() < 3 {
        return None;
    }
    // algebraic: x² + y² + D x + E y + F = 0
    let mut a = [[0.0; 3]; 3];
    let mut b = [0.0; 3];
    for &(x, y, w) in pts {
        let row = [x, y, 1.0];
        let rhs = -(x * x + y * y);
        for i in 0..3 {
            for j in 0..3 {
                a[i][j] += w * row[i] * row[j];
            }
            b[i] += w * row[i] * rhs;
        }
    }
    let s = solve3(a, b)?;
    let (mut cx, mut cy) = (-s[0] * 0.5, -s[1] * 0.5);
    let r2 = cx * cx + cy * cy - s[2];
    if r2 <= 0.0 {
        return None;
    }
    let mut rr = r.unwrap_or(r2.sqrt());
    for _ in 0..8 {
        // residual d_i = |p - c| - r; unknowns (cx, cy, r) or (cx, cy)
        let mut jtj = [[0.0; 3]; 3];
        let mut jtr = [0.0; 3];
        for &(x, y, w) in pts {
            let (dx, dy) = (x - cx, y - cy);
            let l = (dx * dx + dy * dy).sqrt().max(1e-12);
            let res = l - rr;
            let j = [-dx / l, -dy / l, -1.0];
            for i in 0..3 {
                for k in 0..3 {
                    jtj[i][k] += w * j[i] * j[k];
                }
                jtr[i] += w * j[i] * res;
            }
        }
        if r.is_some() {
            jtj[2] = [0.0, 0.0, 1.0];
            jtj[0][2] = 0.0;
            jtj[1][2] = 0.0;
            jtr[2] = 0.0;
        }
        let Some(d) = solve3(jtj, jtr) else { break };
        cx -= d[0];
        cy -= d[1];
        if r.is_none() {
            rr -= d[2];
        }
        if d[0].abs() + d[1].abs() + d[2].abs() < 1e-9 {
            break;
        }
    }
    (rr > 0.0 && rr.is_finite()).then_some((cx, cy, rr))
}

/// Cylinder: axis from the normals (or given), circle in the square plane.
pub fn fit_cylinder(s: &[Sample], axis: Option<DVec3>, radius: Option<f64>) -> Option<Prim> {
    if s.len() < 6 {
        return None;
    }
    let a = match axis {
        Some(a) => a,
        None => axis_of(s).0,
    };
    let (u, v) = frame(a);
    let c = centroid(s);
    let pts: Vec<(f64, f64, f64)> = s.iter().map(|x| ((x.p - c).dot(u), (x.p - c).dot(v), x.w)).collect();
    let (cx, cy, r) = fit_circle(&pts, radius)?;
    Some(Prim::Cylinder { p: c + u * cx + v * cy, a, r })
}

/// Cylinder whose axis line is fixed (only the radius fitted).
pub fn fit_radius(s: &[Sample], p: DVec3, a: DVec3) -> Prim {
    let w = total_weight(s).max(1e-300);
    let r = s.iter().map(|x| {
        let v = x.p - p;
        (v - a * a.dot(v)).length() * x.w
    }).sum::<f64>() / w;
    Prim::Cylinder { p, a, r }
}

/// Sphere: algebraic fit |x|² + D·x + F = 0.
pub fn fit_sphere(s: &[Sample], radius: Option<f64>) -> Option<Prim> {
    if s.len() < 6 {
        return None;
    }
    let c0 = centroid(s);
    let mut a = [[0.0; 4]; 4];
    let mut b = [0.0; 4];
    for x in s {
        let p = x.p - c0;
        let row = [p.x, p.y, p.z, 1.0];
        let rhs = -p.length_squared();
        for i in 0..4 {
            for j in 0..4 {
                a[i][j] += x.w * row[i] * row[j];
            }
            b[i] += x.w * row[i] * rhs;
        }
    }
    let q = solve4(a, b)?;
    let c = DVec3::new(-q[0] * 0.5, -q[1] * 0.5, -q[2] * 0.5);
    let r2 = c.length_squared() - q[3];
    if r2 <= 0.0 {
        return None;
    }
    let c = c + c0;
    let r = match radius {
        Some(r) => r,
        None => {
            let w = total_weight(s).max(1e-300);
            s.iter().map(|x| (x.p - c).length() * x.w).sum::<f64>() / w
        }
    };
    Some(Prim::Sphere { c, r })
}

pub fn solve3(a: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let mut m = [[0.0; 4]; 3];
    for i in 0..3 {
        m[i][..3].copy_from_slice(&a[i]);
        m[i][3] = b[i];
    }
    gauss::<3, 4>(m).map(|x| [x[0], x[1], x[2]])
}

pub fn solve4(a: [[f64; 4]; 4], b: [f64; 4]) -> Option<[f64; 4]> {
    let mut m = [[0.0; 5]; 4];
    for i in 0..4 {
        m[i][..4].copy_from_slice(&a[i]);
        m[i][4] = b[i];
    }
    gauss::<4, 5>(m).map(|x| [x[0], x[1], x[2], x[3]])
}

fn gauss<const N: usize, const M: usize>(mut m: [[f64; M]; N]) -> Option<Vec<f64>> {
    let scale = m.iter().flat_map(|r| r[..N].iter()).fold(0.0f64, |a, &b| a.max(b.abs())).max(1e-300);
    for col in 0..N {
        let piv = (col..N).max_by(|&i, &j| m[i][col].abs().partial_cmp(&m[j][col].abs()).unwrap())?;
        if m[piv][col].abs() < 1e-13 * scale {
            return None;
        }
        m.swap(col, piv);
        for row in 0..N {
            if row != col {
                let f = m[row][col] / m[col][col];
                for k in col..M {
                    m[row][k] -= f * m[col][k];
                }
            }
        }
    }
    Some((0..N).map(|i| m[i][N] / m[i][i]).collect())
}
