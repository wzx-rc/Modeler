//! NURBS curves and surfaces: what the retopology tools lay on the scan
//! (Romain, 18 nov. 2026, 16 h 01: "un style d'outil de retopo mais en NURBS
//! pas poly"). Rational B-splines with clamped knot vectors; circles are
//! exact (rational quadratic). Algorithms from The NURBS Book (Piegl and
//! Tiller): the knot span (A2.1), the basis functions (A2.2).

use glam::DVec3;

/// Where `t` falls in the knots: the span index `i` with
/// knots[i] <= t < knots[i + 1] (the last span holds the end).
fn find_span(knots: &[f64], degree: usize, n: usize, t: f64) -> usize {
    if t >= knots[n] {
        return n - 1;
    }
    if t <= knots[degree] {
        return degree;
    }
    let (mut lo, mut hi) = (degree, n);
    let mut mid = (lo + hi) / 2;
    while t < knots[mid] || t >= knots[mid + 1] {
        if t < knots[mid] {
            hi = mid;
        } else {
            lo = mid;
        }
        mid = (lo + hi) / 2;
    }
    mid
}

/// The `degree + 1` basis functions not zero in `span` at `t`.
fn basis(knots: &[f64], degree: usize, span: usize, t: f64) -> Vec<f64> {
    let mut n = vec![0.0; degree + 1];
    let mut left = vec![0.0; degree + 1];
    let mut right = vec![0.0; degree + 1];
    n[0] = 1.0;
    for j in 1..=degree {
        left[j] = t - knots[span + 1 - j];
        right[j] = knots[span + j] - t;
        let mut saved = 0.0;
        for r in 0..j {
            let den = right[r + 1] + left[j - r];
            let tmp = if den.abs() > 1e-300 { n[r] / den } else { 0.0 };
            n[r] = saved + right[r + 1] * tmp;
            saved = left[j - r] * tmp;
        }
        n[j] = saved;
    }
    n
}

/// A clamped knot vector of `spans` equal spans on [0, 1].
pub fn uniform_knots(degree: usize, spans: usize) -> Vec<f64> {
    let mut k = vec![0.0; degree + 1];
    for i in 1..spans {
        k.push(i as f64 / spans as f64);
    }
    k.extend(std::iter::repeat_n(1.0, degree + 1));
    k
}

/// Where each control point "sits" along the curve (Greville abscissae): a
/// linear function's control values there reproduce it exactly.
pub fn greville(knots: &[f64], degree: usize) -> Vec<f64> {
    let n = knots.len() - degree - 1;
    (0..n).map(|i| if degree == 0 { knots[i] } else { knots[i + 1..=i + degree].iter().sum::<f64>() / degree as f64 }).collect()
}

/// A NURBS curve.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub degree: usize,
    pub knots: Vec<f64>,
    pub pts: Vec<DVec3>,
    /// One weight per control point (1: not rational there).
    pub w: Vec<f64>,
}

impl Curve {
    /// A straight line.
    pub fn line(a: DVec3, b: DVec3) -> Curve {
        Curve { degree: 1, knots: vec![0.0, 0.0, 1.0, 1.0], pts: vec![a, b], w: vec![1.0, 1.0] }
    }

    /// An exact circle around `c` in the plane of `u`, `v` (unit, square),
    /// starting along `u`: degree 2, 9 control points, 4 quarter arcs.
    pub fn circle(c: DVec3, u: DVec3, v: DVec3, r: f64) -> Curve {
        let (pts, w) = unit_circle();
        Curve { degree: 2, knots: circle_knots(), pts: pts.iter().map(|p| c + (u * p[0] + v * p[1]) * r).collect(), w }
    }

    pub fn domain(&self) -> (f64, f64) {
        (self.knots[self.degree], self.knots[self.pts.len()])
    }

    pub fn eval(&self, t: f64) -> DVec3 {
        let n = self.pts.len();
        let s = find_span(&self.knots, self.degree, n, t);
        let b = basis(&self.knots, self.degree, s, t);
        let (mut p, mut wsum) = (DVec3::ZERO, 0.0);
        for (k, bk) in b.iter().enumerate() {
            let i = s - self.degree + k;
            p += self.pts[i] * (bk * self.w[i]);
            wsum += bk * self.w[i];
        }
        p / wsum
    }

    /// `n` + 1 points evenly in the parameter.
    pub fn sample(&self, n: usize) -> Vec<DVec3> {
        let (a, b) = self.domain();
        (0..=n).map(|k| self.eval(a + (b - a) * k as f64 / n as f64)).collect()
    }

    /// A non-rational curve of `degree` with `spans` equal spans fitted to
    /// values `(t in 0..1, value)` by least squares (a little smoothing
    /// keeps it calm where there is no data). One coordinate: x.
    pub fn fit_1d(data: &[(f64, f64)], degree: usize, spans: usize) -> Option<Vec<f64>> {
        let degree = degree.max(1);
        let knots = uniform_knots(degree, spans.max(1));
        let n = knots.len() - degree - 1;
        if data.len() < 2 {
            return None;
        }
        let mut ata = vec![vec![0.0f64; n]; n];
        let mut atb = vec![0.0f64; n];
        for &(t, y) in data {
            let s = find_span(&knots, degree, n, t.clamp(0.0, 1.0));
            let b = basis(&knots, degree, s, t.clamp(0.0, 1.0));
            for (a, ba) in b.iter().enumerate() {
                let i = s - degree + a;
                atb[i] += ba * y;
                for (c, bc) in b.iter().enumerate() {
                    ata[i][s - degree + c] += ba * bc;
                }
            }
        }
        // smoothing: the control values' bend kept small (second divided
        // differences at their Greville abscissae, zero on a straight line),
        // small against the data
        let lam = data.len() as f64 * 1e-4;
        let g = greville(&knots, degree);
        let hm = 1.0 / n.max(2) as f64;
        for i in 1..n.saturating_sub(1) {
            let (h0, h1) = ((g[i] - g[i - 1]).max(1e-9), (g[i + 1] - g[i]).max(1e-9));
            let k = 2.0 * hm * hm;
            let d = [(i - 1, k / (h0 * (h0 + h1))), (i, -k / (h0 * h1)), (i + 1, k / (h1 * (h0 + h1)))];
            for &(a, wa) in &d {
                for &(b, wb) in &d {
                    ata[a][b] += lam * wa * wb;
                }
            }
        }
        solve(ata, atb)
    }

    /// A non-rational curve of `degree` with `spans` equal spans fitted to
    /// weighted points `(t in 0..1, point, weight)` by least squares; `lam`
    /// keeps its bend small (second divided differences of the control
    /// points at their Greville abscissae: zero on a straight line). A
    /// large weight pins the curve on a point.
    pub fn fit_3d(data: &[(f64, DVec3, f64)], degree: usize, spans: usize, lam: f64) -> Option<Curve> {
        let degree = degree.max(1);
        let knots = uniform_knots(degree, spans.max(1));
        let n = knots.len() - degree - 1;
        if data.len() < 2 {
            return None;
        }
        let mut ata = vec![vec![0.0f64; n]; n];
        let mut atb = vec![DVec3::ZERO; n];
        for &(t, p, w) in data {
            let t = t.clamp(0.0, 1.0);
            let s = find_span(&knots, degree, n, t);
            let b = basis(&knots, degree, s, t);
            for (a, ba) in b.iter().enumerate() {
                let i = s - degree + a;
                atb[i] += p * (w * ba);
                for (c, bc) in b.iter().enumerate() {
                    ata[i][s - degree + c] += w * ba * bc;
                }
            }
        }
        let g = greville(&knots, degree);
        let hm = 1.0 / n.max(2) as f64;
        for i in 1..n.saturating_sub(1) {
            let (h0, h1) = ((g[i] - g[i - 1]).max(1e-9), (g[i + 1] - g[i]).max(1e-9));
            let k = 2.0 * hm * hm;
            let d = [(i - 1, k / (h0 * (h0 + h1))), (i, -k / (h0 * h1)), (i + 1, k / (h1 * (h0 + h1)))];
            for &(a, wa) in &d {
                for &(b, wb) in &d {
                    ata[a][b] += lam * wa * wb;
                }
            }
        }
        let xs = solve(ata.clone(), atb.iter().map(|p| p.x).collect())?;
        let ys = solve(ata.clone(), atb.iter().map(|p| p.y).collect())?;
        let zs = solve(ata, atb.iter().map(|p| p.z).collect())?;
        let pts = (0..n).map(|i| DVec3::new(xs[i], ys[i], zs[i])).collect();
        Some(Curve { degree, knots, pts, w: vec![1.0; n] })
    }

    /// The same curve raised to degree 3 (lines and cubics only: a line's
    /// points at its thirds).
    fn cubic(&self) -> Curve {
        if self.degree == 1 && self.pts.len() == 2 {
            let (a, b) = (self.pts[0], self.pts[1]);
            let (k0, k1) = self.domain();
            return Curve { degree: 3, knots: vec![k0, k0, k0, k0, k1, k1, k1, k1], pts: vec![a, a + (b - a) / 3.0, a + (b - a) * (2.0 / 3.0), b], w: vec![1.0; 4] };
        }
        assert!(self.degree == 3, "only lines and cubics are joined");
        self.clone()
    }

    /// Non-rational lines and cubics joined end to end into one cubic
    /// (corners where they meet: a knot of multiplicity 3), each on a part
    /// of 0..1 as long as `lens` says. Also where each piece starts and
    /// ends in the parameter.
    pub fn join(pieces: &[Curve], lens: &[f64]) -> (Curve, Vec<f64>) {
        let total: f64 = lens.iter().map(|l| l.max(1e-9)).sum();
        let mut breaks = vec![0.0];
        for l in lens {
            breaks.push(breaks.last().unwrap() + l.max(1e-9) / total);
        }
        *breaks.last_mut().unwrap() = 1.0;
        let mut pts: Vec<DVec3> = vec![];
        let mut knots: Vec<f64> = vec![0.0; 4];
        for (i, c) in pieces.iter().enumerate() {
            let c = c.cubic();
            let (k0, k1) = c.domain();
            let (s0, s1) = (breaks[i], breaks[i + 1]);
            let map = |k: f64| s0 + (k - k0) / (k1 - k0).max(1e-12) * (s1 - s0);
            let n = c.pts.len();
            // inner knots, then the end three times (four at the very end)
            knots.extend(c.knots[4..n].iter().map(|&k| map(k)));
            knots.extend([s1; 3]);
            if i == 0 {
                pts.extend_from_slice(&c.pts);
            } else {
                pts.extend_from_slice(&c.pts[1..]);
            }
        }
        knots.push(1.0);
        let n = pts.len();
        (Curve { degree: 3, knots, pts, w: vec![1.0; n] }, breaks)
    }

    /// The curve cut in two at `t` (non-rational, `t` inside its domain):
    /// `t` inserted until it is `degree` times a knot (Boehm), the point
    /// there shared.
    pub fn split(&self, t: f64) -> Option<(Curve, Curve)> {
        let p = self.degree;
        let (a, b) = self.domain();
        if !(t > a + 1e-12 && t < b - 1e-12) {
            return None;
        }
        let mut knots = self.knots.clone();
        let mut pts = self.pts.clone();
        let mult = |k: &[f64]| k.iter().filter(|&&x| (x - t).abs() < 1e-12).count();
        while mult(&knots) < p {
            let n = pts.len();
            let k = find_span(&knots, p, n, t);
            let mut q = Vec::with_capacity(n + 1);
            for i in 0..=n {
                if i + p <= k {
                    q.push(pts[i]);
                } else if i > k {
                    q.push(pts[i - 1]);
                } else {
                    let al = (t - knots[i]) / (knots[i + p] - knots[i]).max(1e-300);
                    q.push(pts[i] * al + pts[i - 1] * (1.0 - al));
                }
            }
            knots.insert(k + 1, t);
            pts = q;
        }
        let first = knots.iter().position(|&x| (x - t).abs() < 1e-12)?;
        let mut lk: Vec<f64> = knots[..first + p].to_vec();
        lk.push(t);
        let lp = pts[..first].to_vec();
        let mut rk = vec![t];
        rk.extend_from_slice(&knots[first..]);
        let rp = pts[first - 1..].to_vec();
        let (nl, nr) = (lp.len(), rp.len());
        Some((Curve { degree: p, knots: lk, pts: lp, w: vec![1.0; nl] }, Curve { degree: p, knots: rk, pts: rp, w: vec![1.0; nr] }))
    }

    /// Points along the part of the parameter from `a` to `b`.
    pub fn sample_range(&self, a: f64, b: f64, n: usize) -> Vec<DVec3> {
        (0..=n).map(|k| self.eval(a + (b - a) * k as f64 / n as f64)).collect()
    }
}

/// The unit circle's 9 control points (x, y) and weights.
fn unit_circle() -> (Vec<[f64; 2]>, Vec<f64>) {
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let pts = vec![[1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [-1.0, 1.0], [-1.0, 0.0], [-1.0, -1.0], [0.0, -1.0], [1.0, -1.0], [1.0, 0.0]];
    let w = vec![1.0, s, 1.0, s, 1.0, s, 1.0, s, 1.0];
    (pts, w)
}

fn circle_knots() -> Vec<f64> {
    vec![0.0, 0.0, 0.0, 0.25, 0.25, 0.5, 0.5, 0.75, 0.75, 1.0, 1.0, 1.0]
}

/// Gaussian elimination with partial pivoting.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().partial_cmp(&a[j][col].abs()).unwrap())?;
        if a[piv][col].abs() < 1e-14 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for r in col + 1..n {
            let f = a[r][col] / a[col][col];
            if f == 0.0 {
                continue;
            }
            for c in col..n {
                a[r][c] -= f * a[col][c];
            }
            b[r] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let s: f64 = (r + 1..n).map(|c| a[r][c] * x[c]).sum();
        x[r] = (b[r] - s) / a[r][r];
    }
    Some(x)
}

/// A NURBS surface: `nu` × `nv` control points, row by row (u runs fastest).
#[derive(Clone, Debug, PartialEq)]
pub struct Surface {
    pub du: usize,
    pub dv: usize,
    pub ku: Vec<f64>,
    pub kv: Vec<f64>,
    pub nu: usize,
    pub nv: usize,
    pub pts: Vec<DVec3>,
    pub w: Vec<f64>,
}

impl Surface {
    /// A surface turned around the axis through `c` along `a` (`u`, `v`:
    /// square to it): each control point of `profile` (x: the distance from
    /// the axis, y: the height along it) makes an exact circle. u runs
    /// around, v along the profile.
    pub fn revolve(c: DVec3, a: DVec3, u: DVec3, v: DVec3, profile: &Curve) -> Surface {
        let (cp, cw) = unit_circle();
        let mut pts = vec![];
        let mut w = vec![];
        for (j, q) in profile.pts.iter().enumerate() {
            for (i, p) in cp.iter().enumerate() {
                pts.push(c + a * q.y + (u * p[0] + v * p[1]) * q.x);
                w.push(cw[i] * profile.w[j]);
            }
        }
        Surface { du: 2, dv: profile.degree, ku: circle_knots(), kv: profile.knots.clone(), nu: 9, nv: profile.pts.len(), pts, w }
    }

    pub fn eval(&self, u: f64, v: f64) -> DVec3 {
        let su = find_span(&self.ku, self.du, self.nu, u);
        let sv = find_span(&self.kv, self.dv, self.nv, v);
        let bu = basis(&self.ku, self.du, su, u);
        let bv = basis(&self.kv, self.dv, sv, v);
        let (mut p, mut wsum) = (DVec3::ZERO, 0.0);
        for (b, nb) in bv.iter().enumerate() {
            let j = sv - self.dv + b;
            for (a, na) in bu.iter().enumerate() {
                let i = su - self.du + a;
                let k = j * self.nu + i;
                let f = na * nb * self.w[k];
                p += self.pts[k] * f;
                wsum += f;
            }
        }
        p / wsum
    }

    /// A grid of (`su` + 1) × (`sv` + 1) points and their normals (from
    /// the points next to them), row by row.
    pub fn tessellate(&self, su: usize, sv: usize) -> (Vec<DVec3>, Vec<DVec3>) {
        let (u0, u1) = (self.ku[self.du], self.ku[self.nu]);
        let (v0, v1) = (self.kv[self.dv], self.kv[self.nv]);
        let at = |i: usize, j: usize| (u0 + (u1 - u0) * i as f64 / su as f64, v0 + (v1 - v0) * j as f64 / sv as f64);
        let mut pts = vec![];
        let mut nrm = vec![];
        let e = 1e-4;
        for j in 0..=sv {
            for i in 0..=su {
                let (u, v) = at(i, j);
                pts.push(self.eval(u, v));
                let du = self.eval((u + e).min(u1), v) - self.eval((u - e).max(u0), v);
                let dv = self.eval(u, (v + e).min(v1)) - self.eval(u, (v - e).max(v0));
                nrm.push(du.cross(dv).normalize_or_zero());
            }
        }
        (pts, nrm)
    }

    /// The lines where the knots are (the patches' borders), each sampled:
    /// in u and in v, the surface's own borders included.
    pub fn isos(&self, samples: usize) -> Vec<Vec<DVec3>> {
        let mut out = vec![];
        let mut uk: Vec<f64> = self.ku[self.du..=self.nu].to_vec();
        uk.dedup();
        let mut vk: Vec<f64> = self.kv[self.dv..=self.nv].to_vec();
        vk.dedup();
        let (u0, u1) = (self.ku[self.du], self.ku[self.nu]);
        let (v0, v1) = (self.kv[self.dv], self.kv[self.nv]);
        for &u in &uk {
            out.push((0..=samples).map(|k| self.eval(u, v0 + (v1 - v0) * k as f64 / samples as f64)).collect());
        }
        for &v in &vk {
            out.push((0..=samples * 4).map(|k| self.eval(u0 + (u1 - u0) * k as f64 / (samples * 4) as f64, v)).collect());
        }
        out
    }

    /// The control net: its rows and its columns.
    pub fn hull(&self) -> Vec<Vec<DVec3>> {
        let mut out = vec![];
        for j in 0..self.nv {
            out.push((0..self.nu).map(|i| self.pts[j * self.nu + i]).collect());
        }
        for i in 0..self.nu {
            out.push((0..self.nv).map(|j| self.pts[j * self.nu + i]).collect());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_is_exact() {
        let c = Curve::circle(DVec3::new(1.0, 2.0, 3.0), DVec3::X, DVec3::Y, 5.0);
        for p in c.sample(200) {
            assert!(((p - DVec3::new(1.0, 2.0, 3.0)).length() - 5.0).abs() < 1e-9);
        }
    }

    #[test]
    fn revolved_rings_are_circles() {
        let prof = Curve { degree: 3, knots: uniform_knots(3, 2), pts: vec![DVec3::new(4.0, 0.0, 0.0), DVec3::new(4.5, 1.0, 0.0), DVec3::new(5.0, 2.0, 0.0), DVec3::new(4.0, 3.0, 0.0), DVec3::new(4.2, 4.0, 0.0)], w: vec![1.0; 5] };
        let s = Surface::revolve(DVec3::ZERO, DVec3::Z, DVec3::X, DVec3::Y, &prof);
        for v in [0.0, 0.3, 0.77, 1.0] {
            let r0 = { let p = s.eval(0.0, v); (p.x * p.x + p.y * p.y).sqrt() };
            for k in 0..50 {
                let p = s.eval(k as f64 / 50.0, v);
                assert!(((p.x * p.x + p.y * p.y).sqrt() - r0).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn split_keeps_the_curve() {
        let c = Curve { degree: 3, knots: uniform_knots(3, 4), pts: (0..7).map(|i| DVec3::new(i as f64, ((i * 7) % 5) as f64, (i * i) as f64 * 0.1)).collect(), w: vec![1.0; 7] };
        for t in [0.1, 0.25, 0.6] {
            let (l, r) = c.split(t).unwrap();
            assert_eq!(l.knots.len(), l.pts.len() + 4);
            assert_eq!(r.knots.len(), r.pts.len() + 4);
            for k in 0..=20 {
                let u = t * k as f64 / 20.0;
                assert!((l.eval(u) - c.eval(u)).length() < 1e-9);
                let v = t + (1.0 - t) * k as f64 / 20.0;
                assert!((r.eval(v) - c.eval(v)).length() < 1e-9);
            }
        }
    }

    #[test]
    fn fit_reproduces_a_line() {
        let data: Vec<(f64, f64)> = (0..50).map(|k| (k as f64 / 49.0, 2.0 + 3.0 * k as f64 / 49.0)).collect();
        let c = Curve::fit_1d(&data, 3, 3).unwrap();
        let g = greville(&uniform_knots(3, 3), 3);
        for (ci, gi) in c.iter().zip(&g) {
            assert!((ci - (2.0 + 3.0 * gi)).abs() < 1e-3);
        }
    }
}
