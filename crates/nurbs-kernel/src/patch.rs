//! A linked surface (Romain, 20 nov. 2026, 18 h 12: "une surface… peut
//! avoir les trois états à la fois", NURBS, Poly, and Sculpt later): one
//! object that is both a NURBS patch and a polygon mesh, each made from the
//! other by rules that stay live (Maya's construction history, but both
//! ways).
//!
//! - **The NURBS**: a bicubic Bézier patch defined by its contour, 12
//!   control points (4 corners, two tangent points per side); its 4 inner
//!   points are made from the contour (a Coons patch, `fill_inside`), so it
//!   is edited by its contour only.
//! - **The Poly layer**: the feature's mesh. Made from the NURBS as an iso
//!   grid (what the quad converter gives a four-sided face). Each of its
//!   points is bound to the surface (`bind`): the (u, v) where it sits and
//!   its offset from there along the surface's frame (Tu, Tv, N), in mm.
//!   While the NURBS is edited every point follows (`place`): an extruded
//!   block stays on the surface, square to it. Back from Poly, the points
//!   are bound again.
//! - **A Carve** after it in the history (a box or a cylinder) cuts both
//!   states live (`cut`): moved, the hole moves; the Poly layer under it is
//!   kept whole.

use glam::{Mat4, Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PatchState {
    /// The surface, edited by its contour; the Poly layer's own shapes
    /// (an extrusion) shown on it, not editable.
    #[default]
    Nurbs,
    /// The Poly layer, edited with the Poly tab's tools.
    Poly,
    /// Not made yet (the second step).
    Sculpt,
}

impl PatchState {
    pub fn label(self) -> &'static str {
        match self {
            PatchState::Nurbs => "NURBS",
            PatchState::Poly => "POLY",
            PatchState::Sculpt => "SCULPT",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Patch {
    /// The 16 control points in the mesh's unit box (as its points):
    /// `cv[j * 4 + i]`, i along u, j along v.
    pub cv: Vec<[f32; 3]>,
    pub state: PatchState,
    /// Each point of the Poly layer on the surface: (u, v, a, b, h), its
    /// place and its offset a Tu + b Tv + h N there (mm).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bind: Vec<[f32; 5]>,
}

/// The contour's control points in order round the patch: the corners are
/// at 0, 3, 6 and 9, the two between each pair are its side's tangents.
pub const CONTOUR: [usize; 12] = [0, 1, 2, 3, 7, 11, 15, 14, 13, 12, 8, 4];

/// A control point of the contour that is a corner.
pub fn is_corner(cv: usize) -> bool {
    matches!(cv, 0 | 3 | 12 | 15)
}

/// A point of the Poly layer further than this from the surface (mm) is
/// one of its own shapes (an extrusion): shown, not editable, in NURBS.
pub const OFF: f32 = 0.02;

fn bern(t: f32) -> [f32; 4] {
    let s = 1.0 - t;
    [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t]
}

fn dbern(t: f32) -> [f32; 4] {
    let s = 1.0 - t;
    [-3.0 * s * s, 3.0 * s * s - 6.0 * s * t, 6.0 * s * t - 3.0 * t * t, 3.0 * t * t]
}

impl Patch {
    /// A patch on four corners (corners (0, 0), (1, 0), (1, 1), (0, 1),
    /// in the unit box), its sides straight.
    pub fn new(c: [Vec3; 4]) -> Patch {
        let mut cv = [Vec3::ZERO; 16];
        let at = |i: usize, j: usize| j * 4 + i;
        cv[0] = c[0];
        cv[3] = c[1];
        cv[15] = c[2];
        cv[12] = c[3];
        for k in 1..3 {
            let t = k as f32 / 3.0;
            cv[at(k, 0)] = c[0].lerp(c[1], t);
            cv[at(3, k)] = c[1].lerp(c[2], t);
            cv[at(k, 3)] = c[3].lerp(c[2], t);
            cv[at(0, k)] = c[0].lerp(c[3], t);
        }
        let mut p = Patch { cv: cv.iter().map(|v| v.to_array()).collect(), state: PatchState::Nurbs, bind: vec![] };
        p.fill_inside();
        p
    }

    /// The 4 inner control points made from the contour: the bilinearly
    /// blended Coons patch of its four cubic sides, a bicubic itself.
    pub fn fill_inside(&mut self) {
        let p = |i: usize, j: usize| Vec3::from(self.cv[j * 4 + i]);
        let mut inner = vec![];
        for j in 1..3 {
            for i in 1..3 {
                let (s, t) = (i as f32 / 3.0, j as f32 / 3.0);
                let q = p(i, 0) * (1.0 - t) + p(i, 3) * t + p(0, j) * (1.0 - s) + p(3, j) * s
                    - (p(0, 0) * (1.0 - s) * (1.0 - t) + p(3, 0) * s * (1.0 - t) + p(0, 3) * (1.0 - s) * t + p(3, 3) * s * t);
                inner.push((j * 4 + i, q));
            }
        }
        for (k, q) in inner {
            self.cv[k] = q.to_array();
        }
    }

    /// The control points in mm (the feature's frame), for these sizes.
    pub fn cvs(&self, size: Vec3) -> [Vec3; 16] {
        std::array::from_fn(|k| Vec3::from(self.cv[k]) * size)
    }

    /// The unit box fitted again (`MeshData::renormalized`): the control
    /// points stay where they are.
    pub fn remap(&mut self, old: Vec3, base: Vec3, new: Vec3) {
        for c in self.cv.iter_mut() {
            *c = ((Vec3::from(*c) * old - base) / new).to_array();
        }
    }

    /// The Poly layer's points that are its own shapes (off the surface).
    pub fn off_surface(&self) -> Vec<bool> {
        self.bind.iter().map(|b| b[2].abs() + b[3].abs() + b[4].abs() > OFF).collect()
    }
}

/// The surface's point and its tangents along u and v at (u, v).
pub fn eval(cv: &[Vec3; 16], u: f32, v: f32) -> (Vec3, Vec3, Vec3) {
    let (bu, bv, du, dv) = (bern(u), bern(v), dbern(u), dbern(v));
    let (mut s, mut su, mut sv) = (Vec3::ZERO, Vec3::ZERO, Vec3::ZERO);
    for j in 0..4 {
        for i in 0..4 {
            let p = cv[j * 4 + i];
            s += p * (bu[i] * bv[j]);
            su += p * (du[i] * bv[j]);
            sv += p * (bu[i] * dv[j]);
        }
    }
    (s, su, sv)
}

/// The frame at a point of the surface: along u, across it in the
/// surface, its normal (Su × Sv).
pub fn frame(su: Vec3, sv: Vec3) -> (Vec3, Vec3, Vec3) {
    let n = su.cross(sv).normalize_or(Vec3::Y);
    let tu = (su - n * su.dot(n)).normalize_or(Vec3::X);
    (tu, n.cross(tu), n)
}

/// An iso grid of nu × nv quads on the surface (mm), each counter-clockwise
/// round Su × Sv, with each point's (u, v).
pub fn grid(cv: &[Vec3; 16], nu: usize, nv: usize) -> (Vec<Vec3>, Vec<Vec<u32>>, Vec<[f32; 2]>) {
    let (mut pts, mut uv) = (vec![], vec![]);
    for j in 0..=nv {
        for i in 0..=nu {
            let (u, v) = (i as f32 / nu as f32, j as f32 / nv as f32);
            pts.push(eval(cv, u, v).0);
            uv.push([u, v]);
        }
    }
    let id = |i: usize, j: usize| (j * (nu + 1) + i) as u32;
    let mut faces = vec![];
    for j in 0..nv {
        for i in 0..nu {
            faces.push(vec![id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1)]);
        }
    }
    (pts, faces, uv)
}

/// The surface's normal at each point of an iso grid (as `grid`).
pub fn grid_normals(cv: &[Vec3; 16], uv: &[[f32; 2]]) -> Vec<Vec3> {
    uv.iter().map(|t| {
        let (_, su, sv) = eval(cv, t[0], t[1]);
        frame(su, sv).2
    }).collect()
}

/// Points of the surface to start the search for the nearest from.
pub fn seeds(cv: &[Vec3; 16], n: usize) -> Vec<(Vec3, [f32; 2])> {
    let (pts, _, uv) = grid(cv, n, n);
    pts.into_iter().zip(uv).collect()
}

/// Where a point (mm) sits on the surface: the nearest (u, v) in [0, 1]²
/// (from the nearest seed, then Gauss-Newton), and its offset in the frame
/// there.
pub fn bind(cv: &[Vec3; 16], p: Vec3, seeds: &[(Vec3, [f32; 2])]) -> [f32; 5] {
    let start = seeds.iter().min_by(|a, b| a.0.distance_squared(p).total_cmp(&b.0.distance_squared(p))).map(|s| s.1).unwrap_or([0.5, 0.5]);
    let (mut u, mut v) = (start[0], start[1]);
    for _ in 0..16 {
        let (s, su, sv) = eval(cv, u, v);
        let r = p - s;
        let (a, b, c) = (su.dot(su), su.dot(sv), sv.dot(sv));
        let (g1, g2) = (su.dot(r), sv.dot(r));
        let det = a * c - b * b;
        if det.abs() < 1e-12 {
            break;
        }
        let (nu, nv) = ((u + (c * g1 - b * g2) / det).clamp(0.0, 1.0), (v + (a * g2 - b * g1) / det).clamp(0.0, 1.0));
        let moved = (nu - u).abs() + (nv - v).abs();
        u = nu;
        v = nv;
        if moved < 1e-7 {
            break;
        }
    }
    let (s, su, sv) = eval(cv, u, v);
    let (tu, tv, n) = frame(su, sv);
    let r = p - s;
    [u, v, r.dot(tu), r.dot(tv), r.dot(n)]
}

/// A bound point put back on the surface as it is now (mm).
pub fn place(cv: &[Vec3; 16], b: [f32; 5]) -> Vec3 {
    let (s, su, sv) = eval(cv, b[0], b[1]);
    let (tu, tv, n) = frame(su, sv);
    s + tu * b[2] + tv * b[3] + n * b[4]
}

/// One of the patch's four sides as a polyline (mm): 0 v = 0, 1 u = 1,
/// 2 v = 1, 3 u = 0.
pub fn side(cv: &[Vec3; 16], k: usize, n: usize) -> Vec<Vec3> {
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let (u, v) = match k {
                0 => (t, 0.0),
                1 => (1.0, t),
                2 => (t, 1.0),
                _ => (0.0, t),
            };
            eval(cv, u, v).0
        })
        .collect()
}

/// The surface's iso curves at u and v = k / (count + 1) (mm).
pub fn isolines(cv: &[Vec3; 16], count: usize, n: usize) -> Vec<Vec<Vec3>> {
    let mut out = vec![];
    for k in 1..=count {
        let c = k as f32 / (count + 1) as f32;
        out.push((0..=n).map(|i| eval(cv, c, i as f32 / n as f32).0).collect());
        out.push((0..=n).map(|i| eval(cv, i as f32 / n as f32, c).0).collect());
    }
    out
}

// ------------------------------------------------------------------ cuts

/// A Carve's solid (world): a box or a cylinder standing on its base at its
/// frame's origin, up along its +Y.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Solid {
    /// World to its frame (a turn and a move: distances kept).
    pub to_local: Mat4,
    pub kind: SolidKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SolidKind {
    Box { half: Vec3 },
    Cylinder { r: f32, h: f32 },
}

impl Solid {
    /// The signed distance (< 0 inside).
    pub fn sdf(&self, p: Vec3) -> f32 {
        let l = self.to_local.transform_point3(p);
        match self.kind {
            SolidKind::Box { half } => {
                let q = (l - Vec3::new(0.0, half.y, 0.0)).abs() - half;
                q.max(Vec3::ZERO).length() + q.max_element().min(0.0)
            }
            SolidKind::Cylinder { r, h } => {
                let a = Vec2::new(l.x, l.z).length() - r;
                let b = (l.y - h * 0.5).abs() - h * 0.5;
                Vec2::new(a.max(0.0), b.max(0.0)).length() + a.max(b).min(0.0)
            }
        }
    }
}

/// A polygon's normal (Newell: any polygon, convex or not).
pub fn newell(pts: &[Vec3], f: &[u32]) -> Vec3 {
    let mut n = Vec3::ZERO;
    for k in 0..f.len() {
        let (a, b) = (pts[f[k] as usize], pts[f[(k + 1) % f.len()] as usize]);
        n += Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    n.normalize_or(Vec3::Y)
}

fn basis(n: Vec3) -> (Vec3, Vec3) {
    let e1 = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let e1 = (e1 - n * e1.dot(n)).normalize();
    (e1, n.cross(e1))
}

/// Where an edge crosses the solid's surface: each crossing's place along
/// it (from `a`) and its new point (appended to `out`).
fn edge_crossings(a: Vec3, b: Vec3, s: &Solid, out: &mut Vec<Vec3>) -> Vec<(f32, u32)> {
    const N: usize = 8;
    let mut list = vec![];
    let at = |t: f32| a.lerp(b, t);
    let mut t0 = 0.0;
    let mut d0 = s.sdf(a);
    for k in 1..=N {
        let t1 = k as f32 / N as f32;
        let d1 = s.sdf(at(t1));
        if (d0 < 0.0) != (d1 < 0.0) {
            let (mut lo, mut hi) = (t0, t1);
            let inside_lo = d0 < 0.0;
            for _ in 0..30 {
                let m = 0.5 * (lo + hi);
                if (s.sdf(at(m)) < 0.0) == inside_lo {
                    lo = m;
                } else {
                    hi = m;
                }
            }
            let t = 0.5 * (lo + hi);
            out.push(at(t));
            list.push((t, (out.len() - 1) as u32));
        }
        t0 = t1;
        d0 = d1;
    }
    list
}

/// The polygons less what lies inside the solid: each polygon's part
/// outside it, the new border along the solid's surface sampled every 8°
/// or so (its section must be convex: a box's, a cylinder's). The points
/// keep their index (new ones go after); with each polygon, the one it
/// comes from. A hole wholly inside one polygon, touching none of its
/// points or sides, is not made.
pub fn cut(pts: &[Vec3], faces: &[Vec<u32>], s: &Solid) -> (Vec<Vec3>, Vec<Vec<u32>>, Vec<u32>) {
    let d: Vec<f32> = pts.iter().map(|p| s.sdf(*p)).collect();
    let inside = |x: f32| x < 0.0;
    let mut out = pts.to_vec();
    let (mut out_faces, mut parent) = (vec![], vec![]);
    let mut cross: HashMap<(u32, u32), Vec<(f32, u32)>> = HashMap::new();
    #[derive(Clone, Copy)]
    enum Ev {
        Pt(u32),
        In(u32),
        Out(u32),
    }
    for (fi, f) in faces.iter().enumerate() {
        let n = f.len();
        if n < 3 {
            continue;
        }
        // far from it: as it is
        let span = (0..n).map(|k| pts[f[k] as usize].distance(pts[f[(k + 1) % n] as usize])).fold(0.0f32, f32::max);
        if f.iter().all(|v| d[*v as usize] > span) {
            out_faces.push(f.clone());
            parent.push(fi as u32);
            continue;
        }
        if f.iter().all(|v| d[*v as usize] < -span) {
            continue;
        }
        // the walk round it: the points kept, where it goes in and out
        let mut ev = vec![];
        for k in 0..n {
            let (a, b) = (f[k], f[(k + 1) % n]);
            if !inside(d[a as usize]) {
                ev.push(Ev::Pt(a));
            }
            let key = (a.min(b), a.max(b));
            let list = match cross.get(&key) {
                Some(l) => l.clone(),
                None => {
                    let l = edge_crossings(pts[key.0 as usize], pts[key.1 as usize], s, &mut out);
                    cross.insert(key, l.clone());
                    l
                }
            };
            let mut state = inside(d[a as usize]);
            let along: Vec<u32> = if a == key.0 { list.iter().map(|c| c.1).collect() } else { list.iter().rev().map(|c| c.1).collect() };
            for c in along {
                ev.push(if state { Ev::Out(c) } else { Ev::In(c) });
                state = !state;
            }
        }
        let crossings: Vec<u32> = ev.iter().filter_map(|e| match e {
            Ev::In(c) | Ev::Out(c) => Some(*c),
            _ => None,
        }).collect();
        if crossings.is_empty() {
            if !inside(d[f[0] as usize]) {
                out_faces.push(f.clone());
                parent.push(fi as u32);
            }
            continue;
        }
        if crossings.len() % 2 == 1 {
            // cannot happen round a closed loop; keep it whole
            out_faces.push(f.clone());
            parent.push(fi as u32);
            continue;
        }
        // the runs outside: out, the points kept, in
        let m = ev.len();
        let mut runs: Vec<(u32, Vec<u32>, u32)> = vec![];
        for k in 0..m {
            if let Ev::Out(o) = ev[k] {
                let mut kept = vec![];
                let mut j = (k + 1) % m;
                let mut end = None;
                for _ in 0..m {
                    match ev[j] {
                        Ev::Pt(p) => kept.push(p),
                        Ev::In(c) => {
                            end = Some(c);
                            break;
                        }
                        Ev::Out(_) => break,
                    }
                    j = (j + 1) % m;
                }
                if let Some(e) = end {
                    runs.push((o, kept, e));
                }
            }
        }
        if runs.is_empty() {
            continue;
        }
        // the solid's section seen round a point inside it (the crossings'
        // middle: inside a convex section)
        let nrm = newell(pts, f);
        let (e1, e2) = basis(nrm);
        let c = crossings.iter().map(|k| out[*k as usize]).sum::<Vec3>() / crossings.len() as f32;
        let centred = s.sdf(c) < 0.0;
        let reach = crossings.iter().map(|k| out[*k as usize].distance(c)).fold(0.0f32, f32::max) * 3.0 + 1.0;
        let ang = |p: Vec3| {
            let q = p - c;
            q.dot(e2).atan2(q.dot(e1))
        };
        let tau = std::f32::consts::TAU;
        let mut used = vec![false; runs.len()];
        for r0 in 0..runs.len() {
            if used[r0] {
                continue;
            }
            let mut poly: Vec<u32> = vec![];
            let mut r = r0;
            for _ in 0..=runs.len() {
                used[r] = true;
                poly.push(runs[r].0);
                poly.extend_from_slice(&runs[r].1);
                poly.push(runs[r].2);
                // from where it goes in, along the solid's border with the
                // solid on the right (clockwise round it): the first place
                // it comes out
                let a_in = ang(out[runs[r].2 as usize]);
                let delta = |x: usize| {
                    let dd = (a_in - ang(out[runs[x].0 as usize])).rem_euclid(tau);
                    if dd < 1e-6 { tau } else { dd }
                };
                let next = (0..runs.len()).min_by(|&x, &y| delta(x).total_cmp(&delta(y))).unwrap();
                if centred {
                    let dd = delta(next);
                    let steps = ((dd / 8f32.to_radians()).ceil() as usize).clamp(1, 45);
                    for k in 1..steps {
                        let th = a_in - dd * k as f32 / steps as f32;
                        let dir = e1 * th.cos() + e2 * th.sin();
                        let (mut lo, mut hi) = (0.0f32, reach);
                        let mut ok = s.sdf(c + dir * hi) >= 0.0;
                        for _ in 0..6 {
                            if ok {
                                break;
                            }
                            hi *= 2.0;
                            ok = s.sdf(c + dir * hi) >= 0.0;
                        }
                        if !ok {
                            continue;
                        }
                        for _ in 0..30 {
                            let mid = 0.5 * (lo + hi);
                            if s.sdf(c + dir * mid) < 0.0 {
                                lo = mid;
                            } else {
                                hi = mid;
                            }
                        }
                        out.push(c + dir * (0.5 * (lo + hi)));
                        poly.push((out.len() - 1) as u32);
                    }
                }
                r = next;
                if r == r0 || used[r] {
                    break;
                }
            }
            poly.dedup();
            if poly.len() > 1 && poly[0] == poly[poly.len() - 1] {
                poly.pop();
            }
            if poly.len() >= 3 {
                out_faces.push(poly);
                parent.push(fi as u32);
            }
        }
    }
    (out, out_faces, parent)
}

/// `cut` by each solid in turn; each polygon with the first one it comes from.
pub fn cut_all(pts: &[Vec3], faces: &[Vec<u32>], solids: &[Solid]) -> (Vec<Vec3>, Vec<Vec<u32>>, Vec<u32>) {
    let mut p = pts.to_vec();
    let mut f = faces.to_vec();
    let mut parent: Vec<u32> = (0..faces.len() as u32).collect();
    for s in solids {
        let (p2, f2, par) = cut(&p, &f, s);
        parent = par.iter().map(|k| parent[*k as usize]).collect();
        p = p2;
        f = f2;
    }
    (p, f, parent)
}

impl Solid {
    /// A step fine enough to follow its border: a cut drawn through
    /// polygons smaller than it follows the solid's curve.
    pub fn grain(&self) -> f32 {
        match self.kind {
            SolidKind::Box { half } => (half.min_element() / 3.0).max(0.05),
            SolidKind::Cylinder { r, h } => (r / 6.0).min(h / 4.0).max(0.05),
        }
    }
}

/// The polygons near the solids cut in finer quads before a cut (a quad
/// into k × k by its bilinear map, k the same for all so neighbours share
/// their points), so the cut follows the solids' curves across big faces
/// (an extrusion's walls); each with the polygon it comes from. Polygons
/// other than quads, and those far from every solid, stay as they are.
pub fn refine_near(pts: &[Vec3], faces: &[Vec<u32>], solids: &[Solid]) -> (Vec<Vec3>, Vec<Vec<u32>>, Vec<u32>) {
    let id: Vec<u32> = (0..faces.len() as u32).collect();
    if solids.is_empty() {
        return (pts.to_vec(), faces.to_vec(), id);
    }
    let grain = solids.iter().map(|s| s.grain()).fold(f32::MAX, f32::min);
    let near: Vec<bool> = faces
        .iter()
        .map(|f| {
            if f.len() != 4 {
                return false;
            }
            let c = f.iter().map(|v| pts[*v as usize]).sum::<Vec3>() / 4.0;
            let rad = f.iter().map(|v| pts[*v as usize].distance(c)).fold(0.0f32, f32::max);
            solids.iter().any(|s| s.sdf(c) < rad + grain)
        })
        .collect();
    let longest = faces
        .iter()
        .zip(&near)
        .filter(|(_, n)| **n)
        .map(|(f, _)| (0..4).map(|k| pts[f[k] as usize].distance(pts[f[(k + 1) % 4] as usize])).fold(0.0f32, f32::max))
        .fold(0.0f32, f32::max);
    let k = ((longest / grain).ceil() as usize).clamp(1, 24);
    if k == 1 {
        return (pts.to_vec(), faces.to_vec(), id);
    }
    let mut out = pts.to_vec();
    // the points along an edge (a to b, inside ones only), shared
    let mut along: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    let mut edge_pts = |a: u32, b: u32, out: &mut Vec<Vec3>| -> Vec<u32> {
        let key = (a.min(b), a.max(b));
        let l = along.entry(key).or_insert_with(|| {
            let (p, q) = (out[key.0 as usize], out[key.1 as usize]);
            (1..k)
                .map(|i| {
                    out.push(p.lerp(q, i as f32 / k as f32));
                    (out.len() - 1) as u32
                })
                .collect()
        });
        if a == key.0 { l.clone() } else { l.iter().rev().copied().collect() }
    };
    let (mut nf, mut parent) = (vec![], vec![]);
    for (fi, f) in faces.iter().enumerate() {
        if !near[fi] {
            nf.push(f.clone());
            parent.push(fi as u32);
            continue;
        }
        // the grid's points: g[j][i], i along f0 → f1, j along f0 → f3
        let mut g = vec![vec![0u32; k + 1]; k + 1];
        let bottom = edge_pts(f[0], f[1], &mut out);
        let right = edge_pts(f[1], f[2], &mut out);
        let top = edge_pts(f[3], f[2], &mut out);
        let left = edge_pts(f[0], f[3], &mut out);
        g[0][0] = f[0];
        g[0][k] = f[1];
        g[k][k] = f[2];
        g[k][0] = f[3];
        for i in 1..k {
            g[0][i] = bottom[i - 1];
            g[k][i] = top[i - 1];
            g[i][0] = left[i - 1];
            g[i][k] = right[i - 1];
        }
        let (p0, p1, p2, p3) = (pts[f[0] as usize], pts[f[1] as usize], pts[f[2] as usize], pts[f[3] as usize]);
        for j in 1..k {
            for i in 1..k {
                let (u, v) = (i as f32 / k as f32, j as f32 / k as f32);
                out.push(p0.lerp(p1, u).lerp(p3.lerp(p2, u), v));
                g[j][i] = (out.len() - 1) as u32;
            }
        }
        for j in 0..k {
            for i in 0..k {
                nf.push(vec![g[j][i], g[j][i + 1], g[j + 1][i + 1], g[j + 1][i]]);
                parent.push(fi as u32);
            }
        }
    }
    (out, nf, parent)
}

/// Segments less their pieces inside the solids: each cut where it
/// crosses a solid's border (found by halves), as polylines.
pub fn clip_segments(segs: &[(Vec3, Vec3)], solids: &[Solid]) -> Vec<Vec<Vec3>> {
    if solids.is_empty() {
        return segs.iter().map(|(a, b)| vec![*a, *b]).collect();
    }
    let grain = solids.iter().map(|s| s.grain()).fold(f32::MAX, f32::min);
    let inside = |p: Vec3| solids.iter().any(|s| s.sdf(p) < 0.0);
    let mut out = vec![];
    for (a, b) in segs {
        let n = ((a.distance(*b) / grain).ceil() as usize).clamp(1, 400);
        let at = |t: f32| a.lerp(*b, t);
        let mut cur: Vec<Vec3> = vec![];
        let mut t0 = 0.0;
        let mut in0 = inside(*a);
        if !in0 {
            cur.push(*a);
        }
        for i in 1..=n {
            let t1 = i as f32 / n as f32;
            let in1 = inside(at(t1));
            if in0 != in1 {
                let (mut lo, mut hi) = (t0, t1);
                for _ in 0..30 {
                    let m = 0.5 * (lo + hi);
                    if inside(at(m)) == in0 {
                        lo = m;
                    } else {
                        hi = m;
                    }
                }
                let x = at(0.5 * (lo + hi));
                if in0 {
                    cur = vec![x];
                } else {
                    cur.push(x);
                    if cur.len() >= 2 {
                        out.push(std::mem::take(&mut cur));
                    }
                    cur.clear();
                }
            }
            t0 = t1;
            in0 = in1;
        }
        if !in0 {
            cur.push(*b);
            if cur.len() >= 2 {
                out.push(cur);
            }
        }
    }
    out
}

/// The rims a cut leaves: the edges of one polygon only whose two ends lie
/// on a solid's border.
pub fn rims(pts: &[Vec3], faces: &[Vec<u32>], solids: &[Solid]) -> Vec<(Vec3, Vec3)> {
    let mut count: HashMap<(u32, u32), u32> = HashMap::new();
    for q in faces {
        for i in 0..q.len() {
            let (a, b) = (q[i], q[(i + 1) % q.len()]);
            *count.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let on = |p: Vec3| solids.iter().any(|s| s.sdf(p).abs() < 1e-3 + 1e-4 * s.grain());
    count.into_iter().filter(|(_, n)| *n == 1).map(|((a, b), _)| (pts[a as usize], pts[b as usize])).filter(|(a, b)| on(*a) && on(*b)).collect()
}

/// A polygon as triangles: a fan when convex, else ear clipping in its
/// plane (a polygon a cut bit into is not convex).
pub fn triangulate(pts: &[Vec3], f: &[u32]) -> Vec<[u32; 3]> {
    let n = f.len();
    if n < 3 {
        return vec![];
    }
    if n == 3 {
        return vec![[f[0], f[1], f[2]]];
    }
    let (e1, e2) = basis(newell(pts, f));
    let p2: Vec<Vec2> = f.iter().map(|v| {
        let p = pts[*v as usize];
        Vec2::new(p.dot(e1), p.dot(e2))
    }).collect();
    let turn = |a: Vec2, b: Vec2, c: Vec2| (b - a).perp_dot(c - b);
    let fan = |idx: &[usize]| -> Vec<[u32; 3]> { (1..idx.len() - 1).map(|k| [f[idx[0]], f[idx[k]], f[idx[k + 1]]]).collect() };
    let all: Vec<usize> = (0..n).collect();
    if (0..n).all(|k| turn(p2[k], p2[(k + 1) % n], p2[(k + 2) % n]) >= -1e-9) {
        return fan(&all);
    }
    let in_tri = |p: Vec2, a: Vec2, b: Vec2, c: Vec2| {
        let (d1, d2, d3) = ((b - a).perp_dot(p - a), (c - b).perp_dot(p - b), (a - c).perp_dot(p - c));
        d1 > 1e-12 && d2 > 1e-12 && d3 > 1e-12
    };
    let mut idx = all;
    let mut out = vec![];
    while idx.len() > 3 {
        let m = idx.len();
        let mut cut = None;
        for k in 0..m {
            let (ia, ib, ic) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
            let (a, b, c) = (p2[ia], p2[ib], p2[ic]);
            if turn(a, b, c) <= 1e-12 {
                continue;
            }
            if idx.iter().any(|&j| j != ia && j != ib && j != ic && in_tri(p2[j], a, b, c)) {
                continue;
            }
            cut = Some(k);
            out.push([f[ia], f[ib], f[ic]]);
            break;
        }
        match cut {
            Some(k) => {
                idx.remove(k);
            }
            None => break,
        }
    }
    if idx.len() >= 3 {
        out.extend(fan(&idx));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat() -> Patch {
        // u along +X, v along -Z: the normal up
        Patch::new([Vec3::new(-30.0, 0.0, 20.0), Vec3::new(30.0, 0.0, 20.0), Vec3::new(30.0, 0.0, -20.0), Vec3::new(-30.0, 0.0, -20.0)])
    }

    #[test]
    fn saved_and_read_again() {
        // a document with a linked surface: its patch, state and binding
        // come back from the .fnurbs as they were
        let mut d = crate::doc::Doc::default();
        let pts = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0), Vec3::new(10.0, 0.0, -10.0), Vec3::new(0.0, 0.0, -10.0)];
        let id = d.add_mesh(crate::mesh::MeshKind::Poly, &pts, vec![vec![0, 1, 2, 3]], glam::DVec3::ZERO, glam::DQuat::IDENTITY, "Surface");
        let mut p = flat();
        p.state = PatchState::Poly;
        p.bind = vec![[0.0, 0.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0, 2.5], [1.0, 1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.1, -0.2, 0.0]];
        d.feature_mut(id).unwrap().mesh.as_mut().unwrap().make_mut().patch = Some(p.clone());
        let back = crate::doc::Doc::from_json(&d.to_json()).unwrap();
        let m = back.feature(id).unwrap().mesh.as_ref().unwrap();
        assert_eq!(m.patch.as_ref(), Some(&p));
        // a mesh without one saves no patch field
        let mut d2 = crate::doc::Doc::default();
        d2.add_mesh(crate::mesh::MeshKind::Poly, &pts, vec![vec![0, 1, 2, 3]], glam::DVec3::ZERO, glam::DQuat::IDENTITY, "Plane");
        assert!(!d2.to_json().contains("patch"));
    }

    #[test]
    fn contour_makes_the_inside() {
        let mut p = flat();
        let cv = p.cvs(Vec3::ONE);
        // flat and straight: the patch is the plane, normal up
        let (s, su, sv) = eval(&cv, 0.5, 0.5);
        assert!(s.length() < 1e-4);
        assert!(frame(su, sv).2.y > 0.99);
        // a wave on the front side: the inside follows, the back side not
        p.cv[1][1] = 6.0;
        p.cv[2][1] = -6.0;
        p.fill_inside();
        let cv = p.cvs(Vec3::ONE);
        let front = eval(&cv, 1.0 / 3.0, 0.0).0.y;
        let mid = eval(&cv, 1.0 / 3.0, 0.5).0.y;
        let back = eval(&cv, 1.0 / 3.0, 1.0).0.y;
        assert!(front > 1.0 && mid > 0.2 && mid < front && back.abs() < 1e-4, "{front} {mid} {back}");
    }

    #[test]
    fn bound_points_follow_the_contour() {
        let mut p = flat();
        let cv = p.cvs(Vec3::ONE);
        let seeds = seeds(&cv, 16);
        // a point on the surface and one 3 mm over it (an extrusion's top)
        let on = Vec3::new(5.0, 0.0, 4.0);
        let over = Vec3::new(5.0, 3.0, 4.0);
        let (b0, b1) = (bind(&cv, on, &seeds), bind(&cv, over, &seeds));
        assert!(place(&cv, b0).distance(on) < 1e-3 && place(&cv, b1).distance(over) < 1e-3);
        assert!(b0[4].abs() < 1e-4 && (b1[4] - 3.0).abs() < 1e-3);
        // a corner raised: the point on the surface stays on it, the top
        // stays 3 mm over it, square to it
        // (with its two tangent points, as the app moves a corner)
        for k in [15, 14, 11] {
            p.cv[k][1] = 20.0;
        }
        p.fill_inside();
        let cv = p.cvs(Vec3::ONE);
        let (q0, q1) = (place(&cv, b0), place(&cv, b1));
        let (s, su, sv) = eval(&cv, b0[0], b0[1]);
        assert!(q0.distance(s) < 1e-4);
        assert!(((q1 - q0).length() - 3.0).abs() < 1e-3);
        assert!((q1 - q0).normalize().dot(frame(su, sv).2) > 0.999);
        assert!(q0.y > 1.0);
    }

    #[test]
    fn a_big_face_cut_follows_the_curve() {
        // one leaning wall (10 × 10, 2 mm out at its top) bitten at its foot
        // by an upright cylinder: refined first, the cut follows the curve
        let pts = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0), Vec3::new(10.0, 10.0, 2.0), Vec3::new(0.0, 10.0, 2.0)];
        let faces = vec![vec![0u32, 1, 2, 3]];
        let s = Solid { to_local: Mat4::from_translation(Vec3::new(0.0, 10.0, 1.0)), kind: SolidKind::Cylinder { r: 3.0, h: 40.0 } };
        let (rp, rf, rpar) = refine_near(&pts, &faces, &[s]);
        assert!(rf.len() > 1 && rpar.iter().all(|k| *k == 0));
        let (out, cf, _) = cut(&rp, &rf, &s);
        let mut area = 0.0;
        for f in &cf {
            for t in triangulate(&out, f) {
                let (a, b, c) = (out[t[0] as usize], out[t[1] as usize], out[t[2] as usize]);
                area += 0.5 * (b - a).cross(c - a).length();
            }
        }
        // the area outside, counted on a fine grid of the wall
        let n = 1000;
        let mut kept = 0;
        for j in 0..n {
            for i in 0..n {
                let (u, v) = ((i as f32 + 0.5) / n as f32, (j as f32 + 0.5) / n as f32);
                let q = pts[0].lerp(pts[1], u).lerp(pts[3].lerp(pts[2], u), v);
                if s.sdf(q) >= 0.0 {
                    kept += 1;
                }
            }
        }
        let want = kept as f32 / (n * n) as f32 * 10.0 * (104.0f32).sqrt();
        assert!((area - want).abs() < want * 0.003, "{area} vs {want}");
        // the rim on the cylinder, from the foot up to the wall's side
        let rim = rims(&out, &cf, &[s]);
        assert!(rim.len() >= 6, "{} rim edges", rim.len());
        for (a, b) in &rim {
            assert!(s.sdf(*a).abs() < 1e-3 && s.sdf(*b).abs() < 1e-3);
        }
        // the wall's edges clipped: the foot and the side start on the rim
        let segs = vec![(pts[0], pts[1]), (pts[0], pts[3])];
        let kept = clip_segments(&segs, &[s]);
        assert_eq!(kept.len(), 2);
        for l in &kept {
            assert!(s.sdf(l[0]).abs() < 1e-3);
        }
    }

    #[test]
    fn a_cylinder_cuts_a_grid() {
        let p = flat();
        let cv = p.cvs(Vec3::ONE);
        let (pts, faces, _) = grid(&cv, 12, 8);
        let s = Solid { to_local: Mat4::from_translation(Vec3::new(0.0, 10.0, 0.0)), kind: SolidKind::Cylinder { r: 8.0, h: 20.0 } };
        let (out, cut_faces, parent) = cut(&pts, &faces, &s);
        assert_eq!(cut_faces.len(), parent.len());
        // no point left inside, the area less the disc
        let mut area = 0.0;
        for f in &cut_faces {
            for t in triangulate(&out, f) {
                let (a, b, c) = (out[t[0] as usize], out[t[1] as usize], out[t[2] as usize]);
                area += 0.5 * (b - a).cross(c - a).length();
                let g = (a + b + c) / 3.0;
                assert!(s.sdf(g) > -0.05, "a triangle inside at {g}");
            }
        }
        let want = 60.0 * 40.0 - std::f32::consts::PI * 64.0;
        assert!((area - want).abs() < want * 0.004, "{area} vs {want}");
        // a box through one side only: the polygons there bitten
        let b = Solid { to_local: Mat4::from_translation(Vec3::new(-30.0, 5.0, 0.0)), kind: SolidKind::Box { half: Vec3::new(10.0, 10.0, 5.0) } };
        let (out, cut_faces, _) = cut(&pts, &faces, &b);
        let mut area = 0.0;
        for f in &cut_faces {
            for t in triangulate(&out, f) {
                let (a, bb, c) = (out[t[0] as usize], out[t[1] as usize], out[t[2] as usize]);
                area += 0.5 * (bb - a).cross(c - a).length();
            }
        }
        let want = 60.0 * 40.0 - 10.0 * 10.0;
        assert!((area - want).abs() < 1.0, "{area} vs {want}");
    }

    #[test]
    fn a_polygon_cut_in_four() {
        // a big square, a disc over its middle reaching past its sides but
        // not its corners: four corner pieces
        let pts = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 10.0), Vec3::new(0.0, 0.0, 10.0)];
        let faces = vec![vec![0, 3, 2, 1]];
        let s = Solid { to_local: Mat4::from_translation(Vec3::new(-5.0, 5.0, -5.0)), kind: SolidKind::Cylinder { r: 6.5, h: 20.0 } };
        let (out, f, _) = cut(&pts, &faces, &s);
        assert_eq!(f.len(), 4);
        let area: f32 = f.iter().flat_map(|p| triangulate(&out, p)).map(|t| {
            let (a, b, c) = (out[t[0] as usize], out[t[1] as usize], out[t[2] as usize]);
            0.5 * (b - a).cross(c - a).length()
        }).sum();
        // the square less the disc's part in it (a disc of 6.5 cut by the
        // four sides at 5)
        let r: f32 = 6.5;
        let seg = r * r * (5.0f32 / r).acos() - 5.0 * (r * r - 25.0).sqrt();
        let want = 100.0 - (std::f32::consts::PI * r * r - 4.0 * seg);
        assert!((area - want).abs() < 0.3, "{area} vs {want}");
    }
}
