//! Picking on the part's mesh: the ray hits a triangle, the triangle
//! belongs to a B-rep face, the face carries its tag (which feature, which
//! face of it). Exact on the kernel's triangles.

use glam::Vec3;
use nurbs_kernel::Model;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// Distance along the ray.
    pub t: f32,
    pub point: Vec3,
    /// Index in `Model::faces`.
    pub face: usize,
    /// The triangle's normal, towards the ray's origin.
    pub normal: Vec3,
}

/// Per face: its triangles (contiguous, the kernel writes them face by
/// face) and their bounds.
#[derive(Default)]
pub struct Picker {
    ranges: Vec<(u32, u32)>,
    boxes: Vec<(Vec3, Vec3)>,
}

impl Picker {
    pub fn new(m: &Model) -> Picker {
        let mut p = Picker { ranges: vec![(u32::MAX, 0); m.faces.len()], boxes: vec![(Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)); m.faces.len()] };
        for (t, tri) in m.triangles.iter().enumerate() {
            let f = m.tri_face[t] as usize;
            if f >= p.ranges.len() {
                continue;
            }
            let r = &mut p.ranges[f];
            r.0 = r.0.min(t as u32);
            r.1 = r.1.max(t as u32 + 1);
            let b = &mut p.boxes[f];
            for &v in tri {
                let q = Vec3::from(m.positions[v as usize]);
                b.0 = b.0.min(q);
                b.1 = b.1.max(q);
            }
        }
        p
    }

    /// Nearest hit along the ray.
    pub fn pick(&self, m: &Model, ro: Vec3, rd: Vec3) -> Option<Hit> {
        let inv = Vec3::new(1.0 / rd.x, 1.0 / rd.y, 1.0 / rd.z);
        let mut best: Option<Hit> = None;
        for (f, (&(a, b), &(lo, hi))) in self.ranges.iter().zip(&self.boxes).enumerate() {
            if a >= b {
                continue;
            }
            // slab test against the face's bounds (a little margin)
            let t0 = (lo - Vec3::splat(1e-3) - ro) * inv;
            let t1 = (hi + Vec3::splat(1e-3) - ro) * inv;
            let tmin = t0.min(t1).max_element();
            let tmax = t0.max(t1).min_element();
            if tmax < tmin.max(0.0) || best.map(|h| tmin > h.t).unwrap_or(false) {
                continue;
            }
            for t in a..b {
                if m.tri_face[t as usize] as usize != f {
                    continue;
                }
                let tri = m.triangles[t as usize];
                let p0 = Vec3::from(m.positions[tri[0] as usize]);
                let p1 = Vec3::from(m.positions[tri[1] as usize]);
                let p2 = Vec3::from(m.positions[tri[2] as usize]);
                if let Some(d) = ray_tri(ro, rd, p0, p1, p2) {
                    if best.map(|h| d < h.t).unwrap_or(true) {
                        let mut n = (p1 - p0).cross(p2 - p0).normalize_or_zero();
                        if n.dot(rd) > 0.0 {
                            n = -n;
                        }
                        best = Some(Hit { t: d, point: ro + rd * d, face: f, normal: n });
                    }
                }
            }
        }
        best
    }
}

/// Möller–Trumbore, both sides.
fn ray_tri(ro: Vec3, rd: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = rd.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = ro - a;
    let u = s.dot(p) * inv;
    if !(-1e-6..=1.0 + 1e-6).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = rd.dot(q) * inv;
    if v < -1e-6 || u + v > 1.0 + 1e-6 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 1e-4).then_some(t)
}
